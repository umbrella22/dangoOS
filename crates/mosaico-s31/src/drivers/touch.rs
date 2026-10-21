//! CST9220 电容触摸驱动（ESP-Mosaico v1.2, I2C0 @ SDA=56/SCL=3, 400kHz）
//!
//! 蓝本: 官方组件 `espressif/esp_lcd_touch_cst9220` v0.1.1（逐条对齐其流程）。
//! 16 位寄存器空间的 I2C 访问约定（组件 `cst9220_write_command`）:
//!   - 写寄存器 0xD1F8 = I2C WRITE [0xD1, 0xF8]
//!   - 读寄存器 0xD1F8 = 先 WRITE [0xD1, 0xF8]，再裸 READ n 字节（两次独立事务）
//!
//! 协议分 legacy(0x542F) 与 HYN212(0x6854/未知) 两代。官方注释明确:
//! 未知 project id 一律走 HYN212 路径（报告读后必须 ACK），避免 D11E
//! 探测失败卡死帧循环。本驱动跟随该策略。
//!
//! 触摸坐标 12-bit 拼装; 报告点 0 与帧头重叠（前 5 字节既是 header 也是点 0）。
//!
//! 只有异步路径: 所有访问经 `shared_i2c` 的异步事务助手（单次超时 100 ms），
//! 等待用 `embassy_time::Timer`，不阻塞其他任务。

use embassy_time::{Duration, Timer};
use esp_hal::i2c::master::Error as I2cError;

// ---- 寄存器（16-bit, 高字节为 I2C 首字节）----
const REG_REPORT: u16 = 0xD000;
const REG_DEBUG_INFO_MODE: u16 = 0xD101; // 进入报告页（当前固件从 D101 页读）
const REG_NORMAL_MODE: u16 = 0xD109;
const REG_PROTOCOL_PROBE: u16 = 0xD11E;
const REG_MODE_STATUS: u16 = 0x0002;
const REG_RESOLUTION: u16 = 0xD1F8;
const REG_PROJECT_ID: u16 = 0xD204;

const CHIP_ID: u16 = 0x9220;
const REPORT_ACK: u8 = 0xAB;
const POINT_SIZE: usize = 5;
/// 官方组件 MAX_POINTS = 2 → 报告 2×5+5 = 15 字节
const MAX_POINTS: usize = 2;
const REPORT_LEN: usize = MAX_POINTS * POINT_SIZE + 5;
const PROBE_RETRIES: u8 = 3;
const MODE_SETTLE_MS: u32 = 10;

mod protocol;
pub use protocol::TouchState;

/// 无效数据不代表手指抬起，调用方应保留上一次有效指针状态。
#[derive(Debug, Clone, Copy)]
pub enum TouchError {
    InvalidReport(protocol::ReportError),
    I2c {
        stage: &'static str,
        error: I2cError,
    },
}

impl core::fmt::Display for TouchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidReport(error) => write!(f, "invalid report: {error:?}"),
            Self::I2c { stage, error } => write!(f, "I2C {stage}: {error}"),
        }
    }
}

pub struct Cst9220 {
    addr: u8,
    /// legacy 协议不需要报告 ACK
    ack_reports: bool,
}

impl Cst9220 {
    /// 探测并初始化（总线经 `shared_i2c` 与 IMU 共享 I2C0）。
    pub async fn new() -> Option<Self> {
        let addr = crate::board::TOUCH_I2C_ADDR;
        let mut dev = Self {
            addr,
            ack_reports: true,
        };

        if let Err(stage) = dev.identify().await {
            log::warn!("触摸探测 0x{addr:02X}: {stage}");
            return None;
        }
        log::info!(
            "CST9220 已就绪 (I2C=400kHz @0x{addr:02X}, protocol={})",
            if dev.ack_reports { "HYN212" } else { "legacy" }
        );
        Some(dev)
    }

