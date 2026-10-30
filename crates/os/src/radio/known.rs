//! The eight most recently used Wi-Fi networks, separate from public device
//! preferences. SSID bytes are identities; neither display formatting nor JSON
//! string conversion may change them. Only successful connections are remembered.
use super::{hex, unhex, Credentials, Network};
use alloc::vec::Vec;
use fs::{Errno, OpenFlags, Vfs};

pub const MAX_NETWORKS: usize = 8;
pub const PATH: &str = "/data/settings/wifi.json";
const TEMP: &str = "/data/settings/.wifi-pending";
// Eight records with 32-byte SSIDs and 63-byte keys fit below this file budget.
const MAX_FILE_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub enabled: bool,
    pub automatic: bool,
    networks: Vec<Credentials>,
}
impl Default for Known {
    fn default() -> Self {
        Self {
            enabled: false,
            automatic: true,
            networks: Vec::new(),
        }
    }
}
impl Known {
    pub fn identities(&self) -> Vec<Vec<u8>> {
        self.networks.iter().map(|n| n.ssid.clone()).collect()
    }
    pub fn get(&self, ssid: &[u8]) -> Option<Credentials> {
        self.networks.iter().find(|n| n.ssid == ssid).cloned()
    }
    pub fn available(&self, nearby: &[Network]) -> Option<Credentials> {
        self.networks
            .iter()
            .find(|n| nearby.iter().any(|ap| ap.ssid == n.ssid))
            .cloned()
    }
    /// Call only after association and DHCP succeed. A failed new password must
    /// never replace a known working key. Recency defines the eight-entry limit.
    pub fn remember(&mut self, credentials: Credentials) {
        self.forget(&credentials.ssid);
        self.networks.insert(0, credentials);
        self.networks.truncate(MAX_NETWORKS);
    }
    pub fn forget(&mut self, ssid: &[u8]) -> bool {
        let before = self.networks.len();
        self.networks.retain(|n| n.ssid != ssid);
        before != self.networks.len()
    }
    pub fn encode(&self) -> Vec<u8> {
        let networks: Vec<_> = self
            .networks
            .iter()
            .map(|n| {
                serde_json::json!({
                    "ssid_hex": hex(&n.ssid), "password_hex": hex(&n.password)
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "enabled": self.enabled, "automatic": self.automatic,
            "networks": networks
        }))
        .expect("Wi-Fi record scalars")
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err("Wi-Fi record size");
        }
        let v: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| "Wi-Fi record JSON")?;
        if v["schema"].as_u64() != Some(1) {
            return Err("Wi-Fi record schema");
        }
        let mut result = Self {
            enabled: v["enabled"].as_bool().ok_or("Wi-Fi enabled")?,
            automatic: v["automatic"].as_bool().ok_or("Wi-Fi automatic")?,
            networks: Vec::new(),
        };
        let entries = v["networks"].as_array().ok_or("Wi-Fi networks")?;
        if entries.len() > MAX_NETWORKS {
            return Err("Wi-Fi network count");
        }
        for entry in entries {
            let ssid = unhex(entry["ssid_hex"].as_str().ok_or("Wi-Fi SSID encoding")?, 32)?;
            let key = unhex(
                entry["password_hex"].as_str().ok_or("Wi-Fi key encoding")?,
                63,
            )?;
            let credentials = Credentials::wpa2(ssid, key)?;
            if result.get(&credentials.ssid).is_some() {
                return Err("Wi-Fi duplicate SSID");
            }
            result.networks.push(credentials);
        }
        Ok(result)
    }
}

/// This is the only shared/UI representation: it contains no passwords.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub loaded: bool,
    pub enabled: bool,
    pub automatic: bool,
    pub networks: Vec<Vec<u8>>,
    pub current: Option<Vec<u8>>,
    pub saving: bool,
    pub error: bool,
}

pub fn load(vfs: &mut Vfs) -> Result<Known, Errno> {
    match vfs.stat(PATH) {
        Err(Errno::NOENT) => Ok(Known::default()),
        Err(e) => Err(e),
        Ok(meta) if meta.size as usize > MAX_FILE_BYTES => Err(Errno::INVAL),
        Ok(_) => Known::decode(&vfs.read_file(PATH)?).map_err(|_| Errno::INVAL),
    }
}

