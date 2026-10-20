#include <vector>
#include <string>
#include <mutex>
#include <atomic>
#include <cstring>
#include <algorithm>

#ifdef __APPLE__
#include <CoreFoundation/CoreFoundation.h>
#include <CoreVideo/CoreVideo.h>
#endif

#include <DeckLinkAPI.h>
#include "decklink_output.h"
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

            if (DecklinkIsIUnknownIID(iid) ||
                DecklinkIIDEqual(iid, IID_IDeckLinkVideoBuffer)) {
                *ppv = static_cast<IDeckLinkVideoBuffer*>(this);
                AddRef();
                return S_OK;
            }

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
    DecklinkComScope com_scope;
    if (!o) return false;
    if (o->output) return true;

    IDeckLinkIterator* iterator = nullptr;
    if (FAILED(DecklinkCreateIterator(&iterator))) {
        return false;
    }

    // An empty target name means the first device that can play back.
    const bool first_available = o->target_name.empty();
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

        if (first_available || name == o->target_name) {
            HRESULT hr = decklink->QueryInterface(IID_IDeckLinkOutput, (void**)&o->output);
            decklink->Release();
            if (hr == S_OK) {
                found = true;
                break;
            }
            fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkOutput failed\n");
            continue;
        }
        decklink->Release();
    }
    iterator->Release();

    if (!found) {
        if (first_available) {
            fprintf(stderr, "[decklink] no output device available\n");
        } else {
            fprintf(stderr, "[decklink] output device '%s' not found\n", o->target_name.c_str());
        }
        return false;
    }

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
