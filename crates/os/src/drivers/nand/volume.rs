//! Persistent NAND volume. Chip/transport, Dhara and LittleFS have independent
//! interfaces; replacing the SPI controller does not change the disk format.
use super::{with, Error, BLOCKS};
use alloc::{boxed::Box, format, string::String};
use core::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use fs::{
    global,
    nand::{self, Ftl},
    Errno, Fs,
};
use log::{info, warn};

static MOUNTED: AtomicBool = AtomicBool::new(false);
// The console starts before storage initialization. Explicit formatting must
// wait until boot has finished its mount attempt, including a failed attempt.
static BOOT_PROBE_DONE: AtomicBool = AtomicBool::new(false);
static CORRECTED: AtomicU32 = AtomicU32::new(0);
static UNCORRECTABLE: AtomicU32 = AtomicU32::new(0);
static PROGRAMS: AtomicU32 = AtomicU32::new(0);
static ERASES: AtomicU32 = AtomicU32::new(0);
static RETIRED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Default)]
pub struct Metrics {
    pub reads: u32,
    pub read_bytes: u32,
    pub read_us: u64,
    pub read_max_us: u64,
    pub programs: u32,
    pub program_us: u64,
    pub free_checks: u32,
    pub free_us: u64,
    pub erases: u32,
    pub erase_us: u64,
}
static METRICS: critical_section::Mutex<Cell<Metrics>> =
    critical_section::Mutex::new(Cell::new(Metrics {
        reads: 0,
        read_bytes: 0,
        read_us: 0,
        read_max_us: 0,
        programs: 0,
        program_us: 0,
        free_checks: 0,
        free_us: 0,
        erases: 0,
        erase_us: 0,
    }));
pub fn metrics() -> Metrics {
    critical_section::with(|cs| METRICS.borrow(cs).get())
}
fn record(f: impl FnOnce(&mut Metrics)) {
    // Only counters are protected. No bus or filesystem work runs in this section.
    critical_section::with(|cs| {
        let mut metrics = METRICS.borrow(cs).get();
        f(&mut metrics);
        METRICS.borrow(cs).set(metrics);
    });
}
fn now_us() -> u64 {
    embassy_time::Instant::now().as_micros()
}

pub fn mounted() -> bool {
    MOUNTED.load(Ordering::Acquire)
}

fn classify(error: Error) -> nand::Error {
    match error {
        Error::ProgramFailed | Error::EraseFailed | Error::BadBlock => nand::Error::BadBlock,
        Error::Uncorrectable => nand::Error::Uncorrectable,
        _ => nand::Error::Io,
    }
}

