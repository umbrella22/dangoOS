//! File systems for ESP-Mosaico.
//!
//! - [`lfs`]: littlefs 2.11 (vendored C, `vendor/littlefs-2.11.3`, BSD-3)
//!   on a [`BlockDevice`]. Disk format 2.1, the same as the official
//!   firmware's `system` partition.
//! - [`vfs`]: a mount table (`/system`, `/data`) and POSIX-style descriptors.
//! - [`global`]: the one VFS the firmware installs, and the C entry points
//!   (`mosaico_vfs_*`) that `csrc/mosaico_fs_posix.c` turns into newlib's
//!   syscalls, so C code (`fopen`, `stat`, `opendir`) reaches the mounts.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod global;
pub mod lfs;
pub mod nand;
pub mod vfs;

#[cfg(test)]
mod tests;

pub use lfs::{BlockDevice, DirEntry, Fs, Geometry, Kind, Metadata, OpenFlags, RamDevice, Whence};
pub use vfs::{Space, Vfs};

/// A POSIX errno value (positive). littlefs error codes are negated errnos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Errno(pub i32);

impl Errno {
    pub const NOENT: Self = Self(2);
    pub const IO: Self = Self(5);
    pub const BADF: Self = Self(9);
    pub const NOMEM: Self = Self(12);
    pub const BUSY: Self = Self(16);
    pub const EXIST: Self = Self(17);
    pub const XDEV: Self = Self(18);
    pub const NOTDIR: Self = Self(20);
    pub const ISDIR: Self = Self(21);
    pub const INVAL: Self = Self(22);
    pub const MFILE: Self = Self(24);
    pub const FBIG: Self = Self(27);
    pub const NOSPC: Self = Self(28);
    pub const ROFS: Self = Self(30);
    pub const NAMETOOLONG: Self = Self(91);
    pub const NOTEMPTY: Self = Self(90);
    /// newlib's EILSEQ; littlefs reports corruption as Linux EILSEQ (84).
    pub const CORRUPT: Self = Self(138);
}

impl core::fmt::Display for Errno {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let name = match *self {
            Self::NOENT => "not found",
            Self::IO => "I/O error",
            Self::BADF => "bad descriptor",
            Self::NOMEM => "out of memory",
            Self::BUSY => "busy",
            Self::EXIST => "exists",
            Self::XDEV => "cross-mount rename",
            Self::NOTDIR => "not a directory",
            Self::ISDIR => "is a directory",
            Self::INVAL => "invalid argument",
            Self::MFILE => "too many open files",
            Self::FBIG => "file too large",
            Self::NOSPC => "no space left",
            Self::ROFS => "read-only file system",
            Self::NAMETOOLONG => "name too long",
            Self::NOTEMPTY => "directory not empty",
            Self::CORRUPT => "corrupted",
            _ => return write!(f, "errno {}", self.0),
        };
        f.write_str(name)
    }
}
