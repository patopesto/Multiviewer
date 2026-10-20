#include <cstring>
#include <string>
#include <utility>
#include <vector>

#include <DeckLinkAPI.h>
#include "decklink_discovery.h"
#include "platform.h"


/* ── helpers ─────────────────────────────────────────────────────────── */

static std::string get_model_name(IDeckLink* decklink) {
    DECKLINK_DLSTRING_T name = nullptr;
    if (decklink->GetModelName(&name) != S_OK) {
        return "";
    }
    std::string result = DecklinkStringToStd(name);
    DecklinkStringFree(name);
    return result;
}

static std::string get_mode_name(IDeckLinkDisplayMode* mode) {
    DECKLINK_DLSTRING_T name = nullptr;
    if (mode->GetName(&name) != S_OK) {
        return "";
    }
    std::string result = DecklinkStringToStd(name);
    DecklinkStringFree(name);
    return result;
}

static void copy_name(char* out, size_t out_len, const std::string& name) {
    if (out_len == 0) return;
    strncpy(out, name.c_str(), out_len - 1);
    out[out_len - 1] = '\0';
}


/* ── ports ───────────────────────────────────────────────────────────── */

struct InputPort {
    std::string name;
    bool has_signal = false;
    uint32_t connections = 0;
};

struct OutputMode {
    std::string name;
    BMDDisplayMode mode = bmdModeUnknown;
    long width = 0;
    long height = 0;
    double fps = 0.0;
};

struct OutputPort {
    std::string name;
    std::vector<OutputMode> modes;
};

struct DecklinkDiscovery {
    std::vector<InputPort> inputs;
    std::vector<OutputPort> outputs;
};


/* ── per-device collectors ───────────────────────────────────────────── */

static void collect_input(IDeckLink* decklink, const std::string& model, bool sub_named, int sub_index, DecklinkDiscovery* d) {
    IDeckLinkInput* input = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkInput, (void**)&input) != S_OK) {
        return;
    }
    input->Release();

    InputPort port;
    port.name = sub_named ? model + " - Input " + std::to_string(sub_index + 1) : model;

    IDeckLinkStatus* status = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkStatus, (void**)&status) == S_OK) {
        DecklinkBool locked = 0;
        status->GetFlag(bmdDeckLinkStatusVideoInputSignalLocked, &locked);
        port.has_signal = locked != 0;
        status->Release();
    }

    IDeckLinkProfileAttributes* attr = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
        int64_t conn_mask = 0;
        if (attr->GetInt(BMDDeckLinkVideoInputConnections, &conn_mask) == S_OK) {
            port.connections = static_cast<uint32_t>(conn_mask);
        }
        attr->Release();
    }

    d->inputs.push_back(port);
}

static void collect_output(IDeckLink* decklink, const std::string& model, bool sub_named, int sub_index, DecklinkDiscovery* d) {
    IDeckLinkOutput* output = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkOutput, (void**)&output) != S_OK) {
        return;
    }

    OutputPort port;
    port.name = sub_named ? model + " - Output " + std::to_string(sub_index + 1) : model;

    IDeckLinkDisplayModeIterator* mode_iter = nullptr;
    if (output->GetDisplayModeIterator(&mode_iter) == S_OK && mode_iter) {
        IDeckLinkDisplayMode* mode = nullptr;
        while (mode_iter->Next(&mode) == S_OK) {
            BMDTimeValue frame_duration = 0;
            BMDTimeValue time_scale = 0;
            double fps = 0.0;
            if (mode->GetFrameRate(&frame_duration, &time_scale) == S_OK && frame_duration > 0) {
                fps = static_cast<double>(time_scale) / static_cast<double>(frame_duration);
            }
            port.modes.push_back({
                get_mode_name(mode),
                mode->GetDisplayMode(),
                mode->GetWidth(),
                mode->GetHeight(),
                fps,
            });
            mode->Release();
        }
        mode_iter->Release();
    }

    // The mode list is all the UI needs; holding the interface open would keep
    // a driver handle alive for the whole poll interval.
    output->Release();
    d->outputs.push_back(std::move(port));
}


/* ── discovery ───────────────────────────────────────────────────────── */

DecklinkDiscovery* decklink_discovery_new(void) {
    DecklinkComScope com_scope;
    auto* d = new DecklinkDiscovery();

    IDeckLinkIterator* iterator = nullptr;
    if (FAILED(DecklinkCreateIterator(&iterator))) {
        return d;
    }

    IDeckLink* decklink = nullptr;
    while (iterator->Next(&decklink) == S_OK) {
        int64_t sub_count = 1;
        int64_t sub_index = 0;
        IDeckLinkProfileAttributes* attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            attr->GetInt(BMDDeckLinkNumberOfSubDevices, &sub_count);
            attr->GetInt(BMDDeckLinkSubDeviceIndex, &sub_index);
            attr->Release();
        }

        std::string model = get_model_name(decklink);
        const bool sub_named = sub_count > 1;
        const int index = static_cast<int>(sub_index);

        collect_input(decklink, model, sub_named, index, d);
        collect_output(decklink, model, sub_named, index, d);
        decklink->Release();
    }

    iterator->Release();
    return d;
}

void decklink_discovery_free(DecklinkDiscovery* d) {
    delete d;
}


/* ── accessors ───────────────────────────────────────────────────────── */

int decklink_discovery_input_count(DecklinkDiscovery* d) {
    return static_cast<int>(d->inputs.size());
}

void decklink_discovery_input_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, bool* has_signal, uint32_t* connections) {
    if (idx < 0 || idx >= static_cast<int>(d->inputs.size())) {
        if (name_len > 0) name[0] = '\0';
        *has_signal = false;
        *connections = 0;
        return;
    }
    const auto& port = d->inputs[idx];
    copy_name(name, name_len, port.name);
    *has_signal = port.has_signal;
    *connections = port.connections;
}

int decklink_discovery_output_count(DecklinkDiscovery* d) {
    return static_cast<int>(d->outputs.size());
}

void decklink_discovery_output_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, int* mode_count) {
    if (idx < 0 || idx >= static_cast<int>(d->outputs.size())) {
        if (name_len > 0) name[0] = '\0';
        *mode_count = 0;
        return;
    }
    const auto& port = d->outputs[idx];
    copy_name(name, name_len, port.name);
    *mode_count = static_cast<int>(port.modes.size());
}

void decklink_discovery_output_mode(DecklinkDiscovery* d, int idx, int mode_idx, char* mode_name, size_t mode_name_len, uint32_t* mode_id, int* w, int* h, double* fps) {
    if (mode_name_len > 0) mode_name[0] = '\0';
    *mode_id = 0;
    *w = 0;
    *h = 0;
    *fps = 0.0;
    if (idx < 0 || idx >= static_cast<int>(d->outputs.size())) return;
    const auto& port = d->outputs[idx];
    if (mode_idx < 0 || mode_idx >= static_cast<int>(port.modes.size())) return;
    const auto& mode = port.modes[mode_idx];
    copy_name(mode_name, mode_name_len, mode.name);
    *mode_id = static_cast<uint32_t>(mode.mode);
    *w = static_cast<int>(mode.width);
    *h = static_cast<int>(mode.height);
    *fps = mode.fps;
}
