//! 480×480 CO5300 QSPI AMOLED 驱动（ESP-Mosaico v1.2, 唯一显示管线）
//!
//! 蓝本（唯一事实来源，非 git 历史旧代码）:
//!   - 官方 BSP `esp-mosaico/esp-mosaico-bsp` v0.2.0 `onboard/display.c`
//!   - 官方 `esp-mosaico-claw` bootloader/mosaico_boot_splash.c
//!     (9ea326da1f2a017ba81f1d391862f977b355a03e): 单线命令头/四线像素
//!   - 官方组件 `espressif/esp_lcd_co5300` v2.2.0
//!   - 引脚: `docs/MOSAICO_V12_HARDWARE.md`（v1.2 映射）
//!
//! ┌────────────────────────────────────────────────────────────┐
//! │ 架构（QSPI 命令写 GRAM，无永续扫描）                          │
//! │                                                             │
//! │   Slint 渲染 → PSRAM 帧缓冲（单缓冲, RGB565 小端）           │
//! │        │  present(): 整帧分块, CPU 搬运时 LE→BE 换序          │
//! │        ▼                                                     │
//! │   内部 RAM STAGING (30720B) ←→ 内部 RAM DMA TX buf           │
//! │        │  SPI2 QSPI 40MHz, 4-lane, 每块 32 行                │
//! │        ▼                                                     │
//! │   CO5300 内置 GRAM → 面板自扫描                               │
//! └────────────────────────────────────────────────────────────┘
//!
//! 线序（对齐 esp_lcd_co5300 v2.2.0 `tx_param`/`tx_color`）:
//!   - 寄存器写: [单线] 0x02 → [单线] 24bit 地址 reg<<8 → [单线] 参数
//!   - GRAM 写:  [单线] 0x32 → [单线] 24bit 地址 0x2C<<8 → [四线] 像素
//!   （旧记忆里的 0x12 是组件 v2.1.0 之前的写法，v2.2.0 起为 0x32）
//!
//! GRAM 面板自扫描; present() 同步刷完整帧，尚未通过 TE 做防撕裂同步
//! （≈25ms @40MHz），期间触摸不轮询。
//! 脏区 x 必须 4 像素对齐（官方 BSP round_qspi_area 约束），
//! 当前整帧刷新不涉及；未来做局部刷新时必须遵守。

use core::{
    mem::ManuallyDrop,
    slice,
    sync::atomic::{AtomicUsize, Ordering},
};
use esp_hal::{
    dma_rx_buffer, dma_tx_buffer,
    gpio::interconnect::{PeripheralInput, PeripheralOutput},
    gpio::{Level, Output, OutputConfig},
    spi::master::{Address, Command, Config as SpiConfig, DataMode, Spi},
    spi::Mode,
    time::{Instant, Rate},
    Blocking,
};
use log::{error, info};
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board;

// ---- 传输参数 ----
/// 每块传输的行数: 32 行 = 30720B。内部 RAM 双份（STAGING + DMA buf）共 ~61KB，
/// 与 Korvo BB 64KB 同量级。块越大开销越小: 15 块/帧。
const CHUNK_ROWS: usize = 32;
const ROW_BYTES: usize = board::LCD_WIDTH as usize * 2;
const CHUNK_BYTES: usize = CHUNK_ROWS * ROW_BYTES;
/// 一次传输的 QSPI 时钟。官方 BSP 默认 40MHz（BSP_LCD_PIXEL_CLOCK_HZ）。
const SPI_CLOCK_HZ: u32 = 40_000_000;

// ---- CO5300 命令（与 board_mosaico::co5300 一致，此处为传输路径私有）----
const OP_WRITE_REG: u16 = 0x02; // 单线命令相位
const OP_WRITE_COLOR: u16 = 0x32; // v2.2.0 起 GRAM 写操作码
const GRAM_ADDR: u32 = 0x2C << 8; // 0x2C RAMWR, 地址相位单线
const CMD_CASET: u8 = 0x2A;
const CMD_RASET: u8 = 0x2B;

