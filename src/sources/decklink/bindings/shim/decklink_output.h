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
/* `token_queued` says whether the buffer owns a token that will come back even
   if the call fails (it was created before the failure). */
bool decklink_output_present_frame(DecklinkOutput* o, const uint8_t* bgra, int width, int height, int row_bytes, uint64_t token, bool* token_queued);
bool decklink_output_present_cv(DecklinkOutput* o, void* cv, uint64_t token);
bool decklink_output_take_completed(DecklinkOutput* o, uint64_t* token);
bool decklink_output_completion_available(DecklinkOutput* o);
uint32_t decklink_output_pixel_format(DecklinkOutput* o);

#ifdef __cplusplus
}
#endif

#endif
