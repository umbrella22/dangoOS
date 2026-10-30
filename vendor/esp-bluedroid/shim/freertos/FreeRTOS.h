#pragma once
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <assert.h>
#include "sdkconfig.h"
typedef int BaseType_t;
typedef unsigned UBaseType_t;
typedef uint32_t TickType_t;
typedef uint8_t StackType_t;
typedef void *TaskHandle_t;
typedef void *QueueHandle_t;
typedef void *SemaphoreHandle_t;
typedef void (*TaskFunction_t)(void *);
#define pdTRUE 1
#define pdFALSE 0
#define pdPASS 1
#define pdFAIL 0
#define portMAX_DELAY UINT32_MAX
#define portTICK_PERIOD_MS 1
#define portTICK_RATE_MS 1
#define pdMS_TO_TICKS(x) (x)
#define configTICK_RATE_HZ 1000
#define configMAX_PRIORITIES 25
#define tskNO_AFFINITY (-1)
#define configASSERT(x) assert(x)
#define xTaskCreatePinnedToCore mosaico_bt_task_create
#define vTaskDelete mosaico_bt_task_delete
#define vTaskDelay mosaico_bt_task_delay
#define vTaskPrioritySet mosaico_bt_task_priority
#define xTaskGetCurrentTaskHandle mosaico_bt_task_current
#define xTaskGetTickCount mosaico_bt_ticks
#define xQueueCreate mosaico_bt_queue_create
#define xQueueSend mosaico_bt_queue_send
#define xQueueSendToBack mosaico_bt_queue_send
#define xQueueReceive mosaico_bt_queue_receive
#define uxQueueSpacesAvailable mosaico_bt_queue_spaces
#define uxQueueMessagesWaiting mosaico_bt_queue_count
#define vQueueDelete mosaico_bt_queue_delete
#define xSemaphoreCreateMutex() mosaico_bt_sem_create(1,1,1)
#define xSemaphoreCreateRecursiveMutex() mosaico_bt_sem_create(1,1,2)
#define xSemaphoreCreateCounting(max,init) mosaico_bt_sem_create(max,init,0)
#define xSemaphoreTake mosaico_bt_sem_take
#define xSemaphoreGive mosaico_bt_sem_give
#define xSemaphoreTakeRecursive mosaico_bt_sem_take
#define xSemaphoreGiveRecursive mosaico_bt_sem_give
#define vSemaphoreDelete mosaico_bt_sem_delete
int mosaico_bt_task_create(TaskFunction_t,const char*,uint32_t,void*,unsigned,TaskHandle_t*,int);
void mosaico_bt_task_delete(TaskHandle_t);
void mosaico_bt_task_delay(uint32_t);
void mosaico_bt_task_priority(TaskHandle_t,unsigned);
TaskHandle_t mosaico_bt_task_current(void);
uint32_t mosaico_bt_ticks(void);
void *mosaico_bt_queue_create(unsigned,unsigned);
int mosaico_bt_queue_send(void*,const void*,uint32_t);
int mosaico_bt_queue_receive(void*,void*,uint32_t);
unsigned mosaico_bt_queue_spaces(void*);
unsigned mosaico_bt_queue_count(void*);
void mosaico_bt_queue_delete(void*);
void *mosaico_bt_sem_create(uint32_t,uint32_t,unsigned);
int mosaico_bt_sem_take(void*,uint32_t);
int mosaico_bt_sem_give(void*);
void mosaico_bt_sem_delete(void*);

#define pcTaskGetName mosaico_bt_task_name
const char *mosaico_bt_task_name(void*);

#define xSemaphoreCreateBinary() mosaico_bt_sem_create(1,0,0)
