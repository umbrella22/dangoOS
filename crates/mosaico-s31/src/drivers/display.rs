//! 480×480 CO5300 QSPI AMOLED 驱动（ESP-Mosaico v1.2）。
//!
//! 初始化（厂商序列、清黑屏）走阻塞 SPI，命令短且要等 SLPOUT/DISPON 的毫秒延时；
//! 之后 [`Display::into_async`] 切到 DMA 异步，运行期只有一条上传路径：
//! [`Display::write_canvas_async`]，每次把画布里的一个 ≤32 行窗口打包进内部暂存，
//! 设置 CASET/RASET，再用 GRAM 写操作码 `0x32` + 四线数据相位发出。
//!
//! 蓝本: 官方 BSP `esp-mosaico/esp-mosaico-bsp` onboard/display.c +
//! `espressif/esp_lcd_co5300` v2.2.0（线序: 寄存器单线 0x02, GRAM 四线 0x32）。

use crate::{board, drivers::geometry::DirtyRect};
use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
    slice,
};
use esp_hal::{
    dma_rx_buffer, dma_tx_buffer,
    gpio::interconnect::{PeripheralInput, PeripheralOutput},
    gpio::{Level, Output, OutputConfig},
    spi::master::{Address, Command, Config as SpiConfig, DataMode, Spi, SpiDma},
    spi::Mode,
    time::Rate,
    Async, Blocking, DriverMode,
};

mod packing;

// ---- 传输参数 ----
/// Worst-case rows in one chunk (full panel width).
pub const MAX_CHUNK_ROWS: u16 = 32;
const ROW_BYTES: usize = board::LCD_WIDTH as usize * 2;
const CHUNK_BYTES: usize = MAX_CHUNK_ROWS as usize * ROW_BYTES;

/// QSPI clock.
///
/// The SPI function clock on S31 is 160 MHz (BBPLL/3), so 80 MHz is a clean
/// divide-by-two. The CO5300 QSPI write path is specified for 80 MHz; the
/// previous 40 MHz setting left a 12 ms frame competing with a 50 ms UI
/// cadence, and a drag frame grew to ~49 ms when the UI raster interleaved.
const SPI_CLOCK_HZ: u32 = 80_000_000;

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

/// 独占的内部 RAM 暂存（15360B）。HAL 另有同容量的 DMA TX 缓冲；
/// `half_duplex_write` 会复制到 TX 缓冲，成功返回前已等待传输结束。
/// 异步路径把暂存只读借给 `half_duplex_write_async`（内部 RAM 可由 DMA
/// 直接读取，或在 HAL 内部 TX 缓冲中复制），future 持有 `&mut self`，
/// 传输完成前暂存不会被改写。
struct InternalStaging {
    ptr: NonNull<u8>,
}

impl InternalStaging {
    fn layout() -> Layout {
        Layout::from_size_align(CHUNK_BYTES, 4).expect("invalid staging layout")
    }

    fn new() -> Self {
        let layout = Self::layout();
        // SAFETY: layout has non-zero size. Internal capability excludes PSRAM.
        let ptr = unsafe {
            esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::Internal.into(), layout)
        };
        let ptr = NonNull::new(ptr).unwrap_or_else(|| alloc::alloc::handle_alloc_error(layout));
        // SAFETY: this allocation uniquely owns layout.size() writable bytes.
        unsafe { ptr.as_ptr().write_bytes(0, layout.size()) };
        Self { ptr }
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: initialized allocation is live, uniquely owned, and the slice
        // cannot outlive the exclusive borrow of this owner.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), CHUNK_BYTES) }
    }
}

impl Drop for InternalStaging {
    fn drop(&mut self) {
        // SAFETY: return the allocation exactly once to its original heap with
        // the same layout. HAL DMA uses its own TX buffer, never this pointer.
        unsafe { GlobalAlloc::dealloc(&esp_alloc::HEAP, self.ptr.as_ptr(), Self::layout()) };
    }
}

