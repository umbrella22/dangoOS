//! HAL-backed 16-bit stereo I2S0 with TX/RX sharing BCLK and WS.
//! The ES8311 is a slave. Only TX drives GPIO37/49; RX takes the internal
//! signal-loopback clock, so the two directions cannot drive the pads twice.
use esp_hal::{
    dma::{aligned::DmaAlignedMut, DmaRxStreamBuf, DmaTxStreamBuf},
    dma_descriptors_impl,
    i2s::master::{
        Channels, DataFormat, I2s, I2sRx, I2sRxDmaTransfer, I2sTx, I2sTxDmaTransfer, TdmConfig,
        TdmUnitConfig,
    },
    peripherals::{DMA_CH0, GPIO37, GPIO40, GPIO49, GPIO52, GPIO54, I2S0},
    time::Rate,
    Async,
};
pub struct Io {
    tx: Option<I2sTx<'static, Async>>,
    rx: Option<I2sRx<'static, Async>>,
    tx_buf: Option<DmaTxStreamBuf>,
    rx_buf: Option<DmaRxStreamBuf>,
}
pub struct Stream {
    tx: I2sTxDmaTransfer<'static, Async, DmaTxStreamBuf>,
    rx: I2sRxDmaTransfer<'static, Async, DmaRxStreamBuf>,
}
impl Io {
    pub fn new(
        i2s: I2S0<'static>,
        dma: DMA_CH0<'static>,
        mclk: GPIO54<'static>,
        bclk: GPIO37<'static>,
        ws: GPIO49<'static>,
        dout: GPIO52<'static>,
        din: GPIO40<'static>,
    ) -> Result<Self, &'static str> {
        let i2s = I2s::new(
            i2s,
            dma,
            TdmConfig::new_tdm_philips()
                .with_sample_rate(Rate::from_hz(48_000))
                .with_data_format(DataFormat::Data16Channel16)
                .with_channels(Channels::STEREO)
                .with_signal_loopback(true),
        )
        .map_err(|_| "I2S configuration")?
        .with_mclk(mclk)
        .into_async();
        let (rx_desc, tx_desc) = dma_descriptors_impl!(8192, 8192, 1024);
        Ok(Self {
            tx: Some(
                i2s.i2s_tx
                    .with_bclk(bclk)
                    .with_ws(ws)
                    .with_dout(dout)
                    .build(),
            ),
            rx: Some(i2s.i2s_rx.with_din(din).build()),
            tx_buf: Some(DmaTxStreamBuf::new(tx_desc, buffer()).map_err(|_| "I2S TX buffer")?),
            rx_buf: Some(DmaRxStreamBuf::new(rx_desc, buffer()).map_err(|_| "I2S RX buffer")?),
        })
    }
    pub fn start(&mut self, rate: u32) -> Result<Stream, &'static str> {
        let cfg = TdmUnitConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(rate))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        self.tx
            .as_mut()
            .ok_or("I2S already running")?
            .apply_config(&cfg)
            .map_err(|_| "I2S TX format")?;
        self.rx
            .as_mut()
            .ok_or("I2S already running")?
            .apply_config(&cfg)
            .map_err(|_| "I2S RX format")?;
        let mut tx_buf = self.tx_buf.take().ok_or("I2S TX buffer unavailable")?;
        // The first DMA cycle contains silence, before PA can be enabled.
        tx_buf.push_with(|buf| {
            buf.fill(0);
            buf.len()
        });
        let tx = match self.tx.take().unwrap().write(tx_buf) {
            Ok(tx) => tx,
            Err((_, tx, buf)) => {
                self.tx = Some(tx);
                self.tx_buf = Some(buf);
                return Err("I2S TX start");
            }
        };
        let rx = match self.rx.take().unwrap().read(self.rx_buf.take().unwrap()) {
            Ok(rx) => rx,
            Err((_, rx, buf)) => {
                self.rx = Some(rx);
                self.rx_buf = Some(buf);
                let (tx, buf) = tx.stop();
                self.tx = Some(tx);
                self.tx_buf = Some(buf);
                return Err("I2S RX start");
            }
        };
        Ok(Stream { tx, rx })
    }
    pub fn stop(&mut self, stream: Stream) {
        let (rx, buf) = stream.rx.stop();
        self.rx = Some(rx);
        self.rx_buf = Some(buf);
        let (tx, buf) = stream.tx.stop();
        self.tx = Some(tx);
        self.tx_buf = Some(buf);
    }
}
fn buffer() -> DmaAlignedMut<'static, [u8]> {
    // I2S0's singleton creates this owner once per boot. Buffers stay allocated
    // through every stop/restart, as required by the HAL's static DMA views.
    let memory = crate::platform::runtime::memory::PixelMemory::new(8192, false);
    let bytes = unsafe { core::slice::from_raw_parts_mut(memory.as_ptr(), memory.len()) };
    core::mem::forget(memory);
    DmaAlignedMut::new(bytes).expect("internal audio DMA alignment")
}
impl Stream {
    /// Callbacks return the number of bytes supplied. Missing frames become
    /// silence before descriptors go back to DMA, preventing stale audio replay.
    pub fn fill(&mut self, mut fill: impl FnMut(&mut [u8]) -> usize) -> (usize, usize) {
        let mut supplied = 0;
        let mut silent = 0;
        while self.tx.available_bytes() >= 4 {
            let mut advanced = 0;
            self.tx.push_with(|bytes| {
                let len = bytes.len() / 4 * 4;
                let count = fill(&mut bytes[..len]).min(len) / 4 * 4;
                bytes[count..len].fill(0);
                supplied += count;
                silent += len - count;
                advanced = len;
                len
            });
            if advanced == 0 {
                break;
            }
        }
        (supplied / 4, silent / 4)
    }
    pub fn receive(&mut self, bytes: &mut [u8]) -> usize {
        let count = self.rx.available_bytes().min(bytes.len()) / 4 * 4;
        self.rx.pop(&mut bytes[..count])
    }
}
