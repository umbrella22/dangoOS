# R1 runtime design — interface freeze

> Scope: RFC 0005 §5 (R1 async display and input). This note freezes the
> interfaces shared by the R1 work packages so the presentation, input and UI
> agents can work in parallel without editing the same files.

## Task graph

```text
ui_task (LVGL, medium)
  -> present::Presenter::back_frame()
  -> LVGL flush callback: Presenter::flush_area(area, pixels)
  -> Presenter::submit(damage) -> FrameReady { id, damage }
  -> display_task (high): Presenter::next_job().await
  -> drivers::display::write_chunk_async(area, front_frame)
  -> Presenter::release(id) -> BufferFree { id }
  -> ui_task waits for a free back frame before the next render

input_task (high)
  -> GPIO6 falling edge, async I2C CST9220 read
  -> TouchEvent { at_us, x, y, pressed }
  -> INPUT channel (bounded 32)
  -> ui_task / PointerPort
```

`main` keeps the USB-OTG HS CDC console pump; it is not part of the frame path.

## Presentation contract (`src/present.rs`, host-testable)

Final R1.1 API:

```rust
pub const FRAME_WIDTH: usize = 480; // FRAME_HEIGHT, FRAME_PIXELS, FRAME_BYTES
pub struct FrameId(pub u8);        // A = 0, B = 1
pub enum FrameState { Free, Back, InFlight }
pub struct FrameJob { pub id: FrameId, pub damage: DirtyRect }
pub use crate::framebuffer::DirtyRect; // x0/y0/x1/y1 inclusive, FULL, EMPTY,
                                       // is_empty, is_valid, width, height,
                                       // area, union, align_x4

pub struct Core { /* pure state machine, host-tested */ }
impl Core {
    pub const fn new() -> Self;
    pub fn state(&self, id: FrameId) -> Option<FrameState>;
    pub fn back(&self) -> Option<FrameId>;
    pub fn mark_flushed(&mut self, area: DirtyRect) -> bool;
    pub fn submit(&mut self, damage: DirtyRect) -> Option<FrameJob>;
    pub fn release(&mut self, id: FrameId) -> bool;
}

pub struct Presenter { /* UI side, not Clone */ }
impl Presenter {
    pub fn new() -> Self;
    pub fn display_side(&self) -> DisplayPresenter;
    pub fn back_frame(&self) -> FrameId;
    pub async fn wait_back_free(&mut self);
    pub fn flush_area(&mut self, area: DirtyRect, pixels: &[u16]);
    pub fn flush_area_strided(&mut self, area: DirtyRect, pixels: &[u16], stride: usize);
    pub fn submit(&mut self, damage: DirtyRect) -> FrameId;
    pub async fn next_job(&mut self) -> FrameJob;
    pub fn front_pixels(&self, id: FrameId) -> &[u16];
    pub fn release(&mut self, id: FrameId);
}

pub struct DisplayPresenter { /* display side */ }
impl DisplayPresenter {
    pub async fn next_job(&mut self) -> FrameJob;
    pub fn front_pixels(&self, id: FrameId) -> &[u16];
    pub fn release(&mut self, id: FrameId);
}
```

Rules:

- The UI task writes only `back_frame()`; the display task reads only the frame
  returned by `DisplayPresenter::next_job()`.
- `submit` moves `Back -> InFlight`; if the peer is `Free` it becomes the new
  `Back` immediately. If both frames are in flight, `wait_back_free` waits on an
  embassy `Signal` until `release`.
- `flush_area` copies the LVGL area into the back frame; no LVGL pointer is
  retained. `flush_area_strided` is for LVGL partial buffers with a padded
  stride.
- `DirtyRect::align_x4` is applied before the QSPI transfer.
- The first release submits `DirtyRect::FULL`; later frames use accumulated
  damage.

### Two-frame partial-flush caveat

With two retained frames and LVGL partial flushes, an area changed while the
other frame was current is not present in the recycled frame. R1.3 must do one
of:

1. invalidate the full screen every refresh (simple, recommended for the R1
   touch test), or
2. mirror each flushed area into both frames (needs a presenter extension), or
3. track per-frame damage and re-render the intervening area.

Without this, stale pixels persist on alternating frames.

## Display driver contract (`src/drivers/display.rs`)

Final R1.1 API:

```rust
pub struct Display<Dm: DriverMode = Blocking> { /* SpiDma<'static, Dm> */ }
impl<Dm: DriverMode> Display<Dm> { pub fn error_count(&self) -> usize; }

impl Display<Blocking> {
    pub fn write_chunk(&mut self, x1: u16, y1: u16, x2: u16, y2: u16, frame: &[u16])
        -> Result<(), esp_hal::spi::Error>;
    pub fn set_brightness(&mut self, percent: u8) -> Result<(), esp_hal::spi::Error>;
    pub fn into_async(self) -> Display<Async>;
}

impl Display<Async> {
    pub async fn write_chunk_async(
        &mut self,
        area: DirtyRect,
        frame: &[u16],
    ) -> Result<(), esp_hal::spi::Error>;
    pub async fn set_brightness_async(&mut self, percent: u8)
        -> Result<(), esp_hal::spi::Error>;
}

pub fn init(...) -> Display<Blocking>;
pub fn init_async(...) -> Display<Async>; // init(...).into_async()
```

