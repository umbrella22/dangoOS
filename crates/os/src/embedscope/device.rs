//! Agent and platform Backend, exclusively polled by the existing UI loop.
use super::{
    capture::{CaptureState, LAST_FRAME_MS},
    logs,
    transport::{self, requests, responses, WireFrame},
    tree::Tree,
};
use crate::services::display::geometry::{HEIGHT, WIDTH};
use crate::services::input::touch::{TouchEvent, INPUT};
use crate::shell::{messages::AppMsg, navigation::route::Page};
use alloc::{boxed::Box, format, string::String};
use core::{ptr::NonNull, sync::atomic::Ordering};
use embassy_time::Instant;
use escope_agent::{Agent, Backend};
use escope_lvgl::UiBridge;
use escope_protocol::{
    capability, decode_frame, decode_payload, encode_payload, Cancel, CaptureChunk, CaptureEnd,
    CaptureRead, DeviceInfo, InputTap, Kind, Metrics, Operation, Status, TreeQuery, MAX_FRAME,
    VERSION,
};
use rs_lvgl::sys;
use rs_lvgl_ui::Ui;

struct Tap {
    x: u16,
    y: u16,
    release_ms: u64,
}
struct Device {
    boot_id: u32,
    page: Option<Page>,
    page_name: String,
    tree: UiBridge<Tree>,
    capture: CaptureState,
    tap: Option<Tap>,
    blocked: bool,
    dropped: u64,
}
impl Device {
    fn new(boot_id: u32) -> Self {
        Self {
            boot_id,
            page: None,
            page_name: String::new(),
            tree: UiBridge::new(Tree::new()),
            capture: CaptureState::new(),
            tap: None,
            blocked: false,
            dropped: 0,
        }
    }
    fn release(&mut self, force: bool) {
        if self
            .tap
            .as_ref()
            .is_some_and(|tap| force || now_ms() >= tap.release_ms)
        {
            let tap = self.tap.as_ref().unwrap();
            let event = TouchEvent {
                at_us: now_ms() * 1000,
                x: tap.x,
                y: tap.y,
                pressed: false,
            };
            if force {
                crate::services::input::touch::inject_release(event);
                self.tap = None;
            } else if INPUT.try_send(event).is_ok() {
                self.tap = None;
            }
        }
    }
}
fn now_ms() -> u64 {
    crate::platform::runtime::time::now_us() / 1000
}
impl Backend for Device {
    fn capabilities(&self) -> u32 {
        capability::LOGS
            | capability::METRICS
            | capability::UI_TREE
            | capability::INPUT
            | capability::CAPTURE
            | capability::CRASH
    }
    fn info(&self) -> DeviceInfo<'_> {
        DeviceInfo {
            name: "Dango OS",
            model: "ESP-Mosaico S31 / physical",
            firmware: concat!(env!("CARGO_PKG_VERSION"), "+embedscope"),
            build_id: env!("DANGO_EMBEDSCOPE_BUILD_ID"),
            boot_id: self.boot_id,
            uptime_ms: now_ms(),
        }
    }
    fn handle(&mut self, op: u16, payload: &[u8], reply: &mut [u8]) -> Result<usize, Status> {
        match Operation::try_from(op)? {
            Operation::Metrics => {
                if !payload.is_empty() {
                    return Err(Status::InvalidArgument);
                }
                use app_host::{caps, heap_fields};
                let internal = crate::platform::runtime::heap::snapshot(caps::INTERNAL);
                let psram = crate::platform::runtime::heap::snapshot(caps::SPIRAM);
                let frame = LAST_FRAME_MS.load(Ordering::Relaxed);
                let metrics = Metrics {
                    uptime_ms: now_ms(),
                    heap_free: (internal.valid_fields & heap_fields::SIZES != 0)
                        .then_some(internal.free_size as u64),
                    // This platform publishes a sampled minimum, not allocator low-water.
                    heap_min_free: None,
                    heap_largest_free: (internal.valid_fields & heap_fields::LARGEST_FREE != 0)
                        .then_some(internal.largest_free_block as u64),
                    psram_free: (psram.valid_fields & heap_fields::SIZES != 0)
                        .then_some(psram.free_size as u64),
                    frame_ms: (frame != 0).then_some(frame),
                    queue_depth: Some(requests().len() as u32),
                    dropped_logs: self.dropped,
                };
                encode_payload(&metrics, reply).map_err(|_| Status::Capacity)
            }
            Operation::UiTree => {
                let query: TreeQuery =
                    decode_payload(payload).map_err(|_| Status::InvalidArgument)?;
                if query.cursor == 0 && query.revision == 0 {
                    let root =
                        NonNull::new(unsafe { sys::lv_screen_active() }).ok_or(Status::Busy)?;
                    let top = NonNull::new(unsafe { sys::lv_layer_top() });
                    let mut roots = alloc::vec![root];
                    if let Some(top) = top {
                        if unsafe { sys::lv_obj_get_child_count(top.as_ptr()) } > 0 {
                            roots.push(top);
                        }
                    }
                    // The single UI owner is between layout/render operations. The
                    // snapshot owns all metadata across subsequent cursor requests.
                    unsafe {
                        self.tree.source_mut().refresh(&roots, &self.page_name)?;
                    }
                }
                self.tree.tree(payload, reply)
            }
            Operation::InputTap => {
                let tap: InputTap = decode_payload(payload).map_err(|_| Status::InvalidArgument)?;
                if tap.x >= WIDTH as u32
                    || tap.y >= HEIGHT as u32
                    || !(1..=5000).contains(&tap.hold_ms)
                {
                    return Err(Status::InvalidArgument);
                }
                if self.blocked || self.tap.is_some() {
                    return Err(Status::Busy);
                }
                let now = now_ms();
                INPUT
                    .try_send(TouchEvent {
                        at_us: now * 1000,
                        x: tap.x as u16,
                        y: tap.y as u16,
                        pressed: true,
                    })
                    .map_err(|_| Status::Busy)?;
                self.tap = Some(Tap {
                    x: tap.x as u16,
                    y: tap.y as u16,
                    release_ms: now + tap.hold_ms as u64,
                });
                crate::services::power::coordinator::wake();
                log::info!(target:"embedscope.input","tap x={} y={} hold_ms={}",tap.x,tap.y,tap.hold_ms);
                Ok(0)
            }
            Operation::CaptureStart => {
                if !payload.is_empty() {
                    return Err(Status::InvalidArgument);
                }
                let meta = self.capture.start()?;
                encode_payload(&meta, reply).map_err(|_| Status::Capacity)
            }
            Operation::CaptureRead => {
                let request: CaptureRead =
                    decode_payload(payload).map_err(|_| Status::InvalidArgument)?;
                let mut bytes = [0; 768];
                let count = self.capture.read(
                    request.capture_id,
                    request.offset,
                    request.length,
                    &mut bytes,
                )?;
                let chunk = CaptureChunk {
                    capture_id: request.capture_id,
                    offset: request.offset,
                    data: &bytes[..count],
                    done: request.offset as usize + count == WIDTH * HEIGHT * 2,
                };
                encode_payload(&chunk, reply).map_err(|_| Status::Capacity)
            }
            Operation::CaptureEnd => {
                let request: CaptureEnd =
                    decode_payload(payload).map_err(|_| Status::InvalidArgument)?;
                self.capture.end(request.capture_id)?;
                Ok(0)
            }
            Operation::Crash => {
                if !payload.is_empty() {
                    return Err(Status::InvalidArgument);
                }
                encode_payload(
                    &crate::platform::runtime::crashlog::diagnostic_summary(),
                    reply,
                )
                .map_err(|_| Status::Capacity)
            }
            Operation::Cancel => {
                let _: Cancel = decode_payload(payload).map_err(|_| Status::InvalidArgument)?;
                self.capture.reset();
                self.release(true);
                Ok(0)
            }
            _ => Err(Status::Unsupported),
        }
    }
    fn reset(&mut self) {
        self.capture.reset();
        self.release(true);
    }
}

