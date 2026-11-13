//! Durable device preferences and atomic save protocol.
use alloc::vec::Vec;
/// User preferences persisted by the settings service on the data volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Panel brightness, percent `MIN_BRIGHTNESS..=100`.
    pub brightness: u8,
    /// The interaction-board lights.
    pub lights: bool,
    /// The vibration motor.
    pub haptics: bool,
    pub camera_mirror: bool,
    pub jpeg_quality: u8,
    pub audio_volume: u8,
    pub audio_muted: bool,
    pub auto_update_check: bool,
    pub update_proxy_enabled: bool,
    pub update_proxy: Option<crate::services::update::HttpProxy>,
}

impl Settings {
    pub const MIN_BRIGHTNESS: u8 = 10;
    /// The boot brightness the plane task already uses.
    pub const DEFAULT_BRIGHTNESS: u8 = 100;

    pub const fn new() -> Self {
        Self {
            brightness: Self::DEFAULT_BRIGHTNESS,
            lights: true,
            haptics: true,
            camera_mirror: false,
            jpeg_quality: 85,
            audio_volume: 50,
            audio_muted: false,
            auto_update_check: true,
            update_proxy_enabled: false,
            update_proxy: None,
        }
    }

    pub fn set_brightness(&mut self, percent: i32) {
        self.brightness = percent.clamp(Self::MIN_BRIGHTNESS as i32, 100) as u8;
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

pub const PATH: &str = "/data/settings/device.json";
pub fn encode(settings: Settings) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"schema":1,"audio_volume":settings.audio_volume,"audio_muted":settings.audio_muted,"brightness":settings.brightness,"lights":settings.lights,"haptics":settings.haptics,"camera_mirror":settings.camera_mirror,"jpeg_quality":settings.jpeg_quality,"auto_update_check":settings.auto_update_check,
        "update_proxy_enabled":settings.update_proxy_enabled,
        "update_proxy":settings.update_proxy.map(crate::services::update::HttpProxy::address)})).expect("preference scalars")
}
pub fn decode(bytes: &[u8]) -> Result<Settings, &'static str> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "preferences JSON")?;
    if v["schema"].as_u64() != Some(1) {
        return Err("preferences schema");
    }
    let brightness = v["brightness"]
        .as_u64()
        .filter(|&n| (10..=100).contains(&n))
        .ok_or("preferences brightness")? as u8;
    let quality = v["jpeg_quality"]
        .as_u64()
        .filter(|&n| [75, 85, 95].contains(&n))
        .ok_or("preferences quality")? as u8;
    let proxy = match v.get("update_proxy") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(crate::services::update::HttpProxy::parse(
            value.as_str().ok_or("preferences proxy")?,
        )?),
    };
    let proxy_enabled = match v.get("update_proxy_enabled") {
        None => false,
        Some(value) => value.as_bool().ok_or("preferences proxy enabled")?,
    };
    if proxy_enabled && proxy.is_none() {
        return Err("preferences proxy missing");
    }
    Ok(Settings {
        brightness,
        lights: v["lights"].as_bool().ok_or("preferences lights")?,
        haptics: v["haptics"].as_bool().ok_or("preferences haptics")?,
        camera_mirror: v["camera_mirror"].as_bool().ok_or("preferences mirror")?,
        jpeg_quality: quality,
        audio_volume: match v.get("audio_volume") {
            None => 50,
            Some(value) => value
                .as_u64()
                .filter(|v| *v <= 100)
                .ok_or("preferences audio volume")? as u8,
        },
        audio_muted: match v.get("audio_muted") {
            None => false,
            Some(value) => value.as_bool().ok_or("preferences audio mute")?,
        },
        auto_update_check: match v.get("auto_update_check") {
            None => true,
            Some(value) => value.as_bool().ok_or("preferences update check")?,
        },
        update_proxy_enabled: proxy_enabled,
        update_proxy: proxy,
    })
}
pub fn load(vfs: &mut ::fs::Vfs) -> Result<Option<Settings>, ::fs::Errno> {
    match vfs.read_file(PATH) {
        Ok(bytes) => decode(&bytes).map(Some).map_err(|_| ::fs::Errno::INVAL),
        Err(::fs::Errno::NOENT) => Ok(None),
        Err(e) => Err(e),
    }
}
const TEMP: &str = "/data/settings/.device-pending";

enum SaveStep {
    Prepare,
    Write(i32),
    Commit,
    Done,
}

