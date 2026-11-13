//! Engine-independent device snapshots and output routing.
use app_host::esp_err;
use core::alloc::Layout;
pub fn service_snapshot(controls_blocked: bool) -> app_host::ServiceSnapshot {
    use crate::services::{audio, camera, radio};
    let a = audio::task::snapshot();
    let c = camera::task::snapshot();
    app_host::ServiceSnapshot {
        audio: Some(app_host::AudioStatus {
            available: audio::task::available(),
            controls_available: audio::task::available() && !controls_blocked,
            state: a.state.name(),
            sample_rate_hz: a.rate,
            volume: a.volume,
            output_ceiling_db: audio::volume::OUTPUT_CEILING_DB,
            muted: a.muted,
            tx_frames: a.tx_frames,
            rx_frames: a.rx_frames,
            underruns: a.underruns,
            overruns: a.overruns,
            peak: a.peak,
            error: a.error,
        }),
        camera: Some(app_host::CameraStatus {
            available: crate::platform::drivers::camera::slot::present(),
            state: c.state.name(),
            sensor: c.sensor(),
            width: c.width,
            height: c.height,
            frames: c.frames,
            dropped: c.dropped,
            saved: c.saved,
            error: c.error,
        }),
        network: Some(app_host::NetworkStatus {
            wifi_state: radio::device::wifi::state().name(),
            bluetooth_state: radio::device::classic::state().name(),
            ip: radio::device::wifi::ip(),
        }),
        native_ui: Some(crate::apps::native_ui::limits()),
    }
}

pub fn audio_control(request: app_host::AudioControl, controls_blocked: bool) -> Result<(), i32> {
    if controls_blocked {
        return Err(esp_err::INVALID_STATE);
    }
    if !crate::services::audio::task::available() {
        return Err(esp_err::NOT_SUPPORTED);
    }
    match request {
        app_host::AudioControl::Volume(value) if value <= 100 => {
            crate::services::audio::task::volume(value)
        }
        app_host::AudioControl::Muted(value) => crate::services::audio::task::mute(value),
        _ => return Err(esp_err::INVALID_ARG),
    }
    Ok(())
}

pub fn hardware_snapshot() -> app_host::HardwareSnapshot {
    let mut snapshot = crate::services::input::sensors::lua_snapshot();
    snapshot.onboard = crate::services::power::button_task::snapshot();
    snapshot.led_generation = crate::services::feedback::lights::generation();
    if crate::services::feedback::haptic::available() {
        snapshot.available |= 32;
    } else {
        snapshot.available &= !32;
    }
    if crate::services::feedback::lights::available() {
        snapshot.available |= 16;
    } else {
        snapshot.available &= !16;
    }
    snapshot
}

pub fn hardware_request(request: app_host::HardwareRequest) -> Result<(), i32> {
    use crate::companion::behavior::affect::HapticPattern;
    use app_host::{HapticPreset, HardwareRequest};
    match request {
        HardwareRequest::Haptic(preset) => {
            if !crate::services::feedback::haptic::available() {
                return Err(esp_err::NOT_SUPPORTED);
            }
            if !crate::services::feedback::haptic::enabled() {
                return Err(esp_err::INVALID_STATE);
            }
            let pattern = match preset {
                HapticPreset::Panic => HapticPattern::Panic,
                HapticPreset::Angry => HapticPattern::Angry,
                HapticPreset::Surprised => HapticPattern::Surprised,
                HapticPreset::Puzzled => HapticPattern::Puzzled,
                HapticPreset::Happy => HapticPattern::Happy,
                HapticPreset::Satisfied => HapticPattern::Satisfied,
                HapticPreset::Confused => HapticPattern::Confused,
                HapticPreset::Listening => HapticPattern::Listening,
                HapticPreset::Thinking => HapticPattern::Thinking,
                HapticPreset::Error => HapticPattern::Error,
                HapticPreset::ModuleInsert => HapticPattern::ModuleInsert,
                HapticPreset::ModuleRemove => HapticPattern::ModuleRemove,
                HapticPreset::MagneticContact => HapticPattern::MagneticContact,
            };
            if crate::services::feedback::haptic::play(pattern) {
                Ok(())
            } else {
                Err(esp_err::INVALID_STATE)
            }
        }
        HardwareRequest::LedFrame(frame) => crate::services::feedback::lights::lua_frame(frame),
        HardwareRequest::LedRelease => {
            crate::services::feedback::lights::lua_release();
            Ok(())
        }
    }
}

pub fn heap_probe(caps_mask: u32, layout: Layout) -> Result<bool, i32> {
    let capability = match caps_mask {
        app_host::caps::INTERNAL => esp_alloc::MemoryCapability::Internal,
        app_host::caps::SPIRAM => esp_alloc::MemoryCapability::External,
        _ => return Err(esp_err::INVALID_ARG),
    };
    Ok(esp_alloc::HEAP.probe_caps(capability.into(), layout))
}
