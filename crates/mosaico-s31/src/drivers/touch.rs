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
//! R1 异步路径: [`Cst9220::poll_async`] 通过 `shared_i2c` 的异步事务助手
//! 访问同一协议；寄存器序列、ACK 与解码逻辑与阻塞 [`Cst9220::poll`] 完全
//! 一致。异步事务的单次超时为 100 ms。

use embassy_time::{Duration, Timer};
use esp_hal::i2c::master::Error as I2cError;

// ---- 寄存器（16-bit, 高字节为 I2C 首字节）----
const REG_REPORT: u16 = 0xD000;
const REG_DEBUG_INFO_MODE: u16 = 0xD101; // 进入报告页（当前固件从 D101 页读）
/// 睡眠命令预留（v1 轮询场景不需要; 官方 enter_sleep 用它）
#[allow(dead_code)]
const REG_SLEEP_MODE: u16 = 0xD105;
const REG_NORMAL_MODE: u16 = 0xD109;
const REG_PROTOCOL_PROBE: u16 = 0xD11E;
const REG_MODE_STATUS: u16 = 0x0002;
/// checkcode 仅用于官方日志识别，v1 不读取
#[allow(dead_code)]
const REG_CHECKCODE: u16 = 0xD1FC;
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

/// 触摸统计(部分字段由 M2 心跳明细消费)
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct TouchStats {
    pub polls: u32,
    pub down_reports: u32,
    pub i2c_errors: u32,
    pub invalid_reports: u32,
    pub coordinate_errors: u32,
    pub last_status: u8,
    pub last_down_status: u8,
    pub last_x: u16,
    pub last_y: u16,
    pub last_error: Option<TouchError>,
    /// 自开机以来的最长采样间隔和单次 I2C 读取耗时，包含中断抢占。
    pub poll_gap_max_us: u64,
    pub read_max_us: u64,
}

pub struct Cst9220 {
    addr: u8,
    /// legacy 协议不需要报告 ACK
    ack_reports: bool,
    stats: TouchStats,
    last_poll_us: Option<u64>,
}

/// 异步模式下的 CST9220 句柄（R1 `input_task` 使用）。
///
/// 与阻塞驱动共用同一个状态机；用 [`Cst9220::new_async`] 构造，然后调用
/// [`Cst9220::poll_async`]。阻塞 [`Cst9220::poll`] 仍保留给 R0 路径。
pub type Cst9220Async = Cst9220;

fn now_us() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_micros()
}

impl Cst9220 {
    /// 探测并初始化。总线经 `shared_i2c` 独占访问(触摸+IMU 共挂 I2C0)。
    pub fn new() -> Option<Self> {
        // HAL 接受 7-bit 地址，线上读写位由 HAL 编码。
        let addr = crate::board::TOUCH_I2C_ADDR;
        let mut dev = Self {
            addr,
            ack_reports: true,
            stats: TouchStats::default(),
            last_poll_us: None,
        };

        if let Err(stage) = dev.identify() {
            log::warn!("触摸探测 0x{addr:02X}: {stage}");
            return None;
        }
        log::info!(
            "CST9220 已就绪 (I2C=400kHz @0x{addr:02X}, protocol={})",
            if dev.ack_reports { "HYN212" } else { "legacy" }
        );
        Some(dev)
    }

    /// 异步探测并初始化（R1 路径，使用 `shared_i2c` 异步槽）。
    ///
    /// 寄存器序列与 [`Cst9220::new`] 相同；等待改为 `embassy_time::Timer`，
    /// 不阻塞其他任务。
    pub async fn new_async() -> Option<Self> {
        let addr = crate::board::TOUCH_I2C_ADDR;
        let mut dev = Self {
            addr,
            ack_reports: true,
            stats: TouchStats::default(),
            last_poll_us: None,
        };

        if let Err(stage) = dev.identify_async().await {
            log::warn!("触摸探测 0x{addr:02X}: {stage}");
            return None;
        }
        log::info!(
            "CST9220 已就绪 (I2C=400kHz @0x{addr:02X}, protocol={})",
            if dev.ack_reports { "HYN212" } else { "legacy" }
        );
        Some(dev)
    }

    /// 主循环轮询（R0 阻塞路径）。错误单独返回，避免制造虚假的 PointerReleased。
    pub fn poll(&mut self) -> Result<TouchState, TouchError> {
        let started = self.begin_poll();
        let res = self.poll_inner();
        self.finish_poll(started, res)
    }

