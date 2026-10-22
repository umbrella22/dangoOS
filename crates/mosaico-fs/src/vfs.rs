//! Mount table and descriptor table: POSIX-style calls over [`Fs`] mounts.
//!
//! Paths are absolute. `/system` and `/data` style prefixes select a mount;
//! `/` itself is a read-only directory listing the mounts. File descriptors
//! start at 3 (0-2 are the console, handled by the C layer).

use alloc::string::String;
use alloc::vec::Vec;

use crate::lfs::{DirEntry, File, Fs, Kind, Metadata, OpenFlags, Whence};
use crate::Errno;

/// First file descriptor handed out (0-2 are stdin/stdout/stderr).
pub const FIRST_FD: i32 = 3;
/// Open files at once (Lua scripts plus a push in progress).
pub const MAX_FILES: usize = 16;

struct Mount {
    prefix: String,
    fs: Fs,
}

struct OpenFile {
    mount: usize,
    file: File,
}

struct OpenDir {
    entries: Vec<DirEntry>,
    pos: usize,
}

/// Space on a mount, in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub block_size: u32,
    pub blocks: u32,
    pub free: u32,
}

#[derive(Default)]
pub struct Vfs {
    mounts: Vec<Mount>,
    files: Vec<Option<OpenFile>>,
    dirs: Vec<Option<OpenDir>>,
}

/// Normalise an absolute path: collapse `//`, drop `.`, resolve `..`.
/// Returns the components.
pub fn components(path: &str) -> Result<Vec<&str>, Errno> {
    if !path.starts_with('/') {
        return Err(Errno::NOENT);
    }
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    Ok(out)
}

impl Vfs {
    pub const fn new() -> Self {
        Self {
            mounts: Vec::new(),
            files: Vec::new(),
            dirs: Vec::new(),
        }
    }

    /// Mount `fs` at `/name` (one component, e.g. `"system"`).
    pub fn mount(&mut self, name: &str, fs: Fs) {
        self.mounts.push(Mount {
            prefix: String::from(name),
            fs,
        });
    }

    pub fn mount_names(&self) -> impl Iterator<Item = &str> {
        self.mounts.iter().map(|m| m.prefix.as_str())
    }

    /// Split a path into (mount index, path inside the mount). `None` for
    /// the root directory itself.
    fn resolve(&self, path: &str) -> Result<Option<(usize, String)>, Errno> {
        let parts = components(path)?;
        let Some((first, rest)) = parts.split_first() else {
            return Ok(None);
        };
        let idx = self
            .mounts
            .iter()
            .position(|m| m.prefix == *first)
            .ok_or(Errno::NOENT)?;
        let mut inner = String::from("/");
        inner.push_str(&rest.join("/"));
        Ok(Some((idx, inner)))
    }

    fn resolve_mount(&mut self, path: &str) -> Result<(&mut Fs, String), Errno> {
        match self.resolve(path)? {
            Some((i, inner)) => Ok((&mut self.mounts[i].fs, inner)),
            // The root is a synthetic, read-only directory.
            None => Err(Errno::ROFS),
        }
    }

    pub fn stat(&mut self, path: &str) -> Result<Metadata, Errno> {
        match self.resolve(path)? {
            None => Ok(Metadata {
                kind: Kind::Dir,
                size: 0,
            }),
            Some((i, inner)) => self.mounts[i].fs.stat(&inner),
        }
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), Errno> {
        let (fs, inner) = self.resolve_mount(path)?;
        if inner == "/" {
            return Err(Errno::EXIST);
        }
        fs.mkdir(&inner)
    }

    /// Remove a file (`unlink`).
    pub fn unlink(&mut self, path: &str) -> Result<(), Errno> {
        let (fs, inner) = self.resolve_mount(path)?;
        if fs.stat(&inner)?.kind == Kind::Dir {
            return Err(Errno::ISDIR);
        }
        fs.remove(&inner)
    }

