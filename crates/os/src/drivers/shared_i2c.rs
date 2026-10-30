//! 共享 I2C 总线: I2C0 挂触摸(CST9220 @0x5A)、BMI270 与 BMM150。
//!
//! 总线装在一个异步槽里（[`install_async`]），由 embassy 异步互斥锁保护。
//!
//! 异步事务助手 [`write_async`]/[`read_async`]/[`write_read_async`] 会在
//! 整笔事务期间持有互斥锁（CST9220 的一次写或读即一次事务），并用
//! [`TRANSACTION_TIMEOUT`] 限制每次事务的软件超时。传感器读取复用
//! 同一把锁与同一组助手。
//!
//! v1.2 的扩展槽是**独立的 I2C1**（SDA=GPIO0 / SCL=GPIO1，见
//! `docs/hardware/board-v1.2.md` §7.1 与 BSP `subboard.c`），因此
//! 模块 EEPROM 走 [`install_subboard_async`] 安装的第二条异步总线，避免与
//! 触摸/IMU 的 I2C0 争用。两条总线各有一把互斥锁和同样的 100 ms 事务超时。

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    mutex::{Mutex as AsyncMutex, MutexGuard},
    semaphore::{FairSemaphore, Semaphore, SemaphoreReleaser},
};
use embassy_time::{with_timeout, Duration, Instant};
use esp_hal::{
    i2c::master::{Config as I2cConfig, Error as I2cError, I2c},
    time::Rate,
    Async,
};

/// R1 异步总线类型（接口冻结: `SharedI2c = I2c<'static, Async>`）。
pub type SharedI2c = I2c<'static, Async>;

/// Main I2C0 frequency installed by `main` (CST9220/BMI270/BMM150).
pub const MAIN_BUS_FREQUENCY_KHZ: u32 = 400;

/// 单次异步 I2C 事务的软件超时（接口冻结要求 100 ms）。
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_millis(100);

/// 异步槽位的存储单元。
///
/// esp-hal 把 [`esp_hal::Async`] 标记为 `!Send`（异步驱动的中断处理器绑定到
/// 初始化核心），因此冻结的字面类型
/// `Mutex<CriticalSectionRawMutex, RefCell<Option<I2c<Async>>>>` 无法实现
/// `Sync`，不能作为 `static`。内部仍是同一个
/// `RefCell<Option<I2c<'static, Async>>>`，只是用下面的 `Send` 包装满足
/// 静态变量的约束。
struct AsyncSlot(RefCell<Option<SharedI2c>>);

// SAFETY: I2C 驱动、使用它的硬件任务和异步中断都留在 Core0；Core1 的
// UI/Lua 通过命令与状态通道访问硬件，不调用本模块的总线助手。`AsyncSlot`
// 只被本模块的静态持有，所有访问都经过对应异步互斥锁串行化，且从不移动
// 到其他核心。这避开了 esp-hal 用 `!Send` 防止的跨核使用场景。
unsafe impl Send for AsyncSlot {}

/// R1 异步槽位（I2C0: 触摸/IMU/磁力计共享）。
static SHARED_I2C_ASYNC: AsyncMutex<CriticalSectionRawMutex, AsyncSlot> =
    AsyncMutex::new(AsyncSlot(RefCell::new(None)));

// I2C0 has three concurrent users: touch, sensors and the audio codec. Boot
// initialization is the fourth possible caller. FIFO admission prevents a
// sensor transaction chain from reacquiring the non-fair mutex ahead of touch.
type MainAdmission = FairSemaphore<CriticalSectionRawMutex, 4>;
static MAIN_ADMISSION: MainAdmission = MainAdmission::new(1);
struct MainBusGuard {
    // Drop the slot before returning its admission permit.
    slot: MutexGuard<'static, CriticalSectionRawMutex, AsyncSlot>,
    _admission: SemaphoreReleaser<'static, MainAdmission>,
}
impl core::ops::Deref for MainBusGuard {
    type Target = AsyncSlot;
    fn deref(&self) -> &AsyncSlot {
        &self.slot
    }
}
async fn main_bus_lock() -> MainBusGuard {
    let admission = MAIN_ADMISSION
        .acquire(1)
        .await
        .expect("I2C0 has at most four concurrent callers");
    let slot = SHARED_I2C_ASYNC.lock().await;
    MainBusGuard {
        slot,
        _admission: admission,
    }
}

