#include <vector>
#include <string>
#include <mutex>
#include <atomic>
#include <cstring>
#include <algorithm>
#include <unistd.h>

#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#include <CoreVideo/CoreVideo.h>
#endif

#include <DeckLinkAPI.h>
#include "decklink_output.h"



/* ── helpers ─────────────────────────────────────────────────────────── */

#ifdef __APPLE__
static std::string cfstring_to_std(CFStringRef cf) {
    char buf[256];
    if (CFStringGetCString(cf, buf, sizeof(buf), kCFStringEncodingUTF8)) {
        CFRelease(cf);
        return std::string(buf);
    }
    CFRelease(cf);
    return "";
}
#endif

static std::string get_model_name(IDeckLink* decklink) {
#ifdef __APPLE__
    CFStringRef cf_name;
    if (decklink->GetModelName(&cf_name) == S_OK)
        return cfstring_to_std(cf_name);
    return "";
#else
    const char* name = nullptr;
    if (decklink->GetModelName(&name) == S_OK)
        return std::string(name);
    return "";
#endif
}

static std::string get_mode_name(IDeckLinkDisplayMode* mode) {
#ifdef __APPLE__
    CFStringRef cf_name = nullptr;
    if (mode->GetName(&cf_name) == S_OK && cf_name)
        return cfstring_to_std(cf_name);
#else
    const char* name = nullptr;
    if (mode->GetName(&name) == S_OK && name)
        return std::string(name);
#endif
    return "";
}



/* ── Discovery ────────────────────────────────────────────────── */

struct OutputMode {
    std::string name;
    BMDDisplayMode mode;
    long w = 0;
    long h = 0;
    double fps = 0.0;
};

struct OutputPort {
    std::string display_name;
    IDeckLinkOutput* output = nullptr;
    std::vector<OutputMode> modes;
};

struct DecklinkOutputDiscovery {
    std::vector<OutputPort> ports;
};


DecklinkOutputDiscovery* decklink_output_discovery_new(void) {
    auto* d = new DecklinkOutputDiscovery();

    IDeckLinkIterator* iterator = CreateDeckLinkIteratorInstance();
    if (!iterator) return d;

    IDeckLink* decklink = nullptr;
    while (iterator->Next(&decklink) == S_OK) {
        IDeckLinkOutput* output = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkOutput, (void**)&output) != S_OK) {
            decklink->Release();
            continue;
        }

        std::string model = get_model_name(decklink);

        int64_t num_sub = 1;
        int64_t sub_index = 0;
        IDeckLinkProfileAttributes* attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            attr->GetInt(BMDDeckLinkNumberOfSubDevices, &num_sub);
            attr->GetInt(BMDDeckLinkSubDeviceIndex, &sub_index);
            attr->Release();
        }

        std::string name = (num_sub > 1)
            ? model + " - Output " + std::to_string(static_cast<int>(sub_index) + 1)
            : model;

        OutputPort port;
        port.display_name = name;
        port.output = output;

        IDeckLinkDisplayModeIterator* mode_iter = nullptr;
        if (output->GetDisplayModeIterator(&mode_iter) == S_OK && mode_iter) {
            IDeckLinkDisplayMode* mode = nullptr;
            while (mode_iter->Next(&mode) == S_OK) {
                BMDDisplayMode mode_id = mode->GetDisplayMode();
                long w = mode->GetWidth();
                long h = mode->GetHeight();
                BMDTimeValue frame_duration = 0;
                BMDTimeValue time_scale = 0;
                double fps = 0.0;
                if (mode->GetFrameRate(&frame_duration, &time_scale) == S_OK && frame_duration > 0) {
                    fps = static_cast<double>(time_scale) / static_cast<double>(frame_duration);
                }
                port.modes.push_back({get_mode_name(mode), mode_id, w, h, fps});
                mode->Release();
            }
            mode_iter->Release();
        }

        d->ports.push_back(std::move(port));
        decklink->Release();
    }

    iterator->Release();
    return d;
}


void decklink_output_discovery_free(DecklinkOutputDiscovery* d) {
    if (!d) return;
    for (auto& port : d->ports) {
        if (port.output) {
            port.output->Release();
            port.output = nullptr;
        }
    }
    delete d;
}


int decklink_output_discovery_count(DecklinkOutputDiscovery* d) {
    return static_cast<int>(d->ports.size());
}


