//! Real Dhara + LittleFS traversal must leave the underlying NAND unchanged.
use fs::{
    nand::{self, Ftl, Nand},
    BlockDevice, Errno, Fs, OpenFlags,
};
use recovery::readonly::{ReadOnlyBlock, ReadOnlyNand};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Chip(Arc<Mutex<State>>);
struct State {
    pages: Vec<Vec<u8>>,
    mutations: usize,
    fail_read: bool,
}
impl Chip {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(State {
            pages: vec![vec![255; 2048]; 24 * 64],
            mutations: 0,
            fail_read: false,
        })))
    }
}
impl Nand for Chip {
    fn blocks(&self) -> u32 {
        24
    }
    fn is_bad(&mut self, _: u32) -> Result<bool, nand::Error> {
        Ok(false)
    }
    fn is_free(&mut self, p: u32) -> Result<bool, nand::Error> {
        Ok(self.0.lock().unwrap().pages[p as usize]
            .iter()
            .all(|&b| b == 255))
    }
    fn read(&mut self, p: u32, o: usize, b: &mut [u8]) -> Result<(), nand::Error> {
        let s = self.0.lock().unwrap();
        if s.fail_read {
            return Err(nand::Error::Io);
        }
        b.copy_from_slice(&s.pages[p as usize][o..o + b.len()]);
        Ok(())
    }
    fn program(&mut self, p: u32, b: &[u8]) -> Result<(), nand::Error> {
        let mut s = self.0.lock().unwrap();
        s.mutations += 1;
        s.pages[p as usize].copy_from_slice(b);
        Ok(())
    }
    fn erase(&mut self, b: u32) -> Result<(), nand::Error> {
        let mut s = self.0.lock().unwrap();
        s.mutations += 1;
        for p in b * 64..b * 64 + 64 {
            s.pages[p as usize].fill(255)
        }
        Ok(())
    }
    fn mark_bad(&mut self, _: u32) -> Result<(), nand::Error> {
        self.0.lock().unwrap().mutations += 1;
        Ok(())
    }
}
#[test]
fn mount_enumerate_extract_close_and_drop_do_not_change_nand() {
    let chip = Chip::new();
    let ftl = Ftl::on_erased_media(Box::new(chip.clone())).unwrap();
    let geometry = ftl.geometry();
    let mut fs = Fs::format(Box::new(ftl), geometry).unwrap();
    fs.mkdir("/资源").unwrap();
    let mut f = fs.open("/资源/狐狸 动画.bin", OpenFlags::WRITE).unwrap();
    let bytes: Vec<_> = (0..15000).map(|i| (i ^ (i >> 8)) as u8).collect();
    fs.write(&mut f, &bytes).unwrap();
    fs.close(f).unwrap();
    drop(fs);
    let (snapshot, mutations) = {
        let s = chip.0.lock().unwrap();
        (s.pages.clone(), s.mutations)
    };
    {
        let ftl = Ftl::resume(Box::new(ReadOnlyNand(chip.clone()))).unwrap();
        let mut fs = Fs::mount(Box::new(ReadOnlyBlock(ftl)), geometry, true).unwrap();
        assert_eq!(
            fs.read_dir_page("/资源", 0, 1).unwrap()[0].name,
            "狐狸 动画.bin"
        );
        assert!(fs.read_dir_page("/资源", 1, 1).unwrap().is_empty());
        assert_eq!(
            fs.open("/destroy", OpenFlags::WRITE).err(),
            Some(Errno::ROFS)
        );
        let mut f = fs.open("/资源/狐狸 动画.bin", OpenFlags::READ).unwrap();
        let mut got = vec![0; bytes.len()];
        assert_eq!(fs.read(&mut f, &mut got).unwrap(), bytes.len());
        fs.close(f).unwrap();
        assert_eq!(got, bytes);
    }
    let s = chip.0.lock().unwrap();
    assert_eq!(s.pages, snapshot);
    assert_eq!(s.mutations, mutations);
}
#[test]
fn blank_corrupt_and_io_failure_never_format_or_retire_blocks() {
    for fail in [false, true] {
        let chip = Chip::new();
        chip.0.lock().unwrap().fail_read = fail;
        assert!(Ftl::resume(Box::new(ReadOnlyNand(chip.clone()))).is_err());
        assert_eq!(chip.0.lock().unwrap().mutations, 0);
    }
    let chip = Chip::new();
    let mut ro = ReadOnlyNand(chip.clone());
    assert_eq!(ro.program(0, &[0; 2048]), Err(nand::Error::Io));
    assert_eq!(ro.erase(0), Err(nand::Error::Io));
    assert_eq!(ro.mark_bad(0), Err(nand::Error::Io));
    let mut block = ReadOnlyBlock(fs::RamDevice::new(4096, 4));
    assert_eq!(block.prog(0, 0, &[0; 16]), Err(Errno::ROFS));
    assert_eq!(block.erase(0), Err(Errno::ROFS));
    block.sync().unwrap();
    assert_eq!(chip.0.lock().unwrap().mutations, 0);
}
