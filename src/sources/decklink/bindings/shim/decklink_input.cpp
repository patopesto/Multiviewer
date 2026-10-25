#include <vector>
#include <string>
#include <mutex>
#include <condition_variable>
#include <chrono>
#include <atomic>
#include <cstring>
#include <algorithm>

#include "decklink_input.h"
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



/* ── forward declarations ──────────────────────────────────────────── */

class CaptureCallback;

struct DecklinkSource {
    std::string target_name;
    uint32_t connection = 0;
    IDeckLinkInput* input = nullptr;
    IDeckLinkConfiguration* config = nullptr;
    CaptureCallback* callback = nullptr;
    CapturePlatform* platform = nullptr;
    std::atomic<bool> gpu_mode{false};
    uint32_t pixel_format = 0;
    BMDDisplayMode display_mode = bmdModeUnknown;
};

/* ── COM callback ────────────────────────────────────────────────────── */

class CaptureCallback : public IDeckLinkInputCallback {
    public:
        explicit CaptureCallback(DecklinkSource* source)
            : ref_count_(1), stopping_(false), source_(source) {}

        ~CaptureCallback() override {
            if (cv_frame_) {
                capture_platform_release_cv(cv_frame_);
            }
        }

        HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
            if (!ppv) return E_INVALIDARG;
            if (DecklinkIsIUnknownIID(iid) ||
                DecklinkIIDEqual(iid, IID_IDeckLinkInputCallback)) {
                *ppv = static_cast<IDeckLinkInputCallback*>(this);
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
            if (count == 0) {
                delete this;
            }
            return count;
        }

        HRESULT STDMETHODCALLTYPE VideoInputFormatChanged(
            BMDVideoInputFormatChangedEvents /*notificationEvents*/,
            IDeckLinkDisplayMode* newDisplayMode,
            BMDDetectedVideoInputFormatFlags /*detectedSignalFlags*/) override
        {
            if (!source_ || !newDisplayMode) return S_OK;
            if (stopping_.load()) return S_OK;

            IDeckLinkInput* input = source_->input;
            if (!input) return S_OK;

            BMDDisplayMode newMode = newDisplayMode->GetDisplayMode();

            if (newMode == last_mode_) return S_OK;

            long w = newDisplayMode->GetWidth();
            long h = newDisplayMode->GetHeight();
            fprintf(stderr, "[decklink] VideoInputFormatChanged %ldx%ld mode=%d\n", w, h, static_cast<int>(newMode));

            BMDTimeValue frame_duration = 0;
            BMDTimeValue time_scale = 0;
            if (newDisplayMode->GetFrameRate(&frame_duration, &time_scale) == S_OK && frame_duration > 0) {
                nominal_fps_ = static_cast<double>(time_scale) / static_cast<double>(frame_duration);
            }

            // YUV is the card's reliable native capture format. Requesting
            // BGRA can return S_OK from StartStreams yet never deliver frames
            // (notably on the Recorder 3G after an Hp60 -> Hp30 change).
            const BMDPixelFormat pixelFormat = bmdFormat8BitYUV;

            auto enable_and_start = [&](BMDPixelFormat fmt) -> bool {
                input->PauseStreams();
                HRESULT r = capture_platform_enable(source_->platform, input, newMode, fmt, bmdVideoInputEnableFormatDetection);
                if (r != S_OK) {
                    fprintf(stderr, "[decklink] EnableVideoInput(0x%08X) failed after format change (0x%08X)\n",
                            static_cast<unsigned int>(fmt), static_cast<unsigned int>(r));
                    return false;
                }
                input->FlushStreams();
                if (input->StartStreams() != S_OK) {
                    fprintf(stderr, "[decklink] StartStreams failed after format change (fmt=0x%08X)\n",
                            static_cast<unsigned int>(fmt));
                    return false;
                }
                return true;
            };

            bool started = enable_and_start(pixelFormat);
            if (started) {
                last_mode_ = newMode;
                if (source_) {
                    source_->display_mode = newMode;
                    source_->pixel_format = static_cast<uint32_t>(pixelFormat);
                }
                fprintf(stderr, "[decklink] format change locked %ldx%ld fmt=YUV\n", w, h);
            } else {
                fprintf(stderr, "[decklink] format change could not restart streams\n");
            }
            return S_OK;
        }

