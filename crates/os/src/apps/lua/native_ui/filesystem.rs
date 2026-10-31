//! Scoped read-only D: filesystem driver used by native image decoding.
use super::LuaLvglService;
use alloc::{boxed::Box, format, string::String};
use core::ptr;
use rs_lvgl::sys as s;
pub(super) struct DriverState {
    driver: s::lv_fs_drv_t,
    root: String,
}

impl LuaLvglService {
    pub(super) unsafe fn install_driver(&mut self) -> Result<(), String> {
        unsafe {
            if !s::lv_fs_get_drv(b'D' as _).is_null() {
                return Err("LVGL D: drive already owned".into());
            }
            let mut state = Box::new(DriverState {
                driver: s::lv_fs_drv_t::default(),
                root: self.root.clone(),
            });
            s::lv_fs_drv_init(&mut state.driver);
            state.driver.letter = b'D' as _;
            state.driver.user_data = (&mut *state as *mut DriverState).cast();
            state.driver.open_cb = Some(fs_open);
            state.driver.close_cb = Some(fs_close);
            state.driver.read_cb = Some(fs_read);
            state.driver.seek_cb = Some(fs_seek);
            state.driver.tell_cb = Some(fs_tell);
            s::lv_fs_drv_register(&mut state.driver);
            self.driver = Some(state);
            Ok(())
        }
    }
}

unsafe extern "C" fn fs_open(
    drv: *mut s::lv_fs_drv_t,
    path: *const core::ffi::c_char,
    mode: s::lv_fs_mode_t,
) -> *mut core::ffi::c_void {
    unsafe {
        if mode != s::lv_fs_mode_t_LV_FS_MODE_RD {
            return ptr::null_mut();
        }
        let state = &*((*drv).user_data as *const DriverState);
        let Ok(path) = core::ffi::CStr::from_ptr(path).to_str() else {
            return ptr::null_mut();
        };
        let path = path.trim_start_matches('/');
        if path.is_empty()
            || path.contains('\\')
            || path
                .split('/')
                .any(|v| v.is_empty() || v == "." || v == "..")
        {
            return ptr::null_mut();
        }
        let absolute = format!("{}/{}", state.root, path);
        match ::fs::global::with(|vfs| vfs.open(&absolute, ::fs::OpenFlags::READ)) {
            Ok(fd) => Box::into_raw(Box::new(fd)).cast(),
            Err(_) => ptr::null_mut(),
        }
    }
}
unsafe extern "C" fn fs_close(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
) -> s::lv_fs_res_t {
    unsafe {
        let fd = Box::from_raw(file.cast::<i32>());
        match ::fs::global::with(|vfs| vfs.close(*fd)) {
            Ok(()) => 0,
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_read(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    buf: *mut core::ffi::c_void,
    n: u32,
    read: *mut u32,
) -> s::lv_fs_res_t {
    unsafe {
        *read = 0;
        let fd = *file.cast::<i32>();
        let out = core::slice::from_raw_parts_mut(buf.cast::<u8>(), n as usize);
        match ::fs::global::with(|vfs| vfs.read(fd, out)) {
            Ok(n) => {
                *read = n as u32;
                0
            }
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_seek(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    pos: u32,
    whence: s::lv_fs_whence_t,
) -> s::lv_fs_res_t {
    unsafe {
        let whence = match whence {
            0 => ::fs::Whence::Set,
            1 => ::fs::Whence::Cur,
            2 => ::fs::Whence::End,
            _ => return 11,
        };
        match ::fs::global::with(|vfs| vfs.seek(*file.cast::<i32>(), pos as i64, whence)) {
            Ok(_) => 0,
            Err(_) => 2,
        }
    }
}
unsafe extern "C" fn fs_tell(
    _drv: *mut s::lv_fs_drv_t,
    file: *mut core::ffi::c_void,
    pos: *mut u32,
) -> s::lv_fs_res_t {
    unsafe {
        match ::fs::global::with(|vfs| vfs.seek(*file.cast::<i32>(), 0, ::fs::Whence::Cur)) {
            Ok(n) => {
                *pos = n;
                0
            }
            Err(_) => 2,
        }
    }
}
