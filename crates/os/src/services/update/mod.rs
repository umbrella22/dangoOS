//! Release metadata and status shared by the native UI and network updater.
use alloc::{
    format,
    string::{String, ToString},
};

pub use crate::formats::http;
pub use crate::formats::http::{HttpProxy, Url};
pub mod staging;

pub const MANIFEST_URL: &str =
    "https://github.com/umbrella22/dangoOS/releases/latest/download/manifest.json";
pub const LAYOUT: &str = "S31ONE03";
pub const MANIFEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Check,
    Install,
    Cancel,
    Status,
    InstallUrl(Url),
    SetProxy(Option<HttpProxy>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);
impl Version {
    /// The stable release channel accepts three numeric SemVer components.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let mut parts = text.split('.');
        let mut number = || {
            let value = parts.next().ok_or("release version")?;
            if value.is_empty()
                || !value.bytes().all(|b| b.is_ascii_digit())
                || (value.len() > 1 && value.starts_with('0'))
            {
                return Err("release version");
            }
            value.parse().map_err(|_| "release version")
        };
        let result = Self(number()?, number()?, number()?);
        if parts.next().is_some() {
            return Err("release version");
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub image: Url,
    pub bytes: u32,
    pub sha256: [u8; 32],
}
impl Release {
    pub fn parse(bytes: &[u8], source: &Url) -> Result<Self, &'static str> {
        if bytes.len() > MANIFEST_BYTES {
            return Err("manifest too large");
        }
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "manifest JSON")?;
        if value["schema"].as_u64() != Some(2)
            || value["project"] != "dangoOS"
            || value["chip"] != "esp32s31"
            || value["application"]["project"] != "mosaico-s31"
        {
            return Err("release hardware");
        }
        if value["layout"] != LAYOUT {
            return Err("release partition layout");
        }
        let version = value["version"].as_str().ok_or("release version")?;
        Version::parse(version)?;
        if value["tag"] != format!("v{version}") || value["application"]["version"] != version {
            return Err("release version mismatch");
        }
        let ota = &value["images"]["main"];
        let filename = format!("dangoOS-v{version}-esp32s31-main.bin");
        if ota["file"] != filename {
            return Err("release image name");
        }
        let length = ota["bytes"]
            .as_u64()
            .filter(|n| (288..=u64::from(boot_contract::MAIN_BYTES)).contains(n))
            .ok_or("release image size")? as u32;
        let hash = ota["sha256"].as_str().ok_or("release SHA256")?;
        if hash.len() != 64 {
            return Err("release SHA256");
        }
        let mut sha256 = [0; 32];
        for (out, pair) in sha256.iter_mut().zip(hash.as_bytes().chunks_exact(2)) {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                _ => Err("release SHA256"),
            };
            *out = digit(pair[0])? * 16 + digit(pair[1])?;
        }
        if value["application"]["bytes"].as_u64() != Some(u64::from(length))
            || value["application"]["sha256"].as_str() != Some(hash)
        {
            return Err("release image metadata mismatch");
        }
        let image = if source.host == "github.com" {
            Url::parse(&format!(
                "https://github.com/umbrella22/dangoOS/releases/download/v{version}/{filename}"
            ))?
        } else {
            source.sibling(&filename)?
        };
        Ok(Self {
            version: version.to_string(),
            image,
            bytes: length,
            sha256,
        })
    }
    pub fn newer_than(&self, current: &str) -> Result<bool, &'static str> {
        Ok(Version::parse(&self.version)? > Version::parse(current)?)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Verifying,
    Installing,
    Restarting,
    Failed,
    Incompatible,
    Cancelled,
}
impl Phase {
    pub const fn busy(self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading
                | Self::Verifying
                | Self::Installing
                | Self::Restarting
        )
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "等待检查",
            Self::Checking => "正在检查更新",
            Self::Current => "已是最新版本",
            Self::Available => "发现新版本",
            Self::Downloading => "正在下载",
            Self::Verifying => "正在校验",
            Self::Installing => "正在安装",
            Self::Restarting => "即将重新启动",
            Self::Failed => "更新失败",
            Self::Incompatible => "更新暂不兼容",
            Self::Cancelled => "更新已取消",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub phase: Phase,
    pub version: Option<String>,
    pub received: u32,
    pub total: u32,
    pub error: Option<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata() -> serde_json::Value {
        serde_json::json!({"schema":2,"project":"dangoOS","chip":"esp32s31","layout":LAYOUT,
          "version":"0.10.0","tag":"v0.10.0","application":{"project":"mosaico-s31","version":"0.10.0","bytes":4096,"sha256":"ab".repeat(32)},
          "images":{"main":{"file":"dangoOS-v0.10.0-esp32s31-main.bin","bytes":4096,"sha256":"ab".repeat(32)}}})
    }
    #[test]
    fn release_binds_hardware_layout_size_hash_and_stable_version() {
        let url = Url::parse(MANIFEST_URL).unwrap();
        let mut value = metadata();
        let release = Release::parse(&serde_json::to_vec(&value).unwrap(), &url).unwrap();
        assert!(release.newer_than("0.2.9").unwrap());
        assert!(!release.newer_than("1.0.0").unwrap());
        assert_eq!(release.sha256, [0xab; 32]);
        assert!(release
            .image
            .path
            .ends_with("/v0.10.0/dangoOS-v0.10.0-esp32s31-main.bin"));
        for (field, bad) in [
            ("layout", "MOSAICO_S31_AB_LAYOUT_V1"),
            ("chip", "esp32s3"),
            ("project", "other"),
        ] {
            let mut copy = value.clone();
            copy[field] = bad.into();
            assert!(Release::parse(&serde_json::to_vec(&copy).unwrap(), &url).is_err());
        }
        value["images"]["main"]["bytes"] = (boot_contract::MAIN_BYTES + 1).into();
        assert!(Release::parse(&serde_json::to_vec(&value).unwrap(), &url).is_err());
    }
    #[test]
    fn signed_redirect_bytes_survive_and_transport_downgrades_are_rejected() {
        let url =
            Url::parse("https://release-assets.githubusercontent.com/a@b?token=a%2Bb%3D&x=@1")
                .unwrap();
        assert_eq!(url.path, "/a@b?token=a%2Bb%3D&x=@1");
        assert_eq!(url.redirect("/next?q=%2F").unwrap().path, "/next?q=%2F");
        for bad in [
            "http://github.com/x",
            "https://user@github.com/x",
            "https://github.com/x\r\nHeader:x",
            "https://github.com:0/x",
        ] {
            assert!(Url::parse(bad).is_err());
        }
        for bad in ["0.2", "0.02.1", "0.2.1-rc.1", "0.2.1.0", "4294967296.0.0"] {
            assert!(Version::parse(bad).is_err());
        }
    }
}

pub mod boot;

#[cfg(target_os = "none")]
pub(crate) mod task;
