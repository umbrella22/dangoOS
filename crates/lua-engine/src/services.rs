//! Read-only service snapshots and bounded controls. Buses and DMA stay in the host.
use crate::{Call, Error, Ret, Vm};
use alloc::string::String;

pub use app_host::{
    AudioControl, AudioStatus, CameraStatus, NativeUiLimits, NetworkStatus, ServiceSnapshot,
};

pub(crate) fn register(vm: &mut Vm) -> Result<(), String> {
    vm.register("capabilities", &[("get", capabilities)])?;
    vm.register(
        "audio",
        &[
            ("status", audio),
            ("set_volume", volume),
            ("set_muted", muted),
        ],
    )?;
    vm.register("camera", &[("status", camera)])?;
    vm.register("network", &[("status", network)])
}

fn availability(c: &mut Call, name: &str, supported: bool, available: bool) {
    c.push_table(0, 2);
    c.set_bool("supported", supported);
    c.set_bool("available", available);
    c.set_top(name);
}

fn operations(c: &mut Call, name: &str, names: &[&str]) {
    c.push_table(names.len() as i32, 0);
    for (i, name) in names.iter().enumerate() {
        c.push_str(name);
        c.set_index(i as i64 + 1);
    }
    c.set_top(name);
}

fn capabilities(c: &mut Call) -> Result<Ret, Error> {
    let s = c.ctx.host.service_snapshot();
    let h = c.ctx.host.hardware_snapshot();
    c.push_table(0, 5);
    c.set_integer("api_version", 1);
    c.push_table(0, 10);
    for (name, bit) in [
        ("imu", 1),
        ("magnetometer", 2),
        ("fuel_gauge", 4),
        ("led_strip", 16),
        ("haptic", 32),
    ] {
        availability(c, name, true, h.available & bit != 0);
    }
    availability(
        c,
        "button",
        true,
        h.available & 8 != 0 || h.onboard.ready != 0,
    );
    availability(
        c,
        "audio",
        s.audio.is_some(),
        s.audio.is_some_and(|s| s.available),
    );
    availability(
        c,
        "camera",
        s.camera.is_some(),
        s.camera.is_some_and(|s| s.available),
    );
    availability(
        c,
        "network",
        s.network.is_some(),
        s.network.is_some_and(|s| s.ip.is_some()),
    );
    availability(
        c,
        "lvgl",
        s.native_ui.is_some(),
        s.native_ui.is_some() && c.ctx.ui_allowed,
    );
    c.set_top("modules");
    c.set_bool(
        "audio_controls_allowed",
        c.ctx.service_controls_allowed() && s.audio.is_some_and(|s| s.controls_available),
    );
    c.push_table(0, 3);
    if s.audio.is_some() {
        operations(c, "audio", &["status", "set_volume", "set_muted"]);
    }
    if s.camera.is_some() {
        operations(c, "camera", &["status"]);
    }
    if s.network.is_some() {
        operations(c, "network", &["status"]);
    }
    c.set_top("operations");
    c.push_table(0, 4);
    c.set_integer("lua_heap_bytes", c.ctx.limits.memory_bytes as i64);
    c.set_integer("slice_us", c.ctx.limits.slice_us as i64);
    c.set_integer("max_blocked_us", c.ctx.limits.max_blocked_us as i64);
    if let Some(ui) = s.native_ui {
        c.push_table(0, 2);
        c.set_integer("objects", ui.objects as i64);
        c.set_integer("image_bytes", ui.image_bytes as i64);
        c.set_top("native_ui");
    }
    c.set_top("limits");
    Ok(Ret::Values(1))
}

fn audio(c: &mut Call) -> Result<Ret, Error> {
    let s = c
        .ctx
        .host
        .service_snapshot()
        .audio
        .ok_or_else(|| Error::msg("audio service unsupported"))?;
    c.push_table(0, 13);
    c.set_bool("available", s.available);
    c.set_bool(
        "controls_available",
        s.controls_available && c.ctx.service_controls_allowed(),
    );
    c.set_str("state", s.state);
    c.set_integer("sample_rate_hz", s.sample_rate_hz as i64);
    c.set_integer("volume", s.volume as i64);
    c.set_integer("output_ceiling_db", s.output_ceiling_db as i64);
    c.set_bool("muted", s.muted);
    c.set_integer("tx_frames", s.tx_frames as i64);
    c.set_integer("rx_frames", s.rx_frames as i64);
    c.set_integer("underruns", s.underruns as i64);
    c.set_integer("overruns", s.overruns as i64);
    c.set_integer("peak", s.peak as i64);
    if let Some(error) = s.error {
        c.set_str("error", error);
    }
    Ok(Ret::Values(1))
}

fn camera(c: &mut Call) -> Result<Ret, Error> {
    let s = c
        .ctx
        .host
        .service_snapshot()
        .camera
        .ok_or_else(|| Error::msg("camera service unsupported"))?;
    c.push_table(0, 10);
    c.set_bool("available", s.available);
    c.set_str("state", s.state);
    c.set_str("sensor", s.sensor);
    c.set_integer("width", s.width as i64);
    c.set_integer("height", s.height as i64);
    c.set_integer("frames", s.frames as i64);
    c.set_integer("dropped", s.dropped as i64);
    c.set_integer("saved", s.saved as i64);
    if let Some(error) = s.error {
        c.set_str("error", error);
    }
    Ok(Ret::Values(1))
}

fn network(c: &mut Call) -> Result<Ret, Error> {
    let s = c
        .ctx
        .host
        .service_snapshot()
        .network
        .ok_or_else(|| Error::msg("network service unsupported"))?;
    c.push_table(0, 3);
    c.set_str("wifi_state", s.wifi_state);
    c.set_str("bluetooth_state", s.bluetooth_state);
    if let Some(ip) = s.ip {
        c.push_table(4, 0);
        for (i, octet) in ip.into_iter().enumerate() {
            c.push_integer(octet as i64);
            c.set_index(i as i64 + 1);
        }
        c.set_top("ip");
    }
    Ok(Ret::Values(1))
}

fn volume(c: &mut Call) -> Result<Ret, Error> {
    let value = c.integer_strict(1)?;
    let value = u8::try_from(value)
        .ok()
        .filter(|v| *v <= 100)
        .ok_or_else(|| Error::arg(1, "volume requires 0..100"))?;
    control(c, AudioControl::Volume(value))
}

fn muted(c: &mut Call) -> Result<Ret, Error> {
    let value = c.boolean_strict(1)?;
    control(c, AudioControl::Muted(value))
}

fn control(c: &mut Call, request: AudioControl) -> Result<Ret, Error> {
    if c.arg_count() != 1 {
        return Err(Error::msg("audio control requires one value"));
    }
    if !c.ctx.service_controls_allowed() {
        return Err(Error::msg("audio controls require a foreground app"));
    }
    c.ctx
        .host
        .audio_control(request)
        .map_err(|code| Error::msg(alloc::format!("audio host error {code}")))?;
    Ok(Ret::Values(0))
}
