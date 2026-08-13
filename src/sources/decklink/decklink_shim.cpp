#include "decklink_shim.h"
#include <DeckLinkAPI.h>
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

/* ── helpers ─────────────────────────────────────────────────────────── */

#ifdef __APPLE__
static std::string cfstring_to_std(CFStringRef cf)
{
    char buf[256];
    if (CFStringGetCString(cf, buf, sizeof(buf), kCFStringEncodingUTF8)) {
        CFRelease(cf);
        return std::string(buf);
    }
    CFRelease(cf);
    return "";
}
#endif

static std::string get_display_name(IDeckLink* decklink)
{
#ifdef __APPLE__
    CFStringRef cf_name;
    if (decklink->GetDisplayName(&cf_name) == S_OK)
        return cfstring_to_std(cf_name);
    return "";
#else
    const char* name = nullptr;
    if (decklink->GetDisplayName(&name) == S_OK)
        return std::string(name);
    return "";
#endif
}

static std::string get_model_name(IDeckLink* decklink)
{
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

static std::string connections_to_string(int64_t mask)
{
    std::string s;
    if (mask & bmdVideoConnectionSDI)             { if (!s.empty()) s += ","; s += "SDI"; }
    if (mask & bmdVideoConnectionHDMI)            { if (!s.empty()) s += ","; s += "HDMI"; }
    if (mask & bmdVideoConnectionOpticalSDI)      { if (!s.empty()) s += ","; s += "Optical SDI"; }
    if (mask & bmdVideoConnectionComponent)       { if (!s.empty()) s += ","; s += "Component"; }
    if (mask & bmdVideoConnectionComposite)       { if (!s.empty()) s += ","; s += "Composite"; }
    if (mask & bmdVideoConnectionSVideo)          { if (!s.empty()) s += ","; s += "S-Video"; }
    if (mask & bmdVideoConnectionEthernet)        { if (!s.empty()) s += ","; s += "Ethernet"; }
    if (mask & bmdVideoConnectionOpticalEthernet) { if (!s.empty()) s += ","; s += "Optical Ethernet"; }
    if (mask & bmdVideoConnectionInternal)        { if (!s.empty()) s += ","; s += "Internal"; }
    return s;
}

static BMDVideoConnection parse_connection(const char* name)
{
    if (!name) return 0;
    if (strcmp(name, "SDI") == 0)              return bmdVideoConnectionSDI;
    if (strcmp(name, "HDMI") == 0)             return bmdVideoConnectionHDMI;
    if (strcmp(name, "Optical SDI") == 0)      return bmdVideoConnectionOpticalSDI;
    if (strcmp(name, "Component") == 0)        return bmdVideoConnectionComponent;
    if (strcmp(name, "Composite") == 0)        return bmdVideoConnectionComposite;
    if (strcmp(name, "S-Video") == 0)          return bmdVideoConnectionSVideo;
    if (strcmp(name, "Ethernet") == 0)         return bmdVideoConnectionEthernet;
    if (strcmp(name, "Optical Ethernet") == 0) return bmdVideoConnectionOpticalEthernet;
    if (strcmp(name, "Internal") == 0)         return bmdVideoConnectionInternal;
    return 0;
}

/* ── UYVY → RGBA (BT.601 for SD, BT.709 for HD) ─────────────────────── */

static void uyvy_to_rgba(const uint8_t* src, size_t row_bytes, int w, int h, uint8_t* dst)
{
    // HD (>576 lines) uses BT.709 coefficients, SD uses BT.601.
    const bool hd = h > 576;
    const int rc = hd ? 459 : 409;  // R Cr coefficient
    const int gc_u = hd ?  55 : 100; // G Cb coefficient
    const int gc_v = hd ? 136 : 208; // G Cr coefficient
    const int bc = hd ? 541 : 516;  // B Cb coefficient

    for (int y = 0; y < h; ++y) {
        const uint8_t* row = src + y * row_bytes;
        for (int x = 0; x < w; x += 2) {
            int u  = row[x * 2 + 0] - 128;
            int y0 = row[x * 2 + 1] - 16;
            int v  = row[x * 2 + 2] - 128;
            int y1 = (x + 1 < w) ? row[x * 2 + 3] - 16 : y0;

            int r0 = (298 * y0 + rc * v + 128) >> 8;
            int g0 = (298 * y0 - gc_u * u - gc_v * v + 128) >> 8;
            int b0 = (298 * y0 + bc * u + 128) >> 8;
            int r1 = (298 * y1 + rc * v + 128) >> 8;
            int g1 = (298 * y1 - gc_u * u - gc_v * v + 128) >> 8;
            int b1 = (298 * y1 + bc * u + 128) >> 8;

            auto clamp = [](int v) { return std::max(0, std::min(255, v)); };
            int i0 = (y * w + x) * 4;
            dst[i0 + 0] = clamp(r0); dst[i0 + 1] = clamp(g0); dst[i0 + 2] = clamp(b0); dst[i0 + 3] = 255;
            if (x + 1 < w) {
                int i1 = (y * w + x + 1) * 4;
                dst[i1 + 0] = clamp(r1); dst[i1 + 1] = clamp(g1); dst[i1 + 2] = clamp(b1); dst[i1 + 3] = 255;
            }
        }
    }
}

/* ── forward declarations ──────────────────────────────────────────── */

class CaptureCallback;

struct DecklinkSource {
    std::string target_name;
    char connection[32] = {};  // e.g. "SDI", "HDMI"
    IDeckLinkInput* input = nullptr;
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

    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override
    {
        if (!ppv) return E_INVALIDARG;
#ifdef __APPLE__
        CFUUIDBytes iunknown = CFUUIDGetUUIDBytes(IUnknownUUID);
        if (memcmp(&iid, &iunknown, sizeof(REFIID)) == 0 ||
            memcmp(&iid, &IID_IDeckLinkInputCallback, sizeof(REFIID)) == 0) {
            *ppv = static_cast<IDeckLinkInputCallback*>(this);
            AddRef();
            return S_OK;
        }
#else
        if (memcmp(&iid, &IID_IDeckLinkInputCallback, sizeof(REFIID)) == 0) {
            *ppv = static_cast<IDeckLinkInputCallback*>(this);
            AddRef();
            return S_OK;
        }
#endif
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef(void) override
    {
        return ++ref_count_;
    }

    ULONG STDMETHODCALLTYPE Release(void) override
    {
        // ponytail: intentionally never delete — BMD may call Release while
        // the object is still in flight on its callback thread.  Leaking one
        // small object per source creation is safer than a use-after-free
        // that corrupts the driver and breaks all subsequent capture.
        return --ref_count_;
    }

    HRESULT STDMETHODCALLTYPE VideoInputFormatChanged(
        BMDVideoInputFormatChangedEvents notificationEvents,
        IDeckLinkDisplayMode* newDisplayMode,
        BMDDetectedVideoInputFormatFlags detectedSignalFlags) override
    {
        if (!source_ || !newDisplayMode) return S_OK;
        if (stopping_.load()) return S_OK;

        // BMD serializes callbacks; do NOT hold the app mutex across
        // StopStreams()/StartStreams() to avoid deadlock with decklink_source_stop.
        IDeckLinkInput* input = source_->input;
        if (!input) return S_OK;

        BMDDisplayMode newMode = newDisplayMode->GetDisplayMode();

        // Skip restart if mode hasn't actually changed — prevents loop when
        // the device hasn't locked yet and keeps reporting the same mode.
        if (newMode == last_mode_) return S_OK;

        long w = newDisplayMode->GetWidth();
        long h = newDisplayMode->GetHeight();
        fprintf(stderr, "[decklink] VideoInputFormatChanged %ldx%ld mode=%d\n", w, h, static_cast<int>(newMode));

        // Determine pixel format from detected signal flags.
        BMDPixelFormat pixelFormat = bmdFormat8BitYUV;
        if (notificationEvents & bmdVideoInputColorspaceChanged) {
            if (detectedSignalFlags & bmdDetectedVideoInputRGB444) {
                pixelFormat = bmdFormat8BitBGRA;
            } else if (detectedSignalFlags & bmdDetectedVideoInputYCbCr422) {
                pixelFormat = bmdFormat8BitYUV;
            }
        }

        // BMD sample sequence: Pause -> Enable (keep detection ON) -> Flush -> Start.
        // Do NOT call StopStreams() or DisableVideoInput() here — they can deadlock
        // because this method runs on the BMD callback thread.
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

    HRESULT STDMETHODCALLTYPE VideoInputFrameArrived(
        IDeckLinkVideoInputFrame* videoFrame,
        IDeckLinkAudioInputPacket* /*audioPacket*/) override
    {
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
#else
        void* bytes = nullptr;
        videoFrame->GetBytes(&bytes);
        src = static_cast<const uint8_t*>(bytes);
        row_bytes = videoFrame->GetRowBytes();
#endif

        if (!src || row_bytes <= 0 || w <= 0 || h <= 0) {
#ifdef __APPLE__
            CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
            CFRelease(pixel_buffer);
#endif
            return S_OK;
        }

        size_t rgba_size = static_cast<size_t>(w) * static_cast<size_t>(h) * 4;

        // Write into the back buffer, then swap front/back under the lock.
        int back = back_idx_.load(std::memory_order_relaxed);
        uint8_t* dst = buffer_[back].data();
        if (rgba_size > buffer_[back].size()) {
            fprintf(stderr, "[decklink] frame %zux%zu exceeds pre-allocated buffer\n", w, h);
            return S_OK;
        }

        if (fmt == bmdFormat8BitBGRA) {
            // Phase 2: pass BGRA through untouched — wgpu will use Bgra8UnormSrgb.
            memcpy(dst, src, rgba_size);
        } else if (fmt == bmdFormat8BitYUV) {
            uyvy_to_rgba(src, row_bytes, w, h, dst);
        } else {
            fprintf(stderr, "[decklink] unsupported pixel format 0x%08X\n", fmt);
            return S_OK;
        }

#ifdef __APPLE__
        CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
        CFRelease(pixel_buffer);
#endif

        std::lock_guard<std::mutex> lock(mutex_);
        width_ = w;
        height_ = h;
        format_ = fmt;
        frame_size_ = rgba_size;
        back_idx_.store(1 - back, std::memory_order_relaxed);
        seq_++;
        return S_OK;
    }

public:
    bool poll(uint8_t* out, size_t out_size, int* w, int* h, uint64_t* seq, int* fmt_out)
    {
        std::lock_guard<std::mutex> lock(mutex_);
        if (frame_size_ == 0) return false;
        if (out_size < frame_size_) return false;
        int front = 1 - back_idx_.load(std::memory_order_relaxed);
        memcpy(out, buffer_[front].data(), frame_size_);
        *w = width_;
        *h = height_;
        *seq = seq_;
        *fmt_out = (format_ == bmdFormat8BitBGRA) ? 2 : 1;
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
    DecklinkSource* source_ = nullptr;
    BMDDisplayMode last_mode_ = bmdModeUnknown;
    bool had_signal_ = false;
public:
    void stop() { stopping_.store(true); }
    void detach() { source_ = nullptr; }
};

/* ── Discovery ───────────────────────────────────────────────────────── */

struct DiscoveredPort {
    std::string display_name;
    bool has_signal;
    std::string connections; // comma-separated, e.g. "SDI,HDMI"
};

struct DecklinkDiscovery {
    std::vector<DiscoveredPort> ports;
};

DecklinkDiscovery* decklink_discovery_new(void)
{
    auto* d = new DecklinkDiscovery();

    IDeckLinkIterator* iterator = CreateDeckLinkIteratorInstance();
    if (!iterator) return d;

    IDeckLink* decklink = nullptr;
    while (iterator->Next(&decklink) == S_OK) {
        // Skip devices that don't expose video input
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
            bool locked = false;
            status->GetFlag(bmdDeckLinkStatusVideoInputSignalLocked, &locked);
            has_signal = locked;
            status->Release();
        }

        std::string connections;
        attr = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
            int64_t conn_mask = 0;
            if (attr->GetInt(BMDDeckLinkVideoInputConnections, &conn_mask) == S_OK) {
                connections = connections_to_string(conn_mask);
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

void decklink_discovery_free(DecklinkDiscovery* d)
{
    delete d;
}

int decklink_discovery_count(DecklinkDiscovery* d)
{
    return static_cast<int>(d->ports.size());
}

void decklink_discovery_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, bool* has_signal, char* connections, size_t conn_len)
{
    if (idx < 0 || idx >= static_cast<int>(d->ports.size())) {
        if (name_len > 0) name[0] = '\0';
        if (conn_len > 0) connections[0] = '\0';
        *has_signal = false;
        return;
    }
    const auto& p = d->ports[idx];
    strncpy(name, p.display_name.c_str(), name_len - 1);
    name[name_len - 1] = '\0';
    strncpy(connections, p.connections.c_str(), conn_len - 1);
    connections[conn_len - 1] = '\0';
    *has_signal = p.has_signal;
}

/* ── Source ──────────────────────────────────────────────────────────── */

DecklinkSource* decklink_source_new(const char* display_name)
{
    auto* s = new DecklinkSource();
    s->target_name = display_name ? display_name : "";
    return s;
}

void decklink_source_free(DecklinkSource* s)
{
    if (!s) return;
    decklink_source_stop(s);
    delete s;
}

void decklink_source_set_connection(DecklinkSource* s, const char* connection)
{
    if (!s) return;
    if (connection) {
        strncpy(s->connection, connection, sizeof(s->connection) - 1);
        s->connection[sizeof(s->connection) - 1] = '\0';
    } else {
        s->connection[0] = '\0';
    }
}

bool decklink_source_start(DecklinkSource* s)
{
    if (!s || s->target_name.empty()) return false;

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

    bool supports_fmt_detection = false;
    IDeckLinkProfileAttributes* attr = nullptr;
    if (decklink->QueryInterface(IID_IDeckLinkProfileAttributes, (void**)&attr) == S_OK) {
        attr->GetFlag(BMDDeckLinkSupportsInputFormatDetection, &supports_fmt_detection);
        attr->Release();
    }

    // Set input connection BEFORE getting IDeckLinkInput so the mode list
    // and subsequent EnableVideoInput reflect the selected connection.
    if (s->connection[0] != '\0') {
        BMDVideoConnection conn = parse_connection(s->connection);
        if (conn == 0) {
            fprintf(stderr, "[decklink] unknown connection '%s'\n", s->connection);
            decklink->Release();
            return false;
        }
        IDeckLinkConfiguration* config = nullptr;
        if (decklink->QueryInterface(IID_IDeckLinkConfiguration, (void**)&config) != S_OK) {
            fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkConfiguration failed\n");
            decklink->Release();
            return false;
        }
        HRESULT hr = config->SetInt(bmdDeckLinkConfigVideoInputConnection, conn);
        config->Release();
        if (hr != S_OK) {
            fprintf(stderr, "[decklink] SetInt(connection=%s) failed: 0x%08X\n", s->connection, static_cast<unsigned int>(hr));
            decklink->Release();
            return false;
        }
        fprintf(stderr, "[decklink] set connection=%s ok\n", s->connection);
        // Allow hardware mux to settle (some devices need this)
        usleep(100000);
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
    BMDPixelFormat pixelFormat = bmdFormat8BitBGRA;
    HRESULT hr = E_FAIL;

    // Phase 2: Prefer BGRA — if the device supports it we avoid a CPU-side
    // BGRA→RGBA swizzle.  wgpu will sample the texture as Bgra8UnormSrgb.

    // Strategy 1: Try bmdModeUnknown with format detection (BGRA first).
    if (supports_fmt_detection) {
        mode = bmdModeUnknown;
        hr = s->input->EnableVideoInput(mode, bmdFormat8BitBGRA, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying bmdModeUnknown BGRA detection => 0x%08X\n", static_cast<unsigned int>(hr));
        if (hr != S_OK) {
            hr = s->input->EnableVideoInput(mode, bmdFormat8BitYUV, bmdVideoInputEnableFormatDetection);
            fprintf(stderr, "[decklink] trying bmdModeUnknown YUV detection => 0x%08X\n", static_cast<unsigned int>(hr));
            if (hr == S_OK) pixelFormat = bmdFormat8BitYUV;
        }
    }

    // Strategy 2: Fallback to a concrete mode with detection enabled.
    if (hr != S_OK) {
        mode = bmdModeHD1080p30;
        hr = s->input->EnableVideoInput(mode, bmdFormat8BitBGRA, bmdVideoInputEnableFormatDetection);
        fprintf(stderr, "[decklink] trying bmdModeHD1080p30 BGRA detection => 0x%08X\n", static_cast<unsigned int>(hr));
        if (hr != S_OK) {
            hr = s->input->EnableVideoInput(mode, bmdFormat8BitYUV, bmdVideoInputEnableFormatDetection);
            fprintf(stderr, "[decklink] trying bmdModeHD1080p30 YUV detection => 0x%08X\n", static_cast<unsigned int>(hr));
            if (hr == S_OK) pixelFormat = bmdFormat8BitYUV;
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

    // Some devices require audio to be enabled for video capture to work.
    // s->input->EnableAudioInput(bmdAudioSampleRate48kHz, bmdAudioSampleType16bitInteger, 2);

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

void decklink_source_stop(DecklinkSource* s)
{
    if (!s) return;
    if (s->callback) {
        s->callback->stop();           // tell in-flight callbacks to bail out
    }
    if (s->input) {
        // BMD-recommended teardown order.
        s->input->StopStreams();       // blocks until current callback finishes
        s->input->FlushStreams();      // discard queued frames (prevents late callback)
        s->input->SetCallback(nullptr);
        s->input->DisableVideoInput();
        s->input->DisableAudioInput();
        // UltraStudio Recorder 3G needs time to release the input block
        // before another app (or our next start) can re-acquire it.
        usleep(500000);
    }
    if (s->callback) {
        // Null the back-pointer so any truly-late callback won't touch freed memory.
        s->callback->detach();
        // Intentionally NOT calling Release() — see comment in CaptureCallback::Release().
        s->callback = nullptr;
    }
    if (s->input) {
        s->input->Release();
        s->input = nullptr;
    }
}

bool decklink_source_poll_frame(DecklinkSource* s, uint8_t* out_rgba, size_t out_size, int* w, int* h, uint64_t* seq, int* fmt_out)
{
    if (!s || !s->callback) return false;
    return s->callback->poll(out_rgba, out_size, w, h, seq, fmt_out);
}
