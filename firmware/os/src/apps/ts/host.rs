//! Typed TS host operations, shared by firmware and the native integration harness.
use super::graphics::Graphics;
use alloc::rc::Rc;
use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use app_host::{
    esp_err, AudioControl, HapticPreset, HardwareRequest, HardwareSnapshot, HeapInfo,
    ServiceSnapshot,
};
use core::{alloc::Layout, cell::RefCell};
use serde_json::{json, Value};
use ts_engine::{Host, HostError, HostReply, HostRequest};

/// Snapshot callbacks never perform filesystem, bus, display or network IO.
/// Adapt the OS allocation callbacks to the independent engine function table.
/// The engine copies this table; no OS type or borrowed table crosses VM lifetime.
pub fn allocator(value: &app_host::RawAllocator) -> ts_engine::RawAllocator {
    ts_engine::RawAllocator {
        alloc: value.alloc,
        dealloc: value.dealloc,
        realloc: value.realloc,
    }
}

pub trait Platform {
    fn print(&mut self, text: &str, warning: bool);
    fn now_ms(&self) -> u64;
    fn unix_us(&self) -> Option<i64> {
        None
    }
    fn services(&mut self) -> ServiceSnapshot {
        ServiceSnapshot::default()
    }
    fn hardware(&mut self) -> HardwareSnapshot {
        HardwareSnapshot::default()
    }
    fn audio_control(&mut self, _request: AudioControl) -> Result<(), i32> {
        Err(esp_err::NOT_SUPPORTED)
    }
    fn hardware_request(&mut self, _request: HardwareRequest) -> Result<(), i32> {
        Err(esp_err::NOT_SUPPORTED)
    }
    fn display_info(&mut self) -> Result<app_host::DisplayInfo, i32> {
        Err(esp_err::NOT_SUPPORTED)
    }
    fn display_open(&mut self, _open: bool) {}
    fn touch(&self) -> Vec<app_host::TouchPoint> {
        Vec::new()
    }
    fn heap_info(&mut self, _caps: u32) -> HeapInfo {
        HeapInfo::default()
    }
    fn heap_probe(&mut self, _caps: u32, _layout: Layout) -> Result<bool, i32> {
        Err(esp_err::NOT_SUPPORTED)
    }
}
pub struct AppHost<P> {
    platform: P,
    args: Value,
    root: String,
    pub graphics: Rc<RefCell<Graphics>>,
    limits: ts_engine::Limits,
}
impl<P: Platform> AppHost<P> {
    pub fn new(platform: P, args_json: &str, root: String) -> Result<Self, HostError> {
        let args = serde_json::from_str(args_json)
            .map_err(|_| invalid("invalid application args JSON"))?;
        Ok(Self {
            platform,
            args,
            root,
            graphics: Rc::new(RefCell::new(Graphics::new(
                ts_engine::Limits::default().memory_bytes,
            ))),
            limits: ts_engine::Limits::default(),
        })
    }
    pub fn with_limits(mut self, limits: ts_engine::Limits) -> Self {
        self.graphics = Rc::new(RefCell::new(Graphics::new(limits.memory_bytes)));
        self.limits = limits;
        self
    }
}
pub(super) fn invalid(message: &str) -> HostError {
    HostError::new(esp_err::INVALID_ARG, message)
}
pub(super) fn arguments(text: &str) -> Result<Vec<Value>, HostError> {
    serde_json::from_str::<Vec<Value>>(text)
        .map_err(|_| invalid("host arguments must be a JSON array"))
}
pub(super) fn count(args: &[Value], n: usize) -> Result<(), HostError> {
    if args.len() == n {
        Ok(())
    } else {
        Err(invalid("wrong number of host arguments"))
    }
}
pub(super) fn text(value: &Value) -> Result<&str, HostError> {
    value.as_str().ok_or_else(|| invalid("string required"))
}
pub(super) fn integer(value: &Value) -> Result<u32, HostError> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid("uint32 required"))
}
pub(super) fn bool_value(value: &Value) -> Result<bool, HostError> {
    value.as_bool().ok_or_else(|| invalid("boolean required"))
}
pub(super) fn output<T>(result: Result<T, i32>) -> Result<T, HostError> {
    result.map_err(|code| HostError::new(code, "service rejected output request"))
}
const HAPTICS: [&str; 13] = [
    "panic",
    "angry",
    "surprised",
    "puzzled",
    "happy",
    "satisfied",
    "confused",
    "listening",
    "thinking",
    "error",
    "module_insert",
    "module_remove",
    "magnetic_contact",
];
impl<P: Platform> Host for AppHost<P> {
    fn draw(&mut self, draw: ts_engine::Draw) -> Result<(), HostError> {
        self.graphics.borrow_mut().draw(draw)
    }
    fn now_ms(&mut self) -> Result<u64, HostError> {
        Ok(self.platform.now_ms())
    }
    fn emit(&mut self, value: i32) -> Result<(), ts_engine::Error> {
        self.platform.print(&format!("emit={value}"), false);
        Ok(())
    }
    fn call(&mut self, operation: &str, args: &str, bytes: &[u8]) -> Result<HostReply, HostError> {
        if operation.starts_with("display.")
            || operation.starts_with("image.")
            || operation.starts_with("font.")
        {
            if operation == "display.open" {
                let a = arguments(args)?;
                count(&a, 1)?;
                let info = output(self.platform.display_info())?;
                let reply = self.graphics.borrow_mut().call(
                    operation,
                    &json!([info.width, info.height, a[0]]).to_string(),
                    bytes,
                )?;
                self.platform.display_open(true);
                return Ok(reply);
            }
            if operation == "display.touch" {
                return Ok(HostReply::json(
                    json!(self
                        .platform
                        .touch()
                        .iter()
                        .map(|p| json!({"id":p.id,"x":p.x,"y":p.y}))
                        .collect::<Vec<_>>())
                    .to_string(),
                ));
            }
            let reply = self.graphics.borrow_mut().call(operation, args, bytes)?;
            if operation == "display.close" {
                self.platform.display_open(false)
            }
            return Ok(reply);
        }
        if !bytes.is_empty() {
            return Err(invalid("this synchronous operation accepts no bytes"));
        }
        let a = arguments(args)?;
        let value = match operation {
            "print" | "log" => {
                count(&a, 1)?;
                self.platform.print(text(&a[0])?, operation == "log");
                Value::Null
            }
            "now_ms" => {
                count(&a, 0)?;
                json!(self.platform.now_ms())
            }
            "runtime_limits" => {
                count(&a, 0)?;
                json!({"memory_bytes":self.limits.memory_bytes,"host_bytes":self.limits.host_bytes,"execution_us":self.limits.execution_us,"pending_requests":self.limits.pending_requests})
            }
            "date" => {
                count(&a, 2)?;
                let seconds = a[0]
                    .as_i64()
                    .ok_or_else(|| invalid("integer seconds required"))?;
                json!(output(app_host::graphics::date(seconds, text(&a[1])?))?)
            }
            "unix_us" => {
                count(&a, 0)?;
                json!(self.platform.unix_us())
            }
            "args" => {
                count(&a, 0)?;
                self.args.clone()
            }
            "storage_root" => {
                count(&a, 0)?;
                json!(self.root)
            }
            "services" => {
                count(&a, 0)?;
                json!(self.platform.services())
            }
            "hardware" => {
                count(&a, 0)?;
                let snapshot = self.platform.hardware();
                if snapshot.magnetic.iter().any(|v| !v.is_finite()) {
                    return Err(HostError::new(
                        esp_err::INVALID_STATE,
                        "magnetic snapshot is not finite",
                    ));
                }
                json!(snapshot)
            }
            "volume" => {
                count(&a, 1)?;
                let value = integer(&a[0])?;
                if value > 100 {
                    return Err(invalid("volume must be 0..100"));
                }
                output(
                    self.platform
                        .audio_control(AudioControl::Volume(value as u8)),
                )?;
                Value::Null
            }
            "muted" => {
                count(&a, 1)?;
                output(
                    self.platform
                        .audio_control(AudioControl::Muted(bool_value(&a[0])?)),
                )?;
                Value::Null
            }
            "haptic" => {
                count(&a, 1)?;
                let name = text(&a[0])?;
                let preset = HAPTICS
                    .iter()
                    .position(|n| *n == name)
                    .map(|i| HapticPreset::ALL[i])
                    .ok_or_else(|| invalid("unknown haptic preset"))?;
                output(
                    self.platform
                        .hardware_request(HardwareRequest::Haptic(preset)),
                )?;
                Value::Null
            }
            "led_frame" => {
                count(&a, 1)?;
                let rows = a[0]
                    .as_array()
                    .filter(|v| v.len() == 6)
                    .ok_or_else(|| invalid("six RGB pixels required"))?;
                let mut frame = [[0; 3]; 6];
                for (row, out) in rows.iter().zip(frame.iter_mut()) {
                    let values = row
                        .as_array()
                        .filter(|v| v.len() == 3)
                        .ok_or_else(|| invalid("RGB triplet required"))?;
                    for (v, out) in values.iter().zip(out) {
                        *out = u8::try_from(integer(v)?)
                            .map_err(|_| invalid("RGB channels must be 0..255"))?;
                    }
                }
                output(
                    self.platform
                        .hardware_request(HardwareRequest::LedFrame(frame)),
                )?;
                Value::Null
            }
            "led_release" => {
                count(&a, 0)?;
                output(self.platform.hardware_request(HardwareRequest::LedRelease))?;
                Value::Null
            }
            "heap_info" => {
                count(&a, 1)?;
                heap_json(self.platform.heap_info(integer(&a[0])?))
            }
            "heap_probe" => {
                count(&a, 3)?;
                let caps = integer(&a[0])?;
                let size = integer(&a[1])? as usize;
                let alignment = integer(&a[2])? as usize;
                if size == 0 {
                    return Err(invalid("positive probe bytes required"));
                }
                let layout = Layout::from_size_align(size, alignment)
                    .map_err(|_| invalid("invalid probe layout"))?;
                json!(output(self.platform.heap_probe(caps, layout))?)
            }
            _ => {
                return Err(HostError::new(
                    esp_err::NOT_SUPPORTED,
                    "unsupported synchronous host operation",
                ))
            }
        };
        Ok(HostReply::json(value.to_string()))
    }
}
fn heap_json(h: HeapInfo) -> Value {
    use app_host::heap_fields as f;
    let mut value = json!({});
    let m = value.as_object_mut().unwrap();
    if h.valid_fields & f::SIZES != 0 {
        m.insert("total_size".into(), json!(h.total_size));
        m.insert("free_size".into(), json!(h.free_size));
        m.insert("allocated_size".into(), json!(h.allocated_size));
    }
    if h.valid_fields & f::MINIMUM_FREE != 0 {
        m.insert("minimum_free_size".into(), json!(h.minimum_free_size));
    }
    if h.valid_fields & f::LARGEST_FREE != 0 {
        m.insert("largest_free_block".into(), json!(h.largest_free_block));
    }
    if h.valid_fields & f::BLOCK_COUNTS != 0 {
        m.insert("allocated_blocks".into(), json!(h.allocated_blocks));
        m.insert("free_blocks".into(), json!(h.free_blocks));
        m.insert("total_blocks".into(), json!(h.total_blocks));
    }
    if h.valid_fields & f::SAMPLED_MINIMUM_FREE != 0 {
        m.insert(
            "sampled_minimum_free_size".into(),
            json!(h.sampled_minimum_free_size),
        );
    }
    value
}

