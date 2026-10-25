#include <deque>
#include <memory>
#include <string>
#include <mutex>
#include <atomic>
#include <cstring>
#include <algorithm>
#include <unordered_map>

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

struct OutputCompletionState;
class OutputCallback;

struct DecklinkOutput {
    std::string target_name;
    IDeckLinkOutput* output = nullptr;
    BMDDisplayMode display_mode = bmdModeUnknown;
    long width = 0;
    long height = 0;
    BMDTimeValue frame_duration = 0;
    BMDTimeValue time_scale = 0;
    BMDTimeValue scheduled_time = 0;
    std::shared_ptr<OutputCompletionState> completion;
    OutputCallback* callback = nullptr;
    bool first_release_logged = false;
    bool caps_logged = false;
};


/* Borrowed frames publish their token when the device drops the video frame,
** which drops the buffer's last reference; that also covers frames cancelled
** by stop. CV frames get theirs from ScheduledFrameCompleted instead, so the
** two token sources are disjoint. */
struct OutputCompletionState {
    std::mutex mutex;
    std::deque<uint64_t> completed;
    std::unordered_map<IDeckLinkVideoFrame*, uint64_t> scheduled;
};


class OutputCallback : public IDeckLinkVideoOutputCallback {
    public:
        explicit OutputCallback(std::shared_ptr<OutputCompletionState> state)
            : ref_count_(1), state_(state) {}

        HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
            if (!ppv) return E_INVALIDARG;
            if (DecklinkIsIUnknownIID(iid) ||
                DecklinkIIDEqual(iid, IID_IDeckLinkVideoOutputCallback)) {
                *ppv = static_cast<IDeckLinkVideoOutputCallback*>(this);
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

        HRESULT STDMETHODCALLTYPE ScheduledFrameCompleted(IDeckLinkVideoFrame* completedFrame, BMDOutputFrameCompletionResult /*result*/) override {
            if (!completedFrame || !state_) return S_OK;
            std::lock_guard<std::mutex> lock(state_->mutex);
            auto it = state_->scheduled.find(completedFrame);
            if (it != state_->scheduled.end()) {
                state_->completed.push_back(it->second);
                state_->scheduled.erase(it);
            }
            return S_OK;
        }

        HRESULT STDMETHODCALLTYPE ScheduledPlaybackHasStopped(void) override {
            return S_OK;
        }

    private:
        std::atomic<ULONG> ref_count_;
        std::shared_ptr<OutputCompletionState> state_;
};


class DecklinkOutputBuffer : public IDeckLinkVideoBuffer {

    public:
        DecklinkOutputBuffer(const uint8_t* data, size_t size, uint64_t token,
                             std::shared_ptr<OutputCompletionState> completion)
            : ref_count_(1), data_(data), size_(size), token_(token),
              completion_(completion) {}

        ~DecklinkOutputBuffer() {
            if (completion_) {
                std::lock_guard<std::mutex> lock(completion_->mutex);
                completion_->completed.push_back(token_);
            }
        }

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
            *buffer = const_cast<uint8_t*>(data_);
            return S_OK;
        }

        HRESULT GetSize(uint64_t* size) override {
            if (!size) return E_INVALIDARG;
            *size = static_cast<uint64_t>(size_);
            return S_OK;
        }

        HRESULT StartAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }
        HRESULT EndAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }

    private:
        std::atomic<ULONG> ref_count_;
        const uint8_t* data_;
        size_t size_;
        uint64_t token_;
        std::shared_ptr<OutputCompletionState> completion_;
};


/* ── Output runtime ──────────────────────────────────────────────────── */

