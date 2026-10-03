#pragma once
#include "esp_err.h"
esp_err_t display_init(void);
unsigned int minicloud_display_width(void);
unsigned int minicloud_display_height(void);
const char *minicloud_display_driver(void);
esp_err_t minicloud_display_frame(const char *source, const char *recipient,
    const char *text, int scale, const char *rgb565_path);
