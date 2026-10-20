#ifndef DECKLINK_DISCOVERY_H
#define DECKLINK_DISCOVERY_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One iterator pass over every DeckLink device, collecting both the input
 * ports and the output ports so a poll costs a single driver enumeration. */
typedef struct DecklinkDiscovery DecklinkDiscovery;

DecklinkDiscovery* decklink_discovery_new(void);
void decklink_discovery_free(DecklinkDiscovery* d);

int decklink_discovery_input_count(DecklinkDiscovery* d);
void decklink_discovery_input_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, bool* has_signal, uint32_t* connections);

int decklink_discovery_output_count(DecklinkDiscovery* d);
void decklink_discovery_output_get(DecklinkDiscovery* d, int idx, char* name, size_t name_len, int* mode_count);
void decklink_discovery_output_mode(DecklinkDiscovery* d, int idx, int mode_idx, char* mode_name, size_t mode_name_len, uint32_t* mode_id, int* w, int* h, double* fps);

#ifdef __cplusplus
}
#endif

#endif