    /// R1 异步轮询。错误类型与 [`Cst9220::poll`] 相同。
    ///
    /// 报告读是一笔独立事务（写寄存器地址 + 裸读），ACK 是下一笔事务，
    /// 与阻塞路径的两次事务序列一致。
    pub async fn poll_async(&mut self) -> Result<TouchState, TouchError> {
        let started = self.begin_poll();
        let res = self.poll_inner_async().await;
        self.finish_poll(started, res)
    }

    pub fn stats(&self) -> TouchStats {
        self.stats
    }

    /// 采样间隔统计: 记录本次轮询起点并返回起始微秒时间。
    fn begin_poll(&mut self) -> u64 {
        self.stats.polls += 1;
        let now = now_us();
        if let Some(last) = self.last_poll_us {
            let gap = now.saturating_sub(last);
            self.stats.poll_gap_max_us = self.stats.poll_gap_max_us.max(gap);
        }
        self.last_poll_us = Some(now);
        now
    }

    /// 记录单次读取耗时和错误计数。
    fn finish_poll(
        &mut self,
        started: u64,
        res: Result<TouchState, TouchError>,
    ) -> Result<TouchState, TouchError> {
        let read_us = now_us().saturating_sub(started);
        self.stats.read_max_us = self.stats.read_max_us.max(read_us);
        if let Err(e) = &res {
            if matches!(e, TouchError::I2c { .. }) {
                self.stats.i2c_errors += 1;
            }
            self.stats.last_error = Some(*e);
        }
        res
    }

    /// 解码一份完整报告并更新统计（阻塞/异步共用）。
    fn decode_report(&mut self, report: &[u8; REPORT_LEN]) -> Result<TouchState, TouchError> {
        match protocol::decode(report, crate::board::LCD_WIDTH, crate::board::LCD_HEIGHT) {
            Ok(state) => {
                if let TouchState::Down { x, y } = state {
                    self.stats.down_reports += 1;
                    self.stats.last_status = report[0];
                    self.stats.last_down_status = report[0];
                    self.stats.last_x = x;
                    self.stats.last_y = y;
                }
                Ok(state)
            }
            Err(error) => {
                self.stats.invalid_reports += 1;
                if error == protocol::ReportError::Coordinate {
                    self.stats.coordinate_errors += 1;
                }
                Err(TouchError::InvalidReport(error))
            }
        }
    }

    fn poll_inner(&mut self) -> Result<TouchState, TouchError> {
        let mut report = [0u8; REPORT_LEN];
        self.read_reg(REG_REPORT, &mut report)
            .map_err(|e| TouchError::I2c {
                stage: "report",
                error: e,
            })?;

        // 释放当前帧: 非 legacy 协议读后必须 ACK，否则控制器保持报告到内部超时
        if self.ack_reports {
            self.write_reg_raw(REG_REPORT, &[REPORT_ACK])
                .map_err(|e| TouchError::I2c {
                    stage: "report_ack",
                    error: e,
                })?;
        }

        self.decode_report(&report)
    }

    async fn poll_inner_async(&mut self) -> Result<TouchState, TouchError> {
        let mut report = [0u8; REPORT_LEN];
        self.read_reg_async(REG_REPORT, &mut report)
            .await
            .map_err(|e| TouchError::I2c {
                stage: "report",
                error: e,
            })?;

        // 释放当前帧: 非 legacy 协议读后必须 ACK，否则控制器保持报告到内部超时
        if self.ack_reports {
            self.write_reg_raw_async(REG_REPORT, &[REPORT_ACK])
                .await
                .map_err(|e| TouchError::I2c {
                    stage: "report_ack",
                    error: e,
                })?;
        }

        self.decode_report(&report)
    }

    // ---- 官方 identify 流程: D11E 探测 → D109 正常 → D101 报告页 → 身份读取 ----
    fn identify(&mut self) -> Result<(), &'static str> {
        let mut command_mode = false;
        for _ in 0..PROBE_RETRIES {
            if self.write_reg_raw(REG_PROTOCOL_PROBE, &[]).is_err() {
                continue;
            }
            esp_hal::delay::Delay::new().delay_millis(1);
            let mut mode = [0u8; 2];
            if self.read_reg(REG_MODE_STATUS, &mut mode).is_ok()
                && mode[1] == REG_PROTOCOL_PROBE as u8
            {
                command_mode = true;
                break;
            }
        }
        if command_mode {
            self.write_reg_raw(REG_NORMAL_MODE, &[])
                .map_err(|_| "正常模式恢复失败")?;
            esp_hal::delay::Delay::new().delay_millis(1);
        }