// ---- 厂商初始化序列（esp-mosaico-bsp display.c `s_vendor_init`，逐条对齐）----
// (reg, 参数, 延时 ms)。参数为空 = 无数据相位。
const VENDOR_INIT: &[(u8, &[u8], u32)] = &[
    (0x11, &[], 600),   // SLPOUT
    (0xFE, &[0x20], 0), // 页切换
    (0x19, &[0x10], 0),
    (0x1C, &[0xA0], 0),
    (0xFE, &[0x00], 0), // 页切回
    (0xC4, &[0x80], 0),
    (0x3A, &[0x55], 0), // RGB565
    (0x35, &[0x00], 0), // TE on（v1.2 TE 引脚已布线）
    (0x53, &[0x20], 0), // BCTRL: 打开亮度控制（0x51 生效前提）
    (0x51, &[0xFF], 0), // 亮度 255（初始化全亮，UI 起来后可调）
    (0x63, &[0xFF], 0),
    (0x2A, &[0x00, 0x00, 0x01, 0xDF], 0), // 全窗 0..479
    (0x2B, &[0x00, 0x00, 0x01, 0xDF], 0),
    (0x29, &[], 600), // DISPON
];

/// 硬件复位时序（esp_lcd_co5300 v2.2.0 `panel_co5300_reset`: 低 10ms + 150ms）
const RST_LOW_MS: u32 = 10;
const RST_SETTLE_MS: u32 = 150;

// ---- 统计（平台层心跳读取）----
static FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
static COPY_COUNT: AtomicUsize = AtomicUsize::new(0);
static COPY_TOTAL_US: AtomicUsize = AtomicUsize::new(0);
static COPY_MAX_US: AtomicUsize = AtomicUsize::new(0);
static ERR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 内部 RAM 搬运暂存：CPU 从 PSRAM 读出并 LE→BE 换序后交给 DMA。
/// 静态分配（.bss，内部 RAM），DMA 只读内部 RAM —— 与 Korvo BB 同一铁律
/// （官方 BSP 同样设 `psram_dma_direct = false`）。
static mut STAGING: [u8; CHUNK_BYTES] = [0; CHUNK_BYTES];

pub struct Display {
    /// PSRAM 帧缓冲（Slint 渲染目标，RGB565 小端）
    fb: ManuallyDrop<&'static mut [Rgb565Pixel]>,
    spi: esp_hal::spi::master::SpiDma<'static, Blocking>,
    /// 保持 RST 引脚配置不被回收
    _rst: Output<'static>,
    /// 最近一次发布结果，供可选的边沿轮询消费
    present_result: Option<bool>,
}

impl Display {
    /// Slint 渲染目标（前台缓冲）。
    pub fn fb_mut(&mut self) -> &mut [Rgb565Pixel] {
        &mut self.fb
    }

    /// 驱动没有正在运行的异步 DMA 发布。
    /// （present() 当前为同步实现, 保留此 API 以兼容未来异步刷帧）
    #[allow(dead_code)]
    pub fn is_presenting(&self) -> bool {
        false
    }

    /// Each DMA block has its own row window and RAMWR transaction. No
    /// assumptions about the panel preserving a RAMWR pointer across CS edges.
    /// Command headers are single-line [opcode, 0, register, 0]; pixels are quad.
    pub fn present(&mut self) -> Result<(), esp_hal::spi::Error> {
        let result = self.present_inner();
        self.present_result = Some(result.is_ok());
        if let Err(error) = &result {
            ERR_COUNT.fetch_add(1, Ordering::Relaxed);
            error!("GRAM frame failed: {error:?}; retain framebuffer for retry");
        }
        result
    }