/// Drive to completion, releasing the VFS between steps so the device can
/// service input and render. Readers see the old preferences until Commit.
pub struct Save {
    bytes: Vec<u8>,
    next: SaveStep,
}
impl Save {
    pub fn new(settings: Settings) -> Self {
        Self {
            bytes: encode(settings),
            next: SaveStep::Prepare,
        }
    }

    /// Returns true after the atomic replacement. Write errors still close the
    /// open descriptor; the old preference file remains untouched.
    pub fn step(&mut self, vfs: &mut ::fs::Vfs) -> Result<bool, ::fs::Errno> {
        let next = core::mem::replace(&mut self.next, SaveStep::Done);
        self.next = match next {
            SaveStep::Prepare => {
                vfs.create_dir_all("/data/settings")?;
                SaveStep::Write(vfs.open(TEMP, ::fs::OpenFlags::WRITE)?)
            }
            SaveStep::Write(fd) => {
                let written = vfs.write(fd, &self.bytes).and_then(|n| {
                    if n == self.bytes.len() {
                        Ok(())
                    } else {
                        Err(::fs::Errno::NOSPC)
                    }
                });
                let closed = vfs.close(fd);
                written.and(closed)?;
                SaveStep::Commit
            }
            SaveStep::Commit => {
                vfs.rename(TEMP, PATH)?;
                SaveStep::Done
            }
            SaveStep::Done => SaveStep::Done,
        };
        Ok(matches!(self.next, SaveStep::Done))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    fn save(vfs: &mut ::fs::Vfs, value: Settings) -> Result<(), ::fs::Errno> {
        let mut save = Save::new(value);
        while !save.step(vfs)? {}
        Ok(())
    }
    #[test]
    fn preferences_persist_atomically_and_exclude_radio_secrets() {
        let mut vfs = ::fs::Vfs::default();
        let (volume, _) = ::fs::Fs::mount_or_format(
            alloc::boxed::Box::new(::fs::RamDevice::new(4096, 128)),
            ::fs::Geometry::flash(128),
        )
        .unwrap();
        vfs.mount("data", volume);
        let mut s = Settings::new();
        s.brightness = 44;
        s.haptics = false;
        s.camera_mirror = true;
        s.jpeg_quality = 95;
        s.audio_volume = 27;
        s.audio_muted = true;
        s.update_proxy =
            Some(crate::services::update::HttpProxy::parse("10.26.104.1:7777").unwrap());
        s.update_proxy_enabled = true;
        assert_eq!(load(&mut vfs).unwrap(), None);
        save(&mut vfs, s).unwrap();
        assert_eq!(load(&mut vfs).unwrap(), Some(s));
        let old = s;
        s.brightness = 61;
        let mut saving = Save::new(s);
        assert!(!saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(old));
        assert!(!saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(old));
        assert!(saving.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), Some(s));
        assert!(vfs.stat("/data/settings/.device-pending").is_err());
        let bytes = vfs.read_file(PATH).unwrap();
        assert!(!String::from_utf8(bytes).unwrap().contains("password"));
        let mut v = serde_json::from_slice::<serde_json::Value>(&encode(s)).unwrap();
        v["brightness"] = serde_json::json!(9);
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn update_preferences_default_old_files_and_reject_invalid_proxy() {
        let mut v = serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
        for key in [
            "auto_update_check",
            "update_proxy_enabled",
            "update_proxy",
            "audio_volume",
            "audio_muted",
        ] {
            v.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            decode(&serde_json::to_vec(&v).unwrap()).unwrap(),
            Settings::new()
        );
        v["update_proxy_enabled"] = true.into();
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        v["update_proxy"] = "10.26.104.1:7777".into();
        assert!(
            decode(&serde_json::to_vec(&v).unwrap())
                .unwrap()
                .update_proxy_enabled
        );
        for bad in [
            "http://10.26.104.1:7777",
            "0.0.0.0:7777",
            "10.26.104.1:0",
            "10.26.104.1:65536",
            "10.26.104.1:7777\n@mosaico reboot",
        ] {
            v["update_proxy"] = bad.into();
            assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        }
    }
    #[test]
    fn audio_preferences_reject_invalid_volume_and_mute_types() {
        for bad in [
            serde_json::json!(101),
            serde_json::json!(-1),
            serde_json::json!(50.5),
            serde_json::json!("50"),
            serde_json::Value::Null,
        ] {
            let mut v =
                serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
            v["audio_volume"] = bad;
            assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
        }
        let mut v = serde_json::from_slice::<serde_json::Value>(&encode(Settings::new())).unwrap();
        v["audio_muted"] = serde_json::json!(1);
        assert!(decode(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