/// v1.2 扩展槽异步槽位（I2C1: 左右槽 EEPROM 与模块外设）。
static SHARED_I2C_SUBBOARD_ASYNC: AsyncMutex<CriticalSectionRawMutex, AsyncSlot> =
    AsyncMutex::new(AsyncSlot(RefCell::new(None)));

/// 把初始化好的异步模式 I2C 总线注册为共享（R1 路径）。
///
/// 必须在任何异步事务开始前调用一次。
pub fn install_async(i2c: SharedI2c) {
    install_slot(&SHARED_I2C_ASYNC, i2c, "shared I2C async slot");
}

/// 把 v1.2 扩展槽 I2C1 注册为共享。
///
/// 只有模块扫描/描述符读取使用这条总线；触摸、IMU、BMM150 仍在 I2C0。
pub fn install_subboard_async(i2c: SharedI2c) {
    install_slot(&SHARED_I2C_SUBBOARD_ASYNC, i2c, "subboard I2C async slot");
}

fn install_slot(
    slot: &'static AsyncMutex<CriticalSectionRawMutex, AsyncSlot>,
    i2c: SharedI2c,
    name: &'static str,
) {
    let guard = slot
        .try_lock()
        .expect("shared I2C slot is locked during install");
    assert!(guard.0.borrow().is_none(), "{name} was already installed");
    *guard.0.borrow_mut() = Some(i2c);
}

/// 异步写事务，持有总线直到完成；总线未安装时返回 `None`。
pub async fn write_async(addr: u8, bytes: &[u8]) -> Option<Result<(), I2cError>> {
    let guard = main_bus_lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, bytes)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

/// 异步读事务，持有总线直到完成；总线未安装时返回 `None`。
pub async fn read_async(addr: u8, bytes: &mut [u8]) -> Option<Result<(), I2cError>> {
    let guard = main_bus_lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.read_async(addr, bytes)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

/// CST9220 requires STOP between register selection, report read and ACK.
/// Retain the bus across those transactions so sensor polling cannot delay
/// the report's ACK or replace an unread release with a later contact.
pub async fn touch_report_async(
    addr: u8,
    register: u16,
    report: &mut [u8],
    ack: Option<u8>,
) -> Option<Result<(), I2cError>> {
    let queued = Instant::now();
    let guard = main_bus_lock().await;
    let wait_us = queued.elapsed().as_micros();
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    let mut phases = [0u64; 3];
    let result = async {
        let started = Instant::now();
        let result = with_timeout(
            TRANSACTION_TIMEOUT,
            i2c.write_async(addr, &register.to_be_bytes()),
        )
        .await;
        phases[0] = started.elapsed().as_micros();
        result.unwrap_or(Err(I2cError::Timeout))?;
        let started = Instant::now();
        let result = with_timeout(TRANSACTION_TIMEOUT, i2c.read_async(addr, report)).await;
        phases[1] = started.elapsed().as_micros();
        result.unwrap_or(Err(I2cError::Timeout))?;
        if let Some(ack) = ack {
            let [hi, lo] = register.to_be_bytes();
            let started = Instant::now();
            let result =
                with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, &[hi, lo, ack])).await;
            phases[2] = started.elapsed().as_micros();
            result.unwrap_or(Err(I2cError::Timeout))?;
        }
        Ok(())
    }
    .await;
    drop(borrow);
    drop(guard);
    // Rate-limited phase receipts distinguish bus contention from executor
    // delays without adding work to the ISR or changing transfer deadlines.
    static LAST_RECEIPT_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let now_ms = Instant::now().as_millis() as u32;
    let previous = LAST_RECEIPT_MS.load(core::sync::atomic::Ordering::Relaxed);
    if wait_us + phases.iter().sum::<u64>() >= 30_000 && now_ms.wrapping_sub(previous) >= 1000 {
        LAST_RECEIPT_MS.store(now_ms, core::sync::atomic::Ordering::Relaxed);
        log::info!(
            "touch bus: wait_us={wait_us} select_us={} read_us={} ack_us={} ok={}",
            phases[0],
            phases[1],
            phases[2],
            result.is_ok()
        );
    }
    Some(result)
}

