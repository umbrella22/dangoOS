//! The S31's own capacitive touch sensor (hardware version 3), register level.
//!
//! esp-hal does not drive the S31 touch block yet, so this is a direct port of
//! the IDF sequence (`esp_driver_touch_sens/hw_ver3` plus the S31
//! `touch_sensor_ll.h`): clock and reset, one sample configuration, the
//! hardware benchmark filter, then the FSM timer scanning the enabled
//! channels continuously. Software reads the smooth data and the benchmark
//! and decides presses itself (`subboard::touch_key`).
//!
//! Channel `n` is `GPIO(6 + n)` (`touch_sensor_channel.h`). The pins must be
//! floating with their input buffer off, which is the state
//! `InteractionPort` leaves idle pins in.
//!
//! Only one Interaction board is read at a time, so one owner calls
//! [`start`] / [`stop`]; [`start`] always resets the block first.

use esp32s31::{LP_PERICLKRST, LP_TOUCH, LP_TOUCH_AON, PMU};
use esp_hal::delay::Delay;

/// GPIO range wired to touch channels.
const FIRST_TOUCH_GPIO: u8 = 6;
const CHANNEL_COUNT: u8 = 14;

/// Touch channel of a GPIO, if it has one.
pub const fn channel_of_gpio(gpio: u8) -> Option<u8> {
    if gpio >= FIRST_TOUCH_GPIO && gpio < FIRST_TOUCH_GPIO + CHANNEL_COUNT {
        Some(gpio - FIRST_TOUCH_GPIO)
    } else {
        None
    }
}

// IDF defaults (`TOUCH_SENSOR_DEFAULT_BASIC_CONFIG`, the v3 example sample
// configuration 0 and `TOUCH_SENSOR_DEFAULT_FILTER_CONFIG`).
/// 256 us at the ~20 MHz RTC fast clock.
const POWER_ON_WAIT_TICKS: u16 = 5120;
/// 32 us at the ~136 kHz RTC slow clock.
const MEASURE_INTERVAL_TICKS: u16 = 4;
const OUT_AS_CLOCK: u8 = 1;
const CLOCK_DIV: u8 = 8;
const CHARGE_TIMES: u16 = 500;
const RC_FILTER_RES: u8 = 3;
const RC_FILTER_CAP: u8 = 29;
const DRIVE_HIGH: u8 = 8;
const DRIVE_LOW: u8 = 3;
const BIAS_VOLT: u8 = 5;
const BM_IIR_FILTER_4: u8 = 0;
const SMOOTH_IIR_FILTER_2: u8 = 1;
/// Denoise level 1 -> noise coefficient `3 - 1`.
const NOISE_THRESH: u8 = 2;
const ACTIVE_HYSTERESIS: u8 = 2;
const DEBOUNCE_CNT: u8 = 2;
const NULL_CHANNEL: u8 = 15;

const READ_BENCHMARK: u8 = 2;
const READ_SMOOTH: u8 = 3;

/// Hardware threshold before calibration: the largest value, so the
/// benchmark follows the raw data freely while it settles.
pub const THRESHOLD_UNSET: u16 = u16::MAX;

/// One channel reading.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    pub smooth: u16,
    pub benchmark: u16,
}

