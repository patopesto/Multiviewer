#ifndef DECKLINK_SHIM_H
#define DECKLINK_SHIM_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct DecklinkDiscovery DecklinkDiscovery;
typedef struct DecklinkSource DecklinkSource;

DecklinkDiscovery* decklink_discovery_new(void);
void decklink_discovery_free(DecklinkDiscovery* d);
int decklink_discovery_count(DecklinkDiscovery* d);
void decklink_discovery_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, bool* has_signal, uint32_t* connections);

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
