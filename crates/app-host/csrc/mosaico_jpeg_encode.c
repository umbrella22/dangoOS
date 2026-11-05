/* Native image encoding uses the process allocator, independent of a Lua VM.
 * stb_image_write is vendored under vendor/stb with its original license. */
#include <stdint.h>
#define STB_IMAGE_WRITE_IMPLEMENTATION
#define STB_IMAGE_WRITE_STATIC
#define STBI_WRITE_NO_STDIO
#include "stb_image_write.h"

int mosaico_jpeg_encode_rgb(void (*write)(void *, void *, int), void *context,
                           const uint8_t *rgb, int width, int height, int quality)
{
    return stbi_write_jpg_to_func(write, context, width, height, 3, rgb, quality);
}
