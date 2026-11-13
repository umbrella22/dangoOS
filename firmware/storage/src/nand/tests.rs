use super::*;
use crate::{Fs, Vfs};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Media {
    pages: Vec<Vec<u8>>,
    used: Vec<bool>,
    last: Vec<Option<u32>>,
    bad: Vec<bool>,
    erases: Vec<u32>,
    mutations: usize,
    cut_at: Option<usize>,
    cut: bool,
    fail_program: bool,
    fail_marker: bool,
    transport_error: bool,
    corrupt_page: Option<u32>,
    last_read: Option<u32>,
}
#[derive(Clone)]
struct Sim(Arc<Mutex<Media>>);
impl Sim {
    fn new() -> Self {
        const BLOCKS: usize = 24;
        Self(Arc::new(Mutex::new(Media {
            pages: vec![vec![255; PAGE_SIZE]; BLOCKS * 64],
            used: vec![false; BLOCKS * 64],
            last: vec![None; BLOCKS],
            bad: vec![false; BLOCKS],
            erases: vec![0; BLOCKS],
            mutations: 0,
            cut_at: None,
            cut: false,
            fail_program: false,
            fail_marker: false,
            transport_error: false,
            corrupt_page: None,
            last_read: None,
        })))
    }
    fn from(m: Media) -> Self {
        Self(Arc::new(Mutex::new(m)))
    }
}
impl Nand for Sim {
    fn blocks(&self) -> u32 {
        self.0.lock().unwrap().bad.len() as u32
    }
    fn is_bad(&mut self, b: u32) -> Result<bool, Error> {
        let m = self.0.lock().unwrap();
        if m.cut || m.transport_error {
            return Err(Error::Io);
        }
        Ok(m.bad[b as usize])
    }
    fn mark_bad(&mut self, b: u32) -> Result<(), Error> {
        let mut m = self.0.lock().unwrap();
        if m.cut || m.fail_marker {
            return Err(Error::Io);
        }
        m.bad[b as usize] = true;
        Ok(())
    }
    fn is_free(&mut self, p: u32) -> Result<bool, Error> {
        let m = self.0.lock().unwrap();
        if m.cut || m.transport_error {
            return Err(Error::Io);
        }
        Ok(!m.used[p as usize])
    }
    fn read(&mut self, p: u32, off: usize, data: &mut [u8]) -> Result<(), Error> {
        let mut m = self.0.lock().unwrap();
        m.last_read = Some(p);
        if m.cut || m.transport_error {
            return Err(Error::Io);
        }
        if m.corrupt_page == Some(p) {
            return Err(Error::Uncorrectable);
        }
        data.copy_from_slice(&m.pages[p as usize][off..off + data.len()]);
        Ok(())
    }
    fn program(&mut self, p: u32, data: &[u8]) -> Result<(), Error> {
        let mut m = self.0.lock().unwrap();
        if m.cut || m.transport_error {
            return Err(Error::Io);
        }
        let b = p as usize / 64;
        assert!(!m.bad[b], "programmed a known bad block");
        assert!(!m.used[p as usize], "physical page was programmed twice");
        assert!(
            m.last[b].is_none_or(|last| p > last),
            "out-of-order page program"
        );
        if m.fail_program {
            m.fail_program = false;
            return Err(Error::BadBlock);
        }
        m.used[p as usize] = true;
        m.last[b] = Some(p);
        if m.cut_at == Some(m.mutations) {
            m.pages[p as usize][..PAGE_SIZE / 2].copy_from_slice(&data[..PAGE_SIZE / 2]);
            m.corrupt_page = Some(p);
            m.cut = true;
            return Err(Error::Io);
        }
        m.mutations += 1;
        m.pages[p as usize].copy_from_slice(data);
        Ok(())
    }
    fn erase(&mut self, b: u32) -> Result<(), Error> {
        let mut m = self.0.lock().unwrap();
        if m.cut || m.transport_error {
            return Err(Error::Io);
        }
        let b = b as usize;
        assert!(!m.bad[b], "erased a factory/runtime bad marker");
        let cut = m.cut_at == Some(m.mutations);
        for p in b * 64..b * 64 + if cut { 32 } else { 64 } {
            m.pages[p].fill(255);
            m.used[p] = false;
        }
        if cut {
            m.cut = true;
            return Err(Error::Io);
        }
        m.mutations += 1;
        m.last[b] = None;
        m.erases[b] += 1;
        Ok(())
    }
}
fn fresh(sim: &Sim) -> Ftl {
    Ftl::on_erased_media(Box::new(sim.clone())).unwrap()
}
fn resume(sim: &Sim) -> Ftl {
    Ftl::resume(Box::new(sim.clone())).unwrap()
}
fn get(f: &mut Ftl, block: u32) -> Vec<u8> {
    let mut buf = vec![0; PAGE_SIZE];
    f.read(block, 0, &mut buf).unwrap();
    buf
}

