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
#include "decklink_input.h"
#include "platform.h"



/* ── helpers ─────────────────────────────────────────────────────────── */

static std::string get_display_name(IDeckLink* decklink) {
    DECKLINK_DLSTRING_T name = nullptr;
    if (decklink->GetDisplayName(&name) != S_OK) {
        return "";
    }
    std::string result = DecklinkStringToStd(name);
    DecklinkStringFree(name);
    return result;
}

static std::string get_model_name(IDeckLink* decklink) {
    DECKLINK_DLSTRING_T name = nullptr;
    if (decklink->GetModelName(&name) != S_OK) {
        return "";
    }
    std::string result = DecklinkStringToStd(name);
    DecklinkStringFree(name);
    return result;
}



/* ── Discovery ───────────────────────────────────────────────────────── */

struct DiscoveredPort {
    std::string display_name;
    bool has_signal;
    uint32_t connections;
};

struct DecklinkSourceDiscovery {
    std::vector<DiscoveredPort> ports;
};


DecklinkSourceDiscovery* decklink_source_discovery_new(void) {
    DecklinkComScope com_scope;
    auto* d = new DecklinkSourceDiscovery();

    IDeckLinkIterator* iterator = nullptr;
    if (FAILED(DecklinkCreateIterator(&iterator))) {
        return d;
    }

    IDeckLink* decklink = nullptr;
    while (iterator->Next(&decklink) == S_OK) {
        IDeckLinkInput* input = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkInput, (void**)&input) != S_OK) {
            decklink->Release();
            continue;
        }
        input->Release();

        std::string model = get_model_name(decklink);
        std::string display = get_display_name(decklink);

        int64_t num_sub = 1;
        int64_t sub_index = 0;
        IDeckLinkProfileAttributes* attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            attr->GetInt(BMDDeckLinkNumberOfSubDevices, &num_sub);
            attr->GetInt(BMDDeckLinkSubDeviceIndex, &sub_index);
            attr->Release();
        }

        bool has_signal = false;
        IDeckLinkStatus* status = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkStatus, (void**)&status) == S_OK) {
            DecklinkBool locked = 0;
            status->GetFlag(bmdDeckLinkStatusVideoInputSignalLocked, &locked);
            has_signal = locked != 0;
            status->Release();
        }

        uint32_t connections = 0;
        attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            int64_t conn_mask = 0;
            if (attr->GetInt(BMDDeckLinkVideoInputConnections, &conn_mask) == S_OK) {
                connections = static_cast<uint32_t>(conn_mask);
            }
            attr->Release();
        }

        std::string name;
        if (num_sub > 1) {
            name = model + " - Input " + std::to_string(static_cast<int>(sub_index) + 1);
        } else {
            name = model;
        }

        d->ports.push_back({name, has_signal, connections});
        decklink->Release();
    }

    iterator->Release();
    return d;
}


void decklink_source_discovery_free(DecklinkSourceDiscovery* d) {
    delete d;
}


int decklink_source_discovery_count(DecklinkSourceDiscovery* d) {
    return static_cast<int>(d->ports.size());
}


void decklink_source_discovery_get(DecklinkSourceDiscovery* d, int idx, char* name, size_t name_len, bool* has_signal, uint32_t* connections) {
    if (idx < 0 || idx >= static_cast<int>(d->ports.size())) {
        if (name_len > 0) name[0] = '\0';
        *has_signal = false;
        *connections = 0;
        return;
    }
    const auto& p = d->ports[idx];
    strncpy(name, p.display_name.c_str(), name_len - 1);
    name[name_len - 1] = '\0';
    *has_signal = p.has_signal;
    *connections = p.connections;
}



/* ── forward declarations ──────────────────────────────────────────── */

class CaptureCallback;

struct DecklinkSource {
    std::string target_name;
    uint32_t connection = 0;
    IDeckLinkInput* input = nullptr;
    IDeckLinkConfiguration* config = nullptr;
    CaptureCallback* callback = nullptr;
};

/* ── COM callback ────────────────────────────────────────────────────── */

