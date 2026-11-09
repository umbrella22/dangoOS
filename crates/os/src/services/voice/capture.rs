//! A reusable three-second FIFO. Only the audio owner closes the end fence;
//! a normal finish drains every accepted sample before the network commits.
use crate::platform::runtime::buffers::ExternalBytes;
#[cfg(test)]
use alloc::vec::Vec;

pub const RATE: usize = 24_000;
pub const QUEUE_BYTES: usize = RATE * 2 * 3;
pub const PACKET_BYTES: usize = RATE * 2 * 80 / 1000;

pub struct Buffer {
    bytes: ExternalBytes,
    session: u32,
    read: usize,
    len: usize,
    pub frames: u32,
    pub ready: bool,
    pub finish: bool,
    pub closed: bool,
    pub error: Option<&'static str>,
}
impl Buffer {
    pub fn new() -> Result<Self, &'static str> {
        let bytes = ExternalBytes::try_zeroed(QUEUE_BYTES).map_err(|_| "语音缓冲需要连续 PSRAM")?;
        Ok(Self {
            bytes,
            session: 0,
            read: 0,
            len: 0,
            frames: 0,
            ready: false,
            finish: false,
            closed: true,
            error: None,
        })
    }
    pub fn start(&mut self, session: u32) -> Result<(), &'static str> {
        if !self.closed {
            return Err("麦克风仍在停止");
        }
        self.session = session;
        self.read = 0;
        self.len = 0;
        self.frames = 0;
        self.ready = false;
        self.finish = false;
        self.closed = false;
        self.error = None;
        Ok(())
    }
    pub fn accepts(&self, session: u32) -> bool {
        self.session == session
    }
    pub fn mark_ready(&mut self, session: u32) {
        if self.accepts(session) && !self.closed {
            self.ready = true;
        }
    }
    pub fn request_finish(&mut self, session: u32) {
        if self.accepts(session) {
            self.finish = true;
        }
    }
    pub fn abort(&mut self, session: u32) {
        if self.accepts(session) && !self.closed {
            self.finish = true;
            self.error = Some("语音会话已取消");
        }
    }
    pub fn close(&mut self, session: u32, error: Option<&'static str>) {
        if self.accepts(session) {
            self.closed = true;
            if self.error.is_none() {
                self.error = error;
            }
            if self.error.is_some() {
                self.len = 0;
            }
        }
    }
    pub fn push(&mut self, session: u32, bytes: &[u8]) -> Result<(), &'static str> {
        if !self.accepts(session) || self.closed {
            return Err("过期的麦克风会话");
        }
        if bytes.len() % 2 != 0 {
            return Err("不完整的麦克风采样");
        }
        let maximum = super::MAX_RECORDING_MS as usize * RATE / 1000;
        let bytes = &bytes[..bytes.len().min((maximum - self.frames as usize) * 2)];
        if bytes.len() > QUEUE_BYTES - self.len {
            // The producer still owns the hardware. Leave the fence open until
            // it acknowledges the failure; a new turn cannot reuse it early.
            self.error = Some("语音连接过慢，录音缓冲已满");
            self.finish = true;
            return Err("语音连接过慢，录音缓冲已满");
        }
        let write = (self.read + self.len) % QUEUE_BYTES;
        let first = bytes.len().min(QUEUE_BYTES - write);
        self.bytes[write..write + first].copy_from_slice(&bytes[..first]);
        self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.len += bytes.len();
        self.frames += (bytes.len() / 2) as u32;
        if self.frames as usize == maximum {
            self.finish = true;
        }
        Ok(())
    }
    pub fn pop(&mut self, session: u32, out: &mut [u8]) -> Result<usize, &'static str> {
        if !self.accepts(session) {
            return Err("过期的麦克风会话");
        }
        if let Some(error) = self.error {
            return Err(error);
        }
        let n = self.len.min(out.len() & !1);
        let first = n.min(QUEUE_BYTES - self.read);
        out[..first].copy_from_slice(&self.bytes[self.read..self.read + first]);
        out[first..n].copy_from_slice(&self.bytes[..n - first]);
        self.read = (self.read + n) % QUEUE_BYTES;
        self.len -= n;
        Ok(n)
    }
    pub fn drained(&self, session: u32) -> bool {
        self.accepts(session) && self.closed && self.len == 0 && self.error.is_none()
    }
    pub fn milliseconds(&self) -> u32 {
        self.frames / (RATE as u32 / 1000)
    }
    pub fn queued(&self) -> usize {
        self.len
    }
}