enum Step {
    Prepare,
    Write(i32),
    Commit,
    Done,
}
/// Release the VFS between steps. The old record remains visible until rename;
/// a closed and synced temporary file is the only source of a replacement.
pub struct Save {
    bytes: Vec<u8>,
    next: Step,
}
impl Save {
    pub fn new(known: &Known) -> Self {
        Self {
            bytes: known.encode(),
            next: Step::Prepare,
        }
    }
    pub fn step(&mut self, vfs: &mut Vfs) -> Result<bool, Errno> {
        self.next = match core::mem::replace(&mut self.next, Step::Done) {
            Step::Prepare => {
                vfs.create_dir_all("/data/settings")?;
                Step::Write(vfs.open(TEMP, OpenFlags::WRITE)?)
            }
            Step::Write(fd) => {
                let result = vfs.write(fd, &self.bytes).and_then(|n| {
                    if n == self.bytes.len() {
                        vfs.fsync(fd)
                    } else {
                        Err(Errno::NOSPC)
                    }
                });
                let closed = vfs.close(fd);
                result.and(closed)?;
                Step::Commit
            }
            Step::Commit => {
                vfs.rename(TEMP, PATH)?;
                Step::Done
            }
            Step::Done => Step::Done,
        };
        Ok(matches!(self.next, Step::Done))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn credentials(ssid: &[u8], key: &[u8]) -> Credentials {
        Credentials::wpa2(ssid.to_vec(), key.to_vec()).unwrap()
    }
    fn vfs() -> Vfs {
        let (volume, _) = fs::Fs::mount_or_format(
            alloc::boxed::Box::new(fs::RamDevice::new(4096, 128)),
            fs::Geometry::flash(128),
        )
        .unwrap();
        let mut vfs = Vfs::default();
        vfs.mount("data", volume);
        vfs
    }
    fn save(vfs: &mut Vfs, known: &Known) {
        let mut operation = Save::new(known);
        while !operation.step(vfs).unwrap() {}
    }
    #[test]
    fn identities_and_passphrases_roundtrip_without_normalization_or_debug_leaks() {
        let mut known = Known::default();
        known.enabled = true;
        known.remember(credentials(b" a\0b\xff ", b" secret with spaces "));
        let bytes = known.encode();
        assert_eq!(Known::decode(&bytes).unwrap(), known);
        assert!(!alloc::format!("{known:?}").contains("secret"));
        assert_eq!(
            known.get(b" a\0b\xff ").unwrap().password,
            b" secret with spaces "
        );
        assert!(known.get(b"a\0b\xff").is_none());
    }
    #[test]
    fn recency_replaces_only_same_identity_and_prefers_available_known_networks() {
        let mut known = Known::default();
        for id in 0..MAX_NETWORKS {
            known.remember(credentials(&[id as u8], b"test-key8"));
        }
        known.remember(credentials(&[0], b"new-key8"));
        assert_eq!(known.identities().len(), MAX_NETWORKS);
        assert_eq!(known.identities()[0], [0]);
        assert_eq!(known.get(&[0]).unwrap().password, b"new-key8");
        known.remember(credentials(b"ninth", b"test-key8"));
        assert!(known.get(&[1]).is_none());
        let nearby = alloc::vec![Network {
            ssid: alloc::vec![0],
            channel: 1,
            rssi: -20,
            auth: "WPA2".into()
        }];
        assert_eq!(known.available(&nearby).unwrap().ssid, [0]);
        assert!(known.forget(&[0]));
        assert!(!known.forget(&[0]));
        assert!(known.available(&nearby).is_none());
    }
    #[test]
    fn replacement_is_atomic_and_incomplete_temporary_record_is_not_loaded() {
        let mut vfs = vfs();
        let mut known = Known::default();
        assert_eq!(load(&mut vfs).unwrap(), known);
        known.remember(credentials(b"first", b"test-key8"));
        save(&mut vfs, &known);
        let old = known.clone();
        known.remember(credentials(b"second", b"next-key8"));
        let mut operation = Save::new(&known);
        assert!(!operation.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), old);
        assert!(!operation.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), old);
        assert!(operation.step(&mut vfs).unwrap());
        assert_eq!(load(&mut vfs).unwrap(), known);
        assert!(vfs.stat(TEMP).is_err());
        known.forget(b"second");
        known.enabled = false;
        known.automatic = false;
        save(&mut vfs, &known);
        assert_eq!(load(&mut vfs).unwrap(), known);
    }
    #[test]
    fn corrupt_or_ambiguous_persisted_records_are_rejected_without_echoing_keys() {
        let mut known = Known::default();
        known.remember(credentials(b"ap", b"test-key8"));
        let mut value: serde_json::Value = serde_json::from_slice(&known.encode()).unwrap();
        let duplicate = value["networks"][0].clone();
        value["networks"].as_array_mut().unwrap().push(duplicate);
        assert!(Known::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        for key in ["00", "00ff", "GG", "0102030405060708"] {
            let mut value: serde_json::Value = serde_json::from_slice(&known.encode()).unwrap();
            value["networks"][0]["password_hex"] = key.into();
            assert!(Known::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
}
