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
bool decklink_source_start(DecklinkSource* s);
void decklink_source_stop(DecklinkSource* s);
bool decklink_source_poll_frame(DecklinkSource* s, uint8_t* out_rgba, size_t out_size, int* w, int* h, uint64_t* seq, uint32_t* fmt_out, double* nominal_fps_out);

#ifdef __cplusplus
}
#endif

#endif
