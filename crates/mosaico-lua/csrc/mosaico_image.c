/* Image frames and PNG extension. Buffers use the owning VM's capped ledger. */
#include <stdio.h>
#include <string.h>
#include <math.h>
#include "lauxlib.h"
#include "lua_image.h"
#include "lodepng.h"
#define FRAME_MT "mosaico.image.frame"
#define FILE_MAX (4u*1024u*1024u)
#define PIXEL_MAX (1920u*1080u)
extern void *mosaico_image_realloc(void*,size_t);
void *lodepng_malloc(size_t n){return mosaico_image_realloc(NULL,n);}
void *lodepng_realloc(void*p,size_t n){return mosaico_image_realloc(p,n);}
void lodepng_free(void*p){mosaico_image_realloc(p,0);}
extern int mosaico_jpeg_decode_tracked(const uint8_t*,size_t,uint16_t*,uint16_t*,uint8_t*,size_t);
#define STB_IMAGE_WRITE_IMPLEMENTATION
#define STB_IMAGE_WRITE_STATIC
#define STBI_WRITE_NO_STDIO
#define STBIW_MALLOC(n) mosaico_image_realloc(NULL,n)
#define STBIW_REALLOC(p,n) mosaico_image_realloc(p,n)
#define STBIW_FREE(p) mosaico_image_realloc(p,0)
#include "stb_image_write.h"
typedef struct {uint8_t*data;size_t bytes,capacity;int failed;} jpeg_output;
static void jpeg_write(void*context,void*data,int size){
 jpeg_output*out=context;if(out->failed||size<=0)return;size_t n=(size_t)size;
 if(n>FILE_MAX-out->bytes){out->failed=1;return;}
 if(out->bytes+n>out->capacity){size_t cap=out->capacity?out->capacity*2:4096;while(cap<out->bytes+n)cap*=2;if(cap>FILE_MAX)cap=FILE_MAX;void*p=mosaico_image_realloc(out->data,cap);if(!p){out->failed=1;return;}out->data=p;out->capacity=cap;}
 memcpy(out->data+out->bytes,data,n);out->bytes+=n;
}
typedef struct {uint8_t *data;size_t bytes;unsigned w,h;lua_image_format_t format;} frame_t;
static const char *names[]={"RGBP","RGBR","RGB3","BGR3","GREY","YUYV","UYVY","JPEG","MJPG"};
static int valid_dims(unsigned w,unsigned h){return w&&h&&w<=65535&&h<=65535&&(size_t)w*h<=PIXEL_MAX;}
static frame_t *check(lua_State*L,int i){frame_t*f=luaL_checkudata(L,i,FRAME_MT);if(!f->data)luaL_error(L,"image frame is released");return f;}
static frame_t *push(lua_State*L,unsigned w,unsigned h,lua_image_format_t fmt,size_t bytes){
 frame_t*f=lua_newuserdatauv(L,sizeof(*f),0);memset(f,0,sizeof(*f));luaL_setmetatable(L,FRAME_MT);
 f->data=mosaico_image_realloc(NULL,bytes);if(!f->data)luaL_error(L,"image memory limit exceeded");
 f->bytes=bytes;f->w=w;f->h=h;f->format=fmt;return f;
}
static int release(lua_State*L){frame_t*f=luaL_checkudata(L,1,FRAME_MT);if(f->data){mosaico_image_realloc(f->data,0);f->data=NULL;}return 0;}
static int info(lua_State*L){frame_t*f=luaL_checkudata(L,1,FRAME_MT);lua_createtable(L,0,6);
#define I(k,v) lua_pushinteger(L,v);lua_setfield(L,-2,k)
 I("width",f->w);I("height",f->h);I("bytes",f->bytes);I("timestamp_us",0);
 lua_pushstring(L,names[f->format]);lua_setfield(L,-2,"pixel_format");lua_pushboolean(L,f->data!=NULL);lua_setfield(L,-2,"valid");return 1;
}
static int data(lua_State*L){frame_t*f=check(L,1);lua_pushlstring(L,(char*)f->data,f->bytes);return 1;}
/* JPEG decoder writes LVGL BGR888. Convert once to canonical RGB for sampling. */
static int rgb(frame_t*f,uint8_t**out,int*owned){
 if(f->format==LUA_IMAGE_FORMAT_RGB888){*out=f->data;*owned=0;return 0;}
 size_t n=(size_t)f->w*f->h;uint8_t*p=mosaico_image_realloc(NULL,n*3);if(!p)return ESP_ERR_NO_MEM;
 if(f->format==LUA_IMAGE_FORMAT_JPEG||f->format==LUA_IMAGE_FORMAT_MJPEG){uint16_t w,h;int err=mosaico_jpeg_decode_tracked(f->data,f->bytes,&w,&h,p,n*3);if(err){mosaico_image_realloc(p,0);return ESP_ERR_INVALID_RESPONSE;}for(size_t i=0;i<n;i++){uint8_t t=p[3*i];p[3*i]=p[3*i+2];p[3*i+2]=t;}}
 else for(size_t i=0;i<n;i++){
  if(f->format==LUA_IMAGE_FORMAT_BGR888){p[3*i]=f->data[3*i+2];p[3*i+1]=f->data[3*i+1];p[3*i+2]=f->data[3*i];}
  else if(f->format==LUA_IMAGE_FORMAT_GRAY8){p[3*i]=p[3*i+1]=p[3*i+2]=f->data[i];}
  else if(f->format==LUA_IMAGE_FORMAT_RGB565LE||f->format==LUA_IMAGE_FORMAT_RGB565BE){uint16_t v=f->format==0?f->data[i*2]|f->data[i*2+1]<<8:f->data[i*2]<<8|f->data[i*2+1];p[3*i]=((v>>11)&31)*255/31;p[3*i+1]=((v>>5)&63)*255/63;p[3*i+2]=(v&31)*255/31;}
  else {mosaico_image_realloc(p,0);return ESP_ERR_NOT_SUPPORTED;}
 }
 *out=p;*owned=1;return 0;
}
static size_t bpp(lua_image_format_t f){return f==LUA_IMAGE_FORMAT_GRAY8?1:f==LUA_IMAGE_FORMAT_RGB888||f==LUA_IMAGE_FORMAT_BGR888?3:2;}
static void pixel(uint8_t*out,lua_image_format_t fmt,unsigned r,unsigned g,unsigned b){
 if(fmt==LUA_IMAGE_FORMAT_GRAY8){out[0]=(77*r+150*g+29*b+128)>>8;}
 else if(fmt==LUA_IMAGE_FORMAT_RGB888){out[0]=r;out[1]=g;out[2]=b;}
 else if(fmt==LUA_IMAGE_FORMAT_BGR888){out[0]=b;out[1]=g;out[2]=r;}
 else {uint16_t v=((r>>3)<<11)|((g>>2)<<5)|(b>>3);if(fmt==LUA_IMAGE_FORMAT_RGB565BE){out[0]=v>>8;out[1]=v;}else{out[0]=v;out[1]=v>>8;}}
}
static int convert_view(frame_t*f,lua_image_format_t fmt,lua_image_view_t*v){
 memset(v,0,sizeof(*v));if(fmt<0||fmt>=LUA_IMAGE_FORMAT_COUNT)return ESP_ERR_INVALID_ARG;
 v->width=f->w;v->height=f->h;v->format=fmt;strcpy(v->source_format,names[f->format]);
 if(fmt==f->format){v->data=f->data;v->bytes=f->bytes;return ESP_OK;}
 if(fmt==LUA_IMAGE_FORMAT_YUYV||fmt==LUA_IMAGE_FORMAT_UYVY)return ESP_ERR_NOT_SUPPORTED;
 if(fmt==LUA_IMAGE_FORMAT_JPEG||fmt==LUA_IMAGE_FORMAT_MJPEG){
  uint8_t*src;int owned,err=rgb(f,&src,&owned);if(err)return err;
  jpeg_output output={0};int success=stbi_write_jpg_to_func(jpeg_write,&output,f->w,f->h,3,src,90);
  if(owned)mosaico_image_realloc(src,0);
  if(!success||output.failed||!output.bytes){mosaico_image_realloc(output.data,0);return ESP_ERR_NO_MEM;}
  v->data=output.data;v->bytes=output.bytes;v->owned=true;return ESP_OK;
 }
 uint8_t*src;int owned,err=rgb(f,&src,&owned);if(err)return err;
 size_t n=(size_t)f->w*f->h,step=bpp(fmt);uint8_t*out=mosaico_image_realloc(NULL,n*step);
 if(!out){if(owned)mosaico_image_realloc(src,0);return ESP_ERR_NO_MEM;}
 for(size_t i=0;i<n;i++)pixel(out+i*step,fmt,src[3*i],src[3*i+1],src[3*i+2]);
 if(owned)mosaico_image_realloc(src,0);v->data=out;v->bytes=n*step;v->owned=true;return ESP_OK;
}
esp_err_t lua_image_require_format(lua_State*L,int i,lua_image_format_t fmt,lua_image_view_t*v){frame_t*f=luaL_testudata(L,i,FRAME_MT);if(!f)return ESP_ERR_INVALID_ARG;if(!f->data)return ESP_ERR_INVALID_STATE;return convert_view(f,fmt,v);}
void lua_image_release_view(lua_image_view_t*v){if(v&&v->owned)mosaico_image_realloc((void*)v->data,0);if(v)memset(v,0,sizeof(*v));}
static int convert(lua_State*L){frame_t*f=check(L,1);lua_Integer fmt=luaL_checkinteger(L,2);if(fmt<0||fmt>8)return luaL_error(L,"invalid image format");
 frame_t*out=lua_newuserdatauv(L,sizeof(*out),0);memset(out,0,sizeof(*out));luaL_setmetatable(L,FRAME_MT);
 lua_image_view_t v;int err=convert_view(f,fmt,&v);if(err)return luaL_error(L,"image.convert failed (%d)",err);
 out->w=f->w;out->h=f->h;out->bytes=v.bytes;out->format=fmt;
 if(v.owned)out->data=(uint8_t*)v.data;else{out->data=mosaico_image_realloc(NULL,v.bytes);if(!out->data)return luaL_error(L,"image memory limit exceeded");memcpy(out->data,v.data,v.bytes);}return 1;
}
static int load_file(lua_State*L){const char*path=luaL_checkstring(L,1);frame_t*f=lua_newuserdatauv(L,sizeof(*f),0);memset(f,0,sizeof(*f));luaL_setmetatable(L,FRAME_MT);FILE*file=fopen(path,"rb");if(!file)return luaL_error(L,"image file open failed: %s",path);
 if(fseek(file,0,SEEK_END)||ftell(file)<=0||ftell(file)>FILE_MAX){fclose(file);return luaL_error(L,"image file must contain 1..4194304 bytes");}size_t n=ftell(file);rewind(file);
 /* Allocate frame first: all later failures have a registered finalizer. */
 f->data=mosaico_image_realloc(NULL,n);if(!f->data){fclose(file);return luaL_error(L,"image memory limit exceeded");}f->bytes=n;
 size_t got=fread(f->data,1,n,file);int close=fclose(file);if(got!=n||close)return luaL_error(L,"image file read failed");
 if(n>=8&&!memcmp(f->data,"\x89PNG\r\n\x1a\n",8)){
  LodePNGState state;lodepng_state_init(&state);unsigned w=0,h=0;unsigned err=lodepng_inspect(&w,&h,&state,f->data,n);
  if(err||!valid_dims(w,h)){lodepng_state_cleanup(&state);return luaL_error(L,"PNG invalid or oversized");}
  state.decoder.zlibsettings.max_output_size=(size_t)w*h*8+h*8+65536;
  state.info_raw.colortype=LCT_RGBA;state.info_raw.bitdepth=8;uint8_t*rgba=NULL;
  err=lodepng_decode(&rgba,&w,&h,&state,f->data,n);lodepng_state_cleanup(&state);
  if(err){mosaico_image_realloc(rgba,0);return luaL_error(L,"PNG decode failed (%d)",err);}
  /* Explicit extension policy: composite PNG alpha over black, rounded. */
  for(size_t i=0;i<(size_t)w*h;i++){unsigned a=rgba[4*i+3];for(unsigned c=0;c<3;c++)rgba[3*i+c]=(rgba[4*i+c]*a+127)/255;}
  mosaico_image_realloc(f->data,0);f->data=rgba;f->bytes=(size_t)w*h*3;f->w=w;f->h=h;f->format=LUA_IMAGE_FORMAT_RGB888;
 }else{
  uint16_t w=0,h=0;int err=mosaico_jpeg_decode_tracked(f->data,n,&w,&h,NULL,0);
  if(err||!valid_dims(w,h))return luaL_error(L,"JPEG invalid or oversized (%d)",err);
  f->w=w;f->h=h;f->format=LUA_IMAGE_FORMAT_JPEG;
 }
 return 1;
}
static int resize(lua_State*L){frame_t*f=check(L,1);luaL_checktype(L,2,LUA_TTABLE);
 lua_getfield(L,2,"width");lua_Integer wi=luaL_checkinteger(L,-1);lua_pop(L,1);lua_getfield(L,2,"height");lua_Integer hi=luaL_checkinteger(L,-1);lua_pop(L,1);
 if(wi<1||hi<1||wi>65535||hi>65535||!valid_dims(wi,hi))return luaL_error(L,"resize dimensions exceed 1920x1080 pixel budget");
 unsigned w=wi,h=hi;lua_getfield(L,2,"format");lua_Integer fmt=lua_isnil(L,-1)?(f->format==LUA_IMAGE_FORMAT_GRAY8?LUA_IMAGE_FORMAT_GRAY8:LUA_IMAGE_FORMAT_RGB565LE):luaL_checkinteger(L,-1);lua_pop(L,1);
 if(fmt!=LUA_IMAGE_FORMAT_RGB565LE&&fmt!=LUA_IMAGE_FORMAT_GRAY8)return luaL_error(L,"resize format must be RGB565 or GRAY8");
 lua_getfield(L,2,"filter");const char*filter=luaL_optstring(L,-1,"nearest");int bilinear=!strcmp(filter,"bilinear");if(!bilinear&&strcmp(filter,"nearest"))return luaL_error(L,"unknown resize filter");lua_pop(L,1);
 /* Register destination before any decode that can fail. */
 frame_t*out=push(L,w,h,fmt,(size_t)w*h*bpp(fmt));uint8_t*src;int owned,err=rgb(f,&src,&owned);if(err)return luaL_error(L,"image decode failed (%d)",err);
 for(unsigned y=0;y<h;y++)for(unsigned x=0;x<w;x++){
  unsigned c[3];if(!bilinear){size_t i=((size_t)y*f->h/h*f->w+(size_t)x*f->w/w)*3;for(int k=0;k<3;k++)c[k]=src[i+k];}
  else {
   double sx=((x+0.5)*f->w/w)-0.5,sy=((y+0.5)*f->h/h)-0.5;if(sx<0)sx=0;if(sy<0)sy=0;if(sx>f->w-1)sx=f->w-1;if(sy>f->h-1)sy=f->h-1;
   unsigned x0=sx,y0=sy,x1=x0+1<f->w?x0+1:x0,y1=y0+1<f->h?y0+1:y0;double dx=sx-x0,dy=sy-y0;
   for(int k=0;k<3;k++)c[k]=(unsigned)(src[((size_t)y0*f->w+x0)*3+k]*(1-dx)*(1-dy)+src[((size_t)y0*f->w+x1)*3+k]*dx*(1-dy)+src[((size_t)y1*f->w+x0)*3+k]*(1-dx)*dy+src[((size_t)y1*f->w+x1)*3+k]*dx*dy+0.5);
  }
  pixel(out->data+((size_t)y*w+x)*bpp(fmt),fmt,c[0],c[1],c[2]);
 }
 if(owned)mosaico_image_realloc(src,0);return 1;
}
static int save_file(lua_State*L){
 const char*path=luaL_checkstring(L,1);frame_t*f=check(L,2);const char*ext=strrchr(path,'.');
 if(!ext||(strcmp(ext,".jpg")&&strcmp(ext,".jpeg")&&strcmp(ext,".JPG")&&strcmp(ext,".JPEG")))return luaL_error(L,"save_file supports .jpg/.jpeg only");
 lua_image_view_t view;int status=convert_view(f,LUA_IMAGE_FORMAT_JPEG,&view);if(status)return luaL_error(L,"image.save_file conversion failed (%d)",status);
 FILE*file=fopen(path,"wb");if(!file){lua_image_release_view(&view);return luaL_error(L,"image output open failed");}
 size_t n=fwrite(view.data,1,view.bytes,file);int err=fclose(file);size_t expected=view.bytes;lua_image_release_view(&view);
 if(n!=expected||err)return luaL_error(L,"image output write failed");return 0;
}
int luaopen_image(lua_State*L){
 if(luaL_newmetatable(L,FRAME_MT)){lua_pushcfunction(L,release);lua_setfield(L,-2,"__gc");lua_pushcfunction(L,release);lua_setfield(L,-2,"__close");lua_newtable(L);luaL_Reg methods[]={{"info",info},{"data",data},{"release",release},{NULL,NULL}};luaL_setfuncs(L,methods,0);lua_setfield(L,-2,"__index");}lua_pop(L,1);
 luaL_Reg funcs[]={{"load_file",load_file},{"resize",resize},{"convert",convert},{"save_file",save_file},{NULL,NULL}};luaL_newlib(L,funcs);
 const char*constants[]={"RGB565","RGB565_BE","RGB888","BGR888","GRAY8","YUYV","UYVY","JPEG","MJPEG"};for(int i=0;i<LUA_IMAGE_FORMAT_COUNT;i++){lua_pushinteger(L,i);lua_setfield(L,-2,constants[i]);}return 1;
}
