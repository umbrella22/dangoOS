//! 480×480 CO5300 QSPI AMOLED 驱动（ESP-Mosaico v1.2）。
//!
//! 初始化（厂商序列、清黑屏）走阻塞 SPI，命令短且要等 SLPOUT/DISPON 的毫秒延时；
//! 之后切到 DMA 异步。像素通过两块独占片内 staging 编码并上传，
//! 避免旧 PSRAM 供给路径中实测的发送 FIFO 断流；单块不超过 DMA 上限。
//! 每块独立设置窗口，使用 GRAM `0x32` 与四线数据，失败重试不会依赖连续写游标。
//!
//! 蓝本: 官方 BSP `esp-mosaico/esp-mosaico-bsp` onboard/display.c +
//! `espressif/esp_lcd_co5300` v2.2.0（线序: 寄存器单线 0x02, GRAM 四线 0x32）。

use crate::{
    board,
    geometry::DirtyRect,
    pixels::{PixelRows, PixelSource, RowChunks, PANEL_FORMAT, STAGING_BYTES},
};
use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::NonNull,
    slice,
};
use embassy_time::{with_timeout, Duration};
use esp_hal::{
    dma_rx_buffer, dma_tx_buffer,
    gpio::interconnect::{PeripheralInput, PeripheralOutput},
    gpio::{Level, Output, OutputConfig},
    spi::master::{Address, Command, Config as SpiConfig, DataMode, Spi, SpiDma},
    spi::Mode,
    time::Rate,
    Async, Blocking, DriverMode,
};

// ---- 传输参数 ----
// Two internal buffers keep the SPI FIFO supplied while the renderer accesses
// PSRAM. Limit the reservation to 32 KiB rather than two maximum DMA windows.
const CHUNK_BYTES: usize = STAGING_BYTES;
// Blocking HAL writes always copy into this TX buffer, including cold clear.
const BOOT_TX_BYTES: usize = 4096;
const ROW_BYTES: usize = board::LCD_WIDTH as usize * PANEL_FORMAT.bytes_per_pixel();
const TRANSFER_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Default)]
pub struct TransferStats {
    pub bytes: u32,
    /// Source bytes bypassing staging; zero in the internal-staging pipeline.
    pub direct_bytes: u32,
    pub windows: u32,
    pub pack_us: u64,
    pub transfer_us: u64,
    /// Software timestamp immediately before submitting the first pixel DMA.
    /// This precedes the actual wire byte; an analyzer is needed for that edge.
    pub first_submit_us: u64,
}

/// Transfer failures; a timeout latches the driver until hardware reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Spi(esp_hal::spi::Error),
    DmaFifo(u32),
    Timeout,
    Faulted,
}
impl From<esp_hal::spi::Error> for Error {
    fn from(error: esp_hal::spi::Error) -> Self {
        Self::Spi(error)
    }
}

async fn transfer(
    faulted: &mut bool,
    operation: impl core::future::Future<Output = Result<(), esp_hal::spi::Error>>,
) -> Result<(), Error> {
    if *faulted {
        return Err(Error::Faulted);
    }
    match with_timeout(TRANSFER_TIMEOUT, operation).await {
        Ok(result) => result.map_err(Error::Spi),
        Err(_) => {
            // HAL cancels active data DMA on future drop. Empty commands may
            // leave panel/SPI state uncertain; do not reuse it until reset.
            *faulted = true;
            Err(Error::Timeout)
        }
    }
}

/// The default is 160 MHz / 4, within CO5300 published quad-write timing.
/// The opt-in / 3 experiment exceeds that timing and is not a panel rating.
const SPI_CLOCK_HZ: u32 = if cfg!(feature = "display-clock-53mhz") {
    // The HAL rounds down. Request the ceiling of 160 MHz / 3 so the
    // fractional-Hz difference does not select the next divider (40 MHz).
    53_333_334
} else {
    40_000_000
};
const COLMOD: u8 = if cfg!(feature = "display-rgb888") {
    0x77
} else {
    0x55
};
const MADCTL: u8 = if cfg!(feature = "display-rgb888") {
    0x08
} else {
    0x00
};

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
    (0x36, &[0x00], 0), // MADCTL: replaced by the selected format's channel order
    (0x3A, &[0x55], 0), // COLMOD: replaced by the selected wire format
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