/// 异步写后读（单次重复起始事务），持有总线直到完成；总线未安装时返回
/// `None`。传感器寄存器读用它；CST9220 需要两次独立事务，见 `touch.rs`。
pub async fn write_read_async(
    addr: u8,
    write: &[u8],
    read: &mut [u8],
) -> Option<Result<(), I2cError>> {
    let guard = main_bus_lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.write_read_async(addr, write, read)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

/// 读 8 位寄存器空间的设备（BMI270/BMM150）：单次重复起始事务，
/// `write` 只发送寄存器地址，`read` 接收数据。持有总线至事务结束。
///
/// 事务结束后主动 `yield_now`：传感器任务在初始化阶段会连续发起数百笔
/// 短事务，若不显式让出，释放互斥锁后可能在同一轮 poll 内立即重新获取，
/// 使等待锁的触摸任务得不到执行机会。让出后触摸路径优先获得总线。
pub async fn read_regs_async(addr: u8, reg: u8, data: &mut [u8]) -> Option<Result<(), I2cError>> {
    let result = write_read_async(addr, &[reg], data).await;
    embassy_futures::yield_now().await;
    result
}

/// 写 8 位寄存器空间的设备：单笔 `[reg, value]` 事务；事务后让出执行权。
pub async fn write_reg_async(addr: u8, reg: u8, value: u8) -> Option<Result<(), I2cError>> {
    let result = write_async(addr, &[reg, value]).await;
    embassy_futures::yield_now().await;
    result
}

/// Diagnostic read: register write with STOP, then a separate read with STOP.
///
/// The normal [`read_regs_async`] path uses a repeated start (Bosch
/// `bmm150_i2c_read` / ESP-IDF `i2c_master_transmit_receive`). This variant is
/// used only to determine whether a BMM150 that ACKs but returns `0x00`
/// needs the register write and data read as two independent transactions.
pub async fn read_regs_split_async(
    addr: u8,
    reg: u8,
    data: &mut [u8],
) -> Option<Result<(), I2cError>> {
    read_regs_mode_async(addr, reg, data, MAIN_BUS_FREQUENCY_KHZ, true).await
}

/// Repeated-start read at an alternate SCL frequency. The shared I2C0 is
/// restored to [`MAIN_BUS_FREQUENCY_KHZ`] before the lock is released.
pub async fn read_regs_at_khz_async(
    addr: u8,
    reg: u8,
    data: &mut [u8],
    khz: u32,
) -> Option<Result<(), I2cError>> {
    read_regs_mode_async(addr, reg, data, khz, false).await
}

/// Split (STOP between phases) read at an alternate SCL frequency.
pub async fn read_regs_split_at_khz_async(
    addr: u8,
    reg: u8,
    data: &mut [u8],
    khz: u32,
) -> Option<Result<(), I2cError>> {
    read_regs_mode_async(addr, reg, data, khz, true).await
}

/// Register write at an alternate SCL frequency; restores 400 kHz afterwards.
pub async fn write_reg_at_khz_async(
    addr: u8,
    reg: u8,
    value: u8,
    khz: u32,
) -> Option<Result<(), I2cError>> {
    let guard = main_bus_lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    let changed = khz != MAIN_BUS_FREQUENCY_KHZ;
    if changed && !set_main_frequency(i2c, khz) {
        return Some(Err(I2cError::ExecutionIncomplete));
    }
    let result = with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, &[reg, value]))
        .await
        .unwrap_or(Err(I2cError::Timeout));
    if changed {
        let _ = set_main_frequency(i2c, MAIN_BUS_FREQUENCY_KHZ);
    }
    Some(result)
}

async fn read_regs_mode_async(
    addr: u8,
    reg: u8,
    data: &mut [u8],
    khz: u32,
    split: bool,
) -> Option<Result<(), I2cError>> {
    let guard = main_bus_lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    let changed = khz != MAIN_BUS_FREQUENCY_KHZ;
    if changed && !set_main_frequency(i2c, khz) {
        return Some(Err(I2cError::ExecutionIncomplete));
    }

    let result = if split {
        match with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, &[reg])).await {
            Ok(Ok(())) => {
                match with_timeout(TRANSACTION_TIMEOUT, i2c.read_async(addr, data)).await {
                    Ok(result) => result,
                    Err(_) => Err(I2cError::Timeout),
                }
            }
            Ok(Err(error)) => Err(error),
            Err(_) => Err(I2cError::Timeout),
        }
    } else {
        match with_timeout(
            TRANSACTION_TIMEOUT,
            i2c.write_read_async(addr, &[reg], data),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        }
    };

    if changed {
        let _ = set_main_frequency(i2c, MAIN_BUS_FREQUENCY_KHZ);
    }
    Some(result)
}

/// Apply `khz` to the shared main bus. No-op when the frequency is unchanged.
fn set_main_frequency(i2c: &mut SharedI2c, khz: u32) -> bool {
    let config = I2cConfig::default().with_frequency(Rate::from_khz(khz));
    i2c.apply_config(&config).is_ok()
}