void decklink_output_discovery_get(DecklinkOutputDiscovery* d, int idx, char* name, size_t name_len, int* mode_count) {
    if (idx < 0 || idx >= static_cast<int>(d->ports.size())) {
        if (name_len > 0) name[0] = '\0';
        *mode_count = 0;
        return;
    }
    const auto& p = d->ports[idx];
    strncpy(name, p.display_name.c_str(), name_len - 1);
    name[name_len - 1] = '\0';
    *mode_count = static_cast<int>(p.modes.size());
}


void decklink_output_discovery_get_mode(DecklinkOutputDiscovery* d, int idx, int mode_idx, char* mode_name, size_t mode_name_len, uint32_t* mode_id, int* w, int* h, double* fps) {
    if (mode_name_len > 0) mode_name[0] = '\0';
    *mode_id = 0;
    *w = 0;
    *h = 0;
    *fps = 0.0;
    if (idx < 0 || idx >= static_cast<int>(d->ports.size())) return;
    const auto& port = d->ports[idx];
    if (mode_idx < 0 || mode_idx >= static_cast<int>(port.modes.size())) return;
    const auto& m = port.modes[mode_idx];
    strncpy(mode_name, m.name.c_str(), mode_name_len - 1);
    mode_name[mode_name_len - 1] = '\0';
    *mode_id = static_cast<uint32_t>(m.mode);
    *w = static_cast<int>(m.w);
    *h = static_cast<int>(m.h);
    *fps = m.fps;
}



/* ── forward declarations ──────────────────────────────────────────────────── */

struct DecklinkOutput {
    std::string target_name;
    IDeckLinkOutput* output = nullptr;
    BMDDisplayMode display_mode = bmdModeUnknown;
    long width = 0;
    long height = 0;
    BMDTimeValue frame_duration = 0;
    BMDTimeValue time_scale = 0;
    BMDTimeValue scheduled_time = 0;
};


class DecklinkOutputBuffer : public IDeckLinkVideoBuffer {

    public:
        DecklinkOutputBuffer(const uint8_t* data, size_t size)
            : ref_count_(1), data_(data, data + size) {}

        HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
            if (!ppv) return E_INVALIDARG;

    #ifdef __APPLE__
            CFUUIDBytes iunknown = CFUUIDGetUUIDBytes(IUnknownUUID);
            if (memcmp(&iid, &iunknown, sizeof(REFIID)) == 0 ||
                memcmp(&iid, &IID_IDeckLinkVideoBuffer, sizeof(REFIID)) == 0) {
                *ppv = static_cast<IDeckLinkVideoBuffer*>(this);
                AddRef();
                return S_OK;
            }
    #else
            if (memcmp(&iid, &IID_IDeckLinkVideoBuffer, sizeof(REFIID)) == 0) {
                *ppv = static_cast<IDeckLinkVideoBuffer*>(this);
                AddRef();
                return S_OK;
            }
    #endif
            
            *ppv = nullptr;
            return E_NOINTERFACE;
        }

        ULONG STDMETHODCALLTYPE AddRef(void) override {
            return ++ref_count_;
        }

        ULONG STDMETHODCALLTYPE Release(void) override {
            ULONG count = --ref_count_;
            if (count == 0) delete this;
            return count;
        }

        HRESULT GetBytes(void** buffer) override {
            if (!buffer) return E_INVALIDARG;
            *buffer = data_.data();
            return S_OK;
        }

        HRESULT GetSize(uint64_t* size) override {
            if (!size) return E_INVALIDARG;
            *size = static_cast<uint64_t>(data_.size());
            return S_OK;
        }

        HRESULT StartAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }
        HRESULT EndAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }

    private:
        std::atomic<ULONG> ref_count_;
        std::vector<uint8_t> data_;
};


/* ── Output runtime ──────────────────────────────────────────────────── */

DecklinkOutput* decklink_output_new(const char* display_name) {
    auto* o = new DecklinkOutput();
    o->target_name = display_name ? display_name : "";
    return o;
}


void decklink_output_free(DecklinkOutput* o) {
    if (!o) return;
    decklink_output_stop(o);
    delete o;
}


