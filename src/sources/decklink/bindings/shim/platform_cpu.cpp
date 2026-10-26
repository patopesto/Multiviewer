#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <mutex>
#include <vector>

#include "platform.h"

struct ExternalBuffer {
    uint8_t* data = nullptr;
    size_t size = 0;
    bool in_use = false;
};

struct CapturePlatform {
    bool frame_logged = false;
    bool pinned = false;
    std::mutex mutex;
    std::condition_variable cv;
    std::vector<ExternalBuffer> buffers;
};

class ExternalVideoBuffer : public IDeckLinkVideoBuffer {
    public:
        ExternalVideoBuffer(CapturePlatform* platform, int index)
            : ref_count_(1), platform_(platform), index_(index) {}

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
            *buffer = platform_->buffers[index_].data;
            return S_OK;
        }

        HRESULT GetSize(uint64_t* size) override {
            if (!size) return E_INVALIDARG;
            *size = static_cast<uint64_t>(platform_->buffers[index_].size);
            return S_OK;
        }

        HRESULT StartAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }
        HRESULT EndAccess(BMDBufferAccessFlags /*flags*/) override { return S_OK; }

    private:
        std::atomic<ULONG> ref_count_;
        CapturePlatform* platform_;
        int index_;
};

class ExternalAllocator : public IDeckLinkVideoBufferAllocator {
    public:
        explicit ExternalAllocator(CapturePlatform* platform)
            : ref_count_(1), platform_(platform) {}

        HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
            if (!ppv) return E_INVALIDARG;
            if (DecklinkIsIUnknownIID(iid) ||
                DecklinkIIDEqual(iid, IID_IDeckLinkVideoBufferAllocator)) {
                *ppv = static_cast<IDeckLinkVideoBufferAllocator*>(this);
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

        HRESULT AllocateVideoBuffer(IDeckLinkVideoBuffer** allocatedBuffer) override {
            if (!allocatedBuffer) return E_INVALIDARG;
            std::unique_lock<std::mutex> lock(platform_->mutex);
            platform_->cv.wait_for(lock, std::chrono::milliseconds(100), [&] {
                for (const auto& buffer : platform_->buffers) {
                    if (!buffer.in_use) return true;
                }
                return false;
            });
            for (size_t i = 0; i < platform_->buffers.size(); ++i) {
                if (!platform_->buffers[i].in_use) {
                    platform_->buffers[i].in_use = true;
                    *allocatedBuffer = new ExternalVideoBuffer(platform_, static_cast<int>(i));
                    return S_OK;
                }
            }
            return E_OUTOFMEMORY;
        }

    private:
        std::atomic<ULONG> ref_count_;
        CapturePlatform* platform_;
};

class ExternalAllocatorProvider : public IDeckLinkVideoBufferAllocatorProvider {
    public:
        explicit ExternalAllocatorProvider(CapturePlatform* platform)
            : ref_count_(1), platform_(platform) {}

        HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
            if (!ppv) return E_INVALIDARG;
            if (DecklinkIsIUnknownIID(iid) ||
                DecklinkIIDEqual(iid, IID_IDeckLinkVideoBufferAllocatorProvider)) {
                *ppv = static_cast<IDeckLinkVideoBufferAllocatorProvider*>(this);
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

        HRESULT GetVideoBufferAllocator(uint32_t bufferSize, uint32_t width, uint32_t height,
                                        uint32_t rowBytes, BMDPixelFormat pixelFormat,
                                        IDeckLinkVideoBufferAllocator** allocator) override {
            (void)height;
            if (!allocator || !platform_->pinned) return E_INVALIDARG;
            uint32_t expected_row = 0;
            if (pixelFormat == bmdFormat8BitBGRA) {
                expected_row = width * 4;
            } else if (pixelFormat == bmdFormat8BitYUV) {
                expected_row = width * 2;
            } else {
                return E_INVALIDARG;
            }
            if (rowBytes != expected_row) return E_INVALIDARG;
            for (const auto& buffer : platform_->buffers) {
                if (buffer.size < bufferSize) {
                    return E_INVALIDARG;
                }
            }
            *allocator = new ExternalAllocator(platform_);
            return S_OK;
        }

    private:
        std::atomic<ULONG> ref_count_;
        CapturePlatform* platform_;
};

CapturePlatform* capture_platform_new() {
    return new CapturePlatform();
}

void capture_platform_free(CapturePlatform* platform) {
    delete platform;
}

bool capture_platform_set_buffers(CapturePlatform* platform, void** pointers, const uint32_t* sizes, uint32_t count) {
    if (!platform || !pointers || !sizes || count == 0) return false;
    std::lock_guard<std::mutex> lock(platform->mutex);
    platform->buffers.clear();
    for (uint32_t i = 0; i < count; ++i) {
        ExternalBuffer buffer;
        buffer.data = static_cast<uint8_t*>(pointers[i]);
        buffer.size = sizes[i];
        buffer.in_use = false;
        platform->buffers.push_back(buffer);
    }
    platform->pinned = true;
    fprintf(stderr, "[decklink] capture buffers registered: %u\n", count);
    return true;
}

bool capture_platform_pinned_available(CapturePlatform* platform) {
    if (!platform) return false;
    std::lock_guard<std::mutex> lock(platform->mutex);
    return platform->pinned && !platform->buffers.empty();
}

void capture_platform_return_buffer(CapturePlatform* platform, int index) {
    if (!platform || index < 0) return;
    std::lock_guard<std::mutex> lock(platform->mutex);
    if (static_cast<size_t>(index) < platform->buffers.size()) {
        platform->buffers[index].in_use = false;
        platform->cv.notify_all();
    }
}

void capture_platform_clear_buffers(CapturePlatform* platform) {
    if (!platform) return;
    std::lock_guard<std::mutex> lock(platform->mutex);
    /* Keep the pointers: a driver-released wrapper may still call GetBytes. */
    platform->pinned = false;
    platform->cv.notify_all();
}

HRESULT capture_platform_enable(CapturePlatform* platform, IDeckLinkInput* input, BMDDisplayMode mode, BMDPixelFormat format, BMDVideoInputFlags flags) {
    if (platform && platform->pinned) {
        auto* provider = new ExternalAllocatorProvider(platform);
        HRESULT hr = input->EnableVideoInputWithAllocatorProvider(mode, format, flags, provider);
        provider->Release();
        return hr;
    }
    return input->EnableVideoInput(mode, format, flags);
}

bool capture_platform_open(CapturePlatform* platform, IDeckLinkVideoInputFrame* videoFrame, bool /*gpu*/, PlatformFrame* out) {
    IDeckLinkVideoBuffer* vbuf = nullptr;
    HRESULT hr = videoFrame->QueryInterface(IID_IDeckLinkVideoBuffer, (void**)&vbuf);
    if (hr != S_OK || !vbuf) {
        fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkVideoBuffer failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return false;
    }

    if (platform->pinned) {
        void* bytes = nullptr;
        if (vbuf->GetBytes(&bytes) != S_OK || !bytes) {
            fprintf(stderr, "[decklink] pinned buffer GetBytes failed\n");
            vbuf->Release();
            return false;
        }
        int found = -1;
        {
            std::lock_guard<std::mutex> lock(platform->mutex);
            for (size_t i = 0; i < platform->buffers.size(); ++i) {
                if (platform->buffers[i].data == bytes) {
                    found = static_cast<int>(i);
                    break;
                }
            }
        }
        vbuf->Release();
        if (found < 0) {
            fprintf(stderr, "[decklink] frame is not one of the registered buffers\n");
            return false;
        }
        out->index = found;
        out->data = nullptr;
        out->handle = nullptr;
        return true;
    }

    hr = vbuf->StartAccess(bmdBufferAccessRead);
    if (hr != S_OK) {
        fprintf(stderr, "[decklink] StartAccess failed (0x%08X)\n", static_cast<unsigned int>(hr));
        vbuf->Release();
        return false;
    }

    void* bytes = nullptr;
    hr = vbuf->GetBytes(&bytes);
    if (hr != S_OK || !bytes) {
        fprintf(stderr, "[decklink] GetBytes failed (0x%08X)\n", static_cast<unsigned int>(hr));
        vbuf->EndAccess(bmdBufferAccessRead);
        vbuf->Release();
        return false;
    }

    long row_bytes = videoFrame->GetRowBytes();
    if (row_bytes <= 0) {
        fprintf(stderr, "[decklink] invalid row bytes %ld\n", row_bytes);
        vbuf->EndAccess(bmdBufferAccessRead);
        vbuf->Release();
        return false;
    }

    if (!platform->frame_logged) {
        platform->frame_logged = true;
        fprintf(stderr, "[decklink] input frame %ldx%ld fmt=%s rowBytes=%ld\n",
                videoFrame->GetWidth(), videoFrame->GetHeight(),
                DecklinkPixelFormatName(videoFrame->GetPixelFormat()), row_bytes);
    }

    out->data = static_cast<const uint8_t*>(bytes);
    out->row_bytes = row_bytes;
    out->handle = vbuf;
    return true;
}

void capture_platform_close(CapturePlatform* platform, PlatformFrame* frame) {
    if (frame->gpu || !frame->handle) {
        return;
    }
    (void)platform;
    IDeckLinkVideoBuffer* vbuf = static_cast<IDeckLinkVideoBuffer*>(frame->handle);
    vbuf->EndAccess(bmdBufferAccessRead);
    vbuf->Release();
    frame->handle = nullptr;
}

void capture_platform_release_cv(void* /*cv*/) {}

void playback_platform_log_caps(IDeckLinkOutput* /*output*/) {}

IDeckLinkMutableVideoFrame* playback_platform_create_cv_frame(IDeckLinkOutput* /*output*/, void* /*cv*/) {
    return nullptr;
}