struct Device;
impl nand::Nand for Device {
    fn blocks(&self) -> u32 {
        BLOCKS
    }
    fn is_bad(&mut self, block: u32) -> Result<bool, nand::Error> {
        with(|n| n.is_bad(block)).map_err(classify)
    }
    fn mark_bad(&mut self, block: u32) -> Result<(), nand::Error> {
        // A failed persistent marker is fatal, never a recoverable block error.
        with(|n| n.mark_bad(block)).map_err(|_| nand::Error::Io)?;
        RETIRED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn is_free(&mut self, page: u32) -> Result<bool, nand::Error> {
        let start = now_us();
        let result = with(|n| n.is_free(page)).map_err(classify);
        let elapsed = now_us() - start;
        record(|m| {
            m.free_checks += 1;
            m.free_us += elapsed;
        });
        result
    }
    fn erase(&mut self, block: u32) -> Result<(), nand::Error> {
        let start = now_us();
        let result = with(|n| n.erase_block(block)).map_err(classify);
        let elapsed = now_us() - start;
        record(|m| {
            m.erases += 1;
            m.erase_us += elapsed;
        });
        result?;
        ERASES.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn program(&mut self, page: u32, data: &[u8]) -> Result<(), nand::Error> {
        let start = now_us();
        let result = with(|n| n.program_page(page, data)).map_err(classify);
        let elapsed = now_us() - start;
        record(|m| {
            m.programs += 1;
            m.program_us += elapsed;
        });
        result?;
        PROGRAMS.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn read(&mut self, page: u32, offset: usize, data: &mut [u8]) -> Result<(), nand::Error> {
        let start = now_us();
        let result = with(|n| n.read_data(page, offset, data));
        let elapsed = now_us() - start;
        record(|m| {
            m.reads += 1;
            m.read_bytes += data.len() as u32;
            m.read_us += elapsed;
            m.read_max_us = m.read_max_us.max(elapsed);
        });
        match result {
            Ok(status) => {
                if status.corrected_max != 0 {
                    CORRECTED.fetch_add(1, Ordering::Relaxed);
                }
                Ok(())
            }
            Err(error) => {
                warn!("nand: read fault page={page} off={offset} len={} error={} elapsed_us={elapsed}", data.len(), error.name());
                if error == Error::Uncorrectable {
                    UNCORRECTABLE.fetch_add(1, Ordering::Relaxed);
                }
                Err(classify(error))
            }
        }
    }
}

fn install(ftl: Ftl, format: bool) -> Result<(), Errno> {
    let geometry = ftl.geometry();
    let fs = if format {
        Fs::format(Box::new(ftl), geometry)?
    } else {
        Fs::mount(Box::new(ftl), geometry, false)?
    };
    global::with(|v| {
        if v.mount_names().any(|name| matches!(name, "nand" | "data")) {
            return Err(Errno::BUSY);
        }
        v.mount("data", fs);
        v.alias("nand", "data")
    })?;
    MOUNTED.store(true, Ordering::Release);
    info!(
        "fs: /nand rw {} KiB (Dhara, {}x{} logical blocks)",
        geometry.block_count * geometry.block_size / 1024,
        geometry.block_count,
        geometry.block_size
    );
    Ok(())
}

/// Normal boot only scans/resumes existing media. Failure never formats it.
pub fn mount() {
    let result = with(|n| n.prepare_writes())
        .map_err(|_| Errno::IO)
        .and_then(|()| Ftl::resume(Box::new(Device)))
        .and_then(|ftl| install(ftl, false));
    if let Err(error) = result {
        warn!("fs: /nand not mounted: {error}; explicit nand-format required for a new volume");
    }
    BOOT_PROBE_DONE.store(true, Ordering::Release);
}

/// Explicit initialization, accepted only while the volume is unmounted.
/// Scan first, preserve every factory/runtime marker, and yield between erases.
pub async fn format_volume() -> Result<(), Errno> {
    if !BOOT_PROBE_DONE.load(Ordering::Acquire) || mounted() {
        return Err(Errno::BUSY);
    }
    with(|n| n.prepare_writes()).map_err(|_| Errno::IO)?;
    for block in 0..BLOCKS {
        if !with(|n| n.is_bad(block)).map_err(|_| Errno::IO)? {
            match with(|n| n.erase_block(block)) {
                Ok(()) => {
                    ERASES.fetch_add(1, Ordering::Relaxed);
                }
                Err(Error::EraseFailed) => {
                    with(|n| n.mark_bad(block)).map_err(|_| Errno::IO)?;
                    RETIRED.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => return Err(Errno::IO),
            }
        }
        if block % 128 == 0 {
            info!("nand: format block {block}/{BLOCKS}");
        }
        embassy_futures::yield_now().await;
    }
    install(Ftl::on_erased_media(Box::new(Device))?, true)
}

pub fn status() -> String {
    let bad = with(|n| Ok(n.bad_count()))
        .ok()
        .flatten()
        .map(|n| format!("{n}"))
        .unwrap_or_else(|| "unknown".into());
    let space = global::with(|v| v.space("/nand"));
    let (blocks, free, size) = space
        .map(|s| (s.blocks, s.free, s.block_size))
        .unwrap_or((0, 0, 0));
    format!("@ok nand-status mounted={} ready={} bad={bad} blocks={blocks} free={free} size={size} prog={} erase={} retired={} corrected={} uncorrectable={}",
        mounted() as u8, BOOT_PROBE_DONE.load(Ordering::Acquire) as u8,
        PROGRAMS.load(Ordering::Relaxed), ERASES.load(Ordering::Relaxed),
        RETIRED.load(Ordering::Relaxed), CORRECTED.load(Ordering::Relaxed), UNCORRECTABLE.load(Ordering::Relaxed))
}

/// Bounded owned fixture for write/read/rename, reboot persistence and cleanup.
/// Payload spans multiple NAND pages and logical erase blocks.
pub fn test(phase: &str) -> Result<String, Errno> {
    const ROOT: &str = "/nand/.driver-test";
    let data: alloc::vec::Vec<u8> = (0..24_173u32)
        .map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8)
        .collect();
    global::with(|v| {
        match phase {
            "write" => {
                // Never overwrite a previous run before its persistence check.
                v.mkdir(ROOT)?;
                v.write_file("/nand/.driver-test/payload.tmp", &data)?;
                v.rename(
                    "/nand/.driver-test/payload.tmp",
                    "/nand/.driver-test/payload.bin",
                )?;
                if v.read_file("/nand/.driver-test/payload.bin")? != data {
                    return Err(Errno::CORRUPT);
                }
            }
            "verify" => {
                if v.read_file("/nand/.driver-test/payload.bin")? != data {
                    return Err(Errno::CORRUPT);
                }
                if v.stat("/nand/.driver-test/payload.tmp") != Err(Errno::NOENT) {
                    return Err(Errno::CORRUPT);
                }
            }
            "cleanup" => {
                v.remove_tree(ROOT)?;
            }
            _ => return Err(Errno::INVAL),
        }
        if phase != "cleanup" && v.read_file("/data/.driver-test/payload.bin")? != data {
            return Err(Errno::CORRUPT);
        }
        Ok(format!(
            "@ok nand-test {phase} bytes={} crc={:08x}",
            data.len(),
            os::hub::push::crc32(&data)
        ))
    })
}