    fn present_inner(&mut self) -> Result<(), esp_hal::spi::Error> {
        let started = Instant::now();
        self.write_reg(CMD_CASET, &[0, 0, 0x01, 0xDF])?;
        let staging: &mut [u8] = unsafe { &mut *(&raw mut STAGING) };
        for chunk_idx in 0..board::LCD_HEIGHT as usize / CHUNK_ROWS {
            let first_row = chunk_idx * CHUNK_ROWS;
            let last_row = first_row + CHUNK_ROWS - 1;
            self.write_reg(
                CMD_RASET,
                &[
                    (first_row >> 8) as u8,
                    first_row as u8,
                    (last_row >> 8) as u8,
                    last_row as u8,
                ],
            )?;
            let pixels = &self.fb
                [first_row * board::LCD_WIDTH as usize..(last_row + 1) * board::LCD_WIDTH as usize];
            for (dst, pixel) in staging.chunks_exact_mut(2).zip(pixels) {
                dst.copy_from_slice(&pixel.0.to_be_bytes());
            }
            self.spi.half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                Address::_24Bit(GRAM_ADDR, DataMode::Single),
                0,
                staging,
            )?;
        }
        let dur = started.elapsed().as_micros() as usize;
        COPY_TOTAL_US.fetch_add(dur, Ordering::Relaxed);
        COPY_MAX_US.fetch_max(dur, Ordering::Relaxed);
        COPY_COUNT.fetch_add(1, Ordering::Relaxed);
        FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Both success and failure are consumed exactly once.
    #[allow(dead_code)]
    pub fn poll_present(&mut self) -> Option<bool> {
        self.present_result.take()
    }
}

impl Display {
    /// 寄存器写: 单线 0x02 → 单线地址 reg<<8 → 单线参数。
    fn write_reg(&mut self, reg: u8, params: &[u8]) -> Result<(), esp_hal::spi::Error> {
        self.spi.half_duplex_write(
            DataMode::Single,
            Command::_8Bit(OP_WRITE_REG, DataMode::Single),
            Address::_24Bit((reg as u32) << 8, DataMode::Single),
            0,
            params,
        )
    }

    /// Percentage brightness; a failed write remains pending in the platform.
    /// Initialization must already have enabled BCTRL with 0x53=0x20.
    pub fn set_brightness(&mut self, percent: u8) -> Result<(), esp_hal::spi::Error> {
        let hw = (percent.min(100) as u32 * 255 / 100) as u8;
        self.write_reg(0x51, &[hw])
    }

    /// 熄屏（Display Off 0x28 → Sleep In 0x10）。
    #[allow(dead_code)]
    /// 恢复需 Sleep Out(0x11)+120ms+Display On(0x29)，见 docs/MOSAICO_V12_HARDWARE.md。
    /// Deep Standby(0x4F) 只用于关机路径：退出必须 RESX 硬复位+全量重初始化。
    pub fn set_display_on(&mut self, on: bool) {
        let _ = self.write_reg(if on { 0x29 } else { 0x28 }, &[]);
        if !on {
            let _ = self.write_reg(0x10, &[]);
        }
    }
}