/// Decode only the shared native UI protocol. The UI task validates widget
/// handles, options, quotas and paths before touching LVGL.
pub fn ui_command(request: HostRequest, root: &str) -> Result<app_host::ui::UiCommand, HostError> {
    let op = request
        .operation
        .strip_prefix("ui.")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("UI operation required"))?;
    if matches!(op, "batch" | "batch_commit") {
        if request.args.len() > app_host::ui::BATCH_BYTES_MAX || !request.bytes.is_empty() {
            return Err(invalid("UI batch exceeds its byte contract"));
        }
        // Decode the bounded batch once, on the UI owner where references and
        // command limits are validated. Other UI calls retain their old boundary.
    } else {
        let _: Vec<Value> = arguments(&request.args)?;
    }
    Ok(app_host::ui::UiCommand {
        op: op.into(),
        args: request.args,
        bytes: request.bytes,
        root: root.into(),
    })
}
pub fn sleep_ms(request: &HostRequest) -> Result<u32, HostError> {
    let a = arguments(&request.args)?;
    count(&a, 1)?;
    if !request.bytes.is_empty() {
        return Err(invalid("sleep accepts no bytes"));
    }
    integer(&a[0])
}
pub fn asset_io(operation: &str) -> bool {
    matches!(
        operation,
        "image.load_file" | "image.save_file" | "font.load_file"
    )
}
fn storage_path(root: &str, value: &Value) -> Result<String, HostError> {
    let path = text(value)?;
    if path.contains('\0') {
        return Err(invalid("file path contains NUL"));
    }
    Ok(if path.starts_with('/') {
        path.into()
    } else {
        format!("{root}/{path}")
    })
}
fn stat_json(meta: fs::Metadata) -> Value {
    json!({"type":if meta.kind==fs::Kind::Dir {"dir"} else {"file"},"size":meta.size})
}

