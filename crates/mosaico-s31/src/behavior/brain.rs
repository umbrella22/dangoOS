//! Restricted resident Lua behavior VM. Callback ownership and budgets are
//! independent of the foreground VM. Hardware effects are staged in `Output`.
use super::{
    affect::{HapticPattern, SystemCommand},
    Expression,
};
use crate::{fox::FoxPose, FoxVisual};
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
};
use mosaico_lua::{Call, Error, Host, Limits, RawAllocator, Request, Ret, Step, Vm};

pub const DEFAULT_SOURCE: &str = include_str!("../../assets/brain/default.lua");
pub const SOURCE_MAX: usize = 64 * 1024;
pub const HEAP_MAX: usize = 512 * 1024;
pub const CALLBACK_HOOKS: u32 = 2_000; // 200,000 instructions, across slices/pcall
pub const CALLBACK_US: u64 = 20_000;
const COMPLETE: u32 = 0xB001;
const IDLE: u32 = 0xB002;
const NAMES: [&str; 18] = [
    "Idle",
    "Sleep",
    "Curious",
    "Happy",
    "Puzzled",
    "Confused",
    "Surprised",
    "Panic",
    "Angry",
    "Satisfied",
    "Listening",
    "Thinking",
    "Speaking",
    "Error",
    "Tired",
    "Dying",
    "Peering",
    "Dazzled",
];
const POSES: [&str; 20] = [
    "Idle",
    "Sleep",
    "Curious",
    "Happy",
    "Puzzled",
    "Confused",
    "Surprised",
    "Panic",
    "Angry",
    "Satisfied",
    "Listening",
    "Thinking",
    "Speaking",
    "Error",
    "Tired",
    "Dying",
    "Annoyed",
    "Charging",
    "Peering",
    "Dazzled",
];
const HAPTICS: [HapticPattern; 13] = [
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
];
pub fn expression(name: &str) -> Option<Expression> {
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| Expression::ALL[i])
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub expression: Expression,
    pub visual: FoxVisual,
    pub play: Option<(FoxPose, u64)>,
    pub haptic: Option<HapticPattern>,
    pub command: Option<SystemCommand>,
    pub leds: Option<[[u8; 3]; 6]>,
}
impl Default for Output {
    fn default() -> Self {
        Self {
            expression: Expression::Idle,
            visual: FoxVisual::IDLE,
            play: None,
            haptic: None,
            command: None,
            leds: None,
        }
    }
}
struct BrainHost {
    inbox: Option<String>,
    response: Option<String>,
    output: Output,
    now: u64,
}
impl Host for BrainHost {
    fn print(&mut self, _line: &[u8]) {}
}
fn next(c: &mut Call) -> Result<Ret, Error> {
    let host = c
        .host::<BrainHost>()
        .ok_or_else(|| Error::msg("brain host missing"))?;
    if let Some(input) = host.inbox.take() {
        c.push_str(&input);
        Ok(Ret::Values(1))
    } else {
        Ok(Ret::Yield(Request::Host(IDLE)))
    }
}
fn complete(c: &mut Call) -> Result<Ret, Error> {
    let text = c.str(1)?;
    if text.len() > 4096 {
        return Err(Error::msg("callback result too large"));
    }
    let text = text.to_string();
    c.host::<BrainHost>().unwrap().response = Some(text);
    Ok(Ret::Yield(Request::Host(COMPLETE)))
}
fn set_expression(c: &mut Call) -> Result<Ret, Error> {
    let e = expression(c.str(1)?).ok_or_else(|| Error::arg(1, "unknown expression"))?;
    c.host::<BrainHost>().unwrap().output.expression = e;
    c.push_bool(true);
    Ok(Ret::Values(1))
}
fn play(c: &mut Call) -> Result<Ret, Error> {
    let name = c.str(1)?;
    let pose = POSES
        .iter()
        .position(|n| *n == name)
        .map(|i| FoxPose::ALL[i])
        .ok_or_else(|| Error::arg(1, "unknown pose"))?;
    let ms = c.integer(2)?;
    if !(1..=60000).contains(&ms) {
        return Err(Error::arg(2, "duration must be 1..60000 ms"));
    }
    let h = c.host::<BrainHost>().unwrap();
    h.output.play = Some((pose, h.now.saturating_add(ms as u64)));
    c.push_bool(true);
    Ok(Ret::Values(1))
}
fn visual(c: &mut Call) -> Result<Ret, Error> {
    let mut v = [0.; 4];
    for (i, x) in v.iter_mut().enumerate() {
        *x = c.number(i as i32 + 1)?;
        if !x.is_finite() {
            return Err(Error::arg(i as i32 + 1, "finite number required"));
        }
    }
    if v[0].abs() > 8.
        || !(0. ..=1.).contains(&v[1])
        || !(0. ..=1.).contains(&v[2])
        || v[3].abs() > 6.
    {
        return Err(Error::msg("visual out of range"));
    }
    c.host::<BrainHost>().unwrap().output.visual = FoxVisual {
        scale_x: 1.,
        scale_y: 1.,
        look_x: v[0] as f32,
        pulse: v[1] as f32,
        pet: v[2] as f32,
        lean_x: v[3] as f32,
    };
    Ok(Ret::Values(0))
}
fn haptic(c: &mut Call) -> Result<Ret, Error> {
    let name = c.str(1)?;
    let p = HAPTICS
        .iter()
        .copied()
        .find(|p| format!("{p:?}") == name)
        .ok_or_else(|| Error::arg(1, "unknown haptic"))?;
    c.host::<BrainHost>().unwrap().output.haptic = Some(p);
    c.push_bool(true);
    Ok(Ret::Values(1))
}
fn command(c: &mut Call) -> Result<Ret, Error> {
    let v = match c.str(1)? {
        "Hub" => SystemCommand::Hub,
        "StopActivity" => SystemCommand::StopActivity,
        "StartListening" => SystemCommand::StartListening,
        _ => return Err(Error::arg(1, "unknown system command")),
    };
    c.host::<BrainHost>().unwrap().output.command = Some(v);
    Ok(Ret::Values(0))
}
fn led_set(c: &mut Call) -> Result<Ret, Error> {
    let index = c.integer(1)?;
    if !(1..=6).contains(&index) {
        return Err(Error::arg(1, "LED index must be 1..6"));
    }
    let mut rgb = [0; 3];
    for (i, v) in rgb.iter_mut().enumerate() {
        let n = c.integer(i as i32 + 2)?;
        if !(0..=255).contains(&n) {
            return Err(Error::arg(i as i32 + 2, "RGB must be 0..255"));
        }
        *v = n as u8;
    }
    c.host::<BrainHost>()
        .unwrap()
        .output
        .leds
        .get_or_insert([[0; 3]; 6])[index as usize - 1] = rgb;
    c.push_bool(true);
    Ok(Ret::Values(1))
}
fn led_clear(c: &mut Call) -> Result<Ret, Error> {
    c.host::<BrainHost>().unwrap().output.leds = None;
    Ok(Ret::Values(0))
}
/// Encode arbitrary UTF-8 source safely as one Lua string literal.
fn literal(text: &str) -> String {
    let mut s = String::from("\"");
    for b in text.bytes() {
        if (32..127).contains(&b) && b != b'"' && b != b'\\' {
            s.push(b as char)
        } else {
            s.push_str(&format!("\\{b:03}"));
        }
    }
    s.push('"');
    s
}
const DISPATCH: &str = r#"
local json=require('json')
local host=require('brain_host')
local method_match=string.match
for key in pairs(string) do
    if key~="byte" and key~="char" and key~="len" and key~="sub" and key~="lower" and key~="upper" and key~="reverse" then string[key]=nil end