/// CO5300 panel driver.
///
/// [`init`] returns the blocking variant used for bring-up; [`init_async`]
/// switches it to the DMA-async variant the plane task uses.
pub struct Display<Dm: DriverMode = Blocking> {
    spi: SpiDma<'static, Dm>,
    staging: InternalStaging,
    _rst: Output<'static>,
}

impl Display<Blocking> {
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

    /// Fill the whole panel with black.
    ///
    /// The factory bootloader leaves its own splash in GRAM. Sending an
    /// explicit black field immediately after `DISPON` removes any stale
    /// content, which otherwise shows up as a colored flash between the
    /// bootloader splash and the application's first frame.
    ///
    /// This is a blocking, driver-owned clear: it reuses the 15 KiB staging
    /// buffer and sends one CASET/RASET per 32-row band, so no extra frame
    /// buffer is allocated.
    pub fn clear_black(&mut self) -> Result<(), esp_hal::spi::Error> {
        const BAND_ROWS: u16 = MAX_CHUNK_ROWS;
        self.write_reg(CMD_CASET, &[0, 0, ((board::LCD_WIDTH - 1) >> 8) as u8, (board::LCD_WIDTH - 1) as u8])?;

        // One packed band of zeroes is reused for every row window.
        let pixels_per_band = board::LCD_WIDTH as usize * BAND_ROWS as usize;
        let bytes_per_band = pixels_per_band * 2;
        debug_assert!(bytes_per_band <= CHUNK_BYTES);
        let staging = self.staging.as_mut_slice();
        staging[..bytes_per_band].fill(0);

        let mut y = 0u16;
        while y < board::LCD_HEIGHT {
            let y_end = (y + BAND_ROWS - 1).min(board::LCD_HEIGHT - 1);
            self.write_reg(CMD_RASET, &[(y >> 8) as u8, y as u8, (y_end >> 8) as u8, y_end as u8])?;
            let rows = (y_end - y + 1) as usize;
            let bytes = rows * board::LCD_WIDTH as usize * 2;
            self.spi.half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                Address::_24Bit(GRAM_ADDR, DataMode::Single),
                0,
                &self.staging.as_mut_slice()[..bytes],
            )?;
            y = y_end + 1;
        }
        log::info!("CO5300 GRAM cleared to black ({} bands)", board::LCD_HEIGHT / BAND_ROWS);
        Ok(())
    }

    /// 把已初始化、已验证的面板驱动切换为 DMA 异步模式。
    ///
    /// 面板状态和暂存缓冲保持不变；只把 `SpiDma` 换成
    /// `SpiDma<Async>` 并安装异步中断处理程序。
    pub fn into_async(self) -> Display<Async> {
        Display {
            spi: self.spi.into_async(),
            staging: self.staging,
            _rst: self._rst,
        }
    }
}

impl Display<Async> {
    /// Upload one window from a tightly packed canvas.
    ///
    /// The canvas is smaller than the panel, so the source stride is the
    /// canvas width, not the panel width. `dst` is the panel window and must
    /// be 4-pixel aligned in x with a 4-multiple width. `src` is the matching window
    /// inside the canvas. The height must fit [`MAX_CHUNK_ROWS`].
    pub async fn write_canvas_async(
        &mut self,
        dst: DirtyRect,
        src: DirtyRect,
        canvas_size: (u16, u16),
        canvas: &[u16],
    ) -> Result<(), esp_hal::spi::Error> {
        let byte_count = self.prepare_canvas(dst, src, canvas_size, canvas);
        self.send_prepared(dst, byte_count).await
    }

