/*
 * The P4's TF card: SDMMC slot 0 on its fixed IOMUX pins (CLK 43, CMD 44,
 * D0..D3 39..42), 4-bit, powered by the on-chip LDO channel 4 as on
 * Espressif's P4 reference design. Slot 1 of the same controller carries
 * the ESP32-C6 Wi-Fi link (ESP-Hosted): IDF 5.5 skips the host init when it
 * is already running and tears down per slot, so the two share it.
 *
 * Mounted once at boot and never unmounted. No card, or a card that will
 * not mount: an error, and the board runs without images.
 */
#include "sdkconfig.h"
#if CONFIG_IDF_TARGET_ESP32P4
#include "driver/sdmmc_host.h"
#include "esp_vfs_fat.h"
#include "sd_pwr_ctrl_by_on_chip_ldo.h"
#include "sdmmc_cmd.h"

esp_err_t mastomini_sd_mount(const char *path, uint64_t *capacity_bytes)
{
    sdmmc_host_t host = SDMMC_HOST_DEFAULT();
    host.slot = SDMMC_HOST_SLOT_0;
    host.max_freq_khz = SDMMC_FREQ_HIGHSPEED;

    sd_pwr_ctrl_ldo_config_t ldo = {.ldo_chan_id = 4};
    sd_pwr_ctrl_handle_t power = NULL;
    esp_err_t err = sd_pwr_ctrl_new_on_chip_ldo(&ldo, &power);
    if (err != ESP_OK) {
        return err;
    }
    host.pwr_ctrl_handle = power;

    sdmmc_slot_config_t slot = SDMMC_SLOT_CONFIG_DEFAULT();
    slot.width = 4;
    esp_vfs_fat_sdmmc_mount_config_t mount = {
        .format_if_mount_failed = false,
        .max_files = 6,
        .allocation_unit_size = 16 * 1024,
    };
    sdmmc_card_t *card = NULL;
    err = esp_vfs_fat_sdmmc_mount(path, &host, &slot, &mount, &card);
    if (err != ESP_OK) {
        sd_pwr_ctrl_del_on_chip_ldo(power);
        return err;
    }
    *capacity_bytes = (uint64_t)card->csd.capacity * (uint64_t)card->csd.sector_size;
    return ESP_OK;
}
#endif
