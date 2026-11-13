//! MUIF v1: four MFNT files, identified by their full SHA-256 in the firmware.
//! Payloads use External memory exclusively and are retained once per boot.
use crate::formats::bitmap_font::FontData;
use ::fs::{Kind, OpenFlags, Vfs};
use core::ops::Range;
use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/ui-fonts.rs"));
pub const STAGING: &str = "/data/ui/.incoming";
pub const STAGED: &str = "/data/ui/.incoming/fonts/fonts.bin";

#[cfg(not(target_os = "none"))]
pub fn bundled() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/ui-fonts.bin"))
}

/// Validate the complete format, independently of the firmware's content pin.
pub fn parse_bundle(bytes: &[u8]) -> Result<[Range<usize>; 4], &'static str> {
    if bytes.len() < 24 || &bytes[..8] != b"MUIF\x01\x00\x04\x00" {
        return Err("invalid UI font bundle header");
    }
    let mut ranges = core::array::from_fn(|_| 0..0);
    let mut offset = 24;
    for (i, range) in ranges.iter_mut().enumerate() {
        let at = 8 + i * 4;
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if len > bytes.len() - offset {
            return Err("truncated UI font bundle");
        }
        *range = offset..offset + len;
        FontData::parse(&bytes[range.clone()])?;
        offset += len;
    }
    if offset != bytes.len() {
        return Err("trailing UI font bundle bytes");
    }
    Ok(ranges)
}

#[cfg(not(target_os = "none"))]
struct Bytes(alloc::boxed::Box<[u8]>);
#[cfg(not(target_os = "none"))]
impl Bytes {
    fn new() -> Result<Self, &'static str> {
        Ok(Self(alloc::vec![0; BYTES].into_boxed_slice()))
    }
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}
#[cfg(target_os = "none")]
struct Bytes(core::ptr::NonNull<u8>);
#[cfg(target_os = "none")]
impl Bytes {
    const LAYOUT: core::alloc::Layout = match core::alloc::Layout::from_size_align(BYTES, 4) {
        Ok(layout) => layout,
        Err(_) => panic!("font allocation layout"),
    };
    fn new() -> Result<Self, &'static str> {
        // SAFETY: nonzero layout; the owner frees via the same heap on failure.
        let p = unsafe {
            esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), Self::LAYOUT)
        };
        if !p.is_null() {
            // SAFETY: initialize the allocation before exposing a byte slice.
            unsafe { p.write_bytes(0, BYTES) };
        }
        core::ptr::NonNull::new(p)
            .map(Self)
            .ok_or("UI fonts need contiguous PSRAM")
    }
    fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: exclusive borrow of the full owned allocation.
        unsafe { core::slice::from_raw_parts_mut(self.0.as_ptr(), BYTES) }
    }
    fn as_slice(&self) -> &[u8] {
        // SAFETY: read completes before this immutable view is exposed.
        unsafe { core::slice::from_raw_parts(self.0.as_ptr(), BYTES) }
    }
}
#[cfg(target_os = "none")]
impl Drop for Bytes {
    fn drop(&mut self) {
        use core::alloc::GlobalAlloc;
        // SAFETY: sole owner of an allocation from this heap with this layout.
        unsafe { esp_alloc::HEAP.dealloc(self.0.as_ptr(), Self::LAYOUT) }
    }
}
// SAFETY: owned allocation, with no shared mutable state or device handles.
#[cfg(target_os = "none")]
unsafe impl Send for Bytes {}