bool decklink_output_start(DecklinkOutput* o, uint32_t mode_id) {
    if (!o || o->target_name.empty()) return false;
    if (o->output) return true;

    IDeckLinkIterator* iterator = CreateDeckLinkIteratorInstance();
    if (!iterator) return false;

    IDeckLink* decklink = nullptr;
    bool found = false;
    while (iterator->Next(&decklink) == S_OK) {
        std::string model = get_model_name(decklink);

        int64_t num_sub = 1;
        int64_t sub_index = 0;
        IDeckLinkProfileAttributes* attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            attr->GetInt(BMDDeckLinkNumberOfSubDevices, &num_sub);
            attr->GetInt(BMDDeckLinkSubDeviceIndex, &sub_index);
            attr->Release();
        }

        std::string name = (num_sub > 1)
            ? model + " - Output " + std::to_string(static_cast<int>(sub_index) + 1)
            : model;

        if (name == o->target_name) {
            found = true;
            break;
        }
        decklink->Release();
    }
    iterator->Release();

    if (!found) {
        fprintf(stderr, "[decklink] output device '%s' not found\n", o->target_name.c_str());
        return false;
    }

    if (decklink->QueryInterface(IID_IDeckLinkOutput, (void**)&o->output) != S_OK) {
        fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkOutput failed\n");
        decklink->Release();
        return false;
    }
    decklink->Release();

    BMDDisplayMode mode = static_cast<BMDDisplayMode>(mode_id);
    HRESULT hr = o->output->EnableVideoOutput(mode, bmdVideoOutputFlagDefault);
    if (hr != S_OK) {
        fprintf(stderr, "[decklink] EnableVideoOutput failed (0x%08X)\n", static_cast<unsigned int>(hr));
        o->output->Release();
        o->output = nullptr;
        return false;
    }

    IDeckLinkDisplayMode* display_mode = nullptr;
    if (o->output->GetDisplayMode(mode, &display_mode) == S_OK && display_mode) {
        o->width = display_mode->GetWidth();
        o->height = display_mode->GetHeight();
        BMDTimeValue fd = 0;
        BMDTimeValue ts = 0;
        if (display_mode->GetFrameRate(&fd, &ts) == S_OK && fd > 0) {
            o->frame_duration = fd;
            o->time_scale = ts;
        }
        display_mode->Release();
    }

    if (o->time_scale == 0) {
        o->time_scale = 1;
    }

    hr = o->output->StartScheduledPlayback(0, o->time_scale, 1.0);
    if (hr != S_OK) {
        fprintf(stderr, "[decklink] StartScheduledPlayback failed (0x%08X)\n", static_cast<unsigned int>(hr));
        o->output->DisableVideoOutput();
        o->output->Release();
        o->output = nullptr;
        return false;
    }

    fprintf(stderr, "[decklink] output started '%s' mode=%d %ldx%ld\n",
            o->target_name.c_str(), static_cast<int>(mode), o->width, o->height);
    return true;
}


void decklink_output_stop(DecklinkOutput* o) {
    if (!o || !o->output) return;
    o->output->StopScheduledPlayback(0, nullptr, o->time_scale);
    o->output->DisableVideoOutput();
    o->output->Release();
    o->output = nullptr;
    o->scheduled_time = 0;
}


bool decklink_output_present_frame(DecklinkOutput* o, const uint8_t* bgra, int width, int height, int row_bytes) {
    if (!o || !o->output) return false;
    if (width <= 0 || height <= 0 || !bgra) return false;

    size_t src_pitch = static_cast<size_t>(width) * 4;
    size_t expected = src_pitch * static_cast<size_t>(height);
    if (row_bytes < 0 || static_cast<size_t>(row_bytes) < src_pitch) return false;

    auto* buffer = new DecklinkOutputBuffer(bgra, expected);

    IDeckLinkMutableVideoFrame* frame = nullptr;
    HRESULT hr = o->output->CreateVideoFrameWithBuffer(width, height, row_bytes, bmdFormat8BitBGRA, bmdFrameFlagDefault, buffer, &frame);
    buffer->Release();
    if (hr != S_OK || !frame) {
        fprintf(stderr, "[decklink] CreateVideoFrameWithBuffer failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }

    hr = o->output->ScheduleVideoFrame(frame, o->scheduled_time, o->frame_duration, o->time_scale);
    frame->Release();
    if (hr != S_OK) {
        fprintf(stderr, "[decklink] ScheduleVideoFrame failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }

    o->scheduled_time += o->frame_duration;
    return true;
}