pub(crate) struct UiAgent {
    agent: Agent,
    backend: Device,
    epoch: u32,
    pending: Option<(WireFrame, Instant)>,
    handshaken: bool,
    last_stats: u64,
}
impl UiAgent {
    /// Keep Agent/LogRing construction out of the UI task's permanent poll
    /// frame, which also hosts deep native layout and reconciliation calls.
    #[inline(never)]
    pub fn new() -> Box<Self> {
        let boot_id = esp_hal::rng::Rng::new().random().max(1);
        crate::platform::runtime::crashlog::set_diagnostic_boot(boot_id);
        Box::new(Self {
            agent: Agent::new(boot_id),
            backend: Device::new(boot_id),
            epoch: transport::epoch(),
            pending: None,
            handshaken: false,
            last_stats: 0,
        })
    }
    /// Bounded work only. No USB I/O, LVGL callbacks or display copy awaits.
    pub fn poll(&mut self, ui: &Ui<AppMsg>, page: Page, blocked: bool) {
        let epoch = transport::epoch();
        if epoch != self.epoch {
            self.agent.reset(&mut self.backend);
            self.pending = None;
            self.handshaken = false;
            self.epoch = epoch;
            while responses().try_receive().is_ok() {}
            while logs::queue().try_receive().is_ok() {}
            logs::DROPPED.store(0, Ordering::Relaxed);
        }
        self.backend.release(false);
        self.backend.capture.poll();
        self.backend.blocked = blocked;
        if self.backend.page != Some(page) {
            self.backend.page = Some(page);
            self.backend.page_name = format!("page={page:?}");
        }
        if !transport::connected() {
            return;
        }
        let now = now_ms();
        if now.saturating_sub(self.last_stats) >= 1000 {
            self.last_stats = now;
            let s = ui.stats();
            let q = ui.queue_stats();
            log::info!(target:"rs_lvgl_ui.stats","page={page:?} mounted={} processed={} dropped={} updates={} created={} deleted={} moved={} evicted={} rejected={} merged={}",s.mounted_nodes,s.messages_processed,s.messages_dropped,s.reconciled_updates,s.created_nodes,s.deleted_nodes,s.moved_nodes,q.evicted,q.rejected,q.merged);
        }
        for _ in 0..4 {
            let Ok(log) = logs::queue().try_receive() else {
                break;
            };
            let _ = self.agent.push_log(
                log.at_ms,
                log.level,
                log.target.as_str(),
                log.message.as_str(),
            );
        }
        self.agent
            .record_dropped_logs(logs::DROPPED.swap(0, Ordering::Relaxed) as u64);
        self.backend.dropped = self.agent.dropped_logs();
        for _ in 0..2 {
            if responses().is_full() {
                break;
            }
            let (frame, since) = match self.pending.take() {
                Some(p) => p,
                None => match requests().try_receive() {
                    Ok(f) => (f, Instant::now()),
                    Err(_) => break,
                },
            };
            if frame.epoch != epoch {
                continue;
            }
            let mut scratch = [0; MAX_FRAME];
            let Ok(request) = decode_frame(&frame.bytes[..frame.len], &mut scratch) else {
                continue;
            };
            let valid = request.version == VERSION
                && request.kind == Kind::Request
                && request.id != 0
                && request.status == Status::Ok;
            if valid
                && self.handshaken
                && request.session == self.agent.session_id()
                && request.op == u16::from(Operation::CaptureStart)
                && request.payload.is_empty()
                && since.elapsed().as_millis() < 750
                && !self.backend.capture.prepare()
            {
                self.pending = Some((frame, since));
                break;
            }
            if valid
                && request.op == u16::from(Operation::Hello)
                && request.session == 0
                && request.payload.is_empty()
            {
                self.handshaken = true;
                while logs::queue().try_receive().is_ok() {}
                logs::DROPPED.store(0, Ordering::Relaxed);
            }
            let mut response = WireFrame {
                epoch,
                len: 0,
                bytes: [0; MAX_FRAME],
            };
            if let Ok(len) = self
                .agent
                .handle(request, &mut self.backend, &mut response.bytes)
            {
                response.len = len;
                let _ = responses().try_send(response);
            }
        }
        if !responses().is_full() {
            let mut event = WireFrame {
                epoch,
                len: 0,
                bytes: [0; MAX_FRAME],
            };
            if let Ok(Some(len)) = self.agent.poll_event(&mut event.bytes) {
                event.len = len;
                let _ = responses().try_send(event);
            }
        }
    }
}
