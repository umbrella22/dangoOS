//! Incremental Responses verification and file-backed conversation storage.
#![no_std]
extern crate alloc;
pub mod agent;
mod cursor;
mod json_stream;
mod responses;
pub mod sse;
pub mod storage;
pub use responses::Reply;

/// A reading window, not a limit on stored reply length.
pub const WINDOW_BYTES: usize = 1024;