/// Driver-owned internal staging for pixel windows.
/// The mutable driver borrow covers packing and every DMA completion.
struct DmaStaging {
    ptr: NonNull<u8>,
}

impl DmaStaging {
    fn layout() -> Layout {
        Layout::from_size_align(CHUNK_BYTES, 128).expect("invalid staging layout")
    }

    fn try_new() -> Option<Self> {
        let layout = Self::layout();
        // SAFETY: the internal heap is initialized before panel creation.
        let ptr = unsafe {
            esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::Internal.into(), layout)
        };
        let ptr = NonNull::new(ptr)?;
        // SAFETY: this allocation uniquely owns layout.size() writable bytes.
        unsafe { ptr.as_ptr().write_bytes(0, layout.size()) };
        Some(Self { ptr })
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: initialized allocation is live, uniquely owned, and the slice
        // cannot outlive the exclusive borrow of this owner.
        unsafe { slice::from_raw_parts_mut(self.ptr.as_ptr(), CHUNK_BYTES) }
    }
}

impl Drop for DmaStaging {
    fn drop(&mut self) {
        // SAFETY: return the allocation exactly once to its original heap with
        // the same layout. No transfer can outlive the exclusive driver borrow.
        unsafe { GlobalAlloc::dealloc(&esp_alloc::HEAP, self.ptr.as_ptr(), Self::layout()) };
    }
}

#[derive(Clone, Copy)]
enum ChunkSource<'a> {
    Pixels(PixelSource<'a>, DirtyRect),
    Rows(&'a dyn PixelRows, DirtyRect),
}
impl ChunkSource<'_> {
    fn prepare(self, output: &mut [u8]) -> usize {
        match self {
            Self::Pixels(source, area) => source.pack(area, PANEL_FORMAT, output),
            Self::Rows(rows, area) => rows.pack(area.y0, area.y1, PANEL_FORMAT, output),
        }
    }
}

/// CO5300 panel driver.
///
/// [`init`] returns the blocking variant used for bring-up; [`init_async`]
/// switches it to the DMA-async variant the plane task uses.
pub struct Display<Dm: DriverMode = Blocking> {
    spi: SpiDma<'static, Dm>,
    staging: [DmaStaging; 2],
    _rst: Output<'static>,
    faulted: bool,
    #[cfg(feature = "display-metrics")]
    stats: TransferStats,
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
    /// Cold initialization clears stale GRAM while brightness is zero, before
    /// `DISPON`. A valid bootloader handoff preserves its completed splash and
    /// skips this clear along with the panel reset and wake sequence.
    ///
    /// Blocking HAL writes copy into BOOT_TX_BYTES. Bound each clear band by
    /// that capacity, independently of the larger asynchronous staging buffer.
    pub fn clear_black(&mut self) -> Result<(), esp_hal::spi::Error> {
        const BAND_ROWS: u16 = ((BOOT_TX_BYTES / ROW_BYTES) & !1) as u16;
        self.write_reg(
            CMD_CASET,
            &[
                0,
                0,
                ((board::LCD_WIDTH - 1) >> 8) as u8,
                (board::LCD_WIDTH - 1) as u8,
            ],
        )?;

        // One packed band of zeroes is reused for every row window.
        let pixels_per_band = board::LCD_WIDTH as usize * BAND_ROWS as usize;
        let bytes_per_band = pixels_per_band * PANEL_FORMAT.bytes_per_pixel();
        debug_assert!(bytes_per_band <= CHUNK_BYTES);
        let staging = self.staging[0].as_mut_slice();
        staging[..bytes_per_band].fill(0);

        let mut y = 0u16;
        while y < board::LCD_HEIGHT {
            let y_end = (y + BAND_ROWS - 1).min(board::LCD_HEIGHT - 1);
            self.write_reg(
                CMD_RASET,
                &[(y >> 8) as u8, y as u8, (y_end >> 8) as u8, y_end as u8],
            )?;
            let rows = (y_end - y + 1) as usize;
            let bytes = rows * ROW_BYTES;
            self.spi.half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                Address::_24Bit(GRAM_ADDR, DataMode::Single),
                0,
                &self.staging[0].as_mut_slice()[..bytes],
            )?;
            y = y_end + 1;
        }
        log::info!(
            "CO5300 GRAM cleared to black ({} bands)",
            board::LCD_HEIGHT / BAND_ROWS
        );
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
            faulted: self.faulted,
            #[cfg(feature = "display-metrics")]
            stats: self.stats,
        }
    }
}