        HRESULT STDMETHODCALLTYPE VideoInputFrameArrived(IDeckLinkVideoInputFrame* videoFrame,IDeckLinkAudioInputPacket* /*audioPacket*/) override {
            if (!videoFrame) return S_OK;
            if (stopping_.load()) return S_OK;
            if (!source_) return S_OK;

            long w = videoFrame->GetWidth();
            long h = videoFrame->GetHeight();
            BMDPixelFormat fmt = videoFrame->GetPixelFormat();
            BMDFrameFlags flags = videoFrame->GetFlags();

            if (w <= 0 || h <= 0) return S_OK;

            bool has_signal = !(flags & bmdFrameHasNoInputSource);

            if (!has_signal) {
                if (had_signal_) {
                    had_signal_ = false;
                    fprintf(stderr, "[decklink] signal lost (%ldx%ld fmt=0x%08X)\n", w, h, fmt);
                }
                return S_OK;
            }

            if (!had_signal_) {
                had_signal_ = true;
                fprintf(stderr, "[decklink] signal acquired %ldx%ld fmt=0x%08X flags=0x%08X\n", w, h, fmt, flags);
            }

            size_t frame_size = 0;
            if (fmt == bmdFormat8BitBGRA) {
                frame_size = static_cast<size_t>(w) * static_cast<size_t>(h) * 4;
            } else if (fmt == bmdFormat8BitYUV) {
                frame_size = static_cast<size_t>(w) * static_cast<size_t>(h) * 2;
            } else {
                fprintf(stderr, "[decklink] unsupported pixel format 0x%08X\n", fmt);
                return S_OK;
            }

            bool gpu = source_ && source_->gpu_mode.load();
            PlatformFrame frame;
            if (!capture_platform_open(source_->platform, videoFrame, gpu, &frame)) {
                return S_OK;
            }

            if (frame.index >= 0) {
                std::lock_guard<std::mutex> lock(mutex_);
                pinned_index_ = frame.index;
                width_ = w;
                height_ = h;
                format_ = fmt;
                frame_size_ = frame_size;
                seq_++;
                generation_++;
                frame_cv_.notify_all();
                return S_OK;
            }

            if (frame.gpu) {
                std::lock_guard<std::mutex> lock(mutex_);
                if (cv_frame_) {
                    capture_platform_release_cv(cv_frame_);
                }
                cv_frame_ = frame.cv;
                width_ = w;
                height_ = h;
                format_ = fmt;
                frame_size_ = frame_size;
                seq_++;
                generation_++;
                frame_cv_.notify_all();
                if (!cv_logged_) {
                    cv_logged_ = true;
                    fprintf(stderr, "[decklink] first CV frame %ldx%ld seq=%llu\n",
                            w, h, static_cast<unsigned long long>(seq_));
                }
                return S_OK;
            }

            int back = back_idx_.load(std::memory_order_relaxed);
            if (buffer_[back].size() < frame_size) {
                buffer_[back].resize(frame_size);
            }
            uint8_t* dst = buffer_[back].data();
            const uint8_t* src = frame.data;
            long row_bytes = frame.row_bytes;

            if (fmt == bmdFormat8BitBGRA) {
                if (row_bytes == w * 4) {
                    memcpy(dst, src, frame_size);
                } else {
                    for (long y = 0; y < h; ++y) {
                        memcpy(dst + y * w * 4, src + y * row_bytes, w * 4);
                    }
                }
            } else if (fmt == bmdFormat8BitYUV) {
                if (row_bytes == w * 2) {
                    memcpy(dst, src, frame_size);
                } else {
                    for (long y = 0; y < h; ++y) {
                        memcpy(dst + y * w * 2, src + y * row_bytes, w * 2);
                    }
                }
            }

            capture_platform_close(source_->platform, &frame);

            std::lock_guard<std::mutex> lock(mutex_);
            width_ = w;
            height_ = h;
            format_ = fmt;
            frame_size_ = frame_size;
            back_idx_.store(1 - back, std::memory_order_relaxed);
            seq_++;
            generation_++;
            frame_cv_.notify_all();
            if (gpu) {
                cpu_fallback_.store(true);
            }
            return S_OK;
        }

    public:
        bool poll(uint8_t* out, size_t out_size, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out) {
            std::lock_guard<std::mutex> lock(mutex_);
            if (frame_size_ == 0) return false;
            if (out_size < frame_size_) return false;
            int front = 1 - back_idx_.load(std::memory_order_relaxed);
            memcpy(out, buffer_[front].data(), frame_size_);
            *w = width_;
            *h = height_;
            *seq = seq_;
            *nominal_fps_out = nominal_fps_;
            *fmt_out = static_cast<uint32_t>(format_);
            return true;
        }

