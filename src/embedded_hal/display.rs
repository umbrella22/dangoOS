//! CO5300 QSPI AMOLED 显示驱动(ESP-Mosaico 板载 2.16 寸 480×480)。
//!
//! 传输协议(对齐乐鑫 esp_lcd_co5300 组件 v2.1.0):
//! - 参数写:单线 opcode 0x02 → 四线 [寄存器, 0x00, 0x00] → 四线参数
//! - 帧数据写:单线 opcode 0x12 → 四线 [0x2C, 0x00, 0x00] → 四线像素(RGB565 大端)
//! - CO5300 内置 GRAM 自刷新:写完一帧即可,无需持续扫描
//!
//! 帧缓冲 ≈ 450KB 放 PSRAM(esp-alloc 的 DMA 兼容外部内存分配器)。

use core::cell::RefCell;
use core::{alloc::Layout, slice};

use critical_section::Mutex;
use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Spi;
use esp_hal::spi::master::{Address, Command, DataMode};
use log::{info, warn};
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board;

// CO5300 QSPI 操作码(IDF LCD_OPCODE_WRITE_CMD / LCD_OPCODE_WRITE_COLOR)
const OPCODE_WRITE_PARAM: u16 = 0x02;
const OPCODE_WRITE_COLOR: u16 = 0x12;

// 常用寄存器
const REG_MADCTL: u8 = 0x36;
const REG_COLMOD: u8 = 0x3A;

// 帧缓冲指针(初始化后不再变化) - 使用 usize 存储指针以满足 Send
static FRAME_BUFFER_PTR: Mutex<RefCell<usize>> = Mutex::new(RefCell::new(0));
static FRAME_BUFFER_LEN: Mutex<RefCell<usize>> = Mutex::new(RefCell::new(0));

// 显示资源(QSPI + 控制引脚)
struct DisplayResources {
    spi: Spi<'static, esp_hal::Blocking>,
    cs: Output<'static>,
    rst: Output<'static>,
}

static DISPLAY: Mutex<RefCell<Option<DisplayResources>>> = Mutex::new(RefCell::new(None));

/// 显示驱动错误类型
#[derive(Debug, Clone, Copy)]
pub enum DisplayError {
    NotInitialized,
    SpiError,
}

/// 从 PSRAM 分配帧缓冲字节区,记录内部指针。
/// 必须在 esp_alloc::psram_allocator!(...) 之后调用。
pub fn alloc_frame_buffer_bytes() -> &'static mut [u8] {
    let layout = Layout::from_size_align(board::LCD_FB_BYTES, 64).expect("帧缓冲布局错误");
    use esp_alloc::export::enumset::EnumSet;
    let mut caps: EnumSet<esp_alloc::MemoryCapability> = EnumSet::new();
    caps.insert(esp_alloc::MemoryCapability::External);
    let ptr = unsafe { esp_alloc::HEAP.alloc_caps(caps, layout) };
    if ptr.is_null() {
        panic!("PSRAM 帧缓冲分配失败(S31 16MB PSRAM)");
    }
    let bytes = unsafe { slice::from_raw_parts_mut(ptr, board::LCD_FB_BYTES) };
    bytes.fill(0);

    // 存储为 Rgb565Pixel 指针
    let fb_ptr = bytes.as_mut_ptr() as *mut Rgb565Pixel;
    let fb_len = (board::LCD_WIDTH as usize) * (board::LCD_HEIGHT as usize);

    critical_section::with(|cs| {
        *FRAME_BUFFER_PTR.borrow_ref_mut(cs) = fb_ptr as usize;
        *FRAME_BUFFER_LEN.borrow_ref_mut(cs) = fb_len;
    });

    bytes
}

/// 供渲染循环写入的帧缓冲视图(CPU 与 QSPI DMA 共享同一块 PSRAM)
pub fn frame_buffer() -> &'static mut [Rgb565Pixel] {
    critical_section::with(|cs| {
        let ptr_addr = *FRAME_BUFFER_PTR.borrow_ref(cs);
        let len = *FRAME_BUFFER_LEN.borrow_ref(cs);
        if ptr_addr == 0 {
            panic!("帧缓冲未初始化");
        }
        unsafe { slice::from_raw_parts_mut(ptr_addr as *mut Rgb565Pixel, len) }
    })
}

/// 参数写:opcode 0x02(单线)+ 寄存器(四线)+ 参数(四线)
fn tx_param(reg: u8, params: &[u8]) -> Result<(), DisplayError> {
    critical_section::with(|cs| {
        let mut display = DISPLAY.borrow_ref_mut(cs);
        let res = display.as_mut().ok_or(DisplayError::NotInitialized)?;

        res.cs.set_low();
        let result = res
            .spi
            .half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OPCODE_WRITE_PARAM, DataMode::Single),
                Address::_24Bit((reg as u32) << 16, DataMode::Quad),
                0,
                params,
            )
            .map_err(|_| {
                warn!("CO5300 参数写失败: reg=0x{:02X}", reg);
                DisplayError::SpiError
            });
        res.cs.set_high();
        result
    })
}