impl Display<Async> {
    /// Upload a numeric RGB565 canvas, encoding it to the configured panel format.
    pub async fn write_canvas_async(
        &mut self,
        dst: DirtyRect,
        src: DirtyRect,
        canvas_size: (u16, u16),
        canvas: &[u16],
    ) -> Result<(), Error> {
        self.write_pixels_async(dst, src, PixelSource::rgb565(canvas, canvas_size))
            .await
    }

    /// Upload a whole source window. Each bounded row chunk is independently
    /// addressed, so retrying the rectangle cannot restart at a stale RAMWRC cursor.
    /// Pixel data is staged internally before DMA to avoid the SPI FIFO
    /// starvation observed with the previous PSRAM source path.
    pub async fn write_pixels_async(
        &mut self,
        dst: DirtyRect,
        src: DirtyRect,
        source: PixelSource<'_>,
    ) -> Result<(), Error> {
        if self.faulted {
            return Err(Error::Faulted);
        }
        if dst.is_empty() {
            return Ok(());
        }
        assert!(dst.is_valid() && src.is_valid());
        assert_eq!(dst.width(), src.width());
        assert_eq!(dst.height(), src.height());
        assert!(
            dst.is_panel_window(),
            "CO5300 window requires four columns and two rows"
        );
        assert!(src.x1 < source.size().0 && src.y1 < source.size().1);
        let chunks = RowChunks::new(src, PANEL_FORMAT, CHUNK_BYTES).map(|part| {
            let window = DirtyRect {
                y0: dst.y0 + part.y0 - src.y0,
                y1: dst.y0 + part.y1 - src.y0,
                ..dst
            };
            (window, ChunkSource::Pixels(source, part))
        });
        self.write_chunks(chunks).await
    }

    /// Upload composed full rows (a page push) as row-major whole-width chunks.
    pub async fn write_rows_async(&mut self, rows: impl PixelRows) -> Result<(), Error> {
        if self.faulted {
            return Err(Error::Faulted);
        }
        let area = rows.area();
        assert!(
            area.is_panel_window(),
            "CO5300 window requires four columns and two rows"
        );
        self.write_chunks(
            RowChunks::new(area, PANEL_FORMAT, CHUNK_BYTES)
                .map(|part| (part, ChunkSource::Rows(&rows, part))),
        )
        .await
    }

    /// Pack the next chunk in the other allocation while the current DMA runs.
    /// Both allocations remain owned by this driver through DMA completion.
    async fn write_chunks<'a>(
        &mut self,
        mut chunks: impl Iterator<Item = (DirtyRect, ChunkSource<'a>)>,
    ) -> Result<(), Error> {
        let Some((mut window, first)) = chunks.next() else {
            return Ok(());
        };
        let (left, right) = self.staging.split_at_mut(1);
        let (mut current, mut spare) = (&mut left[0], &mut right[0]);
        #[cfg(feature = "display-metrics")]
        let started = embassy_time::Instant::now();
        let mut length = first.prepare(current.as_mut_slice());
        #[cfg(feature = "display-metrics")]
        {
            self.stats.pack_us += started.elapsed().as_micros();
        }
        loop {
            let next = chunks.next();
            let bytes = &current.as_mut_slice()[..length];
            let pack_next = async {
                #[cfg(feature = "display-metrics")]
                let started = embassy_time::Instant::now();
                let packed =
                    next.map(|(window, source)| (window, source.prepare(spare.as_mut_slice())));
                #[cfg(feature = "display-metrics")]
                let elapsed = started.elapsed().as_micros();
                #[cfg(not(feature = "display-metrics"))]
                let elapsed = 0u64;
                (packed, elapsed)
            };
            let (next, pack_us) = Self::send_window(
                &mut self.spi,
                &mut self.faulted,
                #[cfg(feature = "display-metrics")]
                &mut self.stats,
                window,
                bytes,
                pack_next,
            )
            .await?;
            #[cfg(feature = "display-metrics")]
            {
                self.stats.pack_us += pack_us;
            }
            #[cfg(not(feature = "display-metrics"))]
            let _ = pack_us;
            let Some((next_window, next_length)) = next else {
                break;
            };
            // DMA has released current. Only now can it become the next spare.
            core::mem::swap(&mut current, &mut spare);
            window = next_window;
            length = next_length;
        }
        Ok(())
    }

