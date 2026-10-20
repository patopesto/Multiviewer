#ifndef DECKLINK_SHIM_H
#define DECKLINK_SHIM_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Output runtime ──────────────────────────────────────────────────── */

typedef struct DecklinkOutput DecklinkOutput;

DecklinkOutput* decklink_output_new(const char* display_name);
void decklink_output_free(DecklinkOutput* o);
bool decklink_output_start(DecklinkOutput* o, uint32_t mode_id);
void decklink_output_stop(DecklinkOutput* o);
bool decklink_output_present_frame(DecklinkOutput* o, const uint8_t* bgra, int width, int height, int row_bytes);

#ifdef __cplusplus
}
#endif

#endif
