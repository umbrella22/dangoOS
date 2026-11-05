/* Application raster/codec bridge. All borrowed pixel spans are checked in Rust. */
#include "display_text.c"
#include "display_bitmap.h"
#include "lodepng.h"
#include "mosaico_resize.h"
#include <time.h>

int dango_font_from_bytes(const uint8_t *bytes, size_t length, display_font_handle_t *out) {
    if(!bytes || !out || length<FONT_HEADER_SIZE || length>FONT_MAX_BYTES || memcmp(bytes,"DFN1",4)) return ESP_ERR_INVALID_SIZE;
    *out=NULL;
    int ascent=read_s16(bytes+4),descent=read_s16(bytes+6),advance=read_s16(bytes+8);
    uint32_t count=read32(bytes+12),fallback=read32(bytes+16);
    if(read16(bytes+10)||ascent<0||descent<0||ascent+descent<1||ascent+descent>64||advance<1||advance>64||!count||count>4096||(fallback!=UINT32_MAX&&!valid_codepoint(fallback)))return ESP_ERR_INVALID_SIZE;
    size_t index=(size_t)count*FONT_RECORD_SIZE,data_size=length-FONT_HEADER_SIZE;
    if(data_size<index)return ESP_ERR_INVALID_SIZE;
    display_font_handle_t font=malloc(sizeof(*font)+data_size);if(!font)return ESP_ERR_NO_MEM;
    font->ascent=ascent;font->descent=descent;font->default_advance=advance;font->count=count;font->fallback=fallback;font->bitmap_size=data_size-index;
    memcpy(font->data,bytes+FONT_HEADER_SIZE,data_size);
    size_t offset=0;uint32_t previous=0;
    for(uint32_t i=0;i<count;i++) {
        const uint8_t *r=font->data+i*FONT_RECORD_SIZE;uint32_t cp=read32(r),at=read32(r+4);
        int a=read_s16(r+8),x=read_s16(r+10),y=read_s16(r+12);unsigned w=read16(r+14),h=read16(r+16);size_t size=((w+7)/8)*h;
        if(!valid_codepoint(cp)||(i&&cp<=previous)||at!=offset||a<0||a>64||!((!w&&!h)||(w&&h&&w<=64&&h<=64))||x< -64||x>64||y< -descent||y+(int)h>ascent||offset>font->bitmap_size||size>font->bitmap_size-offset){free(font);return ESP_ERR_INVALID_SIZE;}
        offset+=size;previous=cp;
    }
    if(offset!=font->bitmap_size||(fallback!=UINT32_MAX&&!find_record(font,fallback))){free(font);return ESP_ERR_INVALID_SIZE;}
    *out=font;return ESP_OK;
}
void dango_raster_draw(display_raster_t *r,int op,const int *a,uint32_t color,double start,double end){
    display_color_t c={.full=color};
    switch(op){
        case 0:display_raster_fill_rect(r,a[0],a[1],a[2],a[3],c);break;
        case 1:display_raster_stroke_rect(r,a[0],a[1],a[2],a[3],c);break;
        case 2:display_raster_line(r,a[0],a[1],a[2],a[3],c);break;
        case 3:display_raster_fill_circle(r,a[0],a[1],a[2],c);break;
        case 4:display_raster_stroke_circle(r,a[0],a[1],a[2],c);break;
        case 5:display_raster_arc(r,a[0],a[1],a[2],start,end,c);break;
        case 6:display_raster_fill_round_rect(r,a[0],a[1],a[2],a[3],a[4],c);break;
        case 7:display_raster_stroke_round_rect(r,a[0],a[1],a[2],a[3],a[4],c);break;
        case 8:display_raster_fill_triangle(r,a[0],a[1],a[2],a[3],a[4],a[5],c);break;
    }
}
int dango_png(const uint8_t *input,size_t length,uint8_t **pixels,unsigned *width,unsigned *height){
    LodePNGState state;lodepng_state_init(&state);unsigned err=lodepng_inspect(width,height,&state,input,length);
    if(!err&&(!*width||!*height||*width>65535||*height>65535||(uint64_t)*width**height>1920*1080))err=93;
    if(!err){state.decoder.zlibsettings.max_output_size=(size_t)*width**height*8+*height*8+65536;state.decoder.read_text_chunks=0;state.decoder.remember_unknown_chunks=0;state.info_raw.colortype=LCT_RGBA;state.info_raw.bitdepth=8;err=lodepng_decode(pixels,width,height,&state,input,length);}
    lodepng_state_cleanup(&state);return err;
}
void dango_png_free(uint8_t *pixels){free(pixels);}
void dango_resize(const uint8_t *source,unsigned sw,unsigned sh,uint8_t *out,unsigned w,unsigned h,int gray){mosaico_resize_bilinear(source,sw,sh,out,w,h,gray);}
size_t dango_format_date(int64_t seconds,const char *format,char *out,size_t cap){time_t at=(time_t)seconds;struct tm fields;if(!gmtime_r(&at,&fields))return 0;return strftime(out,cap,format,&fields);}