    async fn write_reg_on(
        spi: &mut SpiDma<'static, Async>,
        faulted: &mut bool,
        reg: u8,
        params: &[u8],
    ) -> Result<(), Error> {
        transfer(
            faulted,
            spi.half_duplex_write_async(
                DataMode::Single,
                Command::_8Bit(OP_WRITE_REG, DataMode::Single),
                Address::_24Bit((reg as u32) << 8, DataMode::Single),
                0,
                params,
            ),
        )
        .await
    }

    /// The transfer future is polled first to start DMA, then next-packing work.
    async fn send_window<T>(
        spi: &mut SpiDma<'static, Async>,
        faulted: &mut bool,
        #[cfg(feature = "display-metrics")] stats: &mut TransferStats,
        window: DirtyRect,
        bytes: &[u8],
        pack_next: impl core::future::Future<Output = T>,
    ) -> Result<T, Error> {
        #[cfg(feature = "display-metrics")]
        {
            stats.bytes += bytes.len() as u32;
            stats.windows += 1;
        }
        let xs = [
            (window.x0 >> 8) as u8,
            window.x0 as u8,
            (window.x1 >> 8) as u8,
            window.x1 as u8,
        ];
        let ys = [
            (window.y0 >> 8) as u8,
            window.y0 as u8,
            (window.y1 >> 8) as u8,
            window.y1 as u8,
        ];
        #[cfg(feature = "display-metrics")]
        let transfer_started = embassy_time::Instant::now();
        Self::write_reg_on(spi, faulted, CMD_CASET, &xs).await?;
        Self::write_reg_on(spi, faulted, CMD_RASET, &ys).await?;
        #[cfg(feature = "display-metrics")]
        if stats.first_submit_us == 0 {
            stats.first_submit_us = esp_hal::time::Instant::now()
                .duration_since_epoch()
                .as_micros();
        }
        let clocked_pack = async {
            // This future is polled after the transfer future starts its DMA,
            // while its function-clock guard still owns the active source.
            #[cfg(feature = "display-metrics")]
            let clock = {
                let source = esp_hal::peripherals::HP_SYS_CLKRST::regs()
                    .gpspi2_ctrl0()
                    .read();
                let divider = esp_hal::peripherals::SPI2::regs().clock().read();
                (
                    source.clk_src_sel().bits(),
                    source.hs_clk_div_num().bits(),
                    divider.clkdiv_pre().bits(),
                    divider.clkcnt_n().bits(),
                )
            };
            let next = pack_next.await;
            #[cfg(not(feature = "display-metrics"))]
            let clock = ();
            (next, clock)
        };
        let (result, (next, clock)) = embassy_futures::join::join(
            transfer(
                faulted,
                spi.half_duplex_write_async(
                    DataMode::Quad,
                    Command::_8Bit(OP_WRITE_COLOR, DataMode::Single),
                    Address::_24Bit(GRAM_ADDR, DataMode::Single),
                    0,
                    bytes,
                ),
            ),
            clocked_pack,
        )
        .await;
        let dma_status = esp_hal::peripherals::SPI2::regs().dma_int_raw().read();
        // The HAL reports transaction completion even when TX starvation has
        // aborted the SPI transaction. Such a window did not reach GRAM in full.
        let fifo_error = dma_status.dma_outfifo_empty_err().bit_is_set()
            || dma_status.mst_tx_afifo_rempty_err().bit_is_set();
        let result = result.and_then(|()| {
            if fifo_error {
                Err(Error::DmaFifo(dma_status.bits()))
            } else {
                Ok(())
            }
        });
        #[cfg(feature = "display-metrics")]
        {
            let elapsed = transfer_started.elapsed().as_micros();
            stats.transfer_us += elapsed;
            let raw_minimum = bytes.len() as u64 * 2_000_000 / u64::from(SPI_CLOCK_HZ);
            if elapsed < raw_minimum {
                log::info!(
                    "display.clock bytes={} us={} src={} div={} pre={} n={} cpu={}",
                    bytes.len(),
                    elapsed,
                    clock.0,
                    clock.1,
                    u32::from(clock.2) + 1,
                    u32::from(clock.3) + 1,
                    esp_hal::clock::cpu_clock().as_mhz()
                );
            }
        }
        #[cfg(not(feature = "display-metrics"))]
        let _ = clock;
        result.map(|()| next)
    }

