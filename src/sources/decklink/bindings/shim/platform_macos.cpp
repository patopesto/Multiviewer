#include <CoreFoundation/CoreFoundation.h>
#include <CoreVideo/CoreVideo.h>

#include <cstdio>

#include "platform.h"

struct CapturePlatform {
    bool frame_logged = false;
    bool fallback_logged = false;
};

CapturePlatform* capture_platform_new() {
    return new CapturePlatform();
}

void capture_platform_free(CapturePlatform* platform) {
    delete platform;
}

HRESULT capture_platform_enable(CapturePlatform* /*platform*/, IDeckLinkInput* input, BMDDisplayMode mode, BMDPixelFormat format, BMDVideoInputFlags flags) {
    return input->EnableVideoInput(mode, format, flags);
}

bool capture_platform_open(CapturePlatform* platform, IDeckLinkVideoInputFrame* videoFrame, bool gpu, PlatformFrame* out) {
    IDeckLinkMacVideoBuffer* mac_buffer = nullptr;
    if (videoFrame->QueryInterface(IID_IDeckLinkMacVideoBuffer, (void**)&mac_buffer) != S_OK) {
        fprintf(stderr, "[decklink] QueryInterface IID_IDeckLinkMacVideoBuffer failed\n");
        return false;
    }

    void* cv_pixel_buffer = nullptr;
    HRESULT hr = mac_buffer->CreateCVPixelBufferRef(&cv_pixel_buffer);
    mac_buffer->Release();
    if (hr != S_OK || !cv_pixel_buffer) {
        fprintf(stderr, "[decklink] CreateCVPixelBufferRef failed\n");
        return false;
    }

    CVPixelBufferRef pixel_buffer = static_cast<CVPixelBufferRef>(cv_pixel_buffer);
    long w = videoFrame->GetWidth();
    long h = videoFrame->GetHeight();
    BMDPixelFormat fmt = videoFrame->GetPixelFormat();

    if (!platform->frame_logged) {
        platform->frame_logged = true;
        fprintf(stderr, "[decklink] input frame %ldx%ld fmt=0x%08X rowBytes=%ld iosurface=%d cvfmt=0x%08X\n",
                w, h, static_cast<unsigned int>(fmt),
                static_cast<long>(CVPixelBufferGetBytesPerRow(pixel_buffer)),
                CVPixelBufferGetIOSurface(pixel_buffer) != nullptr ? 1 : 0,
                static_cast<unsigned int>(CVPixelBufferGetPixelFormatType(pixel_buffer)));
    }

    if (gpu && fmt == bmdFormat8BitBGRA && CVPixelBufferGetIOSurface(pixel_buffer) != nullptr) {
        out->cv = pixel_buffer;
        out->gpu = true;
        return true;
    }

    if (gpu && !platform->fallback_logged) {
        platform->fallback_logged = true;
        fprintf(stderr, "[decklink] GPU import unavailable (fmt=0x%08X iosurface=%d); using the CPU path\n",
                static_cast<unsigned int>(fmt),
                CVPixelBufferGetIOSurface(pixel_buffer) != nullptr ? 1 : 0);
    }

    if (CVPixelBufferLockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        fprintf(stderr, "[decklink] CVPixelBufferLockBaseAddress failed\n");
        CFRelease(pixel_buffer);
        return false;
    }
    const uint8_t* base = static_cast<const uint8_t*>(CVPixelBufferGetBaseAddress(pixel_buffer));
    long row_bytes = static_cast<long>(CVPixelBufferGetBytesPerRow(pixel_buffer));
    if (!base || row_bytes <= 0) {
        fprintf(stderr, "[decklink] empty CVPixelBuffer (%ld row bytes)\n", row_bytes);
        CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
        CFRelease(pixel_buffer);
        return false;
    }

    out->data = base;
    out->row_bytes = row_bytes;
    out->handle = pixel_buffer;
    return true;
}