    private:
        std::atomic<ULONG> ref_count_;
        std::atomic<bool> stopping_;
        std::mutex mutex_;
        std::vector<uint8_t> buffer_[2];
        std::atomic<int> back_idx_{0};
        size_t frame_size_ = 0;
        int width_ = 0;
        int height_ = 0;
        BMDPixelFormat format_ = bmdFormat8BitBGRA;
        uint64_t seq_ = 0;
        double nominal_fps_ = 0.0;
        DecklinkSource* source_ = nullptr;
        BMDDisplayMode last_mode_ = bmdModeUnknown;
        bool had_signal_ = false;
        bool cv_logged_ = false;
        void* cv_frame_ = nullptr;
        std::atomic<bool> cpu_fallback_{false};
        std::condition_variable frame_cv_;
        uint64_t generation_ = 0;
        int pinned_index_ = -1;

    public:
        void stop() { stopping_.store(true); }
        void detach() { source_ = nullptr; }
        void set_nominal_fps(double fps) { nominal_fps_ = fps; }

        bool poll_cv(void** out, int* w, int* h, uint64_t* seq, uint32_t* fmt, double* fps) {
            std::lock_guard<std::mutex> lock(mutex_);
            if (!cv_frame_) return false;
            *out = cv_frame_;
            cv_frame_ = nullptr;
            *w = width_;
            *h = height_;
            *seq = seq_;
            *fmt = static_cast<uint32_t>(format_);
            *fps = nominal_fps_;
            return true;
        }

        bool take_fallback() { return cpu_fallback_.exchange(false); }

        uint64_t generation() {
            std::lock_guard<std::mutex> lock(mutex_);
            return generation_;
        }

        void wait_frame(uint64_t seen, int timeout_ms) {
            std::unique_lock<std::mutex> lock(mutex_);
            frame_cv_.wait_for(lock, std::chrono::milliseconds(timeout_ms), [&] {
                return generation_ != seen || stopping_.load();
            });
        }

        bool poll_pinned(int* index, int* w, int* h, uint64_t* seq, uint32_t* fmt, double* fps) {
            std::lock_guard<std::mutex> lock(mutex_);
            if (pinned_index_ < 0) return false;
            *index = pinned_index_;
            pinned_index_ = -1;
            *w = width_;
            *h = height_;
            *seq = seq_;
            *fmt = static_cast<uint32_t>(format_);
            *fps = nominal_fps_;
            return true;
        }

        void frame_info(uint32_t* w, uint32_t* h, uint32_t* fmt) {
            std::lock_guard<std::mutex> lock(mutex_);
            *w = static_cast<uint32_t>(width_);
            *h = static_cast<uint32_t>(height_);
            *fmt = static_cast<uint32_t>(format_);
        }

        void drop_cv() {
            std::lock_guard<std::mutex> lock(mutex_);
            if (cv_frame_) {
                capture_platform_release_cv(cv_frame_);
                cv_frame_ = nullptr;
            }
            // The next CPU frame must republish; the buffer still holds an
            // older sequence and would otherwise be served as the current one.
            frame_size_ = 0;
        }
};


/* ── Source ──────────────────────────────────────────────────────────── */

DecklinkSource* decklink_source_new(const char* display_name) {
    auto* s = new DecklinkSource();
    s->target_name = display_name ? display_name : "";
    s->platform = capture_platform_new();
    return s;
}


void decklink_source_free(DecklinkSource* s) {
    if (!s) return;
    decklink_source_stop(s);
    capture_platform_free(s->platform);
    delete s;
}


void decklink_source_set_connection(DecklinkSource* s, uint32_t connection) {
    if (!s) return;
    s->connection = connection;
}