// ---------------------------------------------------------------------------
// v1.2 扩展槽 I2C1 事务助手
// ---------------------------------------------------------------------------

pub async fn subboard_write_async(addr: u8, bytes: &[u8]) -> Option<Result<(), I2cError>> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, bytes)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

pub async fn subboard_read_async(addr: u8, bytes: &mut [u8]) -> Option<Result<(), I2cError>> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.read_async(addr, bytes)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

pub async fn subboard_write_read_async(
    addr: u8,
    write: &[u8],
    read: &mut [u8],
) -> Option<Result<(), I2cError>> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut()?;
    Some(
        match with_timeout(TRANSACTION_TIMEOUT, i2c.write_read_async(addr, write, read)).await {
            Ok(result) => result,
            Err(_) => Err(I2cError::Timeout),
        },
    )
}

/// 读扩展槽 8 位寄存器空间的设备（AT24C02 EEPROM）。
pub async fn subboard_read_regs_async(
    addr: u8,
    reg: u8,
    data: &mut [u8],
) -> Option<Result<(), I2cError>> {
    subboard_write_read_async(addr, &[reg], data).await
}

/// 写扩展槽 8 位寄存器空间的设备：单笔 `[reg, value]` 事务。
///
/// H1.2 的 BMM150 备用探测路径需要与主 I2C0 相同的寄存器协议；官方 BSP
/// 把两颗 BMM150 放在 I2C0，但本函数让模块侧 I2C1 也能用同一驱动。
pub async fn subboard_write_reg_async(
    addr: u8,
    reg: u8,
    value: u8,
) -> Option<Result<(), I2cError>> {
    subboard_write_async(addr, &[reg, value]).await
}

/// SCCB sensors use a 16-bit register address and the module's 100 kHz bus
/// rate. Restore I2C1 before releasing the lock, including on I/O failure.
pub async fn camera_read_reg(addr: u8, reg: u16) -> Result<u8, I2cError> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut().ok_or(I2cError::ExecutionIncomplete)?;
    i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .map_err(|_| I2cError::ExecutionIncomplete)?;
    let mut value = [0];
    let result = with_timeout(
        TRANSACTION_TIMEOUT,
        i2c.write_read_async(addr, &reg.to_be_bytes(), &mut value),
    )
    .await
    .unwrap_or(Err(I2cError::Timeout));
    let restored = i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(400)));
    restored.map_err(|_| I2cError::ExecutionIncomplete)?;
    result.map(|()| value[0])
}

pub async fn camera_write_reg(addr: u8, reg: u16, value: u8) -> Result<(), I2cError> {
    let [hi, lo] = reg.to_be_bytes();
    camera_write_bytes(addr, &[hi, lo, value]).await
}

pub async fn camera_write_reg8(addr: u8, reg: u8, value: u8) -> Result<(), I2cError> {
    camera_write_bytes(addr, &[reg, value]).await
}

async fn camera_write_bytes(addr: u8, bytes: &[u8]) -> Result<(), I2cError> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut().ok_or(I2cError::ExecutionIncomplete)?;
    i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .map_err(|_| I2cError::ExecutionIncomplete)?;
    let result = with_timeout(TRANSACTION_TIMEOUT, i2c.write_async(addr, bytes))
        .await
        .unwrap_or(Err(I2cError::Timeout));
    let restored = i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(400)));
    restored.map_err(|_| I2cError::ExecutionIncomplete)?;
    result
}

/// SC101IOT uses a page selector followed by an 8-bit register read. Both
/// phases share the lock; these are not OV3640-style 16-bit addresses.
pub async fn camera_read_paged(addr: u8, reg: u16) -> Result<u8, I2cError> {
    let guard = SHARED_I2C_SUBBOARD_ASYNC.lock().await;
    let mut borrow = guard.0.borrow_mut();
    let i2c = borrow.as_mut().ok_or(I2cError::ExecutionIncomplete)?;
    i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .map_err(|_| I2cError::ExecutionIncomplete)?;
    let mut value = [0];
    let result = with_timeout(TRANSACTION_TIMEOUT, async {
        i2c.write_async(addr, &[0xf0, (reg >> 8) as u8]).await?;
        i2c.write_read_async(addr, &[reg as u8], &mut value).await
    })
    .await
    .unwrap_or(Err(I2cError::Timeout));
    i2c.apply_config(&I2cConfig::default().with_frequency(Rate::from_khz(400)))
        .map_err(|_| I2cError::ExecutionIncomplete)?;
    result.map(|()| value[0])
}