    #[cfg(feature = "display-metrics")]
    pub fn take_stats(&mut self) -> TransferStats {
        core::mem::take(&mut self.stats)
    }

    /// 异步寄存器写: 单线 0x02 → 单线地址 reg<<8 → 单线参数。
    async fn write_reg_async(&mut self, reg: u8, params: &[u8]) -> Result<(), Error> {
        Self::write_reg_on(&mut self.spi, &mut self.faulted, reg, params).await
    }

    /// 亮度百分比；初始化必须已写 0x53=0x20 打开 BCTRL。
    pub async fn set_brightness_async(&mut self, percent: u8) -> Result<(), Error> {
        let hw = (percent.min(100) as u32 * 255 / 100) as u8;
        self.write_reg_async(0x51, &[hw]).await
    }

    /// Recoverable CO5300 standby. Keep VCC and CS ownership intact.
    pub async fn sleep_in(&mut self) -> Result<(), Error> {
        self.write_reg_async(0x28, &[]).await?; // Display Off
        self.write_reg_async(0x10, &[]).await?; // Sleep In
        embassy_time::Timer::after_millis(120).await;
        Ok(())
    }

    /// Use the existing board init's conservative 600 ms settle intervals.
    /// Brightness stays zero until the plane completes a fresh full frame.
    pub async fn sleep_out(&mut self) -> Result<(), Error> {
        self.write_reg_async(0x11, &[]).await?;
        embassy_time::Timer::after_millis(600).await;
        self.write_reg_async(0x36, &[MADCTL]).await?;
        self.write_reg_async(0x3A, &[COLMOD]).await?;
        // Write brightness after Sleep Out has settled, before Display On;
        // do not rely on register writes being accepted while asleep.
        self.set_brightness_async(0).await?;
        self.write_reg_async(0x29, &[]).await?;
        embassy_time::Timer::after_millis(600).await;
        Ok(())
    }
}

/// 初始化 QSPI 总线、面板和 PSRAM 暂存；不分配全屏帧缓冲。
///
/// A committed one-use bootloader handoff transfers an awake RGB565 panel and
/// a completed splash. The application reclaims SPI/DMA without resetting the
/// panel. Missing or damaged handoffs use the full RST/SLPOUT/DISPON sequence;
/// brightness stays zero until the first completed application frame.
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
    try_init(spi2, dma_ch, scl, d0, d1, d2, d3, cs, rst_pin).expect("CO5300 initialization")
}

#[derive(Debug, Clone, Copy)]
pub enum InitError {
    Allocation,
    Dma,
    Spi,
}

