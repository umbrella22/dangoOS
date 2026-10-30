//! SPI3 and AXI DMA channel 1 are dedicated to onboard NAND.
//! Bootstrap uses 4 MHz SIO with HOLD#/WP# high, then 40 MHz QOUT + DMA.
//! Device framing is isolated in `protocol`; only this adapter uses esp-hal.
pub mod bench;
mod protocol;
pub mod volume;

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use esp_hal::{
    dma_rx_buffer, dma_tx_buffer,
    gpio::{Level, Output, OutputConfig},
    peripherals::{DMA_AXI_CH1, GPIO20, GPIO21, GPIO22, GPIO23, GPIO24, GPIO25, SPI3},
    spi::{
        master::{Address, Command, Config, Spi, SpiDma},
        Mode,
    },
    time::Rate,
    Blocking,
};
pub use protocol::{
    Identity, PageStatus, BLOCKS, DATA_BYTES, PAGES_PER_BLOCK, PAGE_BYTES, SPARE_BYTES,
};
use protocol::{Nand, Read, Transport, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    NoResponse,
    UnstableId,
    Busy,
    Transport,
    UnknownModel,
    PageRange,
    BufferSize,
    OtpEnabled,
    EccDisabled,
    ReadTimeout,
    Uncorrectable,
    WritesDisabled,
    BlockRange,
    BadBlock,
    Protected,
    WriteEnable,
    FeatureVerify,
    ConfigurationRestore,
    ProgramTimeout,
    EraseTimeout,
    ProgramFailed,
    EraseFailed,
    MarkerFailed,
}
impl Error {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::NoResponse => "no-response",
            Self::UnstableId => "unstable-id",
            Self::Busy => "busy",
            Self::Transport => "spi-read",
            Self::UnknownModel => "unknown-model",
            Self::PageRange => "page-out-of-range",
            Self::BufferSize => "page-buffer-size",
            Self::OtpEnabled => "otp-enabled",
            Self::EccDisabled => "ecc-disabled",
            Self::ReadTimeout => "page-read-timeout",
            Self::Uncorrectable => "ecc-uncorrectable",
            Self::WritesDisabled => "writes-disabled",
            Self::BlockRange => "block-out-of-range",
            Self::BadBlock => "known-bad-block",
            Self::Protected => "protected",
            Self::WriteEnable => "write-enable",
            Self::FeatureVerify => "feature-readback",
            Self::ConfigurationRestore => "configuration-restore",
            Self::ProgramTimeout => "program-timeout",
            Self::EraseTimeout => "erase-timeout",
            Self::ProgramFailed => "program-failed",
            Self::EraseFailed => "erase-failed",
            Self::MarkerFailed => "bad-marker-failed",
        }
    }
}