/// 初始化 QSPI 总线、面板和 PSRAM 帧缓冲。
///
/// 上电顺序（官方 BSP）: VCC_PW 先行（app.rs 负责）→ RST 时序 → 厂商序列 → DISPON。
pub fn init(
    spi2: esp_hal::peripherals::SPI2<'static>,
    dma_ch: esp_hal::peripherals::DMA_AXI_CH0<'static>,
    scl: impl PeripheralOutput<'static>,
    d0: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d1: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d2: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d3: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    cs: impl PeripheralOutput<'static>,
    rst_pin: esp_hal::peripherals::GPIO44<'static>,
) -> Display {
    // ---- RST 时序 ----
    let mut rst = Output::new(rst_pin, Level::Low, OutputConfig::default());
    esp_hal::delay::Delay::new().delay_millis(RST_LOW_MS);
    rst.set_level(Level::High);
    esp_hal::delay::Delay::new().delay_millis(RST_SETTLE_MS);

    // ---- QSPI 总线（SPI2 + 专用 SPI DMA 引擎）----
    let rx_buf = dma_rx_buffer!(256).expect("DMA RX 缓冲创建失败");
    let tx_buf = dma_tx_buffer!(CHUNK_BYTES).expect("DMA TX 缓冲创建失败");
    let spi = Spi::new(
        spi2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SPI_CLOCK_HZ))
            .with_mode(Mode::_0),
    )
    .expect("SPI2 初始化失败")
    .with_sck(scl)
    .with_sio0(d0)
    .with_sio1(d1)
    .with_sio2(d2)
    .with_sio3(d3)
    .with_cs(cs)
    .with_dma(dma_ch)
    .with_buffers(rx_buf, tx_buf);

    let mut display = Display {
        fb: ManuallyDrop::new(alloc_fb()),
        spi,
        _rst: rst,
        present_result: None,
    };

    // ---- 初始化: MADCTL(旋转 0, RGB 顺序) + COLMOD + 官方厂商序列 ----
    display.write_reg(0x36, &[0x00]).expect("MADCTL 写入失败");
    display.write_reg(0x3A, &[0x55]).expect("COLMOD 写入失败");
    for (reg, params, delay_ms) in VENDOR_INIT {
        if let Err(e) = display.write_reg(*reg, params) {
            panic!("CO5300 初始化命令 0x{reg:02X} 失败: {e:?}");
        }
        if *delay_ms > 0 {
            esp_hal::delay::Delay::new().delay_millis(*delay_ms);
        }
    }

    info!(
        "CO5300 已启动: {}x{} QSPI@{}MHz CS={} SCL={} RST={} D={}/{}/{}/{} 块={}行×{}",
        board::LCD_WIDTH,
        board::LCD_HEIGHT,
        SPI_CLOCK_HZ / 1_000_000,
        board::LCD_CS_GPIO,
        board::LCD_SCL_GPIO_V1_2,
        board::LCD_RST_GPIO_V1_2,
        board::LCD_DATA0_GPIO,
        board::LCD_DATA1_GPIO,
        board::LCD_DATA2_GPIO,
        board::LCD_DATA3_GPIO,
        CHUNK_ROWS,
        board::LCD_HEIGHT as usize / CHUNK_ROWS,
    );
    display
}

/// PSRAM 帧缓冲分配（External capability, 128B 对齐, 永不释放）
fn alloc_fb() -> &'static mut [Rgb565Pixel] {
    let layout = core::alloc::Layout::from_size_align(board::LCD_FB_BYTES, 128).unwrap();
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    unsafe {
        ptr.write_bytes(0, board::LCD_FB_BYTES);
        slice::from_raw_parts_mut(ptr.cast::<Rgb565Pixel>(), board::LCD_FB_PIXELS)
    }
}

/// 显示链路统计快照（累计值, 除法留给调用方）。
/// (frames/copy_* 由 M2 心跳消费, 当前仅 dma_err)
#[allow(dead_code)]
pub struct Stats {
    /// 成功发布的整帧数（= present() 次数, 模型帧率）
    pub frames: usize,
    /// GRAM 整帧刷新次数 / 累计微秒 / 单次最长（自开机累计最大）
    pub copy_count: usize,
    pub copy_total_us: usize,
    pub copy_max_us: usize,
    /// QSPI DMA/SPI 错误次数
    pub dma_err: usize,
}

/// 调试读数快照（全部是累计值，不做除法）
pub fn stats() -> Stats {
    Stats {
        frames: FRAME_COUNT.load(Ordering::Relaxed),
        copy_count: COPY_COUNT.load(Ordering::Relaxed),
        copy_total_us: COPY_TOTAL_US.load(Ordering::Relaxed),
        copy_max_us: COPY_MAX_US.load(Ordering::Relaxed),
        dma_err: ERR_COUNT.load(Ordering::Relaxed),
    }
}