DecklinkOutput* decklink_output_new(const char* display_name) {
    auto* o = new DecklinkOutput();
    o->target_name = display_name ? display_name : "";
    o->completion = std::make_shared<OutputCompletionState>();
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
    if (o->target_name.empty()) {
        fprintf(stderr, "[decklink] no output device selected\n");
        return false;
    }
    if (o->output) return true;

    IDeckLinkIterator* iterator = nullptr;
    if (FAILED(DecklinkCreateIterator(&iterator))) {
        return false;
    }

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
        fprintf(stderr, "[decklink] output device '%s' not found\n", o->target_name.c_str());
        return false;
    }

    if (!o->caps_logged) {
        o->caps_logged = true;
        char api[64] = {0};
        if (decklink_api_version(api, sizeof(api)) > 0) {
            fprintf(stderr, "[decklink] output using API %s\n", api);
        }
        playback_platform_log_caps(o->output);
    }

    BMDDisplayMode mode = static_cast<BMDDisplayMode>(mode_id);
    HRESULT hr = o->output->EnableVideoOutput(mode, bmdVideoOutputFlagDefault);
    if (hr != S_OK) {
        fprintf(stderr, "[decklink] EnableVideoOutput failed (0x%08X)\n", static_cast<unsigned int>(hr));
        o->output->Release();
        o->output = nullptr;
        return false;
    }

    o->callback = new OutputCallback(o->completion);
    if (o->output->SetScheduledFrameCompletionCallback(o->callback) != S_OK) {
        fprintf(stderr, "[decklink] SetScheduledFrameCompletionCallback failed; CV output disabled\n");
        o->callback->Release();
        o->callback = nullptr;
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

    fprintf(stderr, "[decklink] output started '%s' mode=%d %ldx%ld frame=%lld/%lld\n",
            o->target_name.c_str(), static_cast<int>(mode), o->width, o->height,
            static_cast<long long>(o->frame_duration), static_cast<long long>(o->time_scale));
    return true;
}


void decklink_output_stop(DecklinkOutput* o) {
    if (!o || !o->output) return;
    o->output->StopScheduledPlayback(0, nullptr, o->time_scale);
    o->output->SetScheduledFrameCompletionCallback(nullptr);
    o->output->DisableVideoOutput();
    o->output->Release();
    o->output = nullptr;
    o->scheduled_time = 0;
    if (o->callback) {
        o->callback->Release();
        o->callback = nullptr;
    }
    if (o->completion) {
        std::lock_guard<std::mutex> lock(o->completion->mutex);
        o->completion->scheduled.clear();
    }
}


bool decklink_output_present_frame(DecklinkOutput* o, const uint8_t* bgra, int width, int height, int row_bytes, uint64_t token, bool* token_queued) {
    if (token_queued) *token_queued = false;
    if (!o || !o->output) return false;
    if (width <= 0 || height <= 0 || !bgra) return false;

    size_t src_pitch = static_cast<size_t>(width) * 4;
    if (row_bytes < 0 || static_cast<size_t>(row_bytes) < src_pitch) return false;
    size_t size = static_cast<size_t>(row_bytes) * static_cast<size_t>(height);

    auto* buffer = new DecklinkOutputBuffer(bgra, size, token, o->completion);
    if (token_queued) *token_queued = true;

    IDeckLinkMutableVideoFrame* frame = nullptr;
    HRESULT hr = o->output->CreateVideoFrameWithBuffer(width, height, row_bytes, bmdFormat8BitBGRA, bmdFrameFlagDefault, buffer, &frame);
    buffer->Release();
    if (hr != S_OK || !frame) {
        fprintf(stderr, "[decklink] CreateVideoFrameWithBuffer failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }

    hr = o->output->ScheduleVideoFrame(frame, o->scheduled_time, o->frame_duration, o->time_scale);
    if (hr != S_OK) {
        frame->Release();
        fprintf(stderr, "[decklink] ScheduleVideoFrame failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }
    frame->Release();

    o->scheduled_time += o->frame_duration;
    return true;
}


bool decklink_output_present_cv(DecklinkOutput* o, void* cv, uint64_t token) {
    if (!o || !o->output || !cv) return false;
    if (!o->callback) return false;

    IDeckLinkMutableVideoFrame* frame = playback_platform_create_cv_frame(o->output, cv);
    if (!frame) return false;

    {
        std::lock_guard<std::mutex> lock(o->completion->mutex);
        o->completion->scheduled[frame] = token;
    }

    HRESULT hr = o->output->ScheduleVideoFrame(frame, o->scheduled_time, o->frame_duration, o->time_scale);
    if (hr != S_OK) {
        {
            std::lock_guard<std::mutex> lock(o->completion->mutex);
            o->completion->scheduled.erase(frame);
        }
        frame->Release();
        fprintf(stderr, "[decklink] ScheduleVideoFrame(CV) failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }
    frame->Release();

    o->scheduled_time += o->frame_duration;
    return true;
}


bool decklink_output_completion_available(DecklinkOutput* o) {
    return o && o->callback != nullptr;
}


bool decklink_output_take_completed(DecklinkOutput* o, uint64_t* token) {
    if (!o || !o->completion || !token) return false;
    std::lock_guard<std::mutex> lock(o->completion->mutex);
    if (o->completion->completed.empty()) return false;
    *token = o->completion->completed.front();
    o->completion->completed.pop_front();
    if (!o->first_release_logged) {
        o->first_release_logged = true;
        fprintf(stderr, "[decklink] first output frame released\n");
    }
    return true;
}