bool decklink_source_start(DecklinkSource* s) {
    DecklinkComScope com_scope;
    if (!s || s->target_name.empty()) return false;

    {
        char api[64] = {0};
        if (decklink_api_version(api, sizeof(api)) > 0) {
            fprintf(stderr, "[decklink] input using API %s\n", api);
        }
    }

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
            ? model + " - Input " + std::to_string(static_cast<int>(sub_index) + 1)
            : model;

        if (name == s->target_name) {
            found = true;
            break;
        }
        decklink->Release();
    }
    iterator->Release();

    if (!found) {
        fprintf(stderr, "[decklink] device '%s' not found\n", s->target_name.c_str());
        return false;
    }

    DecklinkBool supports_fmt_detection = 0;
    IDeckLinkProfileAttributes* attr = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
        attr->GetFlag(BMDDeckLinkSupportsInputFormatDetection, &supports_fmt_detection);
        attr->Release();
    }

    // Set input connection BEFORE getting IDeckLinkInput
    if (s->connection != 0) {
        if (decklink->QueryInterface(IID_IDeckLinkConfiguration, (void**)&s->config) != S_OK) {
            fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkConfiguration failed\n");
            decklink->Release();
            return false;
        }
        HRESULT hr = s->config->SetInt(bmdDeckLinkConfigVideoInputConnection, s->connection);
        if (hr != S_OK) {
            fprintf(stderr, "[decklink] SetInt(connection=0x%X) failed: 0x%08X\n", s->connection, static_cast<unsigned int>(hr));
            s->config->Release();
            s->config = nullptr;
            decklink->Release();
            return false;
        }
        fprintf(stderr, "[decklink] set connection=0x%X ok\n", s->connection);
        DecklinkSleepMs(100);
    }

    if (decklink->QueryInterface(IID_IDeckLinkInput, (void**)&s->input) != S_OK) {
        fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkInput failed\n");
        decklink->Release();
        return false;
    }
    decklink->Release();

    s->callback = new CaptureCallback(s);
    if (s->input->SetCallback(s->callback) != S_OK) {
        s->callback->Release();
        s->callback = nullptr;
        s->input->Release();
        s->input = nullptr;
        return false;
    }

    const BMDPixelFormat first_format = bmdFormat8BitYUV;
    const BMDPixelFormat second_format = bmdFormat8BitBGRA;
    const char* first_name = "YUV";
    const char* second_name = "BGRA";

    BMDDisplayMode mode = bmdModeUnknown;
    BMDPixelFormat pixelFormat = first_format;
    HRESULT hr = E_FAIL;

    auto try_enable = [&](BMDDisplayMode m) -> HRESULT {
        HRESULT r = capture_platform_enable(s->platform, s->input, m, first_format, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying mode=%d %s detection => 0x%08X\n",
                static_cast<int>(m), first_name, static_cast<unsigned int>(r));
        if (r == S_OK) {
            pixelFormat = first_format;
            return r;
        }
        r = capture_platform_enable(s->platform, s->input, m, second_format, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying mode=%d %s detection => 0x%08X\n",
                static_cast<int>(m), second_name, static_cast<unsigned int>(r));
        if (r == S_OK) {
            pixelFormat = second_format;
        }
        return r;
    };

    if (supports_fmt_detection) {
        mode = bmdModeUnknown;
        hr = try_enable(mode);
    }

    if (hr != S_OK) {
        mode = bmdModeHD1080p30;
        hr = try_enable(mode);
    }

    if (hr != S_OK) {
        fprintf(stderr, "[decklink] EnableVideoInput failed (0x%08X)\n", static_cast<unsigned int>(hr));
        s->input->SetCallback(nullptr);
        s->callback->Release();
        s->callback = nullptr;
        s->input->Release();
        s->input = nullptr;
        return false;
    }

    s->pixel_format = static_cast<uint32_t>(pixelFormat);
    s->display_mode = mode;

    if (mode != bmdModeUnknown) {
        IDeckLinkDisplayMode* display_mode = nullptr;
        if (s->input->GetDisplayMode(mode, &display_mode) == S_OK && display_mode) {
            BMDTimeValue frame_duration = 0;
            BMDTimeValue time_scale = 0;
            if (display_mode->GetFrameRate(&frame_duration, &time_scale) == S_OK && frame_duration > 0) {
                s->callback->set_nominal_fps(static_cast<double>(time_scale) / static_cast<double>(frame_duration));
            }
            display_mode->Release();
        }
    }

    if (s->input->StartStreams() != S_OK) {
        fprintf(stderr, "[decklink] StartStreams failed\n");
        s->input->StopStreams();
        s->input->SetCallback(nullptr);
        s->input->DisableVideoInput();
        s->callback->Release();
        s->callback = nullptr;
        s->input->Release();
        s->input = nullptr;
        return false;
    }

    fprintf(stderr, "[decklink] started '%s' mode=%d fmt=%s detection=%s\n",
            s->target_name.c_str(), static_cast<int>(mode),
            pixelFormat == bmdFormat8BitBGRA ? "BGRA" : "YUV",
            supports_fmt_detection ? "on" : "off");
    return true;
}


void decklink_source_stop(DecklinkSource* s) {
    if (!s) return;
    if (s->callback) {
        s->callback->stop();
    }
    if (s->input) {
        s->input->StopStreams();
        s->input->FlushStreams();
        s->input->SetCallback(nullptr);
        s->input->DisableVideoInput();
        s->input->DisableAudioInput();
        DecklinkSleepMs(500);
    }
    if (s->callback) {
        s->callback->drop_cv();
        s->callback->detach();
        s->callback->Release();
        s->callback = nullptr;
    }
    if (s->config) {
        s->config->Release();
        s->config = nullptr;
    }
    if (s->input) {
        s->input->Release();
        s->input = nullptr;
    }
}


