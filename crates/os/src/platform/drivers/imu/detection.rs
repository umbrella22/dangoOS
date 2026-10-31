//! BMM150 identification and fallback transfer modes.
use super::{
    bmm150::*,
    bus::{bus_read_regs, bus_write_reg, MagBus, MagReadMode},
};
use crate::platform::buses::shared_i2c;
use embassy_time::{Duration, Timer};

/// Result of the startup probe ladder for one address.
pub(super) struct ProbeResult {
    pub(super) chip_id: u8,
    pub(super) read_mode: MagReadMode,
}

/// Read one register with a specific transfer form. Only the main I2C0 has
/// alternate-form helpers; the subboard bus always uses repeated start.
async fn read_identity(bus: MagBus, addr: u8, reg: u8, mode: MagReadMode) -> Option<u8> {
    let mut data = [0u8; 1];
    let result = match (bus, mode) {
        (MagBus::Main, MagReadMode::RepeatedStart) => {
            shared_i2c::read_regs_async(addr, reg, &mut data).await
        }
        (MagBus::Main, MagReadMode::Split) => {
            shared_i2c::read_regs_split_async(addr, reg, &mut data).await
        }
        (MagBus::Main, MagReadMode::RepeatedStart100k) => {
            shared_i2c::read_regs_at_khz_async(addr, reg, &mut data, mode.khz()).await
        }
        (MagBus::Main, MagReadMode::Split100k) => {
            shared_i2c::read_regs_split_at_khz_async(addr, reg, &mut data, mode.khz()).await
        }
        (MagBus::Subboard, _) => shared_i2c::subboard_read_regs_async(addr, reg, &mut data).await,
    };
    match result {
        Some(Ok(())) => Some(data[0]),
        _ => None,
    }
}

/// Bosch `bmm150_init` step: read `0x4B`, set the power-control bit, wait
/// 3 ms, then read the chip ID. Returns `(old_0x4B, chip_id)`.
async fn bmm_power_up(bus: MagBus, addr: u8) -> (Option<u8>, Option<u8>) {
    let mut power = [0u8; 1];
    let old = match bus_read_regs(bus, addr, BMM_POWER_CONTROL, &mut power).await {
        Some(Ok(())) => Some(power[0]),
        _ => None,
    };
    // Bosch reads 0x4B and sets bit 0. If that read failed, write the enable
    // bit directly rather than skipping the power-up.
    let new = old
        .map(|value| value | BMM_POWER_CNTRL_ENABLE)
        .unwrap_or(BMM_POWER_CNTRL_ENABLE);
    if !matches!(
        bus_write_reg(bus, addr, BMM_POWER_CONTROL, new).await,
        Some(Ok(()))
    ) {
        return (old, None);
    }
    // Bosch BMM150_START_UP_TIME = 3000 us.
    Timer::after(Duration::from_micros(3_000)).await;
    let id = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart).await;
    (old, id)
}

/// Bosch `bmm150_soft_reset`: `0x4B |= 0x82`, wait 1 ms, then power up again.
/// Returns the chip ID after the following 3 ms power-up delay.
async fn bmm_soft_reset_and_power_up(bus: MagBus, addr: u8) -> Option<u8> {
    let mut power = [0u8; 1];
    if !matches!(
        bus_read_regs(bus, addr, BMM_POWER_CONTROL, &mut power).await,
        Some(Ok(()))
    ) {
        return None;
    }
    if !matches!(
        bus_write_reg(bus, addr, BMM_POWER_CONTROL, power[0] | BMM_SOFT_RESET).await,
        Some(Ok(()))
    ) {
        return None;
    }
    // Bosch BMM150_DELAY_SOFT_RESET = 1000 us.
    Timer::after(Duration::from_micros(1_000)).await;
    let (_, id) = bmm_power_up(bus, addr).await;
    id
}

/// Power-up variant that also moves the register write to 100 kHz, for the
/// case where the 400 kHz write phase is the broken part of the transfer.
async fn bmm_power_up_100k(bus: MagBus, addr: u8) -> Option<u8> {
    if bus != MagBus::Main {
        return None;
    }
    let mut power = [0u8; 1];
    if !matches!(
        shared_i2c::read_regs_at_khz_async(addr, BMM_POWER_CONTROL, &mut power, 100).await,
        Some(Ok(()))
    ) {
        return None;
    }
    if !matches!(
        shared_i2c::write_reg_at_khz_async(
            addr,
            BMM_POWER_CONTROL,
            power[0] | BMM_POWER_CNTRL_ENABLE,
            100
        )
        .await,
        Some(Ok(()))
    ) {
        return None;
    }
    Timer::after(Duration::from_micros(3_000)).await;
    read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart100k).await
}

/// Startup probe ladder for one address.
///
/// Order: plain repeated-start read, split STOP/STOP read, Bosch power-up,
/// Bosch soft reset + power-up, 100 kHz repeated start, 100 kHz split. The
/// first form that yields `0x32` is kept for normal operation.
pub(super) async fn probe_candidate(bus: MagBus, addr: u8) -> Option<ProbeResult> {
    let initial = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart).await?;

    if initial == BMM150_CHIP_ID {
        return Some(ProbeResult {
            chip_id: initial,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    // Power/reset writes are only safe on the official BMM150 addresses.
    if !matches!(addr, 0x11 | 0x12) {
        return Some(ProbeResult {
            chip_id: initial,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;

    if split == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    let (_, power_id) = bmm_power_up(bus, addr).await;

    if power_id == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split_powered = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;

    if split_powered == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    let reset_id = bmm_soft_reset_and_power_up(bus, addr).await;

    if reset_id == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart,
        });
    }

    let split_reset = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::Split).await;

    if split_reset == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split,
        });
    }

    let power_100k = bmm_power_up_100k(bus, addr).await;

    if power_100k == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart100k,
        });
    }

    let id_100k = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::RepeatedStart100k).await;

    if id_100k == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::RepeatedStart100k,
        });
    }

    let split_100k = read_identity(bus, addr, BMM_CHIP_ID, MagReadMode::Split100k).await;

    if split_100k == Some(BMM150_CHIP_ID) {
        return Some(ProbeResult {
            chip_id: BMM150_CHIP_ID,
            read_mode: MagReadMode::Split100k,
        });
    }

    let final_id = split_100k
        .or(id_100k)
        .or(power_100k)
        .or(split_reset)
        .or(reset_id)
        .or(split_powered)
        .or(power_id)
        .or(split)
        .unwrap_or(initial);

    Some(ProbeResult {
        chip_id: final_id,
        read_mode: MagReadMode::RepeatedStart,
    })
}
