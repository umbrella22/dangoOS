#![no_std]
//! Only the transport-neutral C boundary is exposed. Hardware, scheduling,
//! persistent storage and PCM ownership belong to the native OS adapters.
unsafe extern "C" {
    pub fn mosaico_classic_start() -> i32;
    pub fn mosaico_classic_stop() -> i32;
    pub fn mosaico_classic_receive(data: *mut u8, len: u16) -> i32;
    pub fn mosaico_classic_send_available();
    pub fn mosaico_classic_host_uninitialized() -> bool;
    pub fn mosaico_classic_volume(percent: u32);
}
