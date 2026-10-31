//! Pause acknowledgement at a callback owner's safe point. A native lifecycle
//! operation waits for this acknowledgement before it can preempt a callback.
use core::sync::atomic::{AtomicU8, Ordering};

pub struct Gate(AtomicU8);

impl Gate {
    pub const fn new() -> Self {
        Self(AtomicU8::new(0))
    }

    pub fn request(&self) -> Option<Pause<'_>> {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Pause(self))
    }

    /// Only the callback owner calls this, between callbacks. While it returns
    /// true the owner must yield without entering another callback.
    pub fn checkpoint(&self) -> bool {
        match self
            .0
            .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => true,
            Err(state) => state == 2,
        }
    }
}

pub struct Pause<'a>(&'a Gate);

impl Pause<'_> {
    pub fn acknowledged(&self) -> bool {
        self.0 .0.load(Ordering::Acquire) == 2
    }
}

impl Drop for Pause<'_> {
    fn drop(&mut self) {
        self.0 .0.store(0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn native_work_waits_for_running_callback_and_blocks_the_next_one() {
        let gate = Gate::new();
        std::thread::scope(|scope| {
            let (callback_started, started) = channel();
            let (finish_callback, finish) = channel();
            let (checkpoint, checked) = channel();
            let (resume, resumed) = channel();
            let callback_gate = &gate;
            scope.spawn(move || {
                // The callback is running when the native operation requests a
                // pause; only returning to the owner loop can acknowledge it.
                callback_started.send(()).unwrap();
                finish.recv().unwrap();
                assert!(callback_gate.checkpoint());
                checkpoint.send(()).unwrap();
                resumed.recv().unwrap();
                assert!(!callback_gate.checkpoint());
            });
            started.recv().unwrap();
            let pause = gate.request().unwrap();
            assert!(!pause.acknowledged());
            finish_callback.send(()).unwrap();
            checked.recv().unwrap();
            assert!(pause.acknowledged());
            assert!(gate.request().is_none());
            drop(pause);
            resume.send(()).unwrap();
        });
    }

    #[test]
    fn cancelled_request_never_acknowledges_its_replacement() {
        let gate = Gate::new();
        let cancelled = gate.request().unwrap();
        assert!(!cancelled.acknowledged());
        drop(cancelled);
        assert!(!gate.checkpoint());
        let next = gate.request().unwrap();
        assert!(!next.acknowledged());
        assert!(gate.checkpoint());
        assert!(next.acknowledged());
        drop(next);
        assert!(!gate.checkpoint());
    }
}
