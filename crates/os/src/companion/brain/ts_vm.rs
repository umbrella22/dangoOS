//! Restricted resident bytecode. Candidate state and outputs remain private until commit.
use super::{
    input::{Inbox, Input},
    Output, CALLBACK_US, HEAP_MAX, SOURCE_MAX,
};
use crate::companion::{
    behavior::{
        affect::{EventKind, HapticPattern, SystemCommand},
        Expression,
    },
    render::FoxPose,
};
use alloc::{
    boxed::Box,
    format,
    rc::Rc,
    string::{String, ToString},
    vec::Vec,
};
use app_host::{esp_err, RawAllocator};
use core::cell::RefCell;
use serde_json::{json, Value};
use ts_engine::{Host, HostError, HostReply, Limits, Package, Vm};
pub fn expression(name: &str) -> Option<Expression> {
    Expression::ALL
        .into_iter()
        .find(|e| format!("{e:?}") == name)
}
struct State {
    input: String,
    response: Option<String>,
    output: Output,
    now: u64,
}
struct BrainHost(Rc<RefCell<State>>);
fn invalid(message: &str) -> HostError {
    HostError::new(esp_err::INVALID_ARG, message)
}
impl Host for BrainHost {
    fn now_ms(&mut self) -> Result<u64, HostError> {
        Ok(self.0.borrow().now)
    }
    fn call(&mut self, op: &str, args: &str, bytes: &[u8]) -> Result<HostReply, HostError> {
        if !bytes.is_empty() {
            return Err(invalid("brain operation accepts no bytes"));
        }
        let a: Vec<Value> =
            serde_json::from_str(args).map_err(|_| invalid("brain arguments must be an array"))?;
        let expected = match op {
            "brain.input" | "now_ms" | "brain.led_clear" => 0,
            "brain.play" => 2,
            "brain.visual" | "brain.led" => 4,
            _ => 1,
        };
        if a.len() != expected {
            return Err(invalid("wrong number of brain arguments"));
        }
        let name = || {
            a.first()
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("name required"))
        };
        let mut h = self.0.borrow_mut();
        if op == "brain.input" {
            return Ok(HostReply::json(h.input.clone()));
        }
        let v = match op {
            "now_ms" => json!(h.now),
            "brain.respond" => {
                let raw = a[0].to_string();
                if raw.len() > 4096 {
                    return Err(invalid("callback result too large"));
                }
                h.response = Some(raw);
                Value::Null
            }
            "brain.expression" => {
                h.output.expression =
                    expression(name()?).ok_or_else(|| invalid("unknown expression"))?;
                Value::Null
            }
            "brain.play" => {
                let pose = FoxPose::ALL
                    .into_iter()
                    .find(|p| format!("{p:?}") == name().unwrap_or(""))
                    .ok_or_else(|| invalid("unknown pose"))?;
                let ms = a[1]
                    .as_u64()
                    .filter(|n| (1..=60000).contains(n))
                    .ok_or_else(|| invalid("duration must be 1..60000"))?;
                h.output.play = Some((pose, h.now.saturating_add(ms)));
                Value::Null
            }
            "brain.visual" => {
                let mut v = [0.; 4];
                for (i, n) in v.iter_mut().enumerate() {
                    *n = a[i]
                        .as_f64()
                        .filter(|n| n.is_finite())
                        .ok_or_else(|| invalid("finite visual value required"))?;
                }
                if v[0].abs() > 8.
                    || !(0. ..=1.).contains(&v[1])
                    || !(0. ..=1.).contains(&v[2])
                    || v[3].abs() > 6.
                {
                    return Err(invalid("visual out of range"));
                }
                h.output.visual = crate::FoxVisual {
                    scale_x: 1.,
                    scale_y: 1.,
                    look_x: v[0] as f32,
                    pulse: v[1] as f32,
                    pet: v[2] as f32,
                    lean_x: v[3] as f32,
                };
                Value::Null
            }
            "brain.haptic" => {
                let p = [
                    HapticPattern::Panic,
                    HapticPattern::Angry,
                    HapticPattern::Surprised,
                    HapticPattern::Puzzled,
                    HapticPattern::Happy,
                    HapticPattern::Satisfied,
                    HapticPattern::Confused,
                    HapticPattern::Listening,
                    HapticPattern::Thinking,
                    HapticPattern::Error,
                    HapticPattern::ModuleInsert,
                    HapticPattern::ModuleRemove,
                    HapticPattern::MagneticContact,
                ]
                .into_iter()
                .find(|p| format!("{p:?}") == name().unwrap_or(""))
                .ok_or_else(|| invalid("unknown haptic"))?;
                h.output.haptic = Some(p);
                Value::Null
            }
            "brain.command" => {
                h.output.command = Some(match name()? {
                    "Hub" => SystemCommand::Hub,
                    "StopActivity" => SystemCommand::StopActivity,
                    "StartListening" => SystemCommand::StartListening,
                    _ => return Err(invalid("unknown system command")),
                });
                Value::Null
            }
            "brain.led" => {
                let i = a[0]
                    .as_u64()
                    .filter(|n| (1..=6).contains(n))
                    .ok_or_else(|| invalid("LED index must be 1..6"))?
                    as usize
                    - 1;
                let mut rgb = [0; 3];
                for (n, v) in rgb.iter_mut().enumerate() {
                    *v = a[n + 1]
                        .as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| invalid("RGB must be 0..255"))?;
                }
                h.output.leds.get_or_insert([[0; 3]; 6])[i] = rgb;
                Value::Null
            }
            "brain.led_clear" => {
                h.output.leds = None;
                Value::Null
            }
            _ => {
                return Err(HostError::new(
                    esp_err::NOT_SUPPORTED,
                    "operation unavailable in resident brain",
                ))
            }
        };
        Ok(HostReply::json(v.to_string()))
    }
}
#[cfg(test)]
pub fn encode(package: &[u8]) -> String {
    let mut out = String::from("etb1:");
    for b in package {
        use core::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}
fn decode(source: &str) -> Result<Vec<u8>, String> {
    if source.len() > SOURCE_MAX {
        return Err("brain package envelope exceeds 64 KiB".into());
    }
    let hex = source
        .strip_prefix("etb1:")
        .ok_or("TS brain requires an etb1 bytecode envelope")?;
    if hex.len() % 2 != 0 {
        return Err("invalid brain package envelope".into());
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |n: u8| match n {
                b'0'..=b'9' => Some(n - b'0'),
                b'a'..=b'f' => Some(n - b'a' + 10),
                _ => None,
            };
            Ok(digit(pair[0]).ok_or("invalid bytecode hex")? * 16
                + digit(pair[1]).ok_or("invalid bytecode hex")?)
        })
        .collect()
}
pub struct BrainVm {
    vm: Vm,
    state: Rc<RefCell<State>>,
}
impl BrainVm {
    pub fn new(
        source: &str,
        now: u64,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
    ) -> Result<Self, String> {
        let bytes = decode(source)?;
        let package = Package::parse(&bytes).map_err(|e| e.to_string())?;
        let state = Rc::new(RefCell::new(State {
            input: "null".into(),
            response: None,
            output: Output::default(),
            now,
        }));
        let mut vm = Vm::new(
            Box::new(BrainHost(state.clone())),
            Limits {
                memory_bytes: HEAP_MAX,
                host_bytes: 8192,
                pending_requests: 1,
                clock,
                execution_us: CALLBACK_US,
                interrupt_polls: 200,
                ..Default::default()
            },
            &crate::apps::ts::host::allocator(allocator),
        )
        .map_err(|e| e.to_string())?;
        // Only packages produced and delivered by the controlled host toolchain enter here.
        unsafe { vm.load(package) }.map_err(|e| e.to_string())?;
        if vm.call("main", None).map_err(|e| e.to_string())?.is_none() {
            return Err("brain requires main".into());
        }
        if vm.pending_requests() != 0 || vm.has_pending_jobs() {
            return Err("resident brain callbacks must be synchronous".into());
        }
        let mut out = Self { vm, state };
        out.rpc_inbox(Inbox::Typed(Input::Tick(now)), now)?;
        Ok(out)
    }
    pub fn rpc(&mut self, raw: String, now: u64) -> Result<String, String> {
        self.rpc_inbox(Inbox::Raw(raw), now)
    }
    pub(super) fn rpc_inbox(&mut self, inbox: Inbox, now: u64) -> Result<String, String> {
        let input = match inbox {
            Inbox::Raw(raw) => {
                serde_json::from_str::<Value>(&raw)
                    .map_err(|_| String::from("invalid brain input"))?;
                raw
            }
            Inbox::Typed(input) => input_json(input),
        };
        {
            let mut h = self.state.borrow_mut();
            h.now = now;
            h.input = input;
            h.response = None;
            h.output.haptic = None;
            h.output.command = None;
            if h.output.play.is_some_and(|(_, end)| now >= end) {
                h.output.play = None;
            }
        }
        if self
            .vm
            .call("onEvent", None)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err("brain requires onEvent".into());
        }
        if self.vm.pending_requests() != 0 || self.vm.has_pending_jobs() {
            return Err("resident brain callbacks must be synchronous".into());
        }
        self.state
            .borrow_mut()
            .response
            .take()
            .ok_or_else(|| "brain callback did not respond".into())
    }
    pub fn output(&mut self) -> Output {
        self.state.borrow().output
    }
    pub fn memory_used(&self) -> usize {
        self.vm.memory_stats().used
    }
}
// Internal inputs contain only enum names and scalars. Encode directly so a
// 100 Hz IMU stream does not allocate, clone and serialize a Value tree for
// every callback. Public raw input retains the JSON parser above.
fn input_json(input: Input) -> String {
    match input {
        Input::Tick(now) => format!("{{\"op\":\"tick\",\"input\":{now}}}"),
        Input::Interaction(now) | Input::IgnoreTouch(now) => {
            let kind = if matches!(input, Input::Interaction(_)) {
                "Interaction"
            } else {
                "IgnoreTouch"
            };
            format!("{{\"op\":\"event\",\"input\":{{\"type\":\"{kind}\",\"at_ms\":{now}}}}}")
        }
        Input::Pointer(p) => {
            format!("{{\"op\":\"event\",\"input\":{{\"type\":\"PointerSample\",\"at_ms\":{},\"x\":{},\"y\":{},\"pressed\":{}}}}}", p.at_ms, p.x, p.y, p.pressed)
        }
        Input::Sensor(e) => {
            let k = match e.kind {
                EventKind::Accel {
                    ax_mg,
                    ay_mg,
                    az_mg,
                } => format!("{{\"type\":\"Accel\",\"ax_mg\":{ax_mg},\"ay_mg\":{ay_mg},\"az_mg\":{az_mg}}}"),
                EventKind::Gyro {
                    gx_dps,
                    gy_dps,
                    gz_dps,
                } => format!("{{\"type\":\"Gyro\",\"gx_dps\":{gx_dps},\"gy_dps\":{gy_dps},\"gz_dps\":{gz_dps}}}"),
                EventKind::MagneticEdge { confidence } => {
                    format!("{{\"type\":\"MagneticEdge\",\"confidence\":{confidence}}}")
                }
                EventKind::LightLevel(n) | EventKind::BatteryLevel(n) => {
                    let kind = if matches!(e.kind,EventKind::LightLevel(_)){"LightLevel"}else{"BatteryLevel"};
                    format!("{{\"type\":\"{kind}\",\"args\":[{n}]}}")
                }
                EventKind::Presence(n)
                | EventKind::NetworkConnection(n)
                | EventKind::BluetoothConnection(n)
                | EventKind::MusicPlayback(n) => {
                    let kind = match e.kind{EventKind::Presence(_)=>"Presence",EventKind::NetworkConnection(_)=>"NetworkConnection",EventKind::BluetoothConnection(_)=>"BluetoothConnection",_=>"MusicPlayback"};
                    format!("{{\"type\":\"{kind}\",\"args\":[{n}]}}")
                }
                EventKind::ModuleInserted(n) | EventKind::ModuleRemoved(n) => {
                    let kind = if matches!(e.kind,EventKind::ModuleInserted(_)){"ModuleInserted"}else{"ModuleRemoved"};
                    format!("{{\"type\":\"{kind}\",\"args\":[\"{n:?}\"]}}")
                }
                EventKind::Activity(n) => format!("{{\"type\":\"Activity\",\"args\":[\"{n:?}\"]}}"),
                unit => format!("\"{unit:?}\""),
            };
            format!("{{\"op\":\"event\",\"input\":{{\"type\":\"SensorEvent\",\"at_ms\":{},\"source\":\"{:?}\",\"kind\":{k}}}}}", e.at_ms, e.source)
        }
    }
}
