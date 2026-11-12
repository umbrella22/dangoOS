//! Plane-owned copy work. No capture waits or pixel copies run on the UI task.
use super::frame::FinalFrame;
use crate::services::display::geometry::{DirtyRect, HEIGHT, WIDTH};
use alloc::{boxed::Box, vec};
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use escope_protocol::{CaptureMeta, PixelFormat, Status};

pub(crate) static LAST_FRAME_MS: AtomicU32 = AtomicU32::new(0);
pub(crate) struct CaptureRequest {
    token: u32,
    pixels: Box<[u16]>,
}
struct Snapshot {
    token: u32,
    pixels: Box<[u16]>,
    frame_id: u64,
    source: &'static str,
}
static REQUESTS: Channel<CriticalSectionRawMutex, CaptureRequest, 1> = Channel::new();
static RESULTS: Channel<CriticalSectionRawMutex, Snapshot, 1> = Channel::new();

pub(crate) struct FinalCapture {
    frame: FinalFrame,
    source: &'static str,
}
impl FinalCapture {
    pub fn new() -> Self {
        Self {
            frame: FinalFrame::new(WIDTH, HEIGHT),
            source: "unavailable",
        }
    }
    #[cfg(target_os = "none")]
    pub async fn request() -> CaptureRequest {
        REQUESTS.receive().await
    }
    pub fn serve(&self, mut request: CaptureRequest) {
        request.pixels.copy_from_slice(&self.frame.pixels);
        // There is one owned snapshot buffer; its response slot cannot fill
        // until the UI has taken the preceding response and returned the buffer.
        let _ = RESULTS.try_send(Snapshot {
            token: request.token,
            pixels: request.pixels,
            frame_id: self.frame.id,
            source: self.source,
        });
    }
    pub fn clear_outside(&mut self, at: (u16, u16), size: (u16, u16)) {
        self.frame.clear_outside(at, size);
    }
    pub fn blit_source(
        &mut self,
        source: board_s31::pixels::PixelSource<'_>,
        area: DirtyRect,
        at: (u16, u16),
    ) {
        self.frame.blit_source(source, area, at);
    }
    pub fn complete(&mut self, elapsed_ms: u32, source: &'static str) {
        self.source = source;
        self.frame.complete();
        LAST_FRAME_MS.store(elapsed_ms, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_queues() {
        while REQUESTS.try_receive().is_ok() {}
        while RESULTS.try_receive().is_ok() {}
    }
    fn serve(plane: &FinalCapture, capture: &mut CaptureState) {
        assert!(!capture.prepare());
        plane.serve(REQUESTS.try_receive().unwrap());
        capture.poll();
    }

    #[test]
    fn frozen_odd_byte_reads_repeat_while_the_live_plane_changes() {
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_queues();
        let mut plane = FinalCapture::new();
        // The plane now supplies big-endian Home storage. Capture still
        // exposes the protocol's little-endian logical RGB565 pixels.
        plane.blit_source(
            board_s31::pixels::PixelSource::bytes(
                &[0x11, 0x22, 0x33, 0x44],
                (2, 1),
                4,
                board_s31::pixels::PixelFormat::Rgb565Be,
            ),
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 1,
                y1: 0,
            },
            (0, 0),
        );
        plane.complete(1, "Home");
        let mut capture = CaptureState::new();
        serve(&plane, &mut capture);
        let meta = capture.start().unwrap();
        assert_eq!(meta.frame_id, 1);
        assert!(matches!(capture.start(), Err(Status::Busy)));

        plane.clear_outside((0, 0), (1, 1));
        plane.blit_source(
            board_s31::pixels::PixelSource::rgb565(&[0xabcd], (1, 1)),
            DirtyRect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            },
            (0, 0),
        );
        plane.complete(2, "Screen");
        let mut bytes = [0; 4];
        for _ in 0..2 {
            assert_eq!(capture.read(meta.capture_id, 1, 3, &mut bytes), Ok(3));
            assert_eq!(&bytes[..3], &[0x11, 0x44, 0x33]);
        }
        assert_eq!(
            capture.read(meta.capture_id + 1, 0, 4, &mut bytes),
            Err(Status::Stale)
        );
        assert_eq!(
            capture.read(meta.capture_id, 0, 0, &mut bytes),
            Err(Status::InvalidArgument)
        );
        assert_eq!(
            capture.read(meta.capture_id, meta.total_bytes, 1, &mut bytes),
            Err(Status::InvalidArgument)
        );
        assert_eq!(capture.end(meta.capture_id + 1), Err(Status::Stale));
        capture.end(meta.capture_id).unwrap();
        serve(&plane, &mut capture);
        let next = capture.start().unwrap();
        assert_eq!(next.frame_id, 2);
        assert_eq!(capture.read(next.capture_id, 0, 4, &mut bytes), Ok(4));
        assert_eq!(bytes, [0xcd, 0xab, 0, 0]);
        capture.end(next.capture_id).unwrap();
    }