class CaptureCallback : public IDeckLinkInputCallback {
    public:
        explicit CaptureCallback(DecklinkSource* source)
            : ref_count_(1), stopping_(false), source_(source) {
            constexpr size_t max_rgba = 3840ULL * 2160ULL * 4ULL;
            buffer_[0].resize(max_rgba);
            buffer_[1].resize(max_rgba);
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
            BMDVideoInputFormatChangedEvents notificationEvents,
            IDeckLinkDisplayMode* newDisplayMode,
            BMDDetectedVideoInputFormatFlags detectedSignalFlags) override
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

            BMDPixelFormat pixelFormat = bmdFormat8BitYUV;
            if (notificationEvents & bmdVideoInputColorspaceChanged) {
                if (detectedSignalFlags & bmdDetectedVideoInputRGB444) {
                    pixelFormat = bmdFormat8BitBGRA;
                } else if (detectedSignalFlags & bmdDetectedVideoInputYCbCr422) {
                    pixelFormat = bmdFormat8BitYUV;
                }
            }

            input->PauseStreams();

            HRESULT hr = input->EnableVideoInput(newMode, pixelFormat, bmdVideoInputEnableFormatDetection);
            if (hr == S_OK) {
                input->FlushStreams();
                if (input->StartStreams() == S_OK) {
                    last_mode_ = newMode;
                } else {
                    fprintf(stderr, "[decklink] StartStreams failed after format change\n");
                }
            } else {
                fprintf(stderr, "[decklink] EnableVideoInput failed after format change (0x%08X)\n", static_cast<unsigned int>(hr));
            }
            return S_OK;
        }

        HRESULT STDMETHODCALLTYPE VideoInputFrameArrived(IDeckLinkVideoInputFrame* videoFrame,IDeckLinkAudioInputPacket* /*audioPacket*/) override {
            if (!videoFrame) return S_OK;
            if (stopping_.load()) return S_OK;

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

            // Validate before touching the buffer so no path can skip its release.
            size_t frame_size = 0;
            if (fmt == bmdFormat8BitBGRA) {
                frame_size = static_cast<size_t>(w) * static_cast<size_t>(h) * 4;
            } else if (fmt == bmdFormat8BitYUV) {
                frame_size = static_cast<size_t>(w) * static_cast<size_t>(h) * 2;
            } else {
                fprintf(stderr, "[decklink] unsupported pixel format 0x%08X\n", fmt);
                return S_OK;
            }

            int back = back_idx_.load(std::memory_order_relaxed);
            if (frame_size > buffer_[back].size()) {
                fprintf(stderr, "[decklink] frame %ldx%ld exceeds pre-allocated buffer\n", w, h);
                return S_OK;
            }
            uint8_t* dst = buffer_[back].data();

            long row_bytes = 0;
            const uint8_t* src = nullptr;

    #ifdef __APPLE__
            IDeckLinkMacVideoBuffer* mac_buffer = nullptr;
            if (videoFrame->QueryInterface(IID_IDeckLinkMacVideoBuffer, (void**)&mac_buffer) != S_OK) {
                fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkMacVideoBuffer failed\n");
                return S_OK;
            }
            void* cv_pixel_buffer = nullptr;
            HRESULT hr = mac_buffer->CreateCVPixelBufferRef(&cv_pixel_buffer);
            mac_buffer->Release();
            if (hr != S_OK || !cv_pixel_buffer) {
                fprintf(stderr, "[decklink] CreateCVPixelBufferRef failed\n");
                return S_OK;
            }
            CVPixelBufferRef pixel_buffer = static_cast<CVPixelBufferRef>(cv_pixel_buffer);
            CVPixelBufferLockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
            src = static_cast<const uint8_t*>(CVPixelBufferGetBaseAddress(pixel_buffer));
            row_bytes = static_cast<long>(CVPixelBufferGetBytesPerRow(pixel_buffer));
            if (!src || row_bytes <= 0) {
                fprintf(stderr, "[decklink] empty CVPixelBuffer (%ld row bytes)\n", row_bytes);
                CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
                CFRelease(pixel_buffer);
                return S_OK;
            }
    #else
            // GetBytes returns E_ACCESSDENIED unless StartAccess was called first.
            IDeckLinkVideoBuffer* vbuf = nullptr;
            HRESULT hr = videoFrame->QueryInterface(IID_IDeckLinkVideoBuffer, (void**)&vbuf);
            if (hr != S_OK || !vbuf) {
                fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkVideoBuffer failed (0x%08X)\n", static_cast<unsigned int>(hr));
                return S_OK;
            }
            hr = vbuf->StartAccess(bmdBufferAccessRead);
            if (hr != S_OK) {
                fprintf(stderr, "[decklink] StartAccess failed (0x%08X)\n", static_cast<unsigned int>(hr));
                vbuf->Release();
                return S_OK;
            }
            void* bytes = nullptr;
            hr = vbuf->GetBytes(&bytes);
            if (hr != S_OK || !bytes) {
                fprintf(stderr, "[decklink] GetBytes failed (0x%08X)\n", static_cast<unsigned int>(hr));
                vbuf->EndAccess(bmdBufferAccessRead);
                vbuf->Release();
                return S_OK;
            }
            src = static_cast<const uint8_t*>(bytes);
            row_bytes = videoFrame->GetRowBytes();
            if (row_bytes <= 0) {
                fprintf(stderr, "[decklink] invalid row bytes %ld\n", row_bytes);
                vbuf->EndAccess(bmdBufferAccessRead);
                vbuf->Release();
                return S_OK;
            }
    #endif

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

    #ifdef __APPLE__
            CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
            CFRelease(pixel_buffer);
    #else
            vbuf->EndAccess(bmdBufferAccessRead);
            vbuf->Release();
    #endif

            std::lock_guard<std::mutex> lock(mutex_);
            width_ = w;
            height_ = h;
            format_ = fmt;
            frame_size_ = frame_size;
            back_idx_.store(1 - back, std::memory_order_relaxed);
            seq_++;
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

    public:
        void stop() { stopping_.store(true); }
        void detach() { source_ = nullptr; }
        void set_nominal_fps(double fps) { nominal_fps_ = fps; }
};


