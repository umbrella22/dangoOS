//! Selected script engine RPCs on the sole LVGL task.

use crate::apps::{native_ui::NativeUiService, runtime as script};
use alloc::string::String;
use app_host::ui::{UiCommand, UiReply};

#[inline(always)]
pub(super) fn execute_lua_native(service: &mut NativeUiService, command: UiCommand) {
    complete(service.submit(command));
}

pub(super) fn continue_lua_native(service: &mut NativeUiService) {
    complete(service.resume());
}

fn complete(step: Result<Option<UiReply>, String>) {
    match step {
        Ok(None) => {}
        Ok(Some(reply)) => script::complete_lvgl(Ok(reply)),
        Err(error) => script::complete_lvgl(Err(error)),
    }
}

pub(super) fn release_lua_native(service: &mut NativeUiService) {
    let pending = service.has_pending();
    service.release();
    if pending {
        script::complete_lvgl(Err("UI request cancelled by release".into()));
    }
}