#[test]
fn blank_or_unrecoverable_media_does_not_auto_initialize() {
    let sim = Sim::new();
    assert!(Ftl::resume(Box::new(sim.clone())).is_err());
    assert_eq!(sim.0.lock().unwrap().mutations, 0);
}
#[test]
fn synced_mapping_trim_and_bad_blocks_survive_reopen() {
    let sim = Sim::new();
    sim.0.lock().unwrap().bad[0] = true;
    let mut f = fresh(&sim);
    f.prog(3, 0, &vec![42; PAGE_SIZE]).unwrap();
    f.sync().unwrap();
    drop(f);
    let mut f = resume(&sim);
    assert_eq!(get(&mut f, 3), vec![42; PAGE_SIZE]);
    f.erase(3).unwrap();
    f.sync().unwrap();
    drop(f);
    assert_eq!(get(&mut resume(&sim), 3), vec![255; PAGE_SIZE]);
    assert_eq!(sim.0.lock().unwrap().erases[0], 0);
}
#[test]
fn media_failure_retires_block_but_transport_failure_does_not() {
    let sim = Sim::new();
    sim.0.lock().unwrap().fail_program = true;
    let mut f = fresh(&sim);
    f.prog(0, 0, &vec![7; PAGE_SIZE]).unwrap();
    f.sync().unwrap();
    assert_eq!(sim.0.lock().unwrap().bad.iter().filter(|&&b| b).count(), 1);
    drop(f);
    let mut f = resume(&sim);
    assert_eq!(get(&mut f, 0), vec![7; PAGE_SIZE]);
    sim.0.lock().unwrap().transport_error = true;
    assert_eq!(f.read(0, 0, &mut vec![0; PAGE_SIZE]), Err(Errno::IO));
    sim.0.lock().unwrap().transport_error = false;
    assert_eq!(f.sync(), Err(Errno::IO)); // uncertain instance cannot keep committing
    assert_eq!(sim.0.lock().unwrap().bad.iter().filter(|&&b| b).count(), 1);
}
#[test]
fn failed_persistent_bad_marker_prevents_successful_commit() {
    let sim = Sim::new();
    {
        let mut m = sim.0.lock().unwrap();
        m.fail_program = true;
        m.fail_marker = true;
    }
    let mut f = fresh(&sim);
    assert_eq!(f.prog(0, 0, &vec![8; PAGE_SIZE]), Err(Errno::IO));
    assert_eq!(f.sync(), Err(Errno::IO));
}
#[test]
fn power_cuts_at_every_mutation_preserve_committed_sector() {
    let baseline = Sim::new();
    let mut f = fresh(&baseline);
    f.prog(1, 0, &vec![99; PAGE_SIZE]).unwrap();
    f.prog(0, 0, &vec![1; PAGE_SIZE]).unwrap();
    f.sync().unwrap();
    drop(f);
    let snapshot = baseline.0.lock().unwrap().clone();
    let mut cuts = 0;
    for offset in 0..40 {
        let sim = Sim::from(snapshot.clone());
        let mut f = resume(&sim);
        {
            let mut m = sim.0.lock().unwrap();
            m.cut_at = Some(m.mutations + offset);
        }
        let mut committed = 1;
        for version in 2..=18 {
            if f.prog(0, 0, &vec![version; PAGE_SIZE])
                .and_then(|()| f.sync())
                .is_err()
            {
                break;
            }
            committed = version;
        }
        drop(f);
        {
            let mut m = sim.0.lock().unwrap();
            cuts += m.cut as usize;
            m.cut = false;
            m.cut_at = None;
        }
        let mut reopened = resume(&sim);
        assert_eq!(get(&mut reopened, 1), vec![99; PAGE_SIZE]);
        let actual = get(&mut reopened, 0);
        assert!(actual == vec![committed; PAGE_SIZE] || actual == vec![committed + 1; PAGE_SIZE]);
    }
    assert_eq!(cuts, 40);
}
#[test]
fn filesystem_files_and_atomic_rename_recover_through_mapping() {
    let sim = Sim::new();
    let f = fresh(&sim);
    let g = f.geometry();
    let fs = Fs::format(Box::new(f), g).unwrap();
    let mut v = Vfs::new();
    v.mount("nand", fs);
    let data: Vec<u8> = (0..24_000).map(|i| (i ^ i >> 8) as u8).collect();
    v.create_dir_all("/nand/resources").unwrap();
    v.write_file("/nand/resources/payload.tmp", &data).unwrap();
    v.rename("/nand/resources/payload.tmp", "/nand/resources/payload.bin")
        .unwrap();
    drop(v);
    let f = resume(&sim);
    let g = f.geometry();
    let fs = Fs::mount(Box::new(f), g, false).unwrap();
    let mut v = Vfs::new();
    v.mount("nand", fs);
    assert_eq!(v.read_file("/nand/resources/payload.bin").unwrap(), data);
    assert_eq!(
        v.read_file("/nand/resources/payload.tmp"),
        Err(Errno::NOENT)
    );
}
#[test]
fn garbage_collection_wraps_without_page_reprogramming() {
    let sim = Sim::new();
    let mut f = fresh(&sim);
    for i in 0..1600u32 {
        f.prog(i % 40, 0, &vec![(i % 251) as u8; PAGE_SIZE])
            .unwrap();
        if i % 19 == 0 {
            f.sync().unwrap();
        }
    }
    f.sync().unwrap();
    drop(f);
    let mut f = resume(&sim);
    for b in 0..40 {
        assert_eq!(get(&mut f, b), vec![((1560 + b) % 251) as u8; PAGE_SIZE]);
    }
    assert!(sim.0.lock().unwrap().erases.iter().any(|&n| n > 1));
}