/* ── Source ──────────────────────────────────────────────────────────── */

DecklinkSource* decklink_source_new(const char* display_name) {
    auto* s = new DecklinkSource();
    s->target_name = display_name ? display_name : "";
    return s;
}


void decklink_source_free(DecklinkSource* s) {
    if (!s) return;
    decklink_source_stop(s);
    delete s;
}


void decklink_source_set_connection(DecklinkSource* s, uint32_t connection) {
    if (!s) return;
    s->connection = connection;
}


bool decklink_source_start(DecklinkSource* s) {
    DecklinkComScope com_scope;
    if (!s || s->target_name.empty()) return false;

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

    BMDDisplayMode mode = bmdModeUnknown;
    BMDPixelFormat pixelFormat = bmdFormat8BitYUV;
    HRESULT hr = E_FAIL;

    if (supports_fmt_detection) {
        mode = bmdModeUnknown;
        hr = s->input->EnableVideoInput(mode, bmdFormat8BitYUV, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying bmdModeUnknown YUV detection => 0x%08X\n", static_cast<unsigned int>(hr));
        if (hr != S_OK) {
            hr = s->input->EnableVideoInput(mode, bmdFormat8BitBGRA, bmdVideoInputEnableFormatDetection);
            fprintf(stderr, "[decklink] trying bmdModeUnknown BGRA detection => 0x%08X\n", static_cast<unsigned int>(hr));
            if (hr == S_OK) pixelFormat = bmdFormat8BitBGRA;
        }
    }

    if (hr != S_OK) {
        mode = bmdModeHD1080p30;
        hr = s->input->EnableVideoInput(mode, bmdFormat8BitYUV, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying bmdModeHD1080p30 YUV detection => 0x%08X\n", static_cast<unsigned int>(hr));
        if (hr != S_OK) {
            hr = s->input->EnableVideoInput(mode, bmdFormat8BitBGRA, bmdVideoInputEnableFormatDetection);
            fprintf(stderr, "[decklink] trying bmdModeHD1080p30 BGRA detection => 0x%08X\n", static_cast<unsigned int>(hr));
            if (hr == S_OK) pixelFormat = bmdFormat8BitBGRA;
        }
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
