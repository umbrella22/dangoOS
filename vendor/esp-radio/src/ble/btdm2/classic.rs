//! S31 BR/EDR port for the esp-wifi-sys 5afc3a3 controller ABI.
//! The host supplies the official TinyCrypt callbacks through esp-bluedroid.
use super::*;
mod pool;
static POOL_LOCK: esp_sync::RawMutex = esp_sync::RawMutex::new();

/// Same four-argument ABI as co_assert.c in the pinned controller archive.
/// A real controller failure remains fatal, with its cause retained over reset.
#[unsafe(no_mangle)]
unsafe extern "C" fn r_co_assert_info(file: *const c_char, line: u32, a: u32, b: u32) -> ! {
    let file = unsafe { str_from_c(file) };
    panic!("BR/EDR controller assertion {file}:{line} a={a:#x} b={b:#x}")
}

unsafe extern "C" {
    fn bredr_controller_init(cfg: *mut esp_bredr_controller_config_t) -> i32;
    fn bredr_controller_deinit();
    fn bredr_register_setup_callback(cb: unsafe extern "C" fn() -> i32);
    fn bredr_register_ext_dep_callback(cb: unsafe extern "C" fn() -> i32);
    fn bredr_register_crypto_funcs(ops: *const c_void) -> i32;
    fn bredr_register_bb_funcs(ops: *const BbOps) -> i32;
    fn bredr_ctrl_feat_coex_en() -> i32;
    fn bt_stack_enableSecCtrlVsCmd(enable: bool);
    static MOSAICO_BREDR_CRYPTO_OPS: u8;
    fn bredr_tx_cca_fifo_empty() -> bool;
    fn bredr_tx_cca_fifo_full() -> bool;
    fn bredr_tx_cca_fifo_count() -> u32;
    fn bredr_tx_cca_fifo_read() -> u32;
    fn bredr_tx_cca_period(ru_time: u32, cca_time: u32);
    fn bredr_tx_cca_set(
        enable: u32,
        low: u32,
        drop0: u32,
        high0: u32,
        low0: u32,
        high_en: u32,
        high: u32,
        drop1: u32,
        high1: u32,
        low1: u32,
    );
    fn bredr_tx_cca_fifo_reset();
    fn bredr_set_tx_on_delay(delay: u32);
}
#[repr(C)]
struct CcaParams {
    enable: u32,
    low: u32,
    drop0: u32,
    high0: u32,
    low0: u32,
    high_en: u32,
    high: u32,
    drop1: u32,
    high1: u32,
    low1: u32,
}
#[repr(C)]
struct BbOps {
    empty: unsafe extern "C" fn() -> bool,
    full: unsafe extern "C" fn() -> bool,
    count: unsafe extern "C" fn() -> u32,
    read: unsafe extern "C" fn() -> u32,
    period: unsafe extern "C" fn(u32, u32),
    set: unsafe extern "C" fn(*const CcaParams),
    reset: unsafe extern "C" fn(),
    delay: unsafe extern "C" fn(u32),
}
unsafe extern "C" fn cca_set(p: *const CcaParams) {
    unsafe {
        let p = &*p;
        bredr_tx_cca_set(
            p.enable, p.low, p.drop0, p.high0, p.low0, p.high_en, p.high, p.drop1, p.high1, p.low1,
        );
    }
}
static BB: BbOps = BbOps {
    empty: bredr_tx_cca_fifo_empty,
    full: bredr_tx_cca_fifo_full,
    count: bredr_tx_cca_fifo_count,
    read: bredr_tx_cca_fifo_read,
    period: bredr_tx_cca_period,
    set: cca_set,
    reset: bredr_tx_cca_fifo_reset,
    delay: bredr_set_tx_on_delay,
};

unsafe extern "C" fn pool_create(mem: *mut c_void, bytes: usize, max: usize) -> *mut c_void {
    let pool = POOL_LOCK.lock(|| unsafe { pool::create(mem, bytes) });
    if pool.is_null() {
        panic!("BR/EDR pool rejected base={mem:p} bytes={bytes} max={max}");
    }
    pool
}
unsafe extern "C" fn pool_destroy(mem: *mut c_void) {
    POOL_LOCK.lock(|| unsafe { pool::destroy(mem) });
}
unsafe extern "C" fn pool_malloc(mem: *mut c_void, size: usize) -> *mut c_void {
    POOL_LOCK.lock(|| unsafe { pool::malloc(mem, size) })
}
unsafe extern "C" fn pool_free(mem: *mut c_void, p: *mut c_void) {
    POOL_LOCK.lock(|| unsafe { pool::free(mem, p) });
}
static POOL: orca_mempool_ops = orca_mempool_ops {
    create_with_pool: Some(pool_create),
    destroy: Some(pool_destroy),
    malloc: Some(pool_malloc),
    free: Some(pool_free),
};
unsafe extern "C" fn setup() -> i32 {
    #[cfg(feature = "coex")]
    unsafe {
        return bredr_ctrl_feat_coex_en();
    }
    #[cfg(not(feature = "coex"))]
    0
}
unsafe extern "C" fn dependencies() -> i32 {
    unsafe {
        let rc = bredr_register_crypto_funcs((&raw const MOSAICO_BREDR_CRYPTO_OPS).cast());
        if rc != 0 {
            return rc;
        }
        bredr_register_bb_funcs(&raw const BB)
    }
}
pub(super) unsafe fn init(cfg: *mut esp_bt_controller_config_t) -> i32 {
    unsafe {
        bredr_register_setup_callback(setup);
        bredr_register_ext_dep_callback(dependencies);
        (*cfg).bredr.mempool_size = 1024 * (u32::from((*cfg).bredr.max_acl_conn) + 2);
        (*cfg).bredr.mempool_ops = (&raw const POOL).cast_mut();
        let rc = bredr_controller_init(&raw mut (*cfg).bredr);
        if rc != 0 {
            bredr_controller_deinit();
        }
        rc
    }
}
pub(super) unsafe fn deinit() {
    unsafe {
        bredr_controller_deinit();
    }
}
pub(super) unsafe fn enable() {
    unsafe {
        bt_stack_enableSecCtrlVsCmd(true);
    }
}
pub(super) unsafe fn disable() {
    unsafe {
        bt_stack_enableSecCtrlVsCmd(false);
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bredr_random() -> u32 {
    unsafe { crate::common_adapter::random() as u32 }
}
