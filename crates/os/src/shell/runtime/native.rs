//! Native Lua RPC execution on the sole LVGL task.

#[inline(always)]
pub(super) fn execute_lua_native(
    service: &mut crate::apps::lua::native_ui::LuaLvglService,
    command: lua_engine::ui::UiCommand,
) -> Result<lua_engine::ui::UiReply, alloc::string::String> {
    let result = service.execute(command);

    result
}