/// Reset the block and scan `channels` continuously.
pub fn start(channels: &[u8]) {
    let mask = channels.iter().fold(0u16, |m, &ch| m | (1 << ch));
    // SAFETY: the touch block is owned by the single active Interaction
    // port; nothing else in the firmware touches these registers.
    unsafe {
        let clk = LP_PERICLKRST::steal();
        let aon = LP_TOUCH_AON::steal();
        let sens = LP_TOUCH::steal();
        let pmu = PMU::steal();

        // touch_priv_enable_module + touch_ll_reset_module
        clk.touch_ctrl()
            .modify(|_, w| w.lp_touch_clk_en().set_bit());
        aon.work().modify(|_, w| w.touch_out_gate().set_bit());
        clk.touch_ctrl()
            .modify(|_, w| w.lp_touch_rst_en().set_bit());
        clk.touch_ctrl()
            .modify(|_, w| w.lp_touch_rst_en().clear_bit());

        // touch_hal_config_controller
        aon.scan_ctrl1()
            .modify(|_, w| w.touch_xpd_wait().bits(POWER_ON_WAIT_TICKS));
        pmu.touch_pwr_ctrl()
            .modify(|_, w| w.touch_sleep_cycles().bits(MEASURE_INTERVAL_TICKS));
        aon.slp0()
            .modify(|_, w| w.touch_slp_pad().bits(NULL_CHANNEL));
        aon.scan_ctrl2().modify(|_, w| {
            w.touch_timeout_en()
                .clear_bit()
                .freq_scan_en()
                .set_bit()
                .freq_scan_cnt_limit()
                .bits(1)
                .freq_scan_cnt_rise()
                .bits(1)
        });
        aon.work().modify(|_, w| {
            w.touch_out_sel()
                .bit(OUT_AS_CLOCK != 0)
                .div_num0()
                .bits(CLOCK_DIV - 1)
        });
        aon.work_meas_num()
            .modify(|_, w| w.touch_meas_num0().bits(CHARGE_TIMES));
        aon.freq0_scan_para().modify(|_, w| {
            w.touch_freq0_dcap_lpf()
                .bits(RC_FILTER_CAP)
                .touch_freq0_dres_lpf()
                .bits(RC_FILTER_RES)
                .touch_freq0_drv_ls()
                .bits(DRIVE_LOW)
                .touch_freq0_drv_hs()
                .bits(DRIVE_HIGH)
                .touch_freq0_dbias()
                .bits(BIAS_VOLT)
        });

        // touch_sensor_config_filter
        aon.filter2().modify(|_, w| {
            w.touch_bypass_noise_thres()
                .clear_bit()
                .touch_bypass_nn_thres()
                .clear_bit()
        });
        aon.filter1().modify(|_, w| {
            w.touch_filter_en()
                .set_bit()
                .touch_filter_mode()
                .bits(BM_IIR_FILTER_4)
                .touch_noise_thres()
                .bits(NOISE_THRESH)
                .touch_nn_thres()
                .bits(NOISE_THRESH)
                .touch_nn_limit()
                .bits(5)
                .touch_smooth_lvl()
                .bits(SMOOTH_IIR_FILTER_2)
                .touch_hysteresis()
                .bits(ACTIVE_HYSTERESIS)
                .touch_debounce_limit()
                .bits(DEBOUNCE_CNT)
        });

        // touch_sensor_new_channel: threshold, then reset the benchmark.
        for &ch in channels {
            set_threshold(ch, THRESHOLD_UNSET);
        }
        aon.clr().write(|w| w.touch_channel_clr().bits(mask));

        // touch_sensor_enable
        aon.scan_ctrl1()
            .modify(|_, w| w.touch_scan_pad_map().bits(mask));
        aon.filter2().modify(|_, w| w.touch_outen().bits(mask));
        sens.int_clr().write(|w| w.bits(0x7f));

        // touch_sensor_start_continuous_scanning
        aon.mux0().modify(|_, w| {
            w.touch_fsm_en()
                .set_bit()
                .touch_done_force()
                .clear_bit()
                .touch_start_force()
                .clear_bit()
        });
        force_done(&pmu);
        pmu.touch_pwr_ctrl()
            .modify(|_, w| w.touch_sleep_timer_en().set_bit());
    }
}

/// Stop scanning, release the pads and gate the clock.
pub fn stop() {
    // SAFETY: see `start`.
    unsafe {
        let aon = LP_TOUCH_AON::steal();
        let pmu = PMU::steal();
        pmu.touch_pwr_ctrl()
            .modify(|_, w| w.touch_sleep_timer_en().clear_bit());
        force_done(&pmu);
        aon.scan_ctrl1()
            .modify(|_, w| w.touch_scan_pad_map().bits(0));
        aon.filter2().modify(|_, w| w.touch_outen().bits(0));
        aon.work().modify(|_, w| w.touch_out_gate().clear_bit());
        LP_PERICLKRST::steal()
            .touch_ctrl()
            .modify(|_, w| w.lp_touch_clk_en().clear_bit());
    }
}