pub struct Fonts {
    bytes: Bytes,
    ranges: [Range<usize>; 4],
}
impl Fonts {
    pub fn read(vfs: &mut Vfs, path: &str) -> Result<Self, &'static str> {
        let meta = vfs.stat(path).map_err(|_| "UI font bundle unavailable")?;
        if meta.kind != Kind::File || meta.size as usize != BYTES {
            return Err("UI font bundle size mismatch");
        }
        let mut bytes = Bytes::new()?;
        let fd = vfs
            .open(path, OpenFlags::READ)
            .map_err(|_| "UI font bundle open failed")?;
        let result = (|| {
            let mut offset = 0;
            while offset < BYTES {
                let n = vfs
                    .read(fd, &mut bytes.as_mut()[offset..])
                    .map_err(|_| "UI font bundle read failed")?;
                if n == 0 {
                    return Err("UI font bundle truncated");
                }
                offset += n;
            }
            Ok(())
        })();
        let closed = vfs.close(fd).map_err(|_| "UI font bundle close failed");
        result?;
        closed?;
        if <[u8; 32]>::from(Sha256::digest(bytes.as_slice())) != SHA256 {
            return Err("UI font bundle hash mismatch");
        }
        let ranges = parse_bundle(bytes.as_slice())?;
        Ok(Self { bytes, ranges })
    }
    pub fn load(vfs: &mut Vfs) -> Result<Self, &'static str> {
        Self::read(vfs, DATA_PATH).or_else(|_| Self::read(vfs, SYSTEM_PATH))
    }
    pub fn font(&self, index: usize) -> FontData<'_> {
        // SAFETY: the entire immutable set was checked before construction.
        unsafe { FontData::from_validated(&self.bytes.as_slice()[self.ranges[index].clone()]) }
    }
}

/// Verify before a same-volume atomic rename; older content hashes survive.
pub fn commit(vfs: &mut Vfs) -> Result<(), alloc::string::String> {
    drop(Fonts::read(vfs, STAGED).map_err(alloc::string::String::from)?);
    vfs.rename(STAGED, DATA_PATH)
        .map_err(|e| alloc::format!("UI fonts commit: {e}"))?;
    let _ = vfs.remove_tree(STAGING);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn font_formats_reject_truncation_order_and_bitmap_overruns() {
        let source = bundled();
        assert!(parse_bundle(source).is_ok());
        for end in [0, 23, 24, 100, source.len() - 1] {
            assert!(parse_bundle(&source[..end]).is_err());
        }
        let ranges = parse_bundle(source).unwrap();
        let font = &source[ranges[0].clone()];
        for (at, value) in [(0, 0), (4, 2), (12, 1), (16 + 14, 1)] {
            let mut bad = font.to_vec();
            bad[at] = value;
            assert!(FontData::parse(&bad).is_err());
        }
        let mut bad = font.to_vec();
        bad[32..36].copy_from_slice(&font[16..20]);
        assert!(FontData::parse(&bad).is_err());
        let mut bad = font.to_vec();
        bad[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(FontData::parse(&bad).is_err());
        let mut extra = source.to_vec();
        extra.push(0);
        assert!(parse_bundle(&extra).is_err());
    }
    #[test]
    fn invalid_nand_fonts_fall_back_to_pinned_system_and_close_descriptors() {
        let mut vfs = Vfs::new();
        for name in ["data", "system"] {
            let disk = ::fs::Fs::mount_or_format(
                alloc::boxed::Box::new(::fs::RamDevice::new(4096, 128)),
                ::fs::Geometry::flash(128),
            )
            .unwrap()
            .0;
            vfs.mount(name, disk);
            vfs.create_dir_all(&alloc::format!("/{name}/ui")).unwrap();
        }
        assert!(Fonts::load(&mut vfs).is_err());
        vfs.write_file(SYSTEM_PATH, bundled()).unwrap();
        let mut bad = bundled().to_vec();
        bad[100] ^= 1;
        vfs.write_file(DATA_PATH, &bad).unwrap();
        assert!(Fonts::read(&mut vfs, DATA_PATH).is_err());
        let fonts = Fonts::load(&mut vfs).unwrap();
        assert!(fonts.font(0).find('低' as u32).is_some());
        assert_eq!(vfs.open_files(), 0);
    }
}