        // 进入报告页（官方: 当前固件从 D101 页读取）
        self.write_reg_raw(REG_DEBUG_INFO_MODE, &[])
            .map_err(|_| "报告页进入失败")?;
        esp_hal::delay::Delay::new().delay_millis(MODE_SETTLE_MS);

        // 分辨率（非零校验, 官方拒绝 0x0）
        let mut res = [0u8; 4];
        self.read_reg(REG_RESOLUTION, &mut res)
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
                .map_err(|_| "正常模式进入失败")?;
            esp_hal::delay::Delay::new().delay_millis(MODE_SETTLE_MS);
        }
        Ok(())
    }

    /// `identify` 的异步版本：逐条寄存器序列相同，只把等待改成异步定时器。
    async fn identify_async(&mut self) -> Result<(), &'static str> {
        let mut command_mode = false;
        for _ in 0..PROBE_RETRIES {
            if self
                .write_reg_raw_async(REG_PROTOCOL_PROBE, &[])
                .await
                .is_err()
            {
                continue;
            }
            Timer::after(Duration::from_millis(1)).await;
            let mut mode = [0u8; 2];
            if self
                .read_reg_async(REG_MODE_STATUS, &mut mode)
                .await
                .is_ok()
                && mode[1] == REG_PROTOCOL_PROBE as u8
            {
                command_mode = true;
                break;
            }
        }
        if command_mode {
            self.write_reg_raw_async(REG_NORMAL_MODE, &[])
                .await
                .map_err(|_| "正常模式恢复失败")?;
            Timer::after(Duration::from_millis(1)).await;
        }

        // 进入报告页（官方: 当前固件从 D101 页读取）
        self.write_reg_raw_async(REG_DEBUG_INFO_MODE, &[])
            .await
            .map_err(|_| "报告页进入失败")?;
        Timer::after(Duration::from_millis(MODE_SETTLE_MS as u64)).await;

        // 分辨率（非零校验, 官方拒绝 0x0）
        let mut res = [0u8; 4];
        self.read_reg_async(REG_RESOLUTION, &mut res)
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
        self.read_reg_async(REG_PROJECT_ID, &mut info)
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
            self.write_reg_raw_async(REG_NORMAL_MODE, &[])
                .await
                .map_err(|_| "正常模式进入失败")?;
            Timer::after(Duration::from_millis(MODE_SETTLE_MS as u64)).await;
        }
        Ok(())
    }

    /// 写 16 位寄存器（可带参数）: I2C WRITE [reg_hi, reg_lo, 参数...]
    fn write_reg_raw(&mut self, reg: u16, params: &[u8]) -> Result<(), I2cError> {
        let mut buf = [0u8; 4];
        let len = protocol::register_command(reg, params, &mut buf);
        match super::shared_i2c::with_bus(|i2c| i2c.write(self.addr, &buf[..len])) {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded), // 总线未注册(初始化顺序错误)
        }
    }

    /// 读 16 位寄存器: 先写寄存器地址, 再裸读（与官方两次事务一致）
    fn read_reg(&mut self, reg: u16, data: &mut [u8]) -> Result<(), I2cError> {
        self.write_reg_raw(reg, &[])?;
        match super::shared_i2c::with_bus(|i2c| i2c.read(self.addr, data)) {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded),
        }
    }

    /// 异步写 16 位寄存器（单笔 100 ms 超时事务）。
    async fn write_reg_raw_async(&mut self, reg: u16, params: &[u8]) -> Result<(), I2cError> {
        let mut buf = [0u8; 4];
        let len = protocol::register_command(reg, params, &mut buf);
        match super::shared_i2c::write_async(self.addr, &buf[..len]).await {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded),
        }
    }

    /// 异步读 16 位寄存器: 先写寄存器地址, 再裸读（两次独立事务，与阻塞
    /// 路径及官方组件一致；每次事务 100 ms 超时）。
    async fn read_reg_async(&mut self, reg: u16, data: &mut [u8]) -> Result<(), I2cError> {
        self.write_reg_raw_async(reg, &[]).await?;
        match super::shared_i2c::read_async(self.addr, data).await {
            Some(r) => r,
            None => Err(I2cError::FifoExceeded),
        }
    }
}