/// `force done` must last at least one slow clock tick.
fn force_done(pmu: &PMU) {
    pmu.touch_pwr_ctrl()
        .modify(|_, w| w.touch_force_done().set_bit());
    Delay::new().delay_micros(10);
    pmu.touch_pwr_ctrl()
        .modify(|_, w| w.touch_force_done().clear_bit());
}

/// Hardware active threshold of sample configuration 0. The benchmark stops
/// following the data while it is above this.
pub fn set_threshold(ch: u8, threshold: u16) {
    // SAFETY: see `start`.
    let aon = unsafe { LP_TOUCH_AON::steal() };
    let t = threshold;
    // SAFETY: every 16-bit value is a valid threshold.
    unsafe {
        match ch {
            0 => aon.pad0_th0().write(|w| w.touch_pad0_th0().bits(t)),
            1 => aon.pad1_th0().write(|w| w.touch_pad1_th0().bits(t)),
            2 => aon.pad2_th0().write(|w| w.touch_pad2_th0().bits(t)),
            3 => aon.pad3_th0().write(|w| w.touch_pad3_th0().bits(t)),
            4 => aon.pad4_th0().write(|w| w.touch_pad4_th0().bits(t)),
            5 => aon.pad5_th0().write(|w| w.touch_pad5_th0().bits(t)),
            6 => aon.pad6_th0().write(|w| w.touch_pad6_th0().bits(t)),
            7 => aon.pad7_th0().write(|w| w.touch_pad7_th0().bits(t)),
            8 => aon.pad8_th0().write(|w| w.touch_pad8_th0().bits(t)),
            9 => aon.pad9_th0().write(|w| w.touch_pad9_th0().bits(t)),
            10 => aon.pad10_th0().write(|w| w.touch_pad10_th0().bits(t)),
            11 => aon.pad11_th0().write(|w| w.touch_pad11_th0().bits(t)),
            12 => aon.pad12_th0().write(|w| w.touch_pad12_th0().bits(t)),
            13 => aon.pad13_th0().write(|w| w.touch_pad13_th0().bits(t)),
            _ => return,
        };
    }
}

/// Smooth data and benchmark of sample configuration 0.
pub fn read(ch: u8) -> Reading {
    Reading {
        smooth: read_data(ch, READ_SMOOTH),
        benchmark: read_data(ch, READ_BENCHMARK),
    }
}

fn read_data(ch: u8, kind: u8) -> u16 {
    // SAFETY: see `start`. The data selector only affects these reads.
    let (aon, sens) = unsafe { (LP_TOUCH_AON::steal(), LP_TOUCH::steal()) };
    // SAFETY: 0 and 2/3 are valid selector values.
    aon.mux0()
        .modify(|_, w| unsafe { w.touch_freq_sel().bits(0).touch_data_sel().bits(kind) });
    match ch {
        0 => sens.status_0().read().pad0_data().bits(),
        1 => sens.status_1().read().pad1_data().bits(),
        2 => sens.status_2().read().pad2_data().bits(),
        3 => sens.status_3().read().pad3_data().bits(),
        4 => sens.status_4().read().pad4_data().bits(),
        5 => sens.status_5().read().pad5_data().bits(),
        6 => sens.status_6().read().pad6_data().bits(),
        7 => sens.status_7().read().pad7_data().bits(),
        8 => sens.status_8().read().pad8_data().bits(),
        9 => sens.status_9().read().pad9_data().bits(),
        10 => sens.status_10().read().pad10_data().bits(),
        11 => sens.status_11().read().pad11_data().bits(),
        12 => sens.status_12().read().pad12_data().bits(),
        13 => sens.status_13().read().pad13_data().bits(),
        _ => 0,
    }
}

/// Hardware active mask (bit `n` = channel `n`), for logs.
pub fn active_mask() -> u16 {
    // SAFETY: read-only status register.
    unsafe { LP_TOUCH::steal() }
        .chn_status()
        .read()
        .pad_active()
        .bits()
}
