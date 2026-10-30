//! Transactional blob namespace for Bluedroid bonds. NVS handles stage changes
//! in memory and commit through LittleFS rename on NAND. No key material logs.
use alloc::{collections::BTreeMap, string::String, vec::Vec};
use core::{
    cell::UnsafeCell,
    ffi::{c_char, CStr},
    sync::atomic::{AtomicBool, Ordering},
};
use esp_radio_rtos_driver as rtos;
type Blobs = BTreeMap<String, Vec<u8>>;
struct Session {
    blobs: Blobs,
    writable: bool,
}
struct Store {
    next: u32,
    sessions: BTreeMap<u32, Session>,
}
struct Global {
    busy: AtomicBool,
    store: UnsafeCell<Store>,
}
unsafe impl Sync for Global {}
static STORE: Global = Global {
    busy: AtomicBool::new(false),
    store: UnsafeCell::new(Store {
        next: 1,
        sessions: BTreeMap::new(),
    }),
};
const PATH: &str = "/data/system/bluetooth.json";
fn with<T>(f: impl FnOnce(&mut Store) -> T) -> T {
    while STORE.busy.swap(true, Ordering::Acquire) {
        rtos::usleep(1000);
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            STORE.busy.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;
    f(unsafe { &mut *STORE.store.get() })
}
fn disk<T>(mut f: impl FnMut(&mut fs::vfs::Vfs) -> Result<T, fs::Errno>) -> Result<T, fs::Errno> {
    let deadline = rtos::now() + 2_000_000;
    loop {
        match fs::global::with(&mut f) {
            Err(e) if e == fs::Errno::BUSY && rtos::now() < deadline => rtos::usleep(1000),
            r => return r,
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_nvs_open(name: *const c_char, mode: i32, out: *mut u32) -> i32 {
    if name.is_null() || out.is_null() || CStr::from_ptr(name).to_bytes() != b"bt_config.conf" {
        return 0x102;
    }
    super::phase(30);
    let bytes = match disk(|v| v.read_file(PATH)) {
        Ok(b) => b,
        Err(e) if e == fs::Errno::NOENT => Vec::new(),
        Err(_) => return 0x1101,
    };
    super::phase(31);
    let blobs = if bytes.is_empty() {
        Blobs::new()
    } else {
        match serde_json::from_slice::<Blobs>(&bytes) {
            Ok(b) => b,
            Err(_) => return 0x109,
        }
    };
    super::phase(32);
    with(|s| {
        let id = s.next;
        s.next = s.next.wrapping_add(1).max(1);
        s.sessions.insert(
            id,
            Session {
                blobs,
                writable: mode == 1,
            },
        );
        out.write(id);
    });
    super::phase(33);
    0
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_nvs_close(id: u32) {
    with(|s| {
        s.sessions.remove(&id);
    });
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_nvs_get_blob(
    id: u32,
    key: *const c_char,
    data: *mut u8,
    len: *mut usize,
) -> i32 {
    if key.is_null() || len.is_null() {
        return 0x102;
    }
    let Ok(key) = CStr::from_ptr(key).to_str() else {
        return 0x102;
    };
    with(|s| {
        let Some(session) = s.sessions.get(&id) else {
            return 0x103;
        };
        let Some(blob) = session.blobs.get(key) else {
            return 0x1102;
        };
        let cap = len.read();
        len.write(blob.len());
        if data.is_null() {
            return 0;
        }
        if cap < blob.len() {
            return 0x110c;
        }
        core::ptr::copy_nonoverlapping(blob.as_ptr(), data, blob.len());
        0
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_nvs_set_blob(
    id: u32,
    key: *const c_char,
    data: *const u8,
    len: usize,
) -> i32 {
    if key.is_null() || (data.is_null() && len != 0) || len > 16 * 1024 {
        return 0x102;
    }
    let Ok(key) = CStr::from_ptr(key).to_str() else {
        return 0x102;
    };
    if key.len() > 15 {
        return 0x102;
    }
    let bytes = if len == 0 {
        Vec::new()
    } else {
        core::slice::from_raw_parts(data, len).to_vec()
    };
    with(|s| {
        let Some(session) = s.sessions.get_mut(&id) else {
            return 0x103;
        };
        if !session.writable {
            return 0x103;
        }
        session.blobs.insert(key.into(), bytes);
        0
    })
}
#[unsafe(no_mangle)]
unsafe extern "C" fn mosaico_bt_nvs_erase_key(id: u32, key: *const c_char) -> i32 {
    if key.is_null() {
        return 0x102;
    }
    let Ok(key) = CStr::from_ptr(key).to_str() else {
        return 0x102;
    };
    with(|s| {
        let Some(session) = s.sessions.get_mut(&id) else {
            return 0x103;
        };
        if !session.writable {
            return 0x103;
        }
        if session.blobs.remove(key).is_some() {
            0
        } else {
            0x1102
        }
    })
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_nvs_erase_all(id: u32) -> i32 {
    with(|s| {
        let Some(session) = s.sessions.get_mut(&id) else {
            return 0x103;
        };
        if !session.writable {
            return 0x103;
        }
        session.blobs.clear();
        0
    })
}
#[unsafe(no_mangle)]
extern "C" fn mosaico_bt_nvs_commit(id: u32) -> i32 {
    with(|s| {
        let Some(session) = s.sessions.get(&id) else {
            return 0x103;
        };
        if !session.writable {
            return 0x103;
        }
        let Ok(bytes) = serde_json::to_vec(&session.blobs) else {
            return -1;
        };
        match disk(|v| {
            v.create_dir_all("/data/system")?;
            v.write_file("/data/system/bluetooth.tmp", &bytes)?;
            v.rename("/data/system/bluetooth.tmp", PATH)
        }) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}
