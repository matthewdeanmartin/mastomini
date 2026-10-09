#include "minicloud_display.h"
#include "driver/sdspi_host.h"
#include "esp_vfs_fat.h"
#include <errno.h>
#include <sys/stat.h>

// Waveshare ESP32-C6-LCD-1.47: shared LCD bus GPIO6/5/7, TF CS GPIO4.
// minicloud_bus_init has already initialized SPI2. Never initialize/free it here.
static sdmmc_card_t *card;
esp_err_t minicloud_card_mount(void) {
    sdmmc_host_t host = SDSPI_HOST_DEFAULT();
    host.slot = SPI2_HOST;
    sdspi_device_config_t slot = SDSPI_DEVICE_CONFIG_DEFAULT();
    slot.host_id = SPI2_HOST;
    slot.gpio_cs = GPIO_NUM_4;
    esp_vfs_fat_mount_config_t mount = {
        .format_if_mount_failed = false,
        .max_files = 4,
        .allocation_unit_size = 0,
    };
    esp_err_t result = esp_vfs_fat_sdspi_mount("/sdcard", &host, &slot, &mount, &card);
    if (result != ESP_OK) return result;
    if (mkdir("/sdcard/minicloud", 0777) != 0 && errno != EEXIST) {
        esp_vfs_fat_sdcard_unmount("/sdcard", card);
        card = NULL;
        return ESP_FAIL;
    }
    return ESP_OK;
}
