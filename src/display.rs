//! CO5300 QSPI AMOLED 显示驱动(ESP-Mosaico 板载 2.16 寸 480×480)。
//!
//! 传输协议(对齐乐鑫 esp_lcd_co5300 组件 v2.1.0):
//! - 参数写:单线 opcode 0x02 → 四线 [寄存器, 0x00, 0x00] → 四线参数
//! - 帧数据写:单线 opcode 0x12 → 四线 [0x2C, 0x00, 0x00] → 四线像素(RGB565 大端)
//! - CO5300 内置 GRAM 自刷新:写完一帧即可,无需持续扫描
//!
//! 帧缓冲 ≈ 450KB 放 PSRAM(esp-alloc 的 DMA 兼容外部内存分配器)。

use core::ptr::addr_of_mut;
use core::{alloc::Layout, slice};

use esp_hal::delay::Delay;
use esp_hal::gpio::Output;
use esp_hal::spi::master::Spi;
use esp_hal::spi::master::{Address, Command, DataMode};
use log::info;
use slint::platform::software_renderer::Rgb565Pixel;

use crate::board;

// CO5300 QSPI 操作码(IDF LCD_OPCODE_WRITE_CMD / LCD_OPCODE_WRITE_COLOR)
const OPCODE_WRITE_PARAM: u16 = 0x02;
const OPCODE_WRITE_COLOR: u16 = 0x12;

// 常用寄存器
const REG_MADCTL: u8 = 0x36;
const REG_COLMOD: u8 = 0x3A;

static mut FB_PTR: *mut Rgb565Pixel = core::ptr::null_mut();

static mut SPI: Option<Spi<'static, esp_hal::Blocking>> = None;
static mut CS_PIN: Option<Output<'static>> = None;
static mut RST_PIN: Option<Output<'static>> = None;

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
    bytes
}

/// 供渲染循环写入的帧缓冲视图(CPU 与 QSPI DMA 共享同一块 PSRAM)
pub fn frame_buffer() -> &'static mut [Rgb565Pixel] {
    unsafe {
        let px = *addr_of_mut!(FB_PTR);
        slice::from_raw_parts_mut(px, (board::LCD_WIDTH as usize) * (board::LCD_HEIGHT as usize))
    }
}

fn spi() -> &'static mut Spi<'static, esp_hal::Blocking> {
    unsafe { addr_of_mut!(SPI).as_mut().unwrap().as_mut().unwrap() }
}

fn cs_low() {
    unsafe { addr_of_mut!(CS_PIN).as_mut().unwrap().as_mut().unwrap() }.set_low();
}
fn cs_high() {
    unsafe { addr_of_mut!(CS_PIN).as_mut().unwrap().as_mut().unwrap() }.set_high();
}

/// 参数写:opcode 0x02(单线)+ 寄存器(四线)+ 参数(四线)
fn tx_param(reg: u8, params: &[u8]) {
    cs_low();
    let r = spi().half_duplex_write(
        DataMode::Quad,
        Command::_8Bit(OPCODE_WRITE_PARAM, DataMode::Single),
        Address::_24Bit((reg as u32) << 16, DataMode::Quad),
        0,
        params,
    );
    cs_high();
    r.expect("CO5300 参数写失败");
}

/// 帧数据写:opcode 0x12(单线)+ 寄存器(四线)+ 像素(四线)
fn tx_color(reg: u8, data: &[u8]) {
    cs_low();
    let r = spi().half_duplex_write(
        DataMode::Quad,
        Command::_8Bit(OPCODE_WRITE_COLOR, DataMode::Single),
        Address::_24Bit((reg as u32) << 16, DataMode::Quad),
        0,
        data,
    );
    cs_high();
    r.expect("CO5300 帧写失败");
}

/// 存入 QSPI 总线、片选与复位引脚
pub fn attach(
    spi: Spi<'static, esp_hal::Blocking>,
    cs: Output<'static>,
    rst: Output<'static>,
) {
    unsafe {
        addr_of_mut!(SPI).write(Some(spi));
        addr_of_mut!(CS_PIN).write(Some(cs));
        addr_of_mut!(RST_PIN).write(Some(rst));
    }
}

/// 硬复位(CO5300 复位低有效)
fn hw_reset() {
    let delay = Delay::new();
    let rst = unsafe { addr_of_mut!(RST_PIN).as_mut().unwrap().as_mut().unwrap() };
    rst.set_high();
    delay.delay_millis(10);
    rst.set_low();
    delay.delay_millis(10);
    rst.set_high();
    delay.delay_millis(120);
}

/// CO5300 初始化:硬复位 + 官方默认序列(对齐 esp_lcd_co5300 v2.1.0)。
pub fn init() {
    let delay = Delay::new();
    hw_reset();

    // 基础配置:扫描方向(默认竖屏)+ RGB565
    tx_param(REG_MADCTL, &[0x00]);
    tx_param(REG_COLMOD, &[0x55]);

    // 官方 vendor_specific_init_default(逐条对齐,含延时)
    tx_param(0xFE, &[]); // 命令页 0
    tx_param(0xC4, &[0x80]);
    tx_param(0x35, &[]); // TE 使能
    delay.delay_millis(10);
    tx_param(0x53, &[0x20]); // 亮度控制开
    delay.delay_millis(10);
    tx_param(0x51, &[0xFF]); // 亮度最大
    delay.delay_millis(10);
    tx_param(0x63, &[0xFF]);
    delay.delay_millis(10);
    // 官方默认窗口:x 6..477,y 0..465(flush 时会重设全屏窗口)
    tx_param(0x2A, &[0x00, 0x06, 0x01, 0xDD]);
    tx_param(0x2B, &[0x00, 0x00, 0x01, 0xD1]);
    tx_param(0x11, &[]); // SLPOUT
    delay.delay_millis(60);
    tx_param(0x29, &[]); // DISPON

    info!(
        "CO5300 初始化完成({}x{},QSPI {}MHz)",
        board::LCD_WIDTH,
        board::LCD_HEIGHT,
        board::LCD_SPI_HZ / 1_000_000
    );
}

/// 把整个帧缓冲写到面板 GRAM(设全屏窗口 → RAMWR → 四线像素流)。
///
/// 像素字节序:CO5300 要求大端 RGB565,Slint 输出小端 u16,写前就地交换。
pub fn flush() {
    let fb = frame_buffer();

    for px in fb.iter_mut() {
        px.0 = px.0.swap_bytes();
    }

    let (w, h) = (board::LCD_WIDTH - 1, board::LCD_HEIGHT - 1);
    tx_param(0x2A, &[0x00, 0x00, (w >> 8) as u8, w as u8]); // CASET
    tx_param(0x2B, &[0x00, 0x00, (h >> 8) as u8, h as u8]); // RASET

    let bytes: &[u8] =
        unsafe { slice::from_raw_parts(fb.as_ptr() as *const u8, core::mem::size_of_val(fb)) };
    tx_color(0x2C, bytes); // RAMWR + 整帧(内部自动分块,CS 保持)

    for px in fb.iter_mut() {
        px.0 = px.0.swap_bytes();
    }
}
