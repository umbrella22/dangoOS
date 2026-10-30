// Real queue/list and HCI receive handler; only scheduler primitives are modeled.
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include "osi/mutex.h"
#include "osi/semaphore.h"

struct host_mutex { bool locked; };
struct host_sem { uint32_t count, maximum; };
static osi_mutex_t preempt_mutex;
static bool preempt_after_unlock;
static void run_hci_until_idle(void);

int osi_mutex_new(osi_mutex_t *mutex) {
    *mutex = calloc(1, sizeof(**mutex));
    return *mutex ? 0 : -1;
}
int osi_mutex_lock(osi_mutex_t *mutex, uint32_t timeout) {
    (void)timeout;
    assert(!(*mutex)->locked);
    (*mutex)->locked = true;
    return 0;
}
void osi_mutex_unlock(osi_mutex_t *mutex) {
    assert((*mutex)->locked);
    (*mutex)->locked = false;
    if (*mutex == preempt_mutex && preempt_after_unlock) {
        // Model an already-runnable higher-priority hciT immediately taking over
        // when the producer drops the queue mutex (including inherited priority).
        preempt_after_unlock = false;
        run_hci_until_idle();
    }
}
void osi_mutex_free(osi_mutex_t *mutex) { free(*mutex); *mutex = NULL; }
int osi_sem_new(osi_sem_t *sem, uint32_t maximum, uint32_t initial) {
    *sem = malloc(sizeof(**sem));
    assert(*sem);
    **sem = (struct host_sem){initial, maximum};
    return 0;
}
int osi_sem_take(osi_sem_t *sem, uint32_t timeout) {
    (void)timeout;
    if (!(*sem)->count) return -1;
    --(*sem)->count;
    return 0;
}
void osi_sem_give(osi_sem_t *sem) {
    assert((*sem)->count < (*sem)->maximum);
    ++(*sem)->count;
}
void osi_sem_free(osi_sem_t *sem) { free(*sem); *sem = NULL; }

#include FIXED_QUEUE_SOURCE

typedef struct { uint16_t len, offset; uint8_t data[4]; } BT_HDR;
static struct { fixed_queue_t *rx_q; } hci_hal_env;
static BT_HDR *delivered[4];
static size_t delivered_count, null_dequeues, handler_calls;
static bool repost;
#define TRUE 1
#define FALSE 0
#define BLE_42_SCAN_EN FALSE
#define C2H_FLOW_CONTROL_INCLUDED FALSE
#define OSI_THREAD_MAX_TIMEOUT UINT32_MAX
#define MOSAICO_BT_WORK_DIAGNOSTICS
static void rx_trace(unsigned phase, unsigned value) {
    (void)value;
    if (phase == 3) ++null_dequeues;
}
#define RX_TRACE rx_trace
static void hci_hal_h4_hdl_rx_packet(BT_HDR *packet) {
    assert(delivered_count < sizeof(delivered) / sizeof(delivered[0]));
    delivered[delivered_count++] = packet;
}
static bool hci_upstream_data_post(uint32_t timeout) {
    (void)timeout;
    repost = true;
    return true;
}
#include HCI_HANDLER_SOURCE

static void run_hci_until_idle(void) {
    do {
        repost = false;
        ++handler_calls;
        hci_upstream_data_handler(NULL);
        // The real priority scheduler would keep running hciT indefinitely.
        // Bound this model to report the failure rather than hanging the test.
        if (handler_calls == 16 && repost) return;
    } while (repost);
}

int main(void) {
    fixed_queue_t *queue = fixed_queue_new(2);
    assert(queue);
    BT_HDR first = {.len = 1, .data = {4}};
    BT_HDR second = {.len = 1, .data = {2}};
    assert(fixed_queue_enqueue(queue, &first, 0));
    hci_hal_env.rx_q = queue;
    preempt_mutex = queue->lock;
    preempt_after_unlock = true;
    assert(fixed_queue_enqueue(queue, &second, 0));
    if (repost || null_dequeues || delivered_count != 2) {
        fprintf(stderr, "HCI livelock: calls=%zu null=%zu delivered=%zu pending=%zu\n",
                handler_calls, null_dequeues, delivered_count, fixed_queue_length(queue));
        return 1;
    }
    assert(handler_calls == 1);
    assert(delivered[0] == &first && delivered[1] == &second);
    assert(fixed_queue_is_empty(queue));
    assert(queue->dequeue_sem->count == 0 && queue->enqueue_sem->count == 2);

    // Publication must retain bounded-capacity/FIFO/removal behavior.
    assert(fixed_queue_enqueue(queue, &first, 0));
    assert(fixed_queue_enqueue(queue, &second, 0));
    assert(!fixed_queue_enqueue(queue, &first, 0));
    assert(fixed_queue_try_remove_from_queue(queue, &second) == &second);
    assert(fixed_queue_dequeue(queue, 0) == &first);
    assert(fixed_queue_dequeue(queue, 0) == NULL);
    assert(queue->dequeue_sem->count == 0 && queue->enqueue_sem->count == 2);
    fixed_queue_free(queue, NULL);
    puts("producer preemption and HCI receive progress passed");
}