end
local safe={assert=assert,error=error,ipairs=ipairs,pairs=pairs,next=next,pcall=pcall,xpcall=xpcall,select=select,tonumber=tonumber,tostring=tostring,type=type,
    setmetatable=setmetatable,getmetatable=getmetatable,rawget=rawget,rawset=rawset,rawequal=rawequal,math=math,string=string,table=table,utf8=utf8,
    fox=require('fox'),haptic=require('brain_haptic'),led_strip=require('brain_led')}
safe._G=safe
local module=assert(load(SOURCE,'@brain','t',safe))()
assert(type(module)=='table','brain must return a table')
local app=module.create and module.create(START_MS) or module
assert(type(app.on_event)=='function' and type(app.tick)=='function','brain needs on_event and tick')
app.tick(START_MS)
local engines={}
host.complete('null')
while true do
    local raw=host.next()
    if raw then
        local c=json.decode(raw)
        local result
        if c.op=='event' then result=app.on_event(c.input);assert(result==nil or type(result)=='boolean','on_event must return boolean or nil')
        elseif c.op=='tick' then app.tick(c.input)
        elseif c.op=='oracle' then
            local kind,method=method_match(c.method,'^([^.]+)%.(.+)$')
            local class=kind=='affect' and module.Affect or module.Personality
            if method=='new' then engines[c.engine]=class.new(c.input)
            else result=engines[c.engine][method](engines[c.engine],c.input) end
        else error('unknown brain command') end
        host.complete(result==nil and 'null' or json.encode(result))
    end
