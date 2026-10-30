//! Fixed owned file benchmark. Each 4 KiB VFS call finishes before yielding.
//! Device time covers storage calls; wall time additionally includes scheduling.
use alloc::{format, string::String, vec::Vec};
use embassy_time::Instant;
use fs::{global, Errno, OpenFlags};

const ROOT: &str = "/nand/.nand-bench";
const TEMP: &str = "/nand/.nand-bench/payload.tmp";
const FILE: &str = "/nand/.nand-bench/payload.bin";
const CHUNK: usize = 4096;
const CHUNKS: usize = 64;
const BYTES: usize = CHUNK * CHUNKS;

fn payload(offset: usize, bytes: &mut [u8]) {
    for (i, byte) in bytes.iter_mut().enumerate() {
        let p = offset + i;
        *byte = (p ^ (p >> 7) ^ (p >> 15) ^ 0xa7) as u8;
    }
}
fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    crc
}

pub async fn run(phase: &str) -> Result<String, Errno> {
    if !super::volume::mounted() {
        return Err(Errno::NOENT);
    }
    if phase == "cleanup" {
        global::with(|v| v.remove_tree(ROOT))?;
        return Ok("@ok nand-bench cleanup".into());
    }
    let write = match phase {
        "write" => true,
        "read" => false,
        _ => return Err(Errno::INVAL),
    };
    let wall = Instant::now();
    let before = super::volume::metrics();
    let mut times = Vec::with_capacity(CHUNKS + 4);
    let mut buffer = alloc::vec![0; CHUNK];
    let opened = Instant::now();
    let fd = global::with(|v| {
        if write {
            v.mkdir(ROOT)?; // Preserve any earlier fixture, including an interrupted write.
            v.open(
                TEMP,
                OpenFlags {
                    exclusive: true,
                    ..OpenFlags::WRITE
                },
            )
        } else {
            if v.stat(FILE)?.size as usize != BYTES || v.stat(TEMP) != Err(Errno::NOENT) {
                return Err(Errno::CORRUPT);
            }
            v.open(FILE, OpenFlags::READ)
        }
    })?;
    times.push(opened.elapsed().as_micros());
    let mut crc = !0u32;
    let operation = async {
        for chunk in 0..CHUNKS {
            if write {
                payload(chunk * CHUNK, &mut buffer);
            }
            let start = Instant::now();
            let n = global::with(|v| {
                if write {
                    v.write(fd, &buffer)
                } else {
                    v.read(fd, &mut buffer)
                }
            })?;
            times.push(start.elapsed().as_micros());
            if n != CHUNK {
                return Err(Errno::IO);
            }
            if !write {
                for (i, &byte) in buffer.iter().enumerate() {
                    let p = chunk * CHUNK + i;
                    if byte != (p ^ (p >> 7) ^ (p >> 15) ^ 0xa7) as u8 {
                        return Err(Errno::CORRUPT);
                    }
                }
            }
            crc = crc32_update(crc, &buffer);
            embassy_futures::yield_now().await;
        }
        if write {
            let start = Instant::now();
            global::with(|v| v.fsync(fd))?;
            times.push(start.elapsed().as_micros());
        }
        Ok(())
    }
    .await;
    let start = Instant::now();
    let closed = global::with(|v| v.close(fd));
    times.push(start.elapsed().as_micros());
    operation?;
    closed?;
    if write {
        let start = Instant::now();
        global::with(|v| v.rename(TEMP, FILE))?;
        times.push(start.elapsed().as_micros());
    }
    let after = super::volume::metrics();
    let wall_us = wall.elapsed().as_micros();
    let io_us: u64 = times.iter().sum();
    times.sort_unstable();
    // The console bounds each physical line to 158 text bytes. Emit counters
    // before the final ACK so a host can collect all of them in one request.
    Ok(format!("@nand-bench reads={} bytes={} us={}\n\
        @nand-bench programs={} us={} free={} free_us={}\n\
        @nand-bench erases={} us={}\n\
        @ok nand-bench {phase} bytes={BYTES} crc={:08x} wall_us={wall_us} io_us={io_us} max_us={} p95_us={} calls={}",
        after.reads - before.reads, after.read_bytes - before.read_bytes, after.read_us - before.read_us,
        after.programs - before.programs, after.program_us - before.program_us,
        after.free_checks - before.free_checks, after.free_us - before.free_us,
        after.erases - before.erases, after.erase_us - before.erase_us,
        !crc, times.last().unwrap(), times[(times.len() * 95).div_ceil(100) - 1], times.len()))
}
