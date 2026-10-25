# The P4 board's C side: Wi-Fi on the C6 (dependencies in idf_component.yml)
# and the TF card mount (sd_card.c). Nothing for the S3.
if(IDF_TARGET STREQUAL "esp32p4")
    idf_component_register(SRCS "sd_card.c"
                           PRIV_REQUIRES fatfs sdmmc esp_driver_sdmmc)
else()
    idf_component_register()
endif()
