#include "sdkconfig.h"
#if CONFIG_IDF_TARGET_ESP32P4
#include "esp_wifi.h"
#include "esp_err.h"
#include <stdint.h>
/* components/p4-network/sd_card.c */
esp_err_t mastomini_sd_mount(const char *path, uint64_t *capacity_bytes);
#endif