    #[test]
    fn reset_recycles_late_snapshots_and_invalidates_an_active_capture() {
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_queues();
        let mut plane = FinalCapture::new();
        plane.complete(1, "Home");
        let mut capture = CaptureState::new();
        assert!(!capture.prepare());
        capture.reset();
        // A request already owned by the plane or still in its queue cannot be
        // duplicated. Its late response gives the sole buffer back to spare.
        assert!(!capture.prepare());
        plane.serve(REQUESTS.try_receive().unwrap());
        capture.poll();
        assert!(capture.ready.is_none());
        assert!(capture.spare.is_some());
        serve(&plane, &mut capture);
        let meta = capture.start().unwrap();
        capture.reset();
        assert_eq!(
            capture.read(meta.capture_id, 0, 1, &mut [0]),
            Err(Status::Stale)
        );
        assert!(capture.spare.is_some());
        serve(&plane, &mut capture);
        let next = capture.start().unwrap();
        assert!(next.capture_id > meta.capture_id);
        capture.end(next.capture_id).unwrap();
    }

    #[test]
    fn exhausted_capture_ids_retain_the_only_snapshot_buffer() {
        let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_queues();
        let mut plane = FinalCapture::new();
        plane.complete(1, "Home");
        let mut capture = CaptureState::new();
        serve(&plane, &mut capture);
        capture.next = u32::MAX;
        assert!(matches!(capture.start(), Err(Status::Capacity)));
        assert!(capture.ready.is_some());
        capture.reset();
        assert!(capture.ready.is_none());
        assert_eq!(capture.spare.as_ref().unwrap().len(), WIDTH * HEIGHT);
    }
}

pub(super) struct CaptureState {
    spare: Option<Box<[u16]>>,
    waiting: Option<u32>,
    ready: Option<Snapshot>,
    active: Option<(u32, Snapshot)>,
    next: u32,
    token: u32,
}
impl CaptureState {
    pub fn new() -> Self {
        Self {
            spare: Some(vec![0; WIDTH * HEIGHT].into_boxed_slice()),
            waiting: None,
            ready: None,
            active: None,
            next: 1,
            token: 1,
        }
    }
    pub fn poll(&mut self) {
        while let Ok(snapshot) = RESULTS.try_receive() {
            if self.waiting == Some(snapshot.token) {
                self.waiting = None;
                self.ready = Some(snapshot);
            } else {
                self.spare = Some(snapshot.pixels);
            }
        }
    }
    pub fn prepare(&mut self) -> bool {
        self.poll();
        if self.active.is_some() || self.ready.is_some() {
            return true;
        }
        if self.waiting.is_none() {
            if let Some(pixels) = self.spare.take() {
                self.token = self.token.wrapping_add(1).max(1);
                let request = CaptureRequest {
                    token: self.token,
                    pixels,
                };
                match REQUESTS.try_send(request) {
                    Ok(()) => self.waiting = Some(self.token),
                    Err(embassy_sync::channel::TrySendError::Full(request)) => {
                        self.spare = Some(request.pixels)
                    }
                }
            }
        }
        false
    }
    pub fn start(&mut self) -> Result<CaptureMeta, Status> {
        if self.active.is_some() {
            return Err(Status::Busy);
        }
        // Exhaustion must retain the sole owned snapshot buffer for reset.
        let next = self.next.checked_add(1).ok_or(Status::Capacity)?;
        let snapshot = self.ready.take().ok_or(Status::Busy)?;
        if snapshot.frame_id == 0 {
            self.spare = Some(snapshot.pixels);
            return Err(Status::Busy);
        }
        let capture_id = self.next;
        self.next = next;
        let frame_id = snapshot.frame_id;
        log::info!(target:"embedscope.capture","capture_id={capture_id} frame_id={frame_id} source={}",snapshot.source);
        self.active = Some((capture_id, snapshot));
        Ok(CaptureMeta {
            capture_id,
            width: WIDTH as u32,
            height: HEIGHT as u32,
            stride: (WIDTH * 2) as u32,
            format: PixelFormat::Rgb565Le,
            total_bytes: (WIDTH * HEIGHT * 2) as u32,
            frame_id,
        })
    }
    pub fn read(&self, id: u32, offset: u32, length: u32, out: &mut [u8]) -> Result<usize, Status> {
        let (active, snapshot) = self.active.as_ref().ok_or(Status::Stale)?;
        if *active != id {
            return Err(Status::Stale);
        }
        let total = snapshot.pixels.len() * 2;
        let offset = offset as usize;
        if length == 0 || offset >= total {
            return Err(Status::InvalidArgument);
        }
        let count = (length as usize).min(out.len()).min(total - offset);
        for (index, byte) in out[..count].iter_mut().enumerate() {
            let at = offset + index;
            *byte = snapshot.pixels[at / 2].to_le_bytes()[at % 2];
        }
        Ok(count)
    }
    pub fn end(&mut self, id: u32) -> Result<(), Status> {
        if self.active.as_ref().map(|v| v.0) != Some(id) {
            return Err(Status::Stale);
        }
        self.spare = self.active.take().map(|(_, s)| s.pixels);
        Ok(())
    }
    pub fn reset(&mut self) {
        self.waiting = None;
        if let Some(s) = self.ready.take() {
            self.spare = Some(s.pixels);
        }
        if let Some((_, s)) = self.active.take() {
            self.spare = Some(s.pixels);
        }
        self.poll();
    }
}