/// Runs only on the services executor in firmware. Host tests use a real VFS.
pub fn storage(
    vfs: &mut fs::Vfs,
    root: &str,
    request: &HostRequest,
    reply_bytes: usize,
) -> Result<HostReply, HostError> {
    let reply_bytes = match request.operation.as_str() {
        "image.load_file" => app_host::image::FILE_MAX + 4,
        "font.load_file" => 1024 * 1024 + 4,
        _ => reply_bytes,
    };
    let a = arguments(&request.args)?;
    count(
        &a,
        match request.operation.as_str() {
            "storage.rename" | "storage.write_text" => 2,
            _ => 1,
        },
    )?;
    let path = storage_path(root, &a[0])?;
    let result = (|| -> Result<HostReply, fs::Errno> {
        let value = match request.operation.as_str() {
            "storage.exists" => match vfs.stat(&path) {
                Ok(_) => json!(true),
                Err(fs::Errno::NOENT | fs::Errno::NOTDIR) => json!(false),
                Err(e) => return Err(e),
            },
            "storage.stat" => stat_json(vfs.stat(&path)?),
            "storage.mkdir" => {
                vfs.create_dir_all(&path)?;
                Value::Null
            }
            "storage.list" => Value::Array(
                vfs.read_dir(&path)?
                    .into_iter()
                    .map(|e| {
                        let mut m = stat_json(e.meta);
                        m["name"] = json!(e.name);
                        m
                    })
                    .collect(),
            ),
            "storage.read" | "storage.read_text" | "image.load_file" | "font.load_file" => {
                let meta = vfs.stat(&path)?;
                if meta.kind != fs::Kind::File {
                    return Err(fs::Errno::ISDIR);
                }
                let allowance =
                    reply_bytes.saturating_sub(if request.operation == "storage.read_text" {
                        2
                    } else {
                        4
                    });
                if meta.size as usize > allowance {
                    return Err(fs::Errno::FBIG);
                }
                let bytes = vfs.read_file(&path)?;
                return if request.operation != "storage.read_text" {
                    Ok(HostReply {
                        json: "null".into(),
                        bytes,
                    })
                } else {
                    let text = String::from_utf8(bytes).map_err(|_| fs::Errno::CORRUPT)?;
                    let encoded_size = text.bytes().fold(2usize, |n, b| {
                        n.saturating_add(match b {
                            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
                            0..=31 => 6,
                            _ => 1,
                        })
                    });
                    if encoded_size > reply_bytes {
                        return Err(fs::Errno::FBIG);
                    }
                    Ok(HostReply::json(json!(text).to_string()))
                };
            }
            "storage.write" => {
                vfs.write_file(&path, &request.bytes)?;
                Value::Null
            }
            "storage.write_text" => {
                let value = a[1].as_str().ok_or(fs::Errno::INVAL)?;
                vfs.write_file(&path, value.as_bytes())?;
                Value::Null
            }
            "storage.remove" => {
                vfs.unlink(&path)?;
                Value::Null
            }
            "storage.rmdir" => {
                vfs.rmdir(&path)?;
                Value::Null
            }
            "storage.rename" => {
                let to = storage_path(root, &a[1]).map_err(|_| fs::Errno::INVAL)?;
                vfs.rename(&path, &to)?;
                Value::Null
            }
            _ => return Err(fs::Errno::INVAL),
        };
        Ok(HostReply::json(value.to_string()))
    })();
    let reply = result
        .map_err(|e| HostError::new(e.0, format!("{} {}: {}", request.operation, path, e)))?;
    if reply.json.len().saturating_add(reply.bytes.len()) > reply_bytes {
        return Err(HostError::new(
            fs::Errno::FBIG.0,
            "storage reply exceeds transport budget",
        ));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, rc::Rc};
    use core::cell::RefCell;
    #[derive(Default)]
    struct Record {
        audio: Vec<AudioControl>,
        hardware: Vec<HardwareRequest>,
    }
    struct Board(Rc<RefCell<Record>>);
    impl Platform for Board {
        fn print(&mut self, _: &str, _: bool) {}
        fn now_ms(&self) -> u64 {
            123
        }
        fn audio_control(&mut self, r: AudioControl) -> Result<(), i32> {
            self.0.borrow_mut().audio.push(r);
            Ok(())
        }
        fn hardware_request(&mut self, r: HardwareRequest) -> Result<(), i32> {
            self.0.borrow_mut().hardware.push(r);
            Ok(())
        }
        fn heap_info(&mut self, _: u32) -> HeapInfo {
            HeapInfo {
                free_size: 0,
                valid_fields: app_host::heap_fields::SIZES,
                ..Default::default()
            }
        }
    }
    #[test]
    fn numeric_output_validation_and_absent_heap_metrics_preserve_meaning() {
        let record = Rc::new(RefCell::new(Record::default()));
        let mut host =
            AppHost::new(Board(record.clone()), "{}", "/data/appdata/test".into()).unwrap();
        for args in [
            "[-1]",
            "[100.5]",
            "[101]",
            "[4294967296]",
            "[\"1\"]",
            "[]",
            "[1,2]",
        ] {
            assert_eq!(
                host.call("volume", args, &[]).unwrap_err().code,
                esp_err::INVALID_ARG
            );
        }
        assert!(record.borrow().audio.is_empty());
        host.call("volume", "[0]", &[]).unwrap();
        host.call("muted", "[false]", &[]).unwrap();
        assert_eq!(
            record.borrow().audio,
            [AudioControl::Volume(0), AudioControl::Muted(false)]
        );
        host.call("haptic", "[\"magnetic_contact\"]", &[]).unwrap();
        assert_eq!(
            record.borrow().hardware,
            [HardwareRequest::Haptic(HapticPreset::MagneticContact)]
        );
        let heap: Value =
            serde_json::from_str(&host.call("heap_info", "[4096]", &[]).unwrap().json).unwrap();
        assert_eq!(heap["free_size"], 0);
        assert!(heap.get("largest_free_block").is_none());
    }
    fn request(vm: &mut ts_engine::Vm) -> HostRequest {
        let package = include_bytes!("../../../../../resources/embedts/fixtures/host-request.etb");
        unsafe { vm.load(ts_engine::Package::parse(package).unwrap()) }.unwrap();
        vm.call("main", None).unwrap();
        match vm.take_request().unwrap() {
            ts_engine::Request::Host(request) => request,
            _ => panic!("expected host request"),
        }
    }
    fn storage_call(
        vfs: &mut fs::Vfs,
        operation: &str,
        args: Value,
        bytes: &[u8],
        limit: usize,
    ) -> Result<HostReply, HostError> {
        struct Arguments(Value);
        impl ts_engine::Host for Arguments {
            fn call(&mut self, op: &str, _: &str, _: &[u8]) -> Result<HostReply, HostError> {
                assert_eq!(op, "args");
                Ok(HostReply::json(self.0.to_string()))
            }
        }
        let mut vm = ts_engine::Vm::new(
            Box::new(Arguments(
                json!({"operation": operation, "args": args.to_string(), "bytes": bytes}),
            )),
            Default::default(),
            &ts_engine::GLOBAL_ALLOCATOR,
        )
        .unwrap();
        let request = request(&mut vm);
        storage(vfs, "/data/appdata/test", &request, limit)
    }
    #[test]
    fn real_vfs_preserves_binary_unicode_nul_content_and_existing_error_codes() {
        let mut vfs = fs::Vfs::new();
        vfs.mount(
            "data",
            fs::Fs::mount_or_format(
                Box::new(fs::RamDevice::new(4096, 32)),
                fs::Geometry::flash(32),
            )
            .unwrap()
            .0,
        );
        vfs.create_dir_all("/data/appdata/test").unwrap();
        storage_call(
            &mut vfs,
            "storage.write_text",
            json!(["团子.txt", "团子\0\n"]),
            &[],
            1024,
        )
        .unwrap();
        assert_eq!(
            storage_call(
                &mut vfs,
                "storage.read_text",
                json!(["团子.txt"]),
                &[],
                1024
            )
            .unwrap()
            .json,
            json!("团子\0\n").to_string()
        );
        storage_call(
            &mut vfs,
            "storage.write",
            json!(["bytes"]),
            &[0, 255, 1],
            1024,
        )
        .unwrap();
        assert_eq!(
            storage_call(&mut vfs, "storage.read", json!(["bytes"]), &[], 1024)
                .unwrap()
                .bytes,
            [0, 255, 1]
        );
        assert_eq!(
            storage_call(&mut vfs, "storage.stat", json!(["missing"]), &[], 1024)
                .unwrap_err()
                .code,
            fs::Errno::NOENT.0
        );
        assert_eq!(
            storage_call(&mut vfs, "storage.stat", json!(["bad\0path"]), &[], 1024)
                .unwrap_err()
                .code,
            esp_err::INVALID_ARG
        );
        vfs.write_file("/data/appdata/test/plain", b"12345678")
            .unwrap();
        assert_eq!(
            storage_call(&mut vfs, "storage.read_text", json!(["plain"]), &[], 10)
                .unwrap()
                .json,
            "\"12345678\""
        );
        vfs.write_file("/data/appdata/test/escape", &[0, 0])
            .unwrap();
        assert_eq!(
            storage_call(&mut vfs, "storage.read_text", json!(["escape"]), &[], 10)
                .unwrap_err()
                .code,
            fs::Errno::FBIG.0
        );
        // The text response ceiling does not impose a six-times-smaller file rule.
        assert_eq!(
            vfs.read_file("/data/appdata/test/plain").unwrap(),
            b"12345678"
        );
    }
}