    /// Pack one canvas window into the internal staging buffer and return the
    /// number of staged bytes.
    fn prepare_canvas(
        &mut self,
        dst: DirtyRect,
        src: DirtyRect,
        canvas_size: (u16, u16),
        canvas: &[u16],
    ) -> usize {
        let (canvas_w, canvas_h) = canvas_size;
        if dst.is_empty() {
            return 0;
        }
        assert!(dst.is_valid(), "canvas window outside panel: {dst:?}");
        assert!(src.is_valid(), "canvas source outside panel: {src:?}");
        assert_eq!(dst.width(), src.width(), "canvas window/source width");
        assert_eq!(dst.height(), src.height(), "canvas window/source height");
        assert!(src.x1 < canvas_w && src.y1 < canvas_h, "canvas source outside canvas");
        assert!(
            canvas.len() >= canvas_w as usize * canvas_h as usize,
            "canvas buffer too short",
        );
        assert!(
            dst.height() <= MAX_CHUNK_ROWS as usize,
            "canvas window exceeds row limit",
        );
        assert_eq!(dst.x0 % 4, 0, "canvas window x must be 4-aligned");
        assert_eq!(dst.width() % 4, 0, "canvas window width must be 4-multiple");

        packing::pack_rgb565(
            canvas,
            (canvas_w, canvas_h),
            (src.x0, src.y0, src.x1, src.y1),
            MAX_CHUNK_ROWS,
            self.staging.as_mut_slice(),
        )
    }

    /// Send a window previously packed by [`Display::prepare_canvas`].
    async fn send_prepared(
        &mut self,
        dst: DirtyRect,
        byte_count: usize,
    ) -> Result<(), esp_hal::spi::Error> {
        if dst.is_empty() || byte_count == 0 {
            return Ok(());
        }
        assert!(dst.is_valid(), "prepared window outside panel: {dst:?}");
        assert!(byte_count <= CHUNK_BYTES, "prepared window exceeds staging");
        self.write_reg_async(
            CMD_CASET,
            &[
                (dst.x0 >> 8) as u8,
                dst.x0 as u8,
                (dst.x1 >> 8) as u8,
                dst.x1 as u8,
            ],
        )
        .await?;
        self.write_reg_async(
            CMD_RASET,
            &[
                (dst.y0 >> 8) as u8,
                dst.y0 as u8,
                (dst.y1 >> 8) as u8,
                dst.y1 as u8,
            ],
        )
        .await?;
        self
            .spi
            .half_duplex_write_async(
                DataMode::Quad,
                Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                Address::_24Bit(GRAM_ADDR, DataMode::Single),
                0,
                &self.staging.as_mut_slice()[..byte_count],
            )
            .await
    }

    /// 异步寄存器写: 单线 0x02 → 单线地址 reg<<8 → 单线参数。
    async fn write_reg_async(&mut self, reg: u8, params: &[u8]) -> Result<(), esp_hal::spi::Error> {
        self
            .spi
            .half_duplex_write_async(
                DataMode::Single,
                Command::_8Bit(OP_WRITE_REG, DataMode::Single),
                Address::_24Bit((reg as u32) << 8, DataMode::Single),
                0,
                params,
            )
            .await
    }

    /// 亮度百分比；初始化必须已写 0x53=0x20 打开 BCTRL。
    pub async fn set_brightness_async(&mut self, percent: u8) -> Result<(), esp_hal::spi::Error> {
        let hw = (percent.min(100) as u32 * 255 / 100) as u8;
        self.write_reg_async(0x51, &[hw]).await
    }
}

/// Whether the panel is already powered and initialized by the factory
/// bootloader.
///
/// The bootloader lights the panel and shows the Mosaico wordmark before it
/// jumps to `main_app`. Resetting and replaying SLPOUT/DISPON here would hold
/// that wordmark on the glass for the whole ~1.2 s power sequence, which is
/// exactly the official logo the user still sees. The official application
/// skips the redundant power sequence and only replays the idempotent vendor
/// configuration; we do the same, then blank the glass immediately so the
/// bootloader wordmark disappears as soon as our code runs.
///
/// Set this to `false` only when bringing up a panel that was not initialized
/// by the bootloader (the cold path below is kept for that case).
const PANEL_WARM_INIT: bool = true;

