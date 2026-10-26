use std::sync::{Arc, Mutex};

use crate::global;
use crate::lfs::{BlockDevice, Fs, Geometry, Kind, OpenFlags, RamDevice, Whence};
use crate::vfs::{components, Vfs};
use crate::Errno;

/// A RAM device that outlives the file system (for remount tests).
#[derive(Clone)]
struct Shared(Arc<Mutex<RamDevice>>);

impl Shared {
    fn new(blocks: u32) -> Self {
        Self(Arc::new(Mutex::new(RamDevice::new(4096, blocks))))
    }
}

impl BlockDevice for Shared {
    fn read(&mut self, b: u32, o: u32, buf: &mut [u8]) -> Result<(), Errno> {
        self.0.lock().unwrap().read(b, o, buf)
    }
    fn prog(&mut self, b: u32, o: u32, d: &[u8]) -> Result<(), Errno> {
        self.0.lock().unwrap().prog(b, o, d)
    }
    fn erase(&mut self, b: u32) -> Result<(), Errno> {
        self.0.lock().unwrap().erase(b)
    }
}

fn fresh(blocks: u32) -> (Shared, Fs) {
    let dev = Shared::new(blocks);
    let (fs, formatted) =
        Fs::mount_or_format(Box::new(dev.clone()), Geometry::flash(blocks)).unwrap();
    assert!(formatted);
    (dev, fs)
}

#[test]
fn format_write_read_and_remount() {
    let (dev, mut fs) = fresh(64);
    fs.mkdir("/apps").unwrap();
    let mut f = fs.open("/apps/hello.txt", OpenFlags::WRITE).unwrap();
    assert_eq!(fs.write(&mut f, b"hello littlefs").unwrap(), 14);
    fs.close(f).unwrap();
    drop(fs);

    let mut fs = Fs::mount(Box::new(dev), Geometry::flash(64), false).unwrap();
    let mut f = fs.open("/apps/hello.txt", OpenFlags::READ).unwrap();
    let mut buf = [0u8; 32];
    let n = fs.read(&mut f, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"hello littlefs");
    assert_eq!(fs.seek(&mut f, 6, Whence::Set).unwrap(), 6);
    let n = fs.read(&mut f, &mut buf).unwrap();
    assert_eq!(&buf[..n], b"littlefs");
    fs.close(f).unwrap();
    let list = fs.read_dir("/apps").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "hello.txt");
    assert_eq!(list[0].meta.size, 14);
}

#[test]
fn mounting_blank_flash_fails_and_mount_or_format_formats_once() {
    let dev = Shared::new(32);
    assert!(Fs::mount(Box::new(dev.clone()), Geometry::flash(32), false).is_err());
    let (_, formatted) = Fs::mount_or_format(Box::new(dev.clone()), Geometry::flash(32)).unwrap();
    assert!(formatted);
    let (_, formatted) = Fs::mount_or_format(Box::new(dev), Geometry::flash(32)).unwrap();
    assert!(!formatted);
}

#[test]
fn read_only_mounts_refuse_changes() {
    let (dev, fs) = fresh(32);
    drop(fs);
    let mut fs = Fs::mount(Box::new(dev), Geometry::flash(32), true).unwrap();
    assert_eq!(fs.mkdir("/x").err(), Some(Errno::ROFS));
    assert_eq!(fs.open("/f", OpenFlags::WRITE).err(), Some(Errno::ROFS));
    assert_eq!(fs.remove("/f").err(), Some(Errno::ROFS));
    assert_eq!(
        fs.open("/missing", OpenFlags::READ).err(),
        Some(Errno::NOENT)
    );
}

#[test]
fn paths_normalise() {
    assert_eq!(components("/a//b/./c/../d/").unwrap(), vec!["a", "b", "d"]);
    assert_eq!(components("/").unwrap(), Vec::<&str>::new());
    assert_eq!(components("/..").unwrap(), Vec::<&str>::new());
    assert!(components("rel").is_err());
}

fn two_mounts() -> Vfs {
    let (_, sys) = fresh(32);
    let (_, data) = fresh(32);
    let mut v = Vfs::new();
    v.mount("system", sys);
    v.mount("data", data);
    v
}

#[test]
fn vfs_routes_by_mount_and_lists_the_root() {
    let mut v = two_mounts();
    v.create_dir_all("/data/apps/demo/scripts").unwrap();
    v.write_file("/data/apps/demo/scripts/main.lua", b"print(1)")
        .unwrap();
    assert_eq!(
        v.read_file("/data/apps/demo/scripts/main.lua").unwrap(),
        b"print(1)"
    );
    assert_eq!(v.stat("/data/apps/demo").unwrap().kind, Kind::Dir);
    assert_eq!(v.stat("/system/apps").err(), Some(Errno::NOENT));
    assert_eq!(v.stat("/nope").err(), Some(Errno::NOENT));
    let root: Vec<_> = v
        .read_dir("/")
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(root, vec!["system", "data"]);
    assert_eq!(v.mkdir("/x").err(), Some(Errno::NOENT));
    assert_eq!(
        v.rename("/data/apps", "/system/apps").err(),
        Some(Errno::XDEV)
    );
    assert_eq!(
        v.read_file("/data/apps/demo/../demo/scripts/main.lua")
            .unwrap(),
        b"print(1)"
    );
}

