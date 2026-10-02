//! 480×480 CO5300 QSPI AMOLED 驱动（ESP-Mosaico v1.2）。
//!
//! 从 fan-firmware/drivers/display.rs 裁剪：去掉 Slint 像素类型（帧缓冲
//! 直接用 RGB565 小端 u16），保留 QSPI 命令/GRAM 写路径。LVGL demo 的
//! flush 回调把 LVGL 渲染结果写入 fb，随后 present() 整帧刷到面板。
//!
//! 蓝本: 官方 BSP `esp-mosaico/esp-mosaico-bsp` onboard/display.c +
//! `espressif/esp_lcd_co5300` v2.2.0（线序: 寄存器单线 0x02, GRAM 四线 0x32）。

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
    time::Rate,
    Blocking,
};
use log::error;

use crate::board;

// ---- 传输参数 ----
/// 每块传输的行数: 32 行 = 30720B（内部 RAM 双份 ~61KB）。
const CHUNK_ROWS: usize = 32;
const ROW_BYTES: usize = board::LCD_WIDTH as usize * 2;
const CHUNK_BYTES: usize = CHUNK_ROWS * ROW_BYTES;
const SPI_CLOCK_HZ: u32 = 40_000_000;

// ---- CO5300 命令 ----
const OP_WRITE_REG: u16 = 0x02; // 单线命令相位
const OP_WRITE_COLOR: u16 = 0x32; // GRAM 写操作码（组件 v2.2.0 起）
const GRAM_ADDR: u32 = 0x2C << 8; // 0x2C RAMWR
const CMD_CASET: u8 = 0x2A;
const CMD_RASET: u8 = 0x2B;

// ---- 厂商初始化序列（esp-mosaico-bsp display.c `s_vendor_init`）----
const VENDOR_INIT: &[(u8, &[u8], u32)] = &[
    (0x11, &[], 600),   // SLPOUT
    (0xFE, &[0x20], 0), // 页切换
    (0x19, &[0x10], 0),
    (0x1C, &[0xA0], 0),
    (0xFE, &[0x00], 0), // 页切回
    (0xC4, &[0x80], 0),
    (0x3A, &[0x55], 0), // RGB565
    (0x35, &[0x00], 0), // TE on
    (0x53, &[0x20], 0), // BCTRL: 打开亮度控制
    (0x51, &[0xFF], 0), // 亮度 255
    (0x63, &[0xFF], 0),
    (0x2A, &[0x00, 0x00, 0x01, 0xDF], 0), // 全窗 0..479
    (0x2B, &[0x00, 0x00, 0x01, 0xDF], 0),
    (0x29, &[], 600), // DISPON
];

const RST_LOW_MS: u32 = 10;
const RST_SETTLE_MS: u32 = 150;

static ERR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 内部 RAM 搬运暂存：CPU 从 PSRAM 读出并 LE→BE 换序后交给 DMA。
/// DMA 只读内部 RAM（S31 LCD_CAM 不能直读 PSRAM）。
static mut STAGING: [u8; CHUNK_BYTES] = [0; CHUNK_BYTES];

pub struct Display {
    /// PSRAM 帧缓冲（LVGL flush 目标，RGB565 小端）
    fb: ManuallyDrop<&'static mut [u16]>,
    spi: esp_hal::spi::master::SpiDma<'static, Blocking>,
    _rst: Output<'static>,
}

impl Display {
    /// 面板帧缓冲（LVGL flush 回调的写入目标）。
    pub fn fb_mut(&mut self) -> &mut [u16] {
        &mut self.fb
    }

    /// 帧缓冲以 'static 生命周期交出（底层是永不释放的 PSRAM 分配，
    /// 生命周期绑定在分配而非 &mut self 上），供 LVGL flush 闭包静态访问。
    pub fn fb_static(&mut self) -> &'static mut [u16] {
        // SAFETY: 分配泄漏于堆上且进程内唯一持有者为本驱动；
        // 单核阻塞式固件中由调用方保证不与 present() 并发。
        unsafe { core::mem::transmute::<&mut [u16], &'static mut [u16]>(self.fb_mut()) }
    }

    /// 整帧刷到 GRAM（同步，~25ms @40MHz）。每块各自带行窗口和 RAMWR 事务。
    pub fn present(&mut self) -> Result<(), esp_hal::spi::Error> {
        let result = self.present_inner();
        if let Err(e) = &result {
            ERR_COUNT.fetch_add(1, Ordering::Relaxed);
            error!("GRAM frame failed: {e:?}; retain framebuffer for retry");
        }
        result
    }

    fn present_inner(&mut self) -> Result<(), esp_hal::spi::Error> {
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
                dst.copy_from_slice(&pixel.to_be_bytes());
            }
            self.spi.half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                Address::_24Bit(GRAM_ADDR, DataMode::Single),
                0,
                staging,
            )?;
        }
        Ok(())
    }

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

    /// 亮度百分比；初始化必须已写 0x53=0x20 打开 BCTRL。
    #[allow(dead_code)]
    pub fn set_brightness(&mut self, percent: u8) -> Result<(), esp_hal::spi::Error> {
        let hw = (percent.min(100) as u32 * 255 / 100) as u8;
        self.write_reg(0x51, &[hw])
    }
}

/// 初始化 QSPI 总线、面板和 PSRAM 帧缓冲。
///
/// 上电顺序（官方 BSP）: VCC_PW 先行（入口负责）→ RST 时序 → 厂商序列 → DISPON。
#[allow(clippy::too_many_arguments)]
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

    // ---- QSPI 总线（SPI2 + AXI DMA）----
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

    log::info!(
        "CO5300 已启动: {}x{} QSPI@{}MHz",
        board::LCD_WIDTH,
        board::LCD_HEIGHT,
        SPI_CLOCK_HZ / 1_000_000,
    );
    display
}

/// PSRAM 帧缓冲分配（External capability, 128B 对齐, 永不释放）
fn alloc_fb() -> &'static mut [u16] {
    let layout = core::alloc::Layout::from_size_align(board::LCD_FB_BYTES, 128).unwrap();
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    if ptr.is_null() {
        alloc::alloc::handle_alloc_error(layout);
    }
    unsafe {
        ptr.write_bytes(0, board::LCD_FB_BYTES);
        slice::from_raw_parts_mut(ptr.cast::<u16>(), board::LCD_FB_PIXELS)
    }
}
