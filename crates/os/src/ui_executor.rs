//! UI and Lua stay on one executor pinned to Core1. Hardware drivers and their
//! interrupt bindings stay on Core0; commands and frame leases cross by channel.

use core::alloc::Layout;
use esp_hal::system::{Cpu, Stack};

const STACK_BYTES: usize = 24 * 1024;

pub fn start(
    cpu_control: esp_hal::peripherals::CPU_CTRL<'static>,
    plane: crate::tasks::plane::PlaneUi,
) {
    let layout = Layout::new::<Stack<STACK_BYTES>>();
    // The stack must remain accessible while either core's cache is disabled.
    // Leak this one internal allocation for the lifetime of the second core.
    let stack =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::Internal.into(), layout) };
    if stack.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    let stack = unsafe {
        let ptr = stack.cast::<Stack<STACK_BYTES>>();
        ptr.write(Stack::new());
        &mut *ptr
    };
    crate::drivers::storage::enable_multicore();
    esp_rtos::start_second_core(cpu_control, stack, move || {
        assert_eq!(Cpu::current(), Cpu::AppCpu);
        crate::resident_budget::init();
        let executor =
            alloc::boxed::Box::leak(alloc::boxed::Box::new(esp_rtos::embassy::Executor::new()));
        executor.run(|spawner| {
            spawner.spawn(crate::tasks::ui::ui_task(plane).unwrap());
            spawner.spawn(crate::tasks::lua::lua_task().unwrap());
            log::info!(
                "UI/Lua executor on {:?}, internal stack={} bytes; drivers/USB/radio on Core0",
                Cpu::current(),
                STACK_BYTES
            );
        });
    });
}
