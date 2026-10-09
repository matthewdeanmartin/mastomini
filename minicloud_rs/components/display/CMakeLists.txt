idf_component_register(SRCS "display.c" "card.c" INCLUDE_DIRS "." "include"
  REQUIRES esp_lcd esp_driver_gpio esp_driver_spi esp_driver_ledc fatfs sdmmc esp_driver_sdspi)