bool decklink_source_poll_frame(DecklinkSource* s, uint8_t* out_rgba, size_t out_size, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out) {
    if (!s || !s->callback) return false;
    return s->callback->poll(out_rgba, out_size, w, h, seq, fmt_out, nominal_fps_out);
}


void decklink_source_set_gpu_mode(DecklinkSource* s, bool enable) {
    if (!s) return;
    s->gpu_mode.store(enable);
    if (!enable && s->callback) {
        s->callback->drop_cv();
    }
}


bool decklink_source_is_bgra(DecklinkSource* s) {
    return s && s->pixel_format == static_cast<uint32_t>(bmdFormat8BitBGRA);
}


bool decklink_source_poll_cv_frame(DecklinkSource* s, void** cv_out, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out) {
    if (!s || !s->callback) return false;
    return s->callback->poll_cv(cv_out, w, h, seq, fmt_out, nominal_fps_out);
}


bool decklink_source_take_cpu_fallback(DecklinkSource* s) {
    if (!s || !s->callback) return false;
    return s->callback->take_fallback();
}


uint64_t decklink_source_frame_generation(DecklinkSource* s) {
    if (!s || !s->callback) return 0;
    return s->callback->generation();
}


void decklink_source_wait_frame(DecklinkSource* s, uint64_t seen, int timeout_ms) {
    if (!s || !s->callback) return;
    s->callback->wait_frame(seen, timeout_ms);
}


bool decklink_source_set_capture_buffers(DecklinkSource* s, void** pointers, const uint32_t* sizes, uint32_t count) {
    if (!s || !s->platform) return false;
    return capture_platform_set_buffers(s->platform, pointers, sizes, count);
}


bool decklink_source_pinned_available(DecklinkSource* s) {
    if (!s || !s->platform) return false;
    return capture_platform_pinned_available(s->platform);
}


bool decklink_source_enable_pinned(DecklinkSource* s) {
    if (!s || !s->input || !s->callback) return false;
    if (!capture_platform_pinned_available(s->platform)) return false;
    if (s->display_mode == bmdModeUnknown || s->pixel_format == 0) return false;

    const BMDPixelFormat fmt = static_cast<BMDPixelFormat>(s->pixel_format);
    s->input->PauseStreams();
    HRESULT hr = capture_platform_enable(s->platform, s->input, s->display_mode, fmt, bmdVideoInputEnableFormatDetection);
    if (hr == S_OK) {
        s->input->FlushStreams();
        if (s->input->StartStreams() == S_OK) {
            fprintf(stderr, "[decklink] capture switched to caller buffers\n");
            return true;
        }
    }

    fprintf(stderr, "[decklink] caller-buffer capture failed (0x%08X); reverting\n", static_cast<unsigned int>(hr));
    capture_platform_clear_buffers(s->platform);
    s->input->PauseStreams();
    s->input->EnableVideoInput(s->display_mode, fmt, bmdVideoInputEnableFormatDetection);
    s->input->FlushStreams();
    s->input->StartStreams();
    return false;
}


bool decklink_source_disable_pinned(DecklinkSource* s) {
    if (!s || !s->input) return false;
    capture_platform_clear_buffers(s->platform);
    if (s->display_mode == bmdModeUnknown || s->pixel_format == 0) return false;
    const BMDPixelFormat fmt = static_cast<BMDPixelFormat>(s->pixel_format);
    s->input->PauseStreams();
    HRESULT hr = s->input->EnableVideoInput(s->display_mode, fmt, bmdVideoInputEnableFormatDetection);
    s->input->FlushStreams();
    if (hr == S_OK && s->input->StartStreams() == S_OK) {
        return true;
    }
    return false;
}


bool decklink_source_poll_pinned_frame(DecklinkSource* s, int* index, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out) {
    if (!s || !s->callback) return false;
    return s->callback->poll_pinned(index, w, h, seq, fmt_out, nominal_fps_out);
}


void decklink_source_return_buffer(DecklinkSource* s, int index) {
    if (!s || !s->platform) return;
    capture_platform_return_buffer(s->platform, index);
}


void decklink_source_frame_info(DecklinkSource* s, uint32_t* w, uint32_t* h, uint32_t* fmt_out) {
    if (!s || !s->callback) return;
    s->callback->frame_info(w, h, fmt_out);
}


void decklink_cv_frame_release(void* cv) {
    capture_platform_release_cv(cv);
}
