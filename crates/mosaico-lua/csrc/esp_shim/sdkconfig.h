/* Build configuration matching the esp-claw defaults the modules expect. */
#pragma once
/* All ten builtin sizes are compiled in; the patched display_text.c picks the
 * one matching the requested font_size instead of scaling the 24 px font. */
#define CONFIG_ESP_PAINTER_BASIC_FONT_12 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_16 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_20 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_24 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_28 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_32 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_36 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_40 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_44 1
#define CONFIG_ESP_PAINTER_BASIC_FONT_48 1
#define CONFIG_ESP_LCD_TOUCH_MAX_POINTS 5