#[test]
fn logical_geometry_stays_fixed_when_bad_block_estimate_changes() {
    let sim = Sim::new();
    let mut f = fresh(&sim);
    let original = f.geometry().block_count;
    sim.0.lock().unwrap().fail_program = true;
    f.prog(0, 0, &vec![12; PAGE_SIZE]).unwrap();
    f.sync().unwrap();
    drop(f);
    let mut f = resume(&sim);
    assert_eq!(f.geometry().block_count, original);
    assert_eq!(get(&mut f, 0), vec![12; PAGE_SIZE]);
    assert!(original < unsafe { mfs_nand_capacity(f.raw.as_ptr()) } / 2);
}

#[test]
fn uncorrectable_data_is_reported_without_returning_partial_bytes() {
    let sim = Sim::new();
    let mut f = fresh(&sim);
    f.prog(0, 0, &vec![17; PAGE_SIZE]).unwrap();
    f.sync().unwrap();
    assert_eq!(get(&mut f, 0), vec![17; PAGE_SIZE]);
    {
        let mut m = sim.0.lock().unwrap();
        m.corrupt_page = m.last_read;
    }
    let mut out = vec![0xa5; PAGE_SIZE];
    assert_eq!(f.read(0, 0, &mut out), Err(Errno::CORRUPT));
    assert_eq!(out, vec![0xa5; PAGE_SIZE]);
    assert!(sim.0.lock().unwrap().bad.iter().all(|b| !b));
}