    /// 读一份报告。错误单独返回，避免制造虚假的 PointerReleased。
    ///
    /// 报告读是一笔独立事务（写寄存器地址 + 裸读），ACK 是下一笔事务。
    pub async fn poll(&mut self) -> Result<TouchState, TouchError> {
        let mut report = [0u8; REPORT_LEN];
        self.read_reg(REG_REPORT, &mut report)
            .await
            .map_err(|e| TouchError::I2c {
                stage: "report",
                error: e,
            })?;

        // 释放当前帧: 非 legacy 协议读后必须 ACK，否则控制器保持报告到内部超时
        if self.ack_reports {
            self.write_reg_raw(REG_REPORT, &[REPORT_ACK])
                .await
                .map_err(|e| TouchError::I2c {
                    stage: "report_ack",
                    error: e,
                })?;
        }

        protocol::decode(&report, crate::board::LCD_WIDTH, crate::board::LCD_HEIGHT)
            .map_err(TouchError::InvalidReport)
    }

    // ---- 官方 identify 流程: D11E 探测 → D109 正常 → D101 报告页 → 身份读取 ----
    async fn identify(&mut self) -> Result<(), &'static str> {
        let mut command_mode = false;
        for _ in 0..PROBE_RETRIES {
            if self
                .write_reg_raw(REG_PROTOCOL_PROBE, &[])
                .await
                .is_err()
            {
                continue;
            }
            Timer::after(Duration::from_millis(1)).await;
            let mut mode = [0u8; 2];
            if self
                .read_reg(REG_MODE_STATUS, &mut mode)
                .await
                .is_ok()
                && mode[1] == REG_PROTOCOL_PROBE as u8
            {
                command_mode = true;
                break;
            }
        }
        if command_mode {
            self.write_reg_raw(REG_NORMAL_MODE, &[])
                .await
                .map_err(|_| "正常模式恢复失败")?;
            Timer::after(Duration::from_millis(1)).await;
        }

        // 进入报告页（官方: 当前固件从 D101 页读取）
        self.write_reg_raw(REG_DEBUG_INFO_MODE, &[])
            .await
            .map_err(|_| "报告页进入失败")?;
        Timer::after(Duration::from_millis(MODE_SETTLE_MS as u64)).await;

        // 分辨率（非零校验, 官方拒绝 0x0）
        let mut res = [0u8; 4];
        self.read_reg(REG_RESOLUTION, &mut res)
            .await
            .map_err(|_| "分辨率读取失败")?;
        let (rx, ry) = (
            u16::from_le_bytes([res[0], res[1]]),
            u16::from_le_bytes([res[2], res[3]]),
        );
        if rx == 0 || ry == 0 {
            return Err("控制器分辨率无效");
        }
        log::info!("CST9220 分辨率: {rx}x{ry}");

        // project id + chip id（chip 必须 0x9220）
        let mut info = [0u8; 4];
        self.read_reg(REG_PROJECT_ID, &mut info)
            .await
            .map_err(|_| "项目信息读取失败")?;
        let project = u16::from_le_bytes([info[0], info[1]]);
        let chip = u16::from_le_bytes([info[2], info[3]]);
        if chip != CHIP_ID {
            return Err("触摸芯片 ID 不匹配");
        }
        // legacy(0x542F) 免 ACK; 其余(含未知)按 HYN212 走（官方策略）
        self.ack_reports = project != 0x542F;
        log::info!("CST9220 project=0x{project:04X} chip=0x{chip:04X}");

        // 遗留协议读后回正常模式; 现行固件留在报告页轮询
        if !self.ack_reports {
            self.write_reg_raw(REG_NORMAL_MODE, &[])
                .await
                .map_err(|_| "正常模式进入失败")?;
            Timer::after(Duration::from_millis(MODE_SETTLE_MS as u64)).await;
        }
        Ok(())
    }

    /// 写 16 位寄存器（可带参数）: I2C WRITE [reg_hi, reg_lo, 参数...]
    async fn write_reg_raw(&mut self, reg: u16, params: &[u8]) -> Result<(), I2cError> {
        let mut buf = [0u8; 4];
        let len = protocol::register_command(reg, params, &mut buf);
        match super::shared_i2c::write_async(self.addr, &buf[..len]).await {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded),
        }
    }

    /// 读 16 位寄存器: 先写寄存器地址, 再裸读（两次独立事务，与官方组件一致）。
    async fn read_reg(&mut self, reg: u16, data: &mut [u8]) -> Result<(), I2cError> {
        self.write_reg_raw(reg, &[]).await?;
        match super::shared_i2c::read_async(self.addr, data).await {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded),
        }
    }
}
