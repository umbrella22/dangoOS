//! Native UI command value decoding at the existing RPC boundary.
use alloc::{ffi::CString, string::String};
use rs_lvgl::sys as s;
use serde_json::Value;
pub(super) fn text(v: &Value) -> Result<&str, String> {
    v.as_str().ok_or_else(|| "string required".into())
}
pub(super) fn uint(v: &Value) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| "positive object identifier required".into())
}
pub(super) fn coord(v: &Value) -> Result<i32, String> {
    let n = v.as_i64().ok_or("integer coordinate required")?;
    i32::try_from(n).map_err(|_| "coordinate out of range".into())
}
pub(super) fn optional_coord(v: &Value, default: i32) -> Result<i32, String> {
    if v.is_null() {
        Ok(default)
    } else {
        coord(v)
    }
}
pub(super) fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "text contains NUL".into())
}
pub(super) fn color(v: &Value) -> Result<u32, String> {
    let n = if let Some(v) = v.as_u64() {
        v
    } else {
        u64::from_str_radix(
            text(v)?
                .strip_prefix('#')
                .ok_or("hex color must start with #")?,
            16,
        )
        .map_err(|_| "invalid color")?
    };
    if n > 0xffffff {
        Err("color outside RGB888".into())
    } else {
        Ok(n as u32)
    }
}
pub(super) fn opacity(v: &Value) -> Result<u8, String> {
    u8::try_from(v.as_u64().ok_or("integer opacity required")?)
        .map_err(|_| "opacity outside 0..255".into())
}
pub(super) fn align(v: &str) -> Result<s::lv_align_t, String> {
    Ok(match v {
        "default" | "top_left" => s::lv_align_t_LV_ALIGN_TOP_LEFT,
        "top_mid" | "top" => s::lv_align_t_LV_ALIGN_TOP_MID,
        "top_right" => s::lv_align_t_LV_ALIGN_TOP_RIGHT,
        "bottom_left" => s::lv_align_t_LV_ALIGN_BOTTOM_LEFT,
        "bottom_mid" | "bottom" => s::lv_align_t_LV_ALIGN_BOTTOM_MID,
        "bottom_right" => s::lv_align_t_LV_ALIGN_BOTTOM_RIGHT,
        "left_mid" | "left" => s::lv_align_t_LV_ALIGN_LEFT_MID,
        "right_mid" | "right" => s::lv_align_t_LV_ALIGN_RIGHT_MID,
        "center" | "centre" => s::lv_align_t_LV_ALIGN_CENTER,
        _ => return Err("unsupported alignment".into()),
    })
}
pub(super) fn flex(v: &str) -> Result<s::lv_flex_align_t, String> {
    Ok(match v {
        "start" => s::lv_flex_align_t_LV_FLEX_ALIGN_START,
        "end" => s::lv_flex_align_t_LV_FLEX_ALIGN_END,
        "center" => s::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
        "space_between" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_BETWEEN,
        "space_around" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_AROUND,
        "space_evenly" => s::lv_flex_align_t_LV_FLEX_ALIGN_SPACE_EVENLY,
        _ => return Err("unsupported flex alignment".into()),
    })
}
