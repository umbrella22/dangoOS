//! Optional 20px CJK text resource, loaded once on first voice use. It lives on
//! NAND independently of the main application image.
use crate::formats::bitmap_font::FontData;
use crate::platform::runtime::buffers::ExternalBytes;
pub const PATH: &str = "/data/ui/voice-20.bin";
pub const STAGING: &str = "/data/ui/.voice-font-incoming";
pub const STAGED: &str = "/data/ui/.voice-font-incoming/font/font.bin";
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

pub struct Font {
    bytes: ExternalBytes,
}
impl Font {
    fn size(vfs: &mut fs::Vfs, path: &str) -> Result<usize, &'static str> {
        let size = vfs.stat(path).map_err(|_| "请先安装语音中文字库")?.size as usize;
        if !(16..=MAX_BYTES).contains(&size) {
            return Err("语音字库大小无效");
        }
        Ok(size)
    }
    fn validated(bytes: ExternalBytes) -> Result<Self, &'static str> {
        let data = FontData::parse(&bytes).map_err(|_| "语音字库格式无效")?;
        if data.line_height() != 26 || data.base_line() != 5 {
            return Err("语音字库需要 20px 字号");
        }
        if !(0x4e00..=0x9fff)
            .chain(0x20..=0x7e)
            .all(|cp| data.find(cp).is_some())
        {
            return Err("语音字库缺少中文或英文字符");
        }
        Ok(Self { bytes })
    }
    pub fn read(vfs: &mut fs::Vfs, path: &str) -> Result<Self, &'static str> {
        let size = Self::size(vfs, path)?;
        let mut bytes = ExternalBytes::try_zeroed(size).map_err(|_| "语音字库需要连续 PSRAM")?;
        let fd = vfs
            .open(path, fs::OpenFlags::READ)
            .map_err(|_| "无法打开语音字库")?;
        let result = (|| {
            let mut offset = 0;
            while offset < size {
                let end = (offset + 4096).min(size);
                let n = vfs
                    .read(fd, &mut bytes[offset..end])
                    .map_err(|_| "无法读取语音字库")?;
                if n == 0 {
                    return Err("语音字库被截断");
                }
                offset += n;
            }
            Ok(())
        })();
        let closed = vfs.close(fd);
        result?;
        closed.map_err(|_| "无法关闭语音字库")?;
        Self::validated(bytes)
    }
    pub fn data(&self) -> FontData<'_> {
        // SAFETY: read validated this immutable owned allocation.
        unsafe { FontData::from_validated(&self.bytes) }
    }
}
pub fn commit(vfs: &mut fs::Vfs) -> Result<(), &'static str> {
    drop(Font::read(vfs, STAGED)?);
    vfs.rename(STAGED, PATH).map_err(|_| "无法保存语音字库")
}

#[cfg(target_os = "none")]
mod device {
    use super::*;
    use crate::{platform::runtime::executors::services::run, services::update::staging::File};
    use alloc::boxed::Box;
    use core::sync::atomic::{AtomicPtr, Ordering};
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
    static PREPARED: Channel<CriticalSectionRawMutex, Font, 1> = Channel::new();
    static ACTIVE: AtomicPtr<Font> = AtomicPtr::new(core::ptr::null_mut());
    pub fn active() -> Option<&'static Font> {
        // SAFETY: UI publishes one immutable allocation, retained until reset.
        unsafe { ACTIVE.load(Ordering::Acquire).as_ref() }
    }
    pub async fn prepare() -> Result<(), &'static str> {
        if active().is_some() {
            return Ok(());
        }
        let (file, mut bytes) = run(|| {
            fs::global::with(|v| {
                Ok((|| {
                    let size = Font::size(v, PATH)?;
                    let bytes =
                        ExternalBytes::try_zeroed(size).map_err(|_| "语音字库需要连续 PSRAM")?;
                    let fd = v
                        .open(PATH, fs::OpenFlags::READ)
                        .map_err(|_| "无法打开语音字库")?;
                    Ok((
                        File::new(
                            fd,
                            crate::platform::runtime::executors::services::close_later,
                        ),
                        bytes,
                    ))
                })())
            })
            .map_err(|_| "语音字库存储不可用")
            .and_then(|v| v)
        })
        .await?;
        let mut offset = 0;
        while offset < bytes.len() {
            let owned = file.clone();
            let (next, n) = run(move || {
                let end = (offset + 4096).min(bytes.len());
                fs::global::with(|v| v.read(owned.fd(), &mut bytes[offset..end]))
                    .map(|n| (bytes, n))
                    .map_err(|_| "无法读取语音字库")
            })
            .await?;
            bytes = next;
            if n == 0 {
                return Err("语音字库被截断");
            }
            offset += n;
        }
        run(move || file.close_with(|fd| fs::global::with(|v| v.close(fd))))
            .await
            .ok_or("无法关闭语音字库")?
            .map_err(|_| "无法关闭语音字库")?;
        let font = run(move || Font::validated(bytes)).await?;
        // A cancelled caller may have prepared a valid immutable candidate.
        // Only the single UI owner activates it and clears native font caches.
        let _ = PREPARED.try_send(font);
        Ok(())
    }
    pub fn activate_prepared() {
        if let Ok(font) = PREPARED.try_receive() {
            if active().is_none() {
                ACTIVE.store(Box::into_raw(Box::new(font)), Ordering::Release);
                // SAFETY: called solely on UI, between LVGL render passes.
                unsafe { rs_lvgl::sys::lv_obj_report_style_change(core::ptr::null_mut()) };
            }
        }
    }
}
#[cfg(target_os = "none")]
pub use device::{activate_prepared, active, prepare};
