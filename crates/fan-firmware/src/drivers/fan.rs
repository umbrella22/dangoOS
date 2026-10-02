//! 风扇 PWM：RMT 连续波形（esp-hal 1.x 的 S31 暂无 LEDC/MCPWM 驱动）
//!
//! 25kHz 载波，~10bit 等效分辨率：
//!   RMT 全局计数时钟 40MHz（25ns/tick），通道分频 1
//!   单周期 40us = 1600 tick，高电平 tick 数即占空比
//! 波形 = [周期符号, 结束符]，LoopMode::Infinite 下硬件循环发送；
//! 占空比更新 = 停止 → 改符号 → 重启（风扇无感毛刺）。

#[cfg(feature = "fan-pwm-gpio12")]
use esp_hal::{
    gpio::interconnect::PeripheralOutput,
    rmt::{Rmt, TxChannelConfig, TxChannelCreator},
    time::Rate,
};
use esp_hal::{
    gpio::Level,
    rmt::{Channel, ContinuousTxTransaction, LoopMode, PulseCode, Tx},
    Blocking,
};
#[cfg(feature = "fan-pwm-gpio12")]
use log::info;

const PERIOD_TICKS: u16 = 1600; // 40us @ 40MHz = 25kHz

pub struct FanPwm {
    tx: Option<ContinuousTxTransaction<'static>>,
    channel: Option<Channel<'static, Blocking, Tx>>,
    target: u8,
    cur: u8,
}

impl FanPwm {
    #[cfg(feature = "fan-pwm-gpio12")]
    pub fn new(
        pin: impl PeripheralOutput<'static>,
        rmt: esp_hal::peripherals::RMT<'static>,
    ) -> Self {
        let rmt = Rmt::new(rmt, Rate::from_mhz(40)).expect("RMT 初始化失败");

        let channel = rmt
            .channel0
            .configure_tx(&TxChannelConfig::default().with_clk_divider(1))
            .expect("RMT 通道配置失败")
            .with_pin(pin);

        info!("风扇 PWM 就绪 (RMT, 25kHz)");
        Self {
            tx: None,
            channel: Some(channel),
            target: 0,
            cur: 0,
        }
    }

    /// UI 下发目标占空比（-1 = AUTO）
    #[allow(dead_code)]
    pub fn set_target(&mut self, pct: i32, auto_duty: u8) {
        self.target = if pct < 0 {
            auto_duty
        } else {
            pct.clamp(0, 100) as u8
        };
    }

    /// 主循环周期调用: 占空比向目标平滑趋近，变化时重启波形
    pub fn tick(&mut self) {
        if self.cur != self.target {
            if self.cur < self.target {
                self.cur = (self.cur + 2).min(self.target);
            } else {
                self.cur = self.cur.saturating_sub(2).max(self.target);
            }
            self.apply();
        }
    }

    pub fn duty(&self) -> u8 {
        self.cur
    }

    fn apply(&mut self) {
        // 先停掉上一次的无限循环，拿回通道
        if let Some(tx) = self.tx.take() {
            self.channel = Some(match tx.stop() {
                Ok(ch) => ch,
                Err((_, ch)) => ch, // 错误态也把通道还回来
            });
        }
        let Some(ch) = self.channel.take() else {
            return;
        };

        let high = ((self.cur as u32) * PERIOD_TICKS as u32 / 100) as u16;
        let low = PERIOD_TICKS - high;
        // 连续发送要求数据以结束符收尾；单符号即可表达整个周期
        let waveform = [
            if high == 0 {
                PulseCode::new(Level::Low, PERIOD_TICKS / 2, Level::Low, PERIOD_TICKS / 2)
            } else if low == 0 {
                PulseCode::new(Level::High, PERIOD_TICKS / 2, Level::High, PERIOD_TICKS / 2)
            } else {
                PulseCode::new(Level::High, high, Level::Low, low)
            },
            PulseCode::end_marker(),
        ];
        let tx = ch
            .transmit_continuously(&waveform, LoopMode::Infinite)
            .expect("RMT 连续发送失败");
        self.tx = Some(tx);
    }
}
