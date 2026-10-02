//! 共享 I2C 总线(M3): 触摸(CST9220 @0x5A)与传感器(BMI270 @0x69)同挂 I2C0。
//! esp-hal I2c 不可 Clone, 用 critical-section Mutex 单线程互斥共享。
//! 采样率低(触摸 8ms 轮询 / IMU 10Hz), 锁竞争可忽略。

use core::cell::RefCell;
use critical_section::Mutex;
use esp_hal::i2c::master::I2c;
use esp_hal::Blocking;

static SHARED_I2C: Mutex<RefCell<Option<I2c<'static, Blocking>>>> = Mutex::new(RefCell::new(None));

/// 把初始化好的 I2C 总线注册为共享。
pub fn install(i2c: I2c<'static, Blocking>) {
    critical_section::with(|cs| {
        SHARED_I2C.borrow_ref_mut(cs).replace(i2c);
    });
}

/// 独占访问共享总线执行一个操作(锁内完成, 不跨 await)。
pub fn with_bus<R>(f: impl FnOnce(&mut I2c<'static, Blocking>) -> R) -> Option<R> {
    critical_section::with(|cs| {
        let mut borrow = SHARED_I2C.borrow_ref_mut(cs);
        borrow.as_mut().map(f)
    })
}
