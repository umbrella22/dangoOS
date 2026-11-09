#include <assert.h>
#include <stdbool.h>
#include <stddef.h>

#define UNUSED(x) ((void)(x))
#define BTA_INCLUDED 1
#define TRUE 1
#define BT_STATUS_SUCCESS 0
#define BT_STATUS_NOMEM 1
#define HCI_TRACE_ERROR(...) ((void)0)

static bool core_live, stack_live, bta_live, fail_stack;
static unsigned initialized, freed, completed;
static int last_status;
static void btu_init_core(void) { assert(!core_live); core_live = true; initialized++; }
static void btu_free_core(void) { assert(core_live); core_live = false; freed++; }
static int BTE_InitStack(void) { stack_live = !fail_stack; return fail_stack ? 1 : 0; }
static void BTE_DeinitStack(void) { stack_live = false; }
static void bta_sys_init(void) { assert(stack_live); bta_live = true; }
static void bta_sys_free(void) { bta_live = false; }
static void init_done(int status) { last_status = status; completed++; }
static void (*bluedroid_init_done_cb)(int) = init_done;

/* PRODUCTION_IMPLEMENTATION */

int main(void)
{
    /* Failed BTU task/queue allocation: startup has never executed. */
    btu_task_shut_down();
    btu_task_shut_down();
    assert(initialized == 0 && freed == 0);

    btu_task_start_up(NULL);
    assert(last_status == BT_STATUS_SUCCESS && core_live && bta_live);
    btu_task_shut_down();
    btu_task_shut_down();
    assert(initialized == 1 && freed == 1 && !core_live && !stack_live && !bta_live);

    /* Partial startup still releases the core that was actually created. */
    fail_stack = true;
    btu_task_start_up(NULL);
    assert(last_status == BT_STATUS_NOMEM && core_live && !bta_live);
    btu_task_shut_down();
    btu_task_shut_down();
    assert(initialized == 2 && freed == 2);

    fail_stack = false;
    btu_task_start_up(NULL);
    assert(last_status == BT_STATUS_SUCCESS && completed == 3);
    btu_task_shut_down();
    assert(initialized == 3 && freed == 3);
    return 0;
}
