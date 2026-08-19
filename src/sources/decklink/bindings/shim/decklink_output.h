#ifndef DECKLINK_SHIM_H
#define DECKLINK_SHIM_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── Output discovery ────────────────────────────────────────────────── */

typedef struct DecklinkOutputDiscovery DecklinkOutputDiscovery;
typedef struct DecklinkOutput DecklinkOutput;

DecklinkOutputDiscovery* decklink_output_discovery_new(void);
void decklink_output_discovery_free(DecklinkOutputDiscovery* d);
int decklink_output_discovery_count(DecklinkOutputDiscovery* d);
void decklink_output_discovery_get(DecklinkOutputDiscovery* d, int idx, char* name, size_t name_len, int* mode_count);
void decklink_output_discovery_get_mode(DecklinkOutputDiscovery* d, int idx, int mode_idx, char* mode_name, size_t mode_name_len, uint32_t* mode_id, int* w, int* h, double* fps);

/* ── Output runtime ──────────────────────────────────────────────────── */

DecklinkOutput* decklink_output_new(const char* display_name);
void decklink_output_free(DecklinkOutput* o);
bool decklink_output_start(DecklinkOutput* o, uint32_t mode_id);
void decklink_output_stop(DecklinkOutput* o);
bool decklink_output_present_frame(DecklinkOutput* o, const uint8_t* bgra, int width, int height, int row_bytes);

#ifdef __cplusplus
}
#endif

#endif