    /// Remove an empty directory (`rmdir`).
    pub fn rmdir(&mut self, path: &str) -> Result<(), Errno> {
        let (fs, inner) = self.resolve_mount(path)?;
        if inner == "/" {
            return Err(Errno::BUSY);
        }
        if fs.stat(&inner)?.kind != Kind::Dir {
            return Err(Errno::NOTDIR);
        }
        fs.remove(&inner)
    }

    /// Remove a file or a whole directory tree.
    pub fn remove_tree(&mut self, path: &str) -> Result<(), Errno> {
        let (fs, inner) = self.resolve_mount(path)?;
        if inner == "/" {
            return Err(Errno::BUSY);
        }
        remove_tree(fs, &inner)
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Errno> {
        let a = self.resolve(from)?.ok_or(Errno::BUSY)?;
        let b = self.resolve(to)?.ok_or(Errno::BUSY)?;
        if a.0 != b.0 {
            return Err(Errno::XDEV);
        }
        self.mounts[a.0].fs.rename(&a.1, &b.1)
    }

    pub fn read_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, Errno> {
        match self.resolve(path)? {
            None => Ok(self
                .mounts
                .iter()
                .map(|m| DirEntry {
                    name: m.prefix.clone(),
                    meta: Metadata {
                        kind: Kind::Dir,
                        size: 0,
                    },
                })
                .collect()),
            Some((i, inner)) => self.mounts[i].fs.read_dir(&inner),
        }
    }

    pub fn space(&mut self, path: &str) -> Result<Space, Errno> {
        let (fs, _) = self.resolve_mount(path).or_else(|e| match e {
            // statvfs("/") reports nothing useful; keep it an error.
            Errno::ROFS => Err(Errno::NOENT),
            e => Err(e),
        })?;
        let g = fs.geometry();
        let used = fs.used_blocks()?;
        Ok(Space {
            block_size: g.block_size,
            blocks: g.block_count,
            free: g.block_count.saturating_sub(used),
        })
    }

    // ---- descriptors --------------------------------------------------------

    pub fn open(&mut self, path: &str, flags: OpenFlags) -> Result<i32, Errno> {
        let (i, inner) = match self.resolve(path)? {
            Some(r) => r,
            None => return Err(Errno::ISDIR),
        };
        let slot = match self.files.iter().position(Option::is_none) {
            Some(s) => s,
            None if self.files.len() < MAX_FILES => {
                self.files.push(None);
                self.files.len() - 1
            }
            None => return Err(Errno::MFILE),
        };
        let fs = &mut self.mounts[i].fs;
        if let Ok(meta) = fs.stat(&inner) {
            if meta.kind == Kind::Dir {
                return Err(Errno::ISDIR);
            }
        }
        let file = fs.open(&inner, flags)?;
        self.files[slot] = Some(OpenFile { mount: i, file });
        Ok(FIRST_FD + slot as i32)
    }

    fn file(&mut self, fd: i32) -> Result<(&mut Fs, &mut File), Errno> {
        let slot = usize::try_from(fd - FIRST_FD).map_err(|_| Errno::BADF)?;
        let f = self
            .files
            .get_mut(slot)
            .and_then(Option::as_mut)
            .ok_or(Errno::BADF)?;
        Ok((&mut self.mounts[f.mount].fs, &mut f.file))
    }

    pub fn close(&mut self, fd: i32) -> Result<(), Errno> {
        let slot = usize::try_from(fd - FIRST_FD).map_err(|_| Errno::BADF)?;
        let f = self
            .files
            .get_mut(slot)
            .and_then(Option::take)
            .ok_or(Errno::BADF)?;
        self.mounts[f.mount].fs.close(f.file)
    }

    pub fn read(&mut self, fd: i32, buf: &mut [u8]) -> Result<usize, Errno> {
        let (fs, f) = self.file(fd)?;
        fs.read(f, buf)
    }

    pub fn write(&mut self, fd: i32, data: &[u8]) -> Result<usize, Errno> {
        let (fs, f) = self.file(fd)?;
        fs.write(f, data)
    }

    pub fn seek(&mut self, fd: i32, off: i64, whence: Whence) -> Result<u32, Errno> {
        let (fs, f) = self.file(fd)?;
        fs.seek(f, off, whence)
    }

    pub fn fstat(&mut self, fd: i32) -> Result<Metadata, Errno> {
        let (fs, f) = self.file(fd)?;
        Ok(Metadata {
            kind: Kind::File,
            size: fs.size(f)?,
        })
    }

    pub fn fsync(&mut self, fd: i32) -> Result<(), Errno> {
        let (fs, f) = self.file(fd)?;
        fs.sync(f)
    }

    pub fn ftruncate(&mut self, fd: i32, size: u32) -> Result<(), Errno> {
        let (fs, f) = self.file(fd)?;
        fs.truncate(f, size)
    }

    /// Open files (for leak checks).
    pub fn open_files(&self) -> usize {
        self.files.iter().filter(|f| f.is_some()).count()
    }

    /// Close every descriptor and directory handle (a script ended).
    pub fn close_all(&mut self) {
        for slot in 0..self.files.len() {
            if let Some(f) = self.files[slot].take() {
                let _ = self.mounts[f.mount].fs.close(f.file);
            }
        }
        self.dirs.clear();
    }

    // ---- directory handles (opendir/readdir) --------------------------------

    pub fn opendir(&mut self, path: &str) -> Result<i32, Errno> {
        if self.stat(path)?.kind != Kind::Dir {
            return Err(Errno::NOTDIR);
        }
        let entries = self.read_dir(path)?;
        let d = OpenDir { entries, pos: 0 };
        let slot = match self.dirs.iter().position(Option::is_none) {
            Some(s) => {
                self.dirs[s] = Some(d);
                s
            }
            None if self.dirs.len() < MAX_FILES => {
                self.dirs.push(Some(d));
                self.dirs.len() - 1
            }
            None => return Err(Errno::MFILE),
        };
        Ok(slot as i32 + 1)
    }

    pub fn readdir(&mut self, handle: i32) -> Result<Option<&DirEntry>, Errno> {
        let slot = usize::try_from(handle - 1).map_err(|_| Errno::BADF)?;
        let d = self
            .dirs
            .get_mut(slot)
            .and_then(Option::as_mut)
            .ok_or(Errno::BADF)?;
        let e = d.entries.get(d.pos);
        if e.is_some() {
            d.pos += 1;
        }
        Ok(e)
    }

    pub fn closedir(&mut self, handle: i32) -> Result<(), Errno> {
        let slot = usize::try_from(handle - 1).map_err(|_| Errno::BADF)?;
        self.dirs
            .get_mut(slot)
            .and_then(Option::take)
            .map(|_| ())
            .ok_or(Errno::BADF)
    }

    // ---- whole files --------------------------------------------------------

    pub fn read_file(&mut self, path: &str) -> Result<Vec<u8>, Errno> {
        let fd = self.open(path, OpenFlags::READ)?;
        let mut out = Vec::new();
        let mut buf = [0u8; 512];
        let r = loop {
            match self.read(fd, &mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e) => break Err(e),
            }
        };
        let _ = self.close(fd);
        r.map(|()| out)
    }

    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), Errno> {
        let fd = self.open(path, OpenFlags::WRITE)?;
        let r = self.write(fd, data).and_then(|n| {
            if n == data.len() {
                Ok(())
            } else {
                Err(Errno::NOSPC)
            }
        });
        let c = self.close(fd);
        r.and(c)
    }

    /// `mkdir -p`.
    pub fn create_dir_all(&mut self, path: &str) -> Result<(), Errno> {
        let parts = components(path)?;
        let mut cur = String::new();
        for p in parts {
            cur.push('/');
            cur.push_str(p);
            match self.stat(&cur) {
                Ok(m) if m.kind == Kind::Dir => {}
                Ok(_) => return Err(Errno::NOTDIR),
                Err(Errno::NOENT) => self.mkdir(&cur)?,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn remove_tree(fs: &mut Fs, path: &str) -> Result<(), Errno> {
    if fs.stat(path)?.kind == Kind::Dir {
        for e in fs.read_dir(path)? {
            let mut child = String::from(path.trim_end_matches('/'));
            child.push('/');
            child.push_str(&e.name);
            remove_tree(fs, &child)?;
        }
    }
    fs.remove(path)
}
