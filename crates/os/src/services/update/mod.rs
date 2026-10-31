//! Release metadata and status shared by the native UI and network updater.
use alloc::{
    format,
    string::{String, ToString},
};

pub mod http;
pub mod staging;

pub const MANIFEST_URL: &str =
    "https://github.com/umbrella22/dangoOS/releases/latest/download/manifest.json";
pub const LAYOUT: &str = "MOSAICO_S31_AB_LAYOUT_V2_6M_NAND";
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

/// Manual HTTP CONNECT endpoint. The settings form accepts IPv4 and a port;
/// destination host names remain inside CONNECT for proxy-side DNS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpProxy {
    pub ip: [u8; 4],
    pub port: u16,
}
impl HttpProxy {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let endpoint = text
            .parse::<core::net::SocketAddrV4>()
            .map_err(|_| "代理地址格式应为 IPv4:端口")?;
        if endpoint.ip().is_unspecified()
            || endpoint.ip().is_multicast()
            || endpoint.ip().is_broadcast()
            || endpoint.port() == 0
        {
            return Err("请输入可访问的代理 IPv4 和端口");
        }
        Ok(Self {
            ip: endpoint.ip().octets(),
            port: endpoint.port(),
        })
    }
    pub fn address(self) -> String {
        format!(
            "{}.{}.{}.{}:{}",
            self.ip[0], self.ip[1], self.ip[2], self.ip[3], self.port
        )
    }
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

/// HTTPS location. Path/query bytes are preserved for signed asset redirects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}
impl Url {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let value = text
            .strip_prefix("https://")
            .ok_or("update requires HTTPS")?;
        if value.bytes().any(|b| b <= 32 || b >= 127) || value.contains(['#', '\\']) {
            return Err("update URL");
        }
        let end = value.find(['/', '?']).unwrap_or(value.len());
        let authority = &value[..end];
        if authority.contains('@') {
            return Err("update credentials in URL");
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "update port")?),
            None => (authority, 443),
        };
        if host.is_empty()
            || host.len() > 253
            || port == 0
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        {
            return Err("update host");
        }
        let path = match &value[end..] {
            "" => "/".into(),
            query if query.starts_with('?') => format!("/{query}"),
            path => path.into(),
        };
        Ok(Self {
            host: host.into(),
            port,
            path,
        })
    }
    pub fn authority(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
    pub fn redirect(&self, location: &str) -> Result<Self, &'static str> {
        if location.starts_with('/') && !location.starts_with("//") {
            Self::parse(&format!("https://{}{location}", self.authority()))
        } else {
            Self::parse(location)
        }
    }
    pub fn sibling(&self, file: &str) -> Result<Self, &'static str> {
        let directory = self
            .path
            .split('?')
            .next()
            .unwrap_or("/")
            .rsplit_once('/')
            .map_or("", |p| p.0);
        Self::parse(&format!("https://{}{directory}/{file}", self.authority()))
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
        if value["schema"].as_u64() != Some(1)
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
        let ota = &value["images"]["ota"];
        let filename = format!("dangoOS-v{version}-esp32s31-ota.bin");
        if ota["file"] != filename {
            return Err("release image name");
        }
        let length = ota["bytes"]
            .as_u64()
            .filter(|n| (288..=u64::from(crate::services::update::ota::SLOT_SIZE)).contains(n))
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
        serde_json::json!({"schema":1,"project":"dangoOS","chip":"esp32s31","layout":LAYOUT,
          "version":"0.10.0","tag":"v0.10.0","application":{"project":"mosaico-s31","version":"0.10.0","bytes":4096,"sha256":"ab".repeat(32)},
          "images":{"ota":{"file":"dangoOS-v0.10.0-esp32s31-ota.bin","bytes":4096,"sha256":"ab".repeat(32)}}})
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
            .ends_with("/v0.10.0/dangoOS-v0.10.0-esp32s31-ota.bin"));
        for (field, bad) in [
            ("layout", "MOSAICO_S31_AB_LAYOUT_V1"),
            ("chip", "esp32s3"),
            ("project", "other"),
        ] {
            let mut copy = value.clone();
            copy[field] = bad.into();
            assert!(Release::parse(&serde_json::to_vec(&copy).unwrap(), &url).is_err());
        }
        value["images"]["ota"]["bytes"] = (crate::services::update::ota::SLOT_SIZE + 1).into();
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

pub mod ota;

#[cfg(target_os = "none")]
pub(crate) mod task;