`write_chunk_async` keeps the archived CASET/RASET + opcode `0x32` path, packs
RGB565 big-endian into the existing staging buffer, and uses
`SpiDma<Async>::half_duplex_write_async` for the command/address and GRAM
phases. It yields the executor while the DMA transfer is in flight. The display
task must split a `FrameJob::damage` into `MAX_CHUNK_ROWS` (16-row) chunks.

## Input contract (`src/input_task.rs`, host-testable queue)

```rust
pub const INPUT_CAPACITY: usize = 32;
pub const FALLBACK_POLL_PERIOD: Duration = Duration::from_millis(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TouchEvent {
    pub at_us: u64,
    pub x: u16,
    pub y: u16,
    pub pressed: bool,
}

pub static INPUT: Channel<CriticalSectionRawMutex, TouchEvent, INPUT_CAPACITY>;
pub static INPUT_DROPS: AtomicU32;
pub fn touch_irq_input(gpio: impl InputPin + 'static) -> Input<'static>;
```

- `input_task` waits for GPIO6 falling edge, reads the CST9220 report over
  `I2c<Async>`, decodes points, ACKs HYN212 reports, and pushes the first point.
- A 5 ms `Timer` is the missed-interrupt fallback. Both paths share one async
  I2C mutex.
- On a full queue it drops the oldest event and increments `INPUT_DROPS`; it
  never blocks.
- Touch-down and release edges are logged on the CDC console.
- The UI side drains `INPUT` and updates the pointer state; it never calls I2C.
- `Cst9220Async` is a type alias of `Cst9220`; `new_async()` performs the same
  identify sequence over the async slot, and `poll_async()` shares the decoder
  and stats with the blocking `poll()`.

R1.3 wiring: add `mod input_task;`, install the async bus with
`I2c::new(...).into_async()` + `shared_i2c::install_async`, create the touch
with `Cst9220Async::new_async().await`, build the GPIO6 input with
`touch_irq_input`, and spawn the task with the non-`Send` `Spawner`.

## Shared I2C contract (`src/drivers/shared_i2c.rs`)

The final R1.2 implementation keeps two mutually exclusive slots: the verified
R0 blocking slot (`install`/`with_bus`) and the async slot
(`install_async`/`with_bus_async`). The I2C0 singleton can only be installed in
one of them at runtime.

```rust
pub type SharedI2c = I2c<'static, Async>;
pub type BlockingI2c = I2c<'static, Blocking>;
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_millis(100);

pub fn install(i2c: BlockingI2c);             // R0
pub fn with_bus<R>(f: impl FnOnce(&mut BlockingI2c) -> R) -> Option<R>;
pub fn install_async(i2c: SharedI2c);
pub async fn with_bus_async<R>(f: impl FnOnce(&mut SharedI2c) -> R) -> Option<R>;

pub async fn write_async(addr: u8, bytes: &[u8]) -> Option<Result<(), I2cError>>;
pub async fn read_async(addr: u8, bytes: &mut [u8]) -> Option<Result<(), I2cError>>;
pub async fn write_read_async(
    addr: u8, write: &[u8], read: &mut [u8],
) -> Option<Result<(), I2cError>>;
```

`with_bus_async` cannot run an I2C transaction because its closure cannot await;
use the three transaction helpers, which hold the async mutex for the full
transaction and enforce the 100 ms timeout. Touch reads have priority; R1.5
sensor reads use the same helpers at a lower rate.

## Ownership

| Work package | Files |
| --- | --- |
| R1.1 presentation | `src/present.rs`, `src/drivers/display.rs`, `src/drivers/display/packing.rs`, `src/memory.rs`, `src/framebuffer.rs` |
| R1.2 input | `src/input_task.rs`, `src/drivers/touch.rs`, `src/drivers/shared_i2c.rs`, `src/input_queue.rs` |
| R1.3 UI/integration | `src/lvgl_port.rs`, `src/pointer_port.rs`, `src/ui_task.rs`, `src/ui.rs`, `src/latency.rs`, `src/main.rs`, `Cargo.toml`, `src/lib.rs` |

No package edits another package's files. `R1.3` starts after R1.1/R1.2 report
their final interfaces; any mismatch is resolved by amending this note and the
owning module, not by cross-editing.

## Acceptance

- Host tests for `DirtyRect`, the two-frame handshake and the input queue.
- Target build with `SpiDma<Async>` and `I2c<Async>`.
- Device: P95 touch-to-first-pixel ≤ 50 ms over 100 touches; no
  render/transfer serialization; no frame use-after-free; CDC stats line with
  P50/P95, transfer time and queue depth.
