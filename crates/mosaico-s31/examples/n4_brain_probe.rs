//! Run one externally supplied candidate in a disposable host process. The
//! parent acceptance runner imposes a process timeout, covering teardown too.
use mosaico_s31::behavior::brain::Controller;
use std::{env, fs, sync::OnceLock, time::Instant};

fn clock_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: n4_brain_probe <candidate.lua> [reject|either]");
    let expectation = env::args().nth(2).unwrap_or_else(|| "reject".into());
    let source = fs::read_to_string(&path).expect("read candidate");
    let previous = "return {on_event=function()end,tick=function()fox.set_expression('Happy');led_strip.set(1,3,5,7)end}";
    let mut brain =
        Controller::new(previous, 0, &mosaico_lua::GLOBAL_ALLOCATOR, Some(clock_us)).unwrap();
    let before = brain.output().unwrap();
    let started = Instant::now();
    let mut committed = false;
    let result = brain.replace_with_commit(&source, 1, || {
        committed = true;
        Ok(())
    });
    match result {
        Err(error) => {
            assert!(!committed, "failed candidate reached persistent commit");
            assert_eq!(brain.source(), previous);
            assert_eq!(brain.output().unwrap(), before, "candidate effects escaped");
            brain.tick(2);
            assert!(brain.active(), "previous good brain was lost");
            println!(
                "REJECT {} elapsed_us={} reason={}",
                path,
                started.elapsed().as_micros(),
                error.lines().next().unwrap_or("unknown")
            );
        }
        Ok(()) => {
            assert_eq!(
                expectation, "either",
                "candidate unexpectedly accepted: {path}"
            );
            assert!(committed);
            println!(
                "ACCEPT {} elapsed_us={}",
                path,
                started.elapsed().as_micros()
            );
        }
    }
    drop(brain);
    println!("DISCARD_COMPLETE");
}
