#ifndef DECKLINK_SHIM_H
#define DECKLINK_SHIM_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Source runtime -────────────────────────────────────────────────── */

typedef struct DecklinkSource DecklinkSource;

DecklinkSource* decklink_source_new(const char* display_name);
void decklink_source_free(DecklinkSource* s);
void decklink_source_set_connection(DecklinkSource* s, uint32_t connection);
void decklink_source_set_gpu_mode(DecklinkSource* s, bool enable);
bool decklink_source_is_bgra(DecklinkSource* s);
bool decklink_source_start(DecklinkSource* s);
void decklink_source_stop(DecklinkSource* s);
bool decklink_source_poll_frame(DecklinkSource* s, uint8_t* out_rgba, size_t out_size, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out);
bool decklink_source_poll_cv_frame(DecklinkSource* s, void** cv_out, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out);
bool decklink_source_take_cpu_fallback(DecklinkSource* s);
uint64_t decklink_source_frame_generation(DecklinkSource* s);
void decklink_source_wait_frame(DecklinkSource* s, uint64_t seen, int timeout_ms);
bool decklink_source_set_capture_buffers(DecklinkSource* s, void** pointers, const uint32_t* sizes, uint32_t count);
bool decklink_source_pinned_available(DecklinkSource* s);
bool decklink_source_enable_pinned(DecklinkSource* s);
bool decklink_source_disable_pinned(DecklinkSource* s);
bool decklink_source_poll_pinned_frame(DecklinkSource* s, int* index, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out);
void decklink_source_return_buffer(DecklinkSource* s, int index);
void decklink_source_frame_info(DecklinkSource* s, uint32_t* w, uint32_t* h, uint32_t* fmt_out);
void decklink_cv_frame_release(void* cv);

#ifdef __cplusplus
}
#endif

#endif