/// 帧数据写:opcode 0x12(单线)+ 寄存器(四线)+ 像素(四线)
fn tx_color(reg: u8, data: &[u8]) -> Result<(), DisplayError> {
    critical_section::with(|cs| {
        let mut display = DISPLAY.borrow_ref_mut(cs);
        let res = display.as_mut().ok_or(DisplayError::NotInitialized)?;

        res.cs.set_low();
        let result = res
            .spi
            .half_duplex_write(
                DataMode::Quad,
                Command::_8Bit(OPCODE_WRITE_COLOR, DataMode::Single),
                Address::_24Bit((reg as u32) << 16, DataMode::Quad),
                0,
                data,
            )
            .map_err(|_| {
                warn!("CO5300 帧写失败");
                DisplayError::SpiError
            });
        res.cs.set_high();
        result
    })
}

/// 存入 QSPI 总线、片选与复位引脚
pub fn attach(spi: Spi<'static, esp_hal::Blocking>, cs: Output<'static>, rst: Output<'static>) {
    critical_section::with(|cs_token| {
        let mut display = DISPLAY.borrow_ref_mut(cs_token);
        *display = Some(DisplayResources { spi, cs, rst });
    });
}

/// 硬复位(CO5300 复位低有效)
fn hw_reset() -> Result<(), DisplayError> {
    let delay = Delay::new();
    critical_section::with(|cs| {
        let mut display = DISPLAY.borrow_ref_mut(cs);
        let res = display.as_mut().ok_or(DisplayError::NotInitialized)?;

        res.rst.set_high();
        delay.delay_millis(10);
        res.rst.set_low();
        delay.delay_millis(10);
        res.rst.set_high();
        delay.delay_millis(120);
        Ok(())
    })
}

/// CO5300 初始化:硬复位 + 官方默认序列(对齐 esp_lcd_co5300 v2.1.0)。
pub fn init() -> Result<(), DisplayError> {
    let delay = Delay::new();
    hw_reset()?;

    // 基础配置:扫描方向(默认竖屏)+ RGB565
    tx_param(REG_MADCTL, &[0x00])?;
    tx_param(REG_COLMOD, &[0x55])?;

    // 官方 vendor_specific_init_default(逐条对齐,含延时)
    tx_param(0xFE, &[])?; // 命令页 0
    tx_param(0xC4, &[0x80])?;
    tx_param(0x35, &[])?; // TE 使能
    delay.delay_millis(10);
    tx_param(0x53, &[0x20])?; // 亮度控制开
    delay.delay_millis(10);
    tx_param(0x51, &[0xFF])?; // 亮度最大
    delay.delay_millis(10);
    tx_param(0x63, &[0xFF])?;
    delay.delay_millis(10);

    // 官方默认窗口:x 6..477(0x06..0x1DD),y 0..465(0x00..0x1D1)
    tx_param(0x2A, &[0x00, 0x06, 0x01, 0xDD])?; // CASET
    tx_param(0x2B, &[0x00, 0x00, 0x01, 0xD1])?; // RASET

    tx_param(0x11, &[])?; // SLPOUT
    delay.delay_millis(60);
    tx_param(0x29, &[])?; // DISPON

    info!(
        "CO5300 初始化完成({}x{},QSPI {}MHz)",
        board::LCD_WIDTH,
        board::LCD_HEIGHT,
        board::LCD_SPI_HZ / 1_000_000
    );
    Ok(())
}

/// 把整个帧缓冲写到面板 GRAM(设全屏窗口 → RAMWR → 四线像素流)。
///
/// 像素字节序:CO5300 要求大端 RGB565,Slint 输出小端 u16,写前就地交换。
pub fn flush() -> Result<(), DisplayError> {
    let fb = frame_buffer();

    // 字节序转换:小端 → 大端
    for px in fb.iter_mut() {
        px.0 = px.0.swap_bytes();
    }

    let (w, h) = (board::LCD_WIDTH - 1, board::LCD_HEIGHT - 1);
    tx_param(0x2A, &[0x00, 0x00, (w >> 8) as u8, w as u8])?; // CASET 全屏
    tx_param(0x2B, &[0x00, 0x00, (h >> 8) as u8, h as u8])?; // RASET 全屏

    let bytes: &[u8] =
        unsafe { slice::from_raw_parts(fb.as_ptr() as *const u8, core::mem::size_of_val(fb)) };
    tx_color(0x2C, bytes)?; // RAMWR + 整帧(内部自动分块,CS 保持)

    // 恢复小端(保持帧缓冲内容一致)
    for px in fb.iter_mut() {
        px.0 = px.0.swap_bytes();
    }

    Ok(())
}

/// 尝试从错误状态恢复(重新初始化显示)
pub fn try_recover() -> Result<(), DisplayError> {
    warn!("尝试恢复显示驱动");
    init()
}
