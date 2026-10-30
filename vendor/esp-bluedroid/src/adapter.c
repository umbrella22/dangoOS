// SPDX-License-Identifier: Apache-2.0
#include <stdarg.h>
#include <stdio.h>
#include "esp_heap_caps.h"
#include "esp_log.h"
extern void mosaico_bt_log_bytes(int,const char*,const char*);
void mosaico_bt_log(int level,const char *tag,const char *fmt,...){
    if(level>ESP_LOG_WARN)return;
    char text[320];va_list args;va_start(args,fmt);vsnprintf(text,sizeof(text),fmt,args);va_end(args);
    mosaico_bt_log_bytes(level,tag,text);
}
void *mosaico_bt_heap_malloc_prefer(size_t size,size_t count,...){
    va_list args;va_start(args,count);void *p=NULL;
    for(size_t i=0;i<count;i++){unsigned caps=va_arg(args,unsigned);if(!p)p=mosaico_bt_heap_malloc(size,caps);}
    va_end(args);return p;
}
void *mosaico_bt_heap_calloc_prefer(size_t num,size_t size,size_t count,...){
    va_list args;va_start(args,count);void *p=NULL;
    for(size_t i=0;i<count;i++){unsigned caps=va_arg(args,unsigned);if(!p)p=mosaico_bt_heap_calloc(num,size,caps);}
    va_end(args);return p;
}