end
"#;
pub struct BrainVm {
    vm: Vm,
}
impl BrainVm {
    pub fn new(
        source: &str,
        now: u64,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
    ) -> Result<Self, String> {
        if source.len() > SOURCE_MAX {
            return Err("brain source exceeds 64 KiB".into());
        }
        let mut vm = Vm::new(
            Box::new(BrainHost {
                inbox: None,
                response: None,
                output: Output::default(),
                now,
            }),
            Limits {
                memory_bytes: HEAP_MAX,
                hook_interval: 100,
                slice_hooks: 50,
                max_blocked_hooks: CALLBACK_HOOKS,
                clock,
                slice_us: 1000,
                max_blocked_us: CALLBACK_US as u32,
            },
            allocator,
            1,
        )?;
        vm.set_storage_root("/data/appdata/brain")?;
        vm.set_callback_budget(CALLBACK_HOOKS, CALLBACK_US);
        vm.register("brain_host", &[("next", next), ("complete", complete)])?;
        vm.register(
            "fox",
            &[
                ("set_expression", set_expression),
                ("play", play),
                ("visual", visual),
                ("command", command),
            ],
        )?;
        vm.register("brain_haptic", &[("play", haptic)])?;
        vm.register("brain_led", &[("set", led_set), ("clear", led_clear)])?;
        let wrapped = format!(
            "local SOURCE={}\nlocal START_MS={now}\n{DISPATCH}",
            literal(source)
        );
        vm.load("brain-dispatch", wrapped.as_bytes())?;
        let mut brain = Self { vm };
        brain.finish()?;
        Ok(brain)
    }
    fn finish(&mut self) -> Result<String, String> {
        // Voluntary yields also spend budget, even when no hook fires.
        for _ in 0..128 {
            match self.vm.resume() {
                Step::Request(Request::Host(COMPLETE)) => {
                    return self
                        .vm
                        .host_mut::<BrainHost>()
                        .unwrap()
                        .response
                        .take()
                        .ok_or_else(|| "missing callback receipt".into())
                }
                Step::Slice => {}
                Step::Request(Request::Host(IDLE)) => {
                    return Err("callback did not complete".into())
                }
                Step::Error(e) => return Err(e),
                other => return Err(format!("unexpected brain suspension: {other:?}")),
            }
        }
        Err("callback resume budget exhausted".into())
    }
    pub fn rpc(&mut self, raw: String, now: u64) -> Result<String, String> {
        let h = self.vm.host_mut::<BrainHost>().unwrap();
        h.now = now;
        h.output.haptic = None;
        h.output.command = None;
        if h.output.play.is_some_and(|(_, end)| now >= end) {
            h.output.play = None;
        }
        h.inbox = Some(raw);
        self.vm.set_callback_budget(CALLBACK_HOOKS, CALLBACK_US);
        self.finish()
    }
    pub fn output(&mut self) -> Output {
        self.vm.host_mut::<BrainHost>().unwrap().output
    }
    pub fn memory_used(&self) -> usize {
        self.vm.memory_used()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    fn compare(expected: &Value, actual: &Value, path: &str) {
        match (expected, actual) {
            (Value::Object(e), Value::Object(a)) => {
                for (k, v) in e {
                    compare(v, a.get(k).unwrap_or(&Value::Null), &format!("{path}.{k}"));
                }
                for (k, v) in a {
                    if !e.contains_key(k) {
                        assert!(v.is_null(), "extra {path}.{k}: {v}");
                    }
                }
            }
            (Value::Array(e), Value::Array(a)) => {
                assert_eq!(e.len(), a.len(), "{path}");
                for (i, (e, a)) in e.iter().zip(a).enumerate() {
                    compare(e, a, &format!("{path}[{i}]"));
                }
            }
            (Value::Number(e), Value::Number(a)) if e.is_i64() || e.is_u64() => {
                assert_eq!(e.as_f64(), a.as_f64(), "{path}");
            }
            (Value::Number(e), Value::Number(a)) => {
                let (e, a) = (e.as_f64().unwrap(), a.as_f64().unwrap());
                assert!(
                    (e - a).abs() <= 1e-5_f64.max(e.abs() * 1e-5),
                    "{path}: expected {e}, got {a}"
                );
            }
            _ => assert_eq!(expected, actual, "{path}"),
        }
    }
    #[test]
    fn bounded_adversaries_and_transactional_replacement() {
        let allocator = &mosaico_lua::GLOBAL_ALLOCATOR;
        for source in [
            "while true do end",
            "while true do pcall(function() while true do end end) end",
            "local x <close> = setmetatable({}, {__close=function() while true do end end}); error('close')",
            "local t={};while true do t[#t+1]={} end",
            "local x=setmetatable({}, {__gc=function() while true do pcall(function()error('caught')end) end end});error('drop')",
            "return {on_event=function()end,tick=function()while true do end end}",
        ] { assert!(BrainVm::new(source,0,allocator,None).is_err(),"adversary accepted: {source}"); }
        let mut brain = Controller::new(
            "return {on_event=function()end,tick=function()fox.set_expression('Happy')end}",
            0,
            allocator,
            None,
        )
        .unwrap();
        assert_eq!(brain.output().unwrap().expression, Expression::Happy);
        let old = brain.source().to_string();
        assert!(brain.replace("return {on_event=function()end,tick=function()fox.set_expression('Angry');error('reject')end}",1).is_err());
        assert_eq!(brain.source(), old);
        assert_eq!(brain.output().unwrap().expression, Expression::Happy);
        let next =
            "return {on_event=function()end,tick=function()fox.set_expression('Curious')end}";
        assert!(brain
            .replace_with_commit(next, 2, || Err("disk full".into()))
            .is_err());
        assert_eq!(brain.source(), old);
        assert_eq!(brain.output().unwrap().expression, Expression::Happy);
        brain.replace(next, 3).unwrap();
        assert_eq!(brain.output().unwrap().expression, Expression::Curious);
    }
    #[test]
    fn restricted_gc_and_string_colon_paths_cannot_run_unbounded_code() {
        let source = r#"
            local mt={__gc=function()while true do end end}
            for i=1,10000 do local x=setmetatable({},mt) end
            assert(string.match==nil and ('x').match==nil and string.rep==nil)
            assert(coroutine==nil and debug==nil and io==nil and os==nil and require==nil)
            return {on_event=function()end,tick=function()fox.set_expression('Happy')end}
        "#;
        let mut b = BrainVm::new(source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        assert_eq!(b.output().expression, Expression::Happy);
        assert!(b.memory_used() <= HEAP_MAX);
    }

    #[test]
    fn foreground_vm_and_resident_callbacks_keep_independent_state() {
        struct Foreground;
        impl Host for Foreground {
            fn print(&mut self, _: &[u8]) {}
        }
        let mut foreground = Vm::new(
            Box::new(Foreground),
            Limits::default(),
            &mosaico_lua::GLOBAL_ALLOCATOR,
            1,
        )
        .unwrap();
        foreground
            .set_storage_root("/data/appdata/foreground")
            .unwrap();
        foreground
            .load("foreground", b"for i=1,5 do coroutine.yield() end")
            .unwrap();
        let mut brain =
            Controller::new(DEFAULT_SOURCE, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        for now in 1..=5 {
            assert!(matches!(foreground.resume(), Step::Slice));
            brain.tick(now);
            assert!(brain.active());
        }
        assert!(matches!(foreground.resume(), Step::Done));
        brain.tick(100);
        assert!(brain.active());
    }

    #[test]
    fn frozen_native_oracle_all_calls() {
        let oracle: Value = serde_json::from_slice(include_bytes!(
            "../../testdata/n4-native-oracle/native-contract.json"
        ))
        .unwrap();
        let calls = oracle["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 4161);
        let mut brain = None;
        let mut current = 0;
        for (index, c) in calls.iter().enumerate() {
            let engine = c["engine"].as_u64().unwrap();
            if engine != current {
                brain = Some(
                    BrainVm::new(DEFAULT_SOURCE, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap(),
                );
                current = engine;
            }
            let raw = json!({"op":"oracle","method":c["op"],"engine":engine,"input":c["input"]})
                .to_string();
            let response = brain
                .as_mut()
                .unwrap()
                .rpc(raw, 0)
                .unwrap_or_else(|e| panic!("call {index} {} {}: {e}", c["case"], c["op"]));
            let actual: Value = serde_json::from_str(&response).unwrap();
            compare(
                &c["output"],
                &actual,
                &format!("call {index} {} {}", c["case"], c["op"]),
            );
        }
    }
}

/// Resident brain ownership. Replacement validates init and first tick in an
/// isolated VM; no candidate output escapes before the swap.
pub struct Controller {
    active: Option<BrainVm>,
    source: String,
    allocator: &'static RawAllocator,
    clock: Option<fn() -> u64>,
    error: Option<String>,
}
impl Controller {
    pub fn new(
        source: &str,
        now: u64,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
    ) -> Result<Self, String> {
        let active = BrainVm::new(source, now, allocator, clock)?;
        Ok(Self {
            active: Some(active),
            source: source.into(),
            allocator,
            clock,
            error: None,
        })
    }
    pub fn inactive(
        source: &str,
        allocator: &'static RawAllocator,
        clock: Option<fn() -> u64>,
        error: String,
    ) -> Self {
        Self {
            active: None,
            source: source.into(),
            allocator,
            clock,
            error: Some(error),
        }
    }
    pub fn replace(&mut self, source: &str, now: u64) -> Result<(), String> {
        self.replace_with_commit(source, now, || Ok(()))
    }
    pub fn replace_with_commit(
        &mut self,
        source: &str,
        now: u64,
        commit: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let candidate = BrainVm::new(source, now, self.allocator, self.clock)?;
        commit()?;
        self.active = Some(candidate);
        self.source = source.into();
        self.error = None;
        Ok(())
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn active(&self) -> bool {
        self.active.is_some()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn output(&mut self) -> Option<Output> {
        self.active.as_mut().map(BrainVm::output)
    }
    pub fn dispatch(&mut self, input: String, now: u64) -> Option<String> {
        let result = self.active.as_mut()?.rpc(input, now);
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.error = Some(e);
                self.active = None;
                None
            }
        }
    }
    pub fn tick(&mut self, now: u64) {
        self.dispatch(format!("{{\"op\":\"tick\",\"input\":{now}}}"), now);
    }
    pub fn event(&mut self, event: SensorEvent) {
        self.dispatch(
            format!("{{\"op\":\"event\",\"input\":{}}}", sensor_json(event)),
            event.at_ms,
        );
    }
    pub fn pointer(&mut self, p: super::personality::PointerSample) -> bool {
        self.dispatch(format!("{{\"op\":\"event\",\"input\":{{\"type\":\"PointerSample\",\"at_ms\":{},\"x\":{},\"y\":{},\"pressed\":{}}}}}",p.at_ms,p.x,p.y,p.pressed),p.at_ms).is_some_and(|v|v!="false")
    }
    pub fn interaction(&mut self, now: u64) {
        self.dispatch(
            format!("{{\"op\":\"event\",\"input\":{{\"type\":\"Interaction\",\"at_ms\":{now}}}}}"),
            now,
        );
    }
    pub fn ignore_touch(&mut self, until: u64) {
        self.dispatch(
            format!(
                "{{\"op\":\"event\",\"input\":{{\"type\":\"IgnoreTouch\",\"at_ms\":{until}}}}}"
            ),
            0,
        );
    }
}
use super::affect::{EventKind, SensorEvent};
fn sensor_json(e: SensorEvent) -> String {
    let kind = match e.kind {
        EventKind::Accel {
            ax_mg,
            ay_mg,
            az_mg,
        } => {
            format!("{{\"type\":\"Accel\",\"ax_mg\":{ax_mg},\"ay_mg\":{ay_mg},\"az_mg\":{az_mg}}}")
        }
        EventKind::Gyro {
            gx_dps,
            gy_dps,
            gz_dps,
        } => format!(
            "{{\"type\":\"Gyro\",\"gx_dps\":{gx_dps},\"gy_dps\":{gy_dps},\"gz_dps\":{gz_dps}}}"
        ),
        EventKind::MagneticEdge { confidence } => {
            format!("{{\"type\":\"MagneticEdge\",\"confidence\":{confidence}}}")
        }
        EventKind::LightLevel(v) => format!("{{\"type\":\"LightLevel\",\"args\":[{v}]}}"),
        EventKind::BatteryLevel(v) => format!("{{\"type\":\"BatteryLevel\",\"args\":[{v}]}}"),
        EventKind::Presence(v) => format!("{{\"type\":\"Presence\",\"args\":[{v}]}}"),
        EventKind::ModuleInserted(v) => {
            format!("{{\"type\":\"ModuleInserted\",\"args\":[\"{v:?}\"]}}")
        }
        EventKind::ModuleRemoved(v) => {
            format!("{{\"type\":\"ModuleRemoved\",\"args\":[\"{v:?}\"]}}")
        }
        EventKind::Activity(v) => format!("{{\"type\":\"Activity\",\"args\":[\"{v:?}\"]}}"),
        unit => format!("\"{unit:?}\""),
    };
    format!(
        "{{\"type\":\"SensorEvent\",\"at_ms\":{},\"source\":\"{:?}\",\"kind\":{kind}}}",
        e.at_ms, e.source
    )
}
