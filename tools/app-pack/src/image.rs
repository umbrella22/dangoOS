use ::fs::{BlockDevice, Errno, Fs, Geometry, Kind, OpenFlags, RamDevice};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
pub struct Shared(Arc<Mutex<RamDevice>>);
impl Shared {
    pub fn image(bytes: Vec<u8>) -> Self {
        Self(Arc::new(Mutex::new(RamDevice::from_image(bytes, 4096))))
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().data.clone()
    }
}
impl BlockDevice for Shared {
    fn read(&mut self, b: u32, o: u32, bytes: &mut [u8]) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().read(b, o, bytes)
    }
    fn prog(&mut self, b: u32, o: u32, bytes: &[u8]) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().prog(b, o, bytes)
    }
    fn erase(&mut self, b: u32) -> std::result::Result<(), Errno> {
        self.0.lock().unwrap().erase(b)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Directory,
    File(Vec<u8>),
}
pub type Tree = BTreeMap<String, Entry>;
pub fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
pub fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn read_file(fs: &mut Fs, path: &str) -> Result<Vec<u8>> {
    let size = fs.stat(path).map_err(|e| format!("stat {path}: {e}"))?.size as usize;
    let mut file = fs
        .open(path, OpenFlags::READ)
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut bytes = vec![0; size];
    let mut offset = 0;
    while offset < size {
        let n = fs
            .read(&mut file, &mut bytes[offset..])
            .map_err(|e| e.to_string())?;
        require(n > 0, "file ended before stat size")?;
        offset += n;
    }
    fs.close(file).map_err(|e| e.to_string())?;
    Ok(bytes)
}
pub fn write_file(fs: &mut Fs, path: &str, bytes: &[u8]) -> Result<()> {
    let mut file = fs
        .open(path, OpenFlags::WRITE)
        .map_err(|e| format!("open {path}: {e}"))?;
    let mut offset = 0;
    while offset < bytes.len() {
        let n = fs
            .write(&mut file, &bytes[offset..])
            .map_err(|e| e.to_string())?;
        require(n > 0, "file write made no progress")?;
        offset += n;
    }
    fs.sync(&mut file).map_err(|e| e.to_string())?;
    fs.close(file).map_err(|e| e.to_string())
}
pub fn walk(fs: &mut Fs, path: &str, tree: &mut Tree) -> Result<()> {
    for entry in fs.read_dir(path).map_err(|e| format!("list {path}: {e}"))? {
        let child = format!("{}/{}", path.trim_end_matches('/'), entry.name);
        match entry.meta.kind {
            Kind::Dir => {
                tree.insert(child.clone(), Entry::Directory);
                walk(fs, &child, tree)?;
            }
            Kind::File => {
                tree.insert(child.clone(), Entry::File(read_file(fs, &child)?));
            }
        }
    }
    Ok(())
}
pub fn snapshot(bytes: &[u8]) -> Result<Tree> {
    require(
        !bytes.is_empty() && bytes.len() % 4096 == 0,
        "image must contain complete 4096-byte sectors",
    )?;
    let mut mounted = Fs::mount(
        Box::new(Shared::image(bytes.to_vec())),
        Geometry::flash((bytes.len() / 4096) as u32),
        true,
    )
    .map_err(|e| format!("read-only mount: {e}"))?;
    let mut tree = Tree::new();
    walk(&mut mounted, "/", &mut tree)?;
    Ok(tree)
}
pub fn inventory(tree: &Tree) -> Value {
    Value::Object(
        tree.iter()
            .map(|(path, entry)| {
                let value = match entry {
                    Entry::Directory => json!({"kind":"directory"}),
                    Entry::File(bytes) => {
                        json!({"kind":"file","bytes":bytes.len(),"sha256":sha(bytes)})
                    }
                };
                (path.clone(), value)
            })
            .collect(),
    )
}