#[test]
fn cuts_during_garbage_collection_recover_synced_filesystem_versions() {
    let baseline = Sim::new();
    let mut f = fresh(&baseline);
    for version in 0..1100u32 {
        f.prog(version % 12, 0, &vec![(version % 251) as u8; PAGE_SIZE])
            .unwrap();
    }
    f.sync().unwrap();
    drop(f);
    let snapshot = baseline.0.lock().unwrap().clone();
    let mut cuts = 0;
    for offset in 0..72 {
        let sim = Sim::from(snapshot.clone());
        let mut f = resume(&sim);
        let mut expected: Vec<Vec<u8>> = (0..12).map(|b| get(&mut f, b)).collect();
        {
            let mut m = sim.0.lock().unwrap();
            m.cut_at = Some(m.mutations + offset);
        }
        let mut pending = None;
        for version in 1100..1220u32 {
            let block = version % 12;
            let data = vec![(version % 251) as u8; PAGE_SIZE];
            pending = Some((block as usize, data.clone()));
            if f.prog(block, 0, &data).and_then(|()| f.sync()).is_err() {
                break;
            }
            expected[block as usize] = data;
        }
        drop(f);
        {
            let mut m = sim.0.lock().unwrap();
            cuts += m.cut as usize;
            m.cut = false;
            m.cut_at = None;
        }
        let mut f = resume(&sim);
        for (block, committed) in expected.iter().enumerate() {
            let actual = get(&mut f, block as u32);
            assert!(
                actual == *committed
                    || pending
                        .as_ref()
                        .is_some_and(|(b, data)| *b == block && actual == *data)
            );
        }
    }
    assert_eq!(cuts, 72);
}

#[test]
fn metadata_cache_is_coherent_before_program_erase_and_retirement() {
    let sim = Sim::new();
    let mut c = Context {
        device: Box::new(sim.clone()),
        fault: false,
        metadata: ReadCache::new(),
    };
    let ctx = (&mut c as *mut Context).cast();
    let mut out = [0; 132];
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 64, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(out, [255; 132]);
    sim.0.lock().unwrap().last_read = None;
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 64, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(sim.0.lock().unwrap().last_read, None); // Actual cache hit.
    assert_eq!(
        unsafe { mosaico_nand_prog(ctx, 64, [23; PAGE_SIZE].as_ptr()) },
        0
    );
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 64, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(out, [23; 132]);
    assert_eq!(unsafe { mosaico_nand_erase(ctx, 1) }, 0);
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 64, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(out, [255; 132]);
    unsafe { mosaico_nand_mark_bad(ctx, 1) };
    sim.0.lock().unwrap().last_read = None;
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 64, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(sim.0.lock().unwrap().last_read, Some(64));
}

#[test]
fn metadata_cache_never_bypasses_poison_or_caches_failed_ecc_reads() {
    let sim = Sim::new();
    let mut c = Context {
        device: Box::new(sim.clone()),
        fault: false,
        metadata: ReadCache::new(),
    };
    let ctx = (&mut c as *mut Context).cast();
    let mut out = [0xa5; 132];
    sim.0.lock().unwrap().corrupt_page = Some(0);
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 0, 0, out.len(), out.as_mut_ptr()) },
        2
    );
    assert_eq!(out, [0xa5; 132]);
    sim.0.lock().unwrap().corrupt_page = None;
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 0, 0, out.len(), out.as_mut_ptr()) },
        0
    );
    assert_eq!(out, [255; 132]);
    sim.0.lock().unwrap().transport_error = true;
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 1, 0, out.len(), out.as_mut_ptr()) },
        3
    );
    sim.0.lock().unwrap().transport_error = false;
    out.fill(0xa5);
    assert_eq!(
        unsafe { mosaico_nand_read(ctx, 0, 0, out.len(), out.as_mut_ptr()) },
        3
    );
    assert_eq!(out, [0xa5; 132]);
}

#[test]
fn metadata_cache_keys_include_page_offset_and_length_and_eviction_is_lossless() {
    let mut cache = ReadCache::new();
    cache.insert(7, 16, &[42; 132]);
    assert!(cache.read(7, 16, &mut [0; 132]));
    for (page, offset, len) in [
        (8, 16, 132),
        (7, 17, 132),
        (7, 16, 131),
        (7, 16, 0),
        (7, 16, PAGE_SIZE),
    ] {
        assert!(!cache.read(page, offset, &mut vec![0; len]));
    }
    for page in 100..200 {
        cache.insert(page, 0, &[page as u8; 132]);
    }
    assert!(!cache.read(7, 16, &mut [0; 132]));
    let mut out = [0; 132];
    assert!(cache.read(199, 0, &mut out));
    assert_eq!(out, [199; 132]);
    cache.clear();
    assert!(!cache.read(199, 0, &mut out));
}
