use crate::{
    protocol::{Decoder, Packet, Status},
    server::dispatch,
};
use core::sync::atomic::Ordering;
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, ControlChanged, Receiver, Sender, State},
    Builder, UsbDevice,
};
use esp_hal::{
    peripherals::USB_HS,
    usb::otg::{
        embassy_usb_device::{Config, Driver},
        Usb,
    },
};
use static_cell::StaticCell;
type Tx = Sender<'static, Driver<'static>>;
type Rx = Receiver<'static, Driver<'static>>;
fn build(
    hw: USB_HS<'static>,
) -> (
    UsbDevice<'static, Driver<'static>>,
    Tx,
    Rx,
    ControlChanged<'static>,
) {
    static EP: StaticCell<[u8; 1024]> = StaticCell::new();
    static CONFIG: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS: StaticCell<[u8; 256]> = StaticCell::new();
    static CONTROL: StaticCell<[u8; 64]> = StaticCell::new();
    static CDC: StaticCell<State<'static>> = StaticCell::new();
    let driver = Driver::new(Usb::new_hs(hw), EP.init([0; 1024]), Config::default());
    let mut config = embassy_usb::Config::new(0x303a, 0x4001);
    config.max_packet_size_0 = 64;
    config.manufacturer = Some("dangoOS");
    config.product = Some("dangoOS S31 Recovery");
    config.serial_number = Some("dango-recovery-s31");
    let mut builder = Builder::new(
        driver,
        config,
        CONFIG.init([0; 256]),
        BOS.init([0; 256]),
        &mut [],
        CONTROL.init([0; 64]),
    );
    let cdc = CdcAcmClass::new(&mut builder, CDC.init(State::new()), 512);
    let (tx, rx, control) = cdc.split_with_control();
    (builder.build(), tx, rx, control)
}
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
enum Work {
    Request(Packet),
    Disconnected,
}
static REQUEST: Channel<CriticalSectionRawMutex, Work, 1> = Channel::new();
static REPLY: Channel<CriticalSectionRawMutex, Packet, 1> = Channel::new();
#[embassy_executor::task]
pub async fn run(hw: USB_HS<'static>) {
    let (mut device, tx, rx, control) = build(hw);
    embassy_futures::join::join(device.run(), serve(tx, rx, control)).await;
}
#[embassy_executor::task]
pub async fn media() {
    use embassy_futures::select::{select, Either};
    let mut storage = super::storage::Storage::default();
    let mut install = super::install::InstallMode::default();
    let mut staged = super::staged::Staged::default();
    loop {
        if !staged.started() && super::NAND_READY.load(Ordering::Acquire) {
            staged.start(&mut storage);
        }
        if staged.active() && super::CANCELLED.load(Ordering::Acquire) {
            staged.cancel(&mut storage, 6);
        }
        if staged.active() && staged.step(&mut storage) {
            embassy_time::Timer::after_millis(800).await;
            esp_hal::system::software_reset();
        }
        let work = if staged.active() || !staged.started() {
            match select(REQUEST.receive(), embassy_time::Timer::after_millis(1)).await {
                Either::First(work) => work,
                Either::Second(()) => {
                    continue;
                }
            }
        } else {
            REQUEST.receive().await
        };
        let request = match work {
            Work::Request(p) => p,
            Work::Disconnected => {
                if !staged.active() {
                    install.disconnect();
                }
                continue;
            }
        };
        let reply =
            if super::CANCELLED.load(Ordering::Acquire) && !matches!(request.operation, 1 | 8) {
                install.disconnect();
                Packet {
                    id: request.id,
                    operation: request.operation,
                    status: Status::Cancelled as u8,
                    data: alloc::vec::Vec::new(),
                }
            } else {
                if request.operation == 1 {
                    super::CANCELLED.store(false, Ordering::Release);
                    // Reconnecting does not cancel an autonomous NAND update.
                    if !staged.active() {
                        install.disconnect();
                    }
                }
                if staged.active() && request.operation >= super::install::ENTER {
                    let status =
                        if request.operation == super::install::LEAVE && request.data.is_empty() {
                            staged.cancel(&mut storage, 6);
                            Status::Ok
                        } else {
                            Status::Unavailable
                        };
                    Packet {
                        id: request.id,
                        operation: request.operation,
                        status: status as u8,
                        data: if status == Status::Ok {
                            b"read-only".into()
                        } else {
                            alloc::vec::Vec::new()
                        },
                    }
                } else {
                    install
                        .handle(&request)
                        .await
                        .unwrap_or_else(|| dispatch(&mut storage, &request))
                }
            };
        REPLY.send(reply).await;
    }
}
async fn closed(control: &ControlChanged<'_>) {
    while control.dtr() {
        control.control_changed().await;
    }
}
async fn write_reply(tx: &mut Tx, wire: &[u8]) -> Result<(), ()> {
    for packet in wire.chunks(512) {
        tx.write_packet(packet).await.map_err(|_| ())?;
    }
    if wire.len() % 512 == 0 {
        tx.write_packet(&[]).await.map_err(|_| ())?;
    }
    Ok(())
}
async fn serve(mut tx: Tx, mut rx: Rx, control: ControlChanged<'static>) {
    use embassy_futures::select::{select, Either};
    let mut decoder = Decoder::default();
    let mut buffer = [0; 512];
    loop {
        rx.wait_connection().await;
        while !control.dtr() {
            control.control_changed().await;
        }
        decoder.reset();
        super::CONNECTED.store(true, Ordering::Release);
        'connection: loop {
            let n = match select(rx.read_packet(&mut buffer), control.control_changed()).await {
                Either::First(Ok(n)) => n,
                Either::First(Err(_)) => break,
                Either::Second(()) if !control.dtr() => break,
                Either::Second(()) => continue,
            };
            for &byte in &buffer[..n] {
                if !control.dtr() {
                    break 'connection;
                }
                if let Some(Ok(request)) = decoder.push(byte) {
                    super::ACTIVE.store(request.operation, Ordering::Release);
                    if request.data.len() >= 4 {
                        super::POSITION.store(
                            u32::from_le_bytes(request.data[..4].try_into().unwrap()),
                            Ordering::Release,
                        );
                    }
                    REQUEST.send(Work::Request(request)).await;
                    let reply = match select(REPLY.receive(), closed(&control)).await {
                        Either::First(reply) => reply,
                        Either::Second(()) => {
                            super::CONNECTED.store(false, Ordering::Release);
                            // Drain exactly this request before accepting a new
                            // connection, including a cancelled verification.
                            let _ = REPLY.receive().await;
                            break 'connection;
                        }
                    };
                    if let Ok(wire) = reply.encode() {
                        let sent = control.dtr()
                            && matches!(
                                select(write_reply(&mut tx, &wire), closed(&control)).await,
                                Either::First(Ok(()))
                            );
                        super::ACTIVE.store(0, Ordering::Release);
                        if sent && reply.operation == super::install::BOOT && reply.status == 0 {
                            embassy_time::Timer::after_millis(150).await;
                            esp_hal::system::software_reset();
                        }
                        if sent
                            && reply.operation == super::install::ROM_DOWNLOAD
                            && reply.status == 0
                        {
                            embassy_time::Timer::after_millis(150).await;
                            board_s31::reset::to_rom();
                        }
                        if !sent {
                            break 'connection;
                        }
                    }
                    embassy_futures::yield_now().await;
                }
            }
        }
        super::CONNECTED.store(false, Ordering::Release);
        super::ACTIVE.store(0, Ordering::Release);
        REQUEST.send(Work::Disconnected).await;
    }
}