enum Controller {
    Pio(Spi<'static, Blocking>),
    Dma(SpiDma<'static, Blocking>),
}
struct Bus {
    spi: Controller,
    quad: bool,
}

impl Transport for Bus {
    type Error = esp_hal::spi::Error;
    fn write(&mut self, command: Write, bytes: &[u8]) -> Result<(), Self::Error> {
        use esp_hal::spi::master::DataMode::{Quad, SingleTwoDataLines};
        let (opcode, address, bits) = command.frame_for(self.quad);
        let mode = if self.quad && matches!(command, Write::Load(_)) {
            Quad
        } else {
            SingleTwoDataLines
        };
        let address = match bits {
            0 => Address::None,
            8 => Address::_8Bit(address, SingleTwoDataLines),
            16 => Address::_16Bit(address, SingleTwoDataLines),
            24 => Address::_24Bit(address, SingleTwoDataLines),
            _ => unreachable!(),
        };
        let command = Command::_8Bit(u16::from(opcode), SingleTwoDataLines);
        match &mut self.spi {
            Controller::Pio(spi) => spi.half_duplex_write(mode, command, address, 0, bytes),
            Controller::Dma(spi) => spi.half_duplex_write(mode, command, address, 0, bytes),
        }
    }
    fn read(&mut self, command: Read, bytes: &mut [u8]) -> Result<(), Self::Error> {
        use esp_hal::spi::master::DataMode::{Quad, SingleTwoDataLines};
        // The board has separate MOSI/MISO. HAL's `Single` uses only SIO0.
        let frame = command.frame_for(self.quad);
        let mode = if self.quad && matches!(command, Read::Cache(_)) {
            Quad
        } else {
            SingleTwoDataLines
        };
        let address = if frame.address_bits == 16 {
            Address::_16Bit(u32::from(frame.address), SingleTwoDataLines)
        } else {
            Address::_8Bit(u32::from(frame.address), SingleTwoDataLines)
        };
        let command = Command::_8Bit(u16::from(frame.opcode), SingleTwoDataLines);
        match &mut self.spi {
            Controller::Pio(spi) => {
                spi.half_duplex_read(mode, command, address, frame.dummy_cycles, bytes)
            }
            Controller::Dma(spi) => {
                spi.half_duplex_read(mode, command, address, frame.dummy_cycles, bytes)
            }
        }
    }
    fn load_page(&mut self, page: u16) -> Result<(), Self::Error> {
        use esp_hal::spi::master::DataMode::SingleTwoDataLines;
        let command = Command::_8Bit(0x13, SingleTwoDataLines);
        let address = Address::_24Bit(u32::from(page), SingleTwoDataLines);
        match &mut self.spi {
            Controller::Pio(spi) => {
                spi.half_duplex_write(SingleTwoDataLines, command, address, 0, &[])
            }
            Controller::Dma(spi) => {
                spi.half_duplex_write(SingleTwoDataLines, command, address, 0, &[])
            }
        }
    }
    fn now_us(&self) -> u64 {
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros()
    }
    fn delay_us(&mut self, us: u32) {
        esp_hal::delay::Delay::new().delay_micros(us);
    }
}

static DEVICE: Mutex<CriticalSectionRawMutex, Option<Nand<Bus>>> = Mutex::new(None);

// Synchronous filesystem callbacks and async diagnostics share the chip. All
// protocol operations finish without awaiting; re-entry returns BUSY.
fn with<R>(
    f: impl FnOnce(&mut Nand<Bus>) -> Result<R, protocol::Error<esp_hal::spi::Error>>,
) -> Result<R, Error> {
    let mut device = DEVICE.try_lock().map_err(|_| Error::Busy)?;
    f(device.as_mut().ok_or(Error::Unavailable)?).map_err(map_error)
}

#[allow(clippy::too_many_arguments)]
pub fn init(
    spi: SPI3<'static>,
    dma: DMA_AXI_CH1<'static>,
    sck: GPIO20<'static>,
    mosi: GPIO21<'static>,
    miso: GPIO22<'static>,
    cs: GPIO23<'static>,
    mut hold: GPIO24<'static>,
    mut wp: GPIO25<'static>,
) {
    let hold_guard = Output::new(hold.reborrow(), Level::High, OutputConfig::default());
    let wp_guard = Output::new(wp.reborrow(), Level::High, OutputConfig::default());
    let bus = Spi::new(
        spi,
        Config::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    )
    .expect("NAND SPI3 configuration")
    .with_sck(sck)
    .with_sio0(mosi)
    .with_sio1(miso)
    .with_cs(cs);
    let mut nand = Nand::new(Bus {
        spi: Controller::Pio(bus),
        quad: false,
    });
    if let Err(error) = nand.enable_quad() {
        log::warn!("nand: quad bootstrap failed: {}", map_error(error).name());
        return;
    }
    drop(hold_guard);
    drop(wp_guard);
    // All phases fit one page, including the raw spare area. DMA buffers live
    // in internal RAM and also handle unaligned Dhara metadata/marker slices.
    let rx = dma_rx_buffer!(PAGE_BYTES).expect("NAND DMA RX buffer");
    let tx = dma_tx_buffer!(PAGE_BYTES).expect("NAND DMA TX buffer");
    let nand = nand.map_transport(|bus| {
        let Controller::Pio(mut spi) = bus.spi else {
            unreachable!()
        };
        spi.apply_config(
            &Config::default()
                .with_frequency(Rate::from_mhz(40))
                .with_mode(Mode::_0),
        )
        .expect("NAND 40 MHz configuration");
        let spi = spi
            .with_sio2(wp)
            .with_sio3(hold)
            .with_dma(dma)
            .with_buffers(rx, tx);
        Bus {
            spi: Controller::Dma(spi),
            quad: true,
        }
    });
    *DEVICE.try_lock().expect("NAND installed once at boot") = Some(nand);
    log::info!("nand: SPI3 QOUT 40MHz, AXI DMA1 (QE verified)");
}

pub async fn identify() -> Result<Identity, Error> {
    let mut device = DEVICE.lock().await;
    let Some(device) = device.as_mut() else {
        return Err(Error::Unavailable);
    };
    device.identify().map_err(map_error)
}

pub async fn read_page(page: u32, buffer: &mut [u8]) -> Result<PageStatus, Error> {
    let mut device = DEVICE.lock().await;
    let Some(device) = device.as_mut() else {
        return Err(Error::Unavailable);
    };
    device.read_page(page, buffer).map_err(map_error)
}

fn map_error(error: protocol::Error<esp_hal::spi::Error>) -> Error {
    match error {
        protocol::Error::NoResponse => Error::NoResponse,
        protocol::Error::UnstableId => Error::UnstableId,
        protocol::Error::Busy => Error::Busy,
        protocol::Error::Transport(error) => {
            log::warn!("nand: SPI3 transport error={error:?}");
            Error::Transport
        }
        protocol::Error::UnknownModel => Error::UnknownModel,
        protocol::Error::PageRange => Error::PageRange,
        protocol::Error::BufferSize => Error::BufferSize,
        protocol::Error::OtpEnabled => Error::OtpEnabled,
        protocol::Error::EccDisabled => Error::EccDisabled,
        protocol::Error::ReadTimeout => Error::ReadTimeout,
        protocol::Error::Uncorrectable => Error::Uncorrectable,
        protocol::Error::WritesDisabled => Error::WritesDisabled,
        protocol::Error::BlockRange => Error::BlockRange,
        protocol::Error::BadBlock => Error::BadBlock,
        protocol::Error::Protected => Error::Protected,
        protocol::Error::WriteEnable => Error::WriteEnable,
        protocol::Error::FeatureVerify => Error::FeatureVerify,
        protocol::Error::ConfigurationRestore => Error::ConfigurationRestore,
        protocol::Error::ProgramTimeout => Error::ProgramTimeout,
        protocol::Error::EraseTimeout => Error::EraseTimeout,
        protocol::Error::ProgramFailed => Error::ProgramFailed,
        protocol::Error::EraseFailed => Error::EraseFailed,
        protocol::Error::MarkerFailed => Error::MarkerFailed,
    }
}