/// 初始化 QSPI 总线、面板和内部 RAM 暂存；不分配帧缓冲。
///
/// Warm boot (the normal path): the bootloader already did RST/SLPOUT/DISPON.
/// We replay only the idempotent vendor configuration, set brightness 0 first
/// so the bootloader wordmark goes dark immediately, then clear GRAM before
/// turning the display on again. Cold boot retains the full RST/SLPOUT/DISPON
/// sequence with its datasheet delays.
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
) -> Display<Blocking> {
    // ---- RST ----
    // Warm: hold RST high without a low pulse; the bootloader owns the panel
    // state. Cold: the full low/high reset pulse.
    let mut rst = Output::new(rst_pin, Level::High, OutputConfig::default());
    if !PANEL_WARM_INIT {
        rst.set_level(Level::Low);
        esp_hal::delay::Delay::new().delay_millis(RST_LOW_MS);
        rst.set_level(Level::High);
        esp_hal::delay::Delay::new().delay_millis(RST_SETTLE_MS);
    }

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

    let mut display: Display<Blocking> = Display {
        spi,
        staging: InternalStaging::new(),
        _rst: rst,
    };

    // ---- 初始化: 立刻压黑，再重放厂商配置 ----
    //
    // On a warm boot the glass is showing the bootloader wordmark. Brightness
    // 0 is the very first command our app sends, so that image goes dark
    // before the (idempotent) configuration replay and the GRAM clear. The
    // official app skips `0x11`/`0x51`/`0x29` entirely; we keep `0x29` as an
    // idempotent safety net but send it after the clear with brightness still
    // 0, so nothing can flash on the way in.
    if let Err(error) = display.write_reg(0x51, &[0x00]) {
        log::warn!("CO5300 boot brightness off failed: {error:?}");
    }
    display.write_reg(0x36, &[0x00]).expect("MADCTL 写入失败");
    display.write_reg(0x3A, &[0x55]).expect("COLMOD 写入失败");
    for (reg, params, delay_ms) in VENDOR_INIT {
        if PANEL_WARM_INIT && (*reg == 0x11 || *reg == 0x51) {
            // SLPOUT and the vendor brightness 0xFF belong to a cold power-up;
            // replaying them would delay the app or light the wordmark again.
            continue;
        }
        if *reg == 0x29 {
            if let Err(error) = display.clear_black() {
                log::warn!("CO5300 pre-DISPON black clear failed: {error:?}");
            }
            if let Err(error) = display.write_reg(0x51, &[0x00]) {
                log::warn!("CO5300 pre-DISPON brightness off failed: {error:?}");
            }
        }
        if let Err(e) = display.write_reg(*reg, params) {
            panic!("CO5300 初始化命令 0x{reg:02X} 失败: {e:?}");
        }
        if *delay_ms > 0 && !PANEL_WARM_INIT {
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

/// 与 [`init`] 相同的上电/初始化序列，返回 DMA 异步驱动。
///
/// 初始化阶段仍使用阻塞传输（命令很短，且等待 SLPOUT/DISPON 的毫秒延时）。
/// 厂商序列成功后调用 [`Display::into_async`] 切换模式。
#[allow(clippy::too_many_arguments)]
pub fn init_async(
    spi2: esp_hal::peripherals::SPI2<'static>,
    dma_ch: esp_hal::peripherals::DMA_AXI_CH0<'static>,
    scl: impl PeripheralOutput<'static>,
    d0: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d1: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d2: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d3: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    cs: impl PeripheralOutput<'static>,
    rst_pin: esp_hal::peripherals::GPIO44<'static>,
) -> Display<Async> {
    init(spi2, dma_ch, scl, d0, d1, d2, d3, cs, rst_pin).into_async()
}