#[test]
fn vfs_unlink_rmdir_rename_and_remove_tree() {
    let mut v = two_mounts();
    v.create_dir_all("/data/a/b").unwrap();
    v.write_file("/data/a/b/f", b"1").unwrap();
    v.write_file("/data/a/g", b"2").unwrap();
    assert_eq!(v.unlink("/data/a").err(), Some(Errno::ISDIR));
    assert_eq!(v.rmdir("/data/a/g").err(), Some(Errno::NOTDIR));
    assert_eq!(v.rmdir("/data/a").err(), Some(Errno::NOTEMPTY));
    v.rename("/data/a/g", "/data/h").unwrap();
    assert_eq!(v.read_file("/data/h").unwrap(), b"2");
    v.remove_tree("/data/a").unwrap();
    assert_eq!(v.stat("/data/a").err(), Some(Errno::NOENT));
    let s = v.space("/data").unwrap();
    assert_eq!((s.block_size, s.blocks), (4096, 32));
    assert!(s.free > 0 && s.free < 32);
}

#[test]
fn vfs_descriptors_append_seek_and_limits() {
    let mut v = two_mounts();
    let fd = v.open("/data/log", OpenFlags::WRITE).unwrap();
    assert_eq!(fd, 3);
    v.write(fd, b"abc").unwrap();
    v.close(fd).unwrap();
    let append = OpenFlags {
        write: true,
        append: true,
        ..OpenFlags::default()
    };
    let fd = v.open("/data/log", append).unwrap();
    v.write(fd, b"def").unwrap();
    assert_eq!(v.fstat(fd).unwrap().size, 6);
    v.close(fd).unwrap();
    assert_eq!(v.read_file("/data/log").unwrap(), b"abcdef");
    let fd = v.open("/data/log", OpenFlags::READ).unwrap();
    assert_eq!(v.seek(fd, -2, Whence::End).unwrap(), 4);
    let mut b = [0u8; 4];
    assert_eq!(v.read(fd, &mut b).unwrap(), 2);
    assert_eq!(&b[..2], b"ef");
    assert_eq!(v.close(fd), Ok(()));
    assert_eq!(v.close(fd).err(), Some(Errno::BADF));
    assert_eq!(v.open("/data", OpenFlags::READ).err(), Some(Errno::ISDIR));

    let fds: Vec<_> = (0..crate::vfs::MAX_FILES)
        .map(|_| v.open("/data/log", OpenFlags::READ).unwrap())
        .collect();
    assert_eq!(
        v.open("/data/log", OpenFlags::READ).err(),
        Some(Errno::MFILE)
    );
    assert_eq!(v.open_files(), fds.len());
    v.close_all();
    assert_eq!(v.open_files(), 0);
}

#[test]
fn vfs_dir_handles() {
    let mut v = two_mounts();
    v.write_file("/data/one", b"").unwrap();
    v.mkdir("/data/two").unwrap();
    let h = v.opendir("/data").unwrap();
    let mut names = Vec::new();
    while let Some(e) = v.readdir(h).unwrap() {
        names.push((e.name.clone(), e.meta.kind));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        names,
        vec![("one".into(), Kind::File), ("two".into(), Kind::Dir)]
    );
    v.closedir(h).unwrap();
    assert_eq!(v.closedir(h).err(), Some(Errno::BADF));
    assert_eq!(v.opendir("/data/one").err(), Some(Errno::NOTDIR));
}

#[test]
fn c_entry_points_reach_the_installed_vfs() {
    use std::ffi::CString;
    global::install(two_mounts());
    let p = CString::new("/data/c.txt").unwrap();
    unsafe {
        let fd = global::mosaico_vfs_open(p.as_ptr(), global::flags::WRITE | global::flags::CREATE);
        assert_eq!(fd, 3);
        assert_eq!(global::mosaico_vfs_write(fd, b"xyz".as_ptr(), 3), 3);
        assert_eq!(global::mosaico_vfs_close(fd), 0);
        let (mut kind, mut size) = (0, 0);
        assert_eq!(
            global::mosaico_vfs_stat(p.as_ptr(), &mut kind, &mut size),
            0
        );
        assert_eq!((kind, size), (1, 3));
        let missing = CString::new("/data/none").unwrap();
        assert_eq!(
            global::mosaico_vfs_stat(missing.as_ptr(), &mut kind, &mut size),
            -2
        );
        let ro = CString::new("/").unwrap();
        assert_eq!(global::mosaico_vfs_mkdir(ro.as_ptr()), -Errno::ROFS.0);
    }
    // Re-entry is refused instead of aliasing.
    let nested = global::with(|_| Ok(global::with(|_| Ok(()))));
    assert_eq!(nested, Ok(Err(Errno::BUSY)));
}