/// 48 kHz hardware capture -> 24 kHz PCM16 mono. A 15-tap half-band FIR
/// attenuates frequencies that would alias; the codec clock is unchanged.
/// Coefficients sum to 32768, with signed 64-bit accumulation for headroom.
pub struct Downsample {
    history: [i16; 15],
    at: usize,
    odd: bool,
}
impl Default for Downsample {
    fn default() -> Self {
        Self {
            history: [0; 15],
            at: 0,
            odd: false,
        }
    }
}
impl Downsample {
    pub fn convert_in_place(&mut self, stereo: &mut [u8]) -> usize {
        const FIR: [i32; 15] = [
            -119, 0, 529, 0, -2255, 0, 10037, 16384, 10037, 0, -2255, 0, 529, 0, -119,
        ];
        let mut written = 0;
        for frame in 0..stereo.len() / 4 {
            self.history[self.at] = i16::from_le_bytes([stereo[frame * 4], stereo[frame * 4 + 1]]);
            self.at = (self.at + 1) % 15;
            self.odd = !self.odd;
            if self.odd {
                continue;
            }
            let mut sum = 0i64;
            for (i, coefficient) in FIR.iter().enumerate() {
                sum += i64::from(self.history[(self.at + i) % 15]) * i64::from(*coefficient);
            }
            let sample = (sum / 32768).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            stereo[written..written + 2].copy_from_slice(&sample.to_le_bytes());
            written += 2;
        }
        written
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn end_fence_drains_tail_and_old_sessions_cannot_change_a_new_turn() {
        let mut q = Buffer::new().unwrap();
        q.start(1).unwrap();
        q.push(1, &[1, 2]).unwrap();
        q.request_finish(1);
        q.push(1, &[3, 4]).unwrap(); // final RX service after finish request
        assert!(!q.drained(1));
        assert!(q.start(2).is_err());
        q.close(1, None);
        let mut out = [0; 8];
        assert_eq!(q.pop(1, &mut out), Ok(4));
        assert_eq!(&out[..4], &[1, 2, 3, 4]);
        assert!(q.drained(1));
        q.start(2).unwrap();
        q.close(1, Some("old failure"));
        assert!(!q.closed);
        assert!(q.push(1, &[0; 2]).is_err());
        q.push(2, &[5, 6]).unwrap();
        assert_eq!(q.pop(2, &mut out), Ok(2));
        assert_eq!(&out[..2], &[5, 6]);
    }
    #[test]
    fn wrap_and_overflow_never_silently_drop_audio() {
        let mut q = Buffer::new().unwrap();
        q.start(1).unwrap();
        q.push(1, &alloc::vec![7; QUEUE_BYTES - 2]).unwrap();
        let mut out = alloc::vec![0; QUEUE_BYTES - 4];
        q.pop(1, &mut out).unwrap();
        q.push(1, &[8; 8]).unwrap();
        let mut last = [0; 10];
        assert_eq!(q.pop(1, &mut last), Ok(10));
        assert_eq!(last, [7, 7, 8, 8, 8, 8, 8, 8, 8, 8]);
        q.push(1, &alloc::vec![0; QUEUE_BYTES]).unwrap();
        assert!(q.push(1, &[1; 2]).is_err());
        assert!(q.finish && !q.closed);
        assert!(q.pop(1, &mut last).is_err());
        q.close(1, None);
        assert!(!q.drained(1));
    }
    #[test]
    fn downsampling_is_chunk_independent_and_preserves_dc_without_overflow() {
        let source: Vec<u8> = (0..2048)
            .flat_map(|i| {
                let n = if i < 1024 { i16::MAX } else { i16::MIN };
                [n.to_le_bytes(), [0, 0]].concat()
            })
            .collect();
        let mut whole = source.clone();
        let n = Downsample::default().convert_in_place(&mut whole);
        let mut filter = Downsample::default();
        let mut parts = Vec::new();
        for c in source.chunks(28) {
            let mut c = c.to_vec();
            let n = filter.convert_in_place(&mut c);
            parts.extend_from_slice(&c[..n]);
        }
        assert_eq!(&whole[..n], parts);
        assert_eq!(n, 2048);
        assert_eq!(i16::from_le_bytes([whole[100], whole[101]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([whole[1800], whole[1801]]), i16::MIN);
    }
}
