//! Independent full-image OTA and NOR fault probe. No device access.
//! Run with a real compatible BIN, a generator-produced table, and an old BIN.
use mosaico_s31::ota::{self, Entry, Error, Flash, Phase, Updater};
use sha2::{Digest, Sha256};
use std::{env, fs};

const META: usize = 0x9000;
const SLOT: usize = 0x300000;
#[derive(Clone, Copy, Debug)]
enum Fault {
    Before,
    Partial,
    After,
}
#[derive(Clone)]
struct Nor {
    data: Vec<u8>,
    booted: u8,
    calls: usize,
    writes: usize,
    fault: Option<(usize, Fault)>,
}
impl Nor {
    fn new(table: &[u8], image: &[u8]) -> Self {
        let mut data: Vec<_> = (0..16 * 1024 * 1024)
            .map(|i| ((i * 13 + i / 4096) % 251) as u8)
            .collect();
        data[0x8000..0x8000 + table.len()].copy_from_slice(table);
        data[META..META + 8192].fill(255);
        data[0x210000..0x210000 + SLOT].fill(255);
        data[0x510000..0x510000 + SLOT].fill(255);
        data[0x210000..0x210000 + image.len()].copy_from_slice(image);
        Self {
            data,
            booted: 0,
            calls: 0,
            writes: 0,
            fault: None,
        }
    }
    fn op(&mut self) -> Option<Fault> {
        self.calls += 1;
        self.fault.filter(|(n, _)| *n == self.calls).map(|(_, f)| f)
    }
    fn writable(&self, at: usize, len: usize) {
        let target = ota::slot_offset(1 - self.booted).unwrap() as usize;
        assert!(
            (at >= META && at + len <= META + 8192) || (at >= target && at + len <= target + SLOT),
            "write touched running slot/protected bytes: {at:#x}+{len:#x}"
        );
    }
    fn stable(&self, before: &[u8]) {
        let running = ota::slot_offset(self.booted).unwrap() as usize;
        for (lo, hi) in [
            (0, META),
            (META + 8192, 0x210000),
            (0x810000, 0x1000000),
            (running, running + SLOT),
        ] {
            assert_eq!(
                self.data[lo..hi],
                before[lo..hi],
                "changed protected/running region {lo:#x}"
            );
        }
    }
    fn selected(&mut self) -> Entry {
        ota::selected(ota::read_entries(self).unwrap())
            .unwrap()
            .unwrap()
            .1
    }
}
impl Flash for Nor {
    fn read(&mut self, address: u32, out: &mut [u8]) -> ota::Result<()> {
        let at = address as usize;
        assert_eq!(at % 4, 0);
        assert_eq!(out.len() % 4, 0);
        if self.op().is_some() {
            return Err(Error::Flash);
        }
        out.copy_from_slice(&self.data[at..at + out.len()]);
        Ok(())
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> ota::Result<()> {
        let at = address as usize;
        assert_eq!(at % 4, 0);
        assert_eq!(bytes.len() % 4, 0);
        self.writable(at, bytes.len());
        let fault = self.op();
        if matches!(fault, Some(Fault::Before)) {
            return Err(Error::Flash);
        }
        let count = if matches!(fault, Some(Fault::Partial)) {
            bytes.len() / 2
        } else {
            bytes.len()
        };
        for (dst, &src) in self.data[at..at + count].iter_mut().zip(bytes) {
            assert_eq!(*dst & src, src, "write requires erase");
            *dst = src;
        }
        self.writes += 1;
        if fault.is_some() {
            Err(Error::Flash)
        } else {
            Ok(())
        }
    }
    fn erase(&mut self, address: u32) -> ota::Result<()> {
        let at = address as usize;
        assert_eq!(at % 4096, 0);
        self.writable(at, 4096);
        let fault = self.op();
        if matches!(fault, Some(Fault::Before)) {
            return Err(Error::Flash);
        }
        let count = if matches!(fault, Some(Fault::Partial)) {
            2048
        } else {
            4096
        };
        self.data[at..at + count].fill(255);
        self.writes += 1;
        if fault.is_some() {
            Err(Error::Flash)
        } else {
            Ok(())
        }
    }
}
fn crc(bytes: &[u8]) -> u32 {
    // Independent IEEE CRC, not the updater's helper.
    let mut c = !0u32;
    for byte in bytes {
        c ^= *byte as u32;
        for _ in 0..8 {
            c = (c >> 1) ^ if c & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    !c
}
fn receive(up: &mut Updater, flash: &mut Nor, image: &[u8]) {
    up.begin(image.len() as u32, Sha256::digest(image).into())
        .unwrap();
    for (i, block) in image.chunks(4096).enumerate() {
        assert_eq!(
            up.chunk(flash, (i * 4096) as u32, block, crc(block))
                .unwrap(),
            ((i * 4096) + block.len()) as u32
        );
    }
    up.start_verify().unwrap();
}
fn finish(up: &mut Updater, flash: &mut Nor) -> ota::Result<()> {
    while !up.verify_step(flash)? {}
    Ok(())
}
fn main() {
    let args: Vec<_> = env::args().collect();
    assert_eq!(
        args.len(),
        4,
        "n6_update_probe <compatible.bin> <partitions.bin> <legacy.bin>"
    );
    let image = fs::read(&args[1]).unwrap();
    let table = fs::read(&args[2]).unwrap();
    let old_image = fs::read(&args[3]).unwrap();
    let layout = ota::layout::parse(&table, 16 << 20).unwrap();
    ota::layout::verify_ab_early(&table, 16 << 20).unwrap();
    assert!(layout.ab);
    let mut base = Nor::new(&table, &image);
    let mut unconfirmed = Updater::new(&layout, 0, &mut base).unwrap();
    assert!(unconfirmed
        .begin(image.len() as u32, Sha256::digest(&image).into())
        .is_err());
    assert_eq!(base.writes, 0);
    ota::confirm(&mut base, 0).unwrap();
    assert_eq!(base.selected(), Entry { seq: 1, state: 2 });
    assert_eq!(&base.data[META + 28..META + 32], &[0x9a, 0x98, 0x43, 0x47]);
    println!("PASS blank metadata refuses begin; real IDF CRC baseline established");

    let mut flash = base.clone();
    let data_hash = Sha256::digest(&flash.data[0x810000..0x9e0000]);
    for iteration in 1..=10 {
        let before = flash.data.clone();
        let target = 1 - flash.booted;
        let mut up = Updater::new(&layout, flash.booted, &mut flash).unwrap();
        receive(&mut up, &mut flash, &image);
        finish(&mut up, &mut flash).unwrap();
        assert_eq!(up.phase, Phase::Activated);
        flash.stable(&before);
        let selected = flash.selected();
        assert_eq!((selected.slot(), selected.state), (target, 0));
        flash.booted = target;
        let writes = flash.writes;
        let mut pending = Updater::new(&layout, target, &mut flash).unwrap();
        assert!(pending
            .begin(image.len() as u32, Sha256::digest(&image).into())
            .is_err());
        assert_eq!(writes, flash.writes);
        ota::confirm(&mut flash, target).unwrap();
        assert_eq!(flash.selected().state, 2);
        assert_eq!(Sha256::digest(&flash.data[0x810000..0x9e0000]), data_hash);
        println!("PASS update {iteration}: booted={target} Valid; running slot, protected bytes and data preserved");
    }

    for scenario in [
        "incomplete",
        "crc",
        "offset",
        "transport",
        "readback",
        "legacy",
    ] {
        let mut flash = base.clone();
        let mut up = Updater::new(&layout, 0, &mut flash).unwrap();
        if scenario == "legacy" {
            receive(&mut up, &mut flash, &old_image);
            let error = finish(&mut up, &mut flash).unwrap_err();
            assert_eq!(
                error,
                Error::Image("missing A/B layout compatibility marker")
            );
        } else if scenario == "readback" {
            receive(&mut up, &mut flash, &image);
            flash.data[0x510000 + image.len() / 2] ^= 1;
            assert!(finish(&mut up, &mut flash).is_err());
        } else {
            let sha = if scenario == "transport" {
                [0; 32]
            } else {
                Sha256::digest(&image).into()
            };
            up.begin(image.len() as u32, sha).unwrap();
            if scenario == "transport" {
                for (i, block) in image.chunks(4096).enumerate() {
                    up.chunk(&mut flash, (i * 4096) as u32, block, crc(block))
                        .unwrap();
                }
                assert!(up.start_verify().is_err());
            } else if scenario == "incomplete" {
                up.chunk(&mut flash, 0, &image[..4096], crc(&image[..4096]))
                    .unwrap();
                assert!(up.start_verify().is_err());
            } else {
                assert!(up
                    .chunk(
                        &mut flash,
                        if scenario == "offset" { 4096 } else { 0 },
                        &image[..4096],
                        crc(&image[..4096]) ^ u32::from(scenario == "crc")
                    )
                    .is_err());
            }
        }
        flash.stable(&base.data);
        assert_eq!(flash.data[META..META + 8192], base.data[META..META + 8192]);
        assert_eq!(flash.selected().slot(), 0);
        println!("PASS rejection {scenario}: no activation, old Valid selection intact");
    }

    let mut matrix = 0;
    for fault in [Fault::Before, Fault::Partial, Fault::After] {
        // Every I/O boundary in alternate metadata activation: read x2, erase,
        // program, readback. Failure after persistence may still select candidate.
        for point in 1..=5 {
            let mut flash = base.clone();
            flash.fault = Some((flash.calls + point, fault));
            assert!(ota::select_slot(&mut flash, 0, 1, 0).is_err());
            flash.fault = None;
            assert_eq!(flash.data[META..META + 4096], base.data[META..META + 4096]);
            let entries = ota::read_entries(&mut flash).unwrap();
            assert_eq!(entries[0], Some(Entry { seq: 1, state: 2 }));
            assert!([0, 1].contains(&flash.selected().slot()));
            flash.stable(&base.data);
            matrix += 1;
        }
        // Every sector I/O boundary in the first chunk leaves metadata intact.
        for point in 1..=3 {
            let mut flash = base.clone();
            let mut up = Updater::new(&layout, 0, &mut flash).unwrap();
            up.begin(image.len() as u32, Sha256::digest(&image).into())
                .unwrap();
            flash.fault = Some((flash.calls + point, fault));
            assert!(up
                .chunk(&mut flash, 0, &image[..4096], crc(&image[..4096]))
                .is_err());
            flash.fault = None;
            flash.stable(&base.data);
            assert_eq!(flash.data[META..META + 8192], base.data[META..META + 8192]);
            matrix += 1;
        }
        // Confirmation rewrites candidate metadata; original Valid copy survives.
        for point in 1..=5 {
            let mut flash = base.clone();
            ota::select_slot(&mut flash, 0, 1, 0).unwrap();
            flash.booted = 1;
            flash.fault = Some((flash.calls + point, fault));
            assert!(ota::confirm(&mut flash, 1).is_err());
            flash.fault = None;
            assert_eq!(flash.data[META..META + 4096], base.data[META..META + 4096]);
            assert_eq!(
                ota::read_entries(&mut flash).unwrap()[0],
                Some(Entry { seq: 1, state: 2 })
            );
            matrix += 1;
        }
    }
    let mut mismatch = base.clone();
    assert!(Updater::new(&layout, 1, &mut mismatch).is_err());
    println!("PASS {matrix} before/partial/after I/O fault cases; baseline copy retained");
    println!("UPDATE_PROBE_COMPLETE image_sha={:x} alternating_updates=10 rejections=6 fault_cases={matrix} hardware=not-run", Sha256::digest(&image));
}
