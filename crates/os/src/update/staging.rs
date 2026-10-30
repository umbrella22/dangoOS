//! An open staging descriptor stays alive until every queued job releases it.
use alloc::sync::Arc;

#[derive(Clone)]
pub struct File(Arc<Descriptor>);
struct Descriptor {
    fd: Option<i32>,
    defer_close: fn(i32),
}
impl File {
    /// Construct inside the open job, before returning its result to a caller.
    pub fn new(fd: i32, defer_close: fn(i32)) -> Self {
        Self(Arc::new(Descriptor {
            fd: Some(fd),
            defer_close,
        }))
    }
    /// Each job must capture a clone, not just this descriptor number.
    pub fn fd(&self) -> i32 {
        self.0.fd.unwrap()
    }
    /// Explicit close is allowed only when no queued job still owns the file.
    /// Cancellation at any queue boundary falls back to the deferred close.
    pub fn close_with<R>(self, close: impl FnOnce(i32) -> R) -> Option<R> {
        let mut descriptor = Arc::try_unwrap(self.0).ok()?;
        Some(close(descriptor.fd.take().unwrap()))
    }
}
impl Drop for Descriptor {
    fn drop(&mut self) {
        if let Some(fd) = self.fd.take() {
            (self.defer_close)(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    static CLOSED: AtomicUsize = AtomicUsize::new(0);
    fn defer(fd: i32) {
        assert_eq!(fd, 7);
        CLOSED.fetch_add(1, Ordering::SeqCst);
    }
    #[test]
    fn abandoned_open_and_queued_io_keep_exactly_one_close_owner() {
        CLOSED.store(0, Ordering::SeqCst);
        // Open completes after its caller has cancelled: dropping the result closes it.
        drop(Ok::<_, ()>(File::new(7, defer)));
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
        let file = File::new(7, defer);
        let queued = file.clone();
        let write = move || {
            assert_eq!(queued.fd(), 7);
            drop(queued);
        };
        drop(file); // Caller cancels with its write still queued.
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
        write();
        assert_eq!(CLOSED.load(Ordering::SeqCst), 2);
        let file = File::new(7, defer);
        let queued = file.clone();
        assert_eq!(
            file.close_with(|_| panic!("queued file cannot close")),
            None
        );
        assert_eq!(CLOSED.load(Ordering::SeqCst), 2);
        drop(queued);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 3);
        assert_eq!(File::new(7, defer).close_with(|fd| fd), Some(7));
        assert_eq!(
            CLOSED.load(Ordering::SeqCst),
            3,
            "explicit close does not defer twice"
        );
    }
}