#[allow(clippy::too_many_arguments)]
pub fn try_init(
    spi2: esp_hal::peripherals::SPI2<'static>,
    dma_ch: esp_hal::peripherals::DMA_AXI_CH0<'static>,
    scl: impl PeripheralOutput<'static>,
    d0: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d1: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d2: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    d3: impl PeripheralInput<'static> + PeripheralOutput<'static>,
    cs: impl PeripheralOutput<'static>,
    rst_pin: esp_hal::peripherals::GPIO44<'static>,
) -> Result<Display<Blocking>, InitError> {
    let handoff = crate::boot_display::take();
    // RST remains high when taking over a completed bootloader splash.
    let mut rst = Output::new(rst_pin, Level::High, OutputConfig::default());
    if handoff.is_none() {
        // A reset without this boot's committed handoff has unspecified state.
        rst.set_level(Level::Low);
        esp_hal::delay::Delay::new().delay_millis(RST_LOW_MS);
        rst.set_level(Level::High);
        esp_hal::delay::Delay::new().delay_millis(RST_SETTLE_MS);
    }

    // ---- QSPI 总线（SPI2 + AXI DMA）----
    let rx_buf = dma_rx_buffer!(256).map_err(|_| InitError::Dma)?;
    let tx_buf = dma_tx_buffer!(BOOT_TX_BYTES).map_err(|_| InitError::Dma)?;
    let spi = Spi::new(
        spi2,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SPI_CLOCK_HZ))
            .with_min_async_transfer_size(32)
            .with_mode(Mode::_0),
    )
    .map_err(|_| InitError::Spi)?
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
        staging: [
            DmaStaging::try_new().ok_or(InitError::Allocation)?,
            DmaStaging::try_new().ok_or(InitError::Allocation)?,
        ],
        _rst: rst,
        faulted: false,
        #[cfg(feature = "display-metrics")]
        stats: TransferStats::default(),
    };

    if let Some(record) = handoff {
        // Boot contract is RGB565. Apply the selected wire format before OS
        // uploads; leave the inherited splash in GRAM until the first OS frame.
        display
            .write_reg(0x36, &[MADCTL])
            .map_err(|_| InitError::Spi)?;
        display
            .write_reg(0x3A, &[COLMOD])
            .map_err(|_| InitError::Spi)?;
        crate::boot_display::inherited(record);
        log::info!(
            "boot-display: inherited CO5300, splash={}us selected={}us (loader clock)",
            record.first_frame_us,
            record.selected_us,
        );
        return Ok(display);
    }

    // ---- 初始化: 完整退出休眠，配置面板，清黑后打开显示 ----
    //
    // Brightness writes while asleep may be ignored. The complete vendor
    // sequence starts with Sleep Out and its settle time; the pre-DISPON
    // clear below sets brightness zero again after the panel is awake.
    if let Err(error) = display.write_reg(0x51, &[0x00]) {
        log::warn!("CO5300 boot brightness off failed: {error:?}");
    }
    // MADCTL/COLMOD are written from the table after Sleep Out has settled,
    // matching the bootloader and `sleep_out`; writes while asleep may be lost.
    for (reg, params, delay_ms) in VENDOR_INIT {
        if *reg == 0x29 {
            if let Err(error) = display.clear_black() {
                log::warn!("CO5300 pre-DISPON black clear failed: {error:?}");
            }
            if let Err(error) = display.write_reg(0x51, &[0x00]) {
                log::warn!("CO5300 pre-DISPON brightness off failed: {error:?}");
            }
        }
        let colmod = [COLMOD];
        let madctl = [MADCTL];
        let params = match *reg {
            0x3A => &colmod[..],
            0x36 => &madctl[..],
            _ => params,
        };
        if let Err(e) = display.write_reg(*reg, params) {
            log::warn!("CO5300 command {reg:02X}: {e:?}");
            return Err(InitError::Spi);
        }
        if *delay_ms > 0 {
            esp_hal::delay::Delay::new().delay_millis(*delay_ms);
        }
    }

    log::info!(
        "CO5300 已启动: {}x{} QSPI@{}Hz (RST/SLPOUT/DISPON complete)",
        board::LCD_WIDTH,
        board::LCD_HEIGHT,
        SPI_CLOCK_HZ,
    );
    log::info!(
        "CO5300 input={:?}, staging=PSRAM/{CHUNK_BYTES}B tx={BOOT_TX_BYTES}B",
        PANEL_FORMAT
    );
    #[cfg(feature = "display-metrics")]
    {
        // The driver owns SPI2 and no transfer is active here. Reading CLOCK
        // has no side effect; these are register values, not measured edges.
        let clock = esp_hal::peripherals::SPI2::regs().clock().read();
        log::info!(
            "display.clock requested={} raw={} pre={} n={} h={} l={} bypass={}",
            SPI_CLOCK_HZ,
            clock.bits(),
            clock.clkdiv_pre().bits(),
            clock.clkcnt_n().bits(),
            clock.clkcnt_h().bits(),
            clock.clkcnt_l().bits(),
            clock.clk_equ_sysclk().bit_is_set()
        );
    }
    Ok(display)
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