void capture_platform_close(CapturePlatform* /*platform*/, PlatformFrame* frame) {
    if (frame->gpu || !frame->handle) {
        return;
    }
    CVPixelBufferRef pixel_buffer = static_cast<CVPixelBufferRef>(frame->handle);
    CVPixelBufferUnlockBaseAddress(pixel_buffer, kCVPixelBufferLock_ReadOnly);
    CFRelease(pixel_buffer);
    frame->handle = nullptr;
}

void capture_platform_release_cv(void* cv) {
    if (cv) {
        CFRelease(static_cast<CVPixelBufferRef>(cv));
    }
}

bool capture_platform_set_buffers(CapturePlatform* /*platform*/, void** /*pointers*/, const uint32_t* /*sizes*/, uint32_t /*count*/) {
    return false;
}

bool capture_platform_pinned_available(CapturePlatform* /*platform*/) {
    return false;
}

void capture_platform_return_buffer(CapturePlatform* /*platform*/, int /*index*/) {}

void capture_platform_clear_buffers(CapturePlatform* /*platform*/) {}

void playback_platform_log_caps(IDeckLinkOutput* output) {
    if (!output) {
        return;
    }
    IDeckLinkMacOutput* mac_output = nullptr;
    bool available = output->QueryInterface(IID_IDeckLinkMacOutput, (void**)&mac_output) == S_OK;
    if (mac_output) {
        mac_output->Release();
    }
    fprintf(stderr, "[decklink] IDeckLinkMacOutput %s\n", available ? "available" : "unavailable");
}

extern "C" void* decklink_cv_buffer_new(int width, int height) {
    CFMutableDictionaryRef attrs = CFDictionaryCreateMutable(
        kCFAllocatorDefault, 0, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (!attrs) {
        return nullptr;
    }

    int32_t pixel_format = kCVPixelFormatType_32BGRA;
    CFNumberRef format = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &pixel_format);
    if (format) {
        CFDictionarySetValue(attrs, kCVPixelBufferPixelFormatTypeKey, format);
        CFRelease(format);
    }

    CFDictionaryRef empty = CFDictionaryCreate(
        kCFAllocatorDefault, nullptr, nullptr, 0,
        &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    if (empty) {
        CFDictionarySetValue(attrs, kCVPixelBufferIOSurfacePropertiesKey, empty);
        CFRelease(empty);
    }
    CFDictionarySetValue(attrs, kCVPixelBufferMetalCompatibilityKey, kCFBooleanTrue);

    CVPixelBufferRef cv = nullptr;
    CVReturn status = CVPixelBufferCreate(
        kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA, attrs, &cv);
    CFRelease(attrs);
    if (status != kCVReturnSuccess) {
        return nullptr;
    }
    return cv;
}

extern "C" void* decklink_cv_buffer_iosurface(void* cv) {
    if (!cv) {
        return nullptr;
    }
    return CVPixelBufferGetIOSurface(static_cast<CVPixelBufferRef>(cv));
}

extern "C" void decklink_cv_buffer_release(void* cv) {
    if (cv) {
        CFRelease(static_cast<CVPixelBufferRef>(cv));
    }
}

IDeckLinkMutableVideoFrame* playback_platform_create_cv_frame(IDeckLinkOutput* output, void* cv) {
    if (!output || !cv) return nullptr;
    IDeckLinkMacOutput* mac_output = nullptr;
    if (output->QueryInterface(IID_IDeckLinkMacOutput, (void**)&mac_output) != S_OK || !mac_output) {
        return nullptr;
    }
    IDeckLinkMutableVideoFrame* frame = nullptr;
    HRESULT hr = mac_output->CreateVideoFrameFromCVPixelBufferRef(cv, &frame);
    mac_output->Release();
    if (hr != S_OK || !frame) {
        fprintf(stderr, "[decklink] CreateVideoFrameFromCVPixelBufferRef failed (0x%08X)\n", static_cast<unsigned int>(hr));
        return nullptr;
    }
    return frame;
}
