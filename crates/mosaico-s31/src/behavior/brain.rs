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
/// Commands generated internally; raw JSON remains the public compatibility path.
#[derive(Clone, Copy, Debug)]
enum Input {
    Tick(u64),
    Sensor(SensorEvent),
    Pointer(super::personality::PointerSample),
    Interaction(u64),
    IgnoreTouch(u64),
}
enum Inbox {
    Raw(String),
    Typed(Input),
}
struct BrainHost {
    inbox: Option<Inbox>,
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
        match input {
            Inbox::Raw(raw) => {
                c.push_str(&raw);
                Ok(Ret::Values(1))
            }
            Inbox::Typed(input) => Ok(input.push(c)),
        }
    } else {
        Ok(Ret::Yield(Request::Host(IDLE)))
    }
}
impl Input {
    /// Return at most seven scalars through the existing host trampoline.
    /// Allocate tables only in Lua, under lua_resume's protected boundary and
    /// heap cap. JSON's decoder pushes all numbers as floats, including u64s.
    fn push(self, c: &mut Call) -> Ret {
        match self {
            Self::Tick(now) => {
                c.push_number(1.);
                c.push_number(now as f64);
                Ret::Values(2)
            }
            Self::Pointer(p) => {
                c.push_number(2.);
                c.push_number(p.at_ms as f64);
                c.push_number(p.x as f64);
                c.push_number(p.y as f64);
                c.push_bool(p.pressed);
                Ret::Values(5)
            }
            Self::Interaction(now) | Self::IgnoreTouch(now) => {
                c.push_number(if matches!(self, Self::Interaction(_)) {
                    3.
                } else {
                    4.
                });
                c.push_number(now as f64);
                Ret::Values(2)
            }
            Self::Sensor(e) => {
                c.push_number(5.);
                c.push_number(e.at_ms as f64);
                c.push_str(&format!("{:?}", e.source));
                match e.kind {
                    EventKind::Accel {
                        ax_mg,
                        ay_mg,
                        az_mg,
                    } => {
                        c.push_str("Accel");
                        for n in [ax_mg, ay_mg, az_mg] {
                            c.push_number(n as f64);
                        }
                        Ret::Values(7)
                    }
                    EventKind::Gyro {
                        gx_dps,
                        gy_dps,
                        gz_dps,
                    } => {
                        c.push_str("Gyro");
                        for n in [gx_dps, gy_dps, gz_dps] {
                            c.push_number(n as f64);
                        }
                        Ret::Values(7)
                    }
                    EventKind::MagneticEdge { confidence } => {
                        c.push_str("MagneticEdge");
                        c.push_number(confidence as f64);
                        Ret::Values(5)
                    }
                    EventKind::LightLevel(n) | EventKind::BatteryLevel(n) => {
                        c.push_str(if matches!(e.kind, EventKind::LightLevel(_)) {
                            "LightLevel"
                        } else {
                            "BatteryLevel"
                        });
                        c.push_number(n as f64);
                        Ret::Values(5)
                    }
                    EventKind::Presence(value) => {
                        c.push_str("Presence");
                        c.push_bool(value);
                        Ret::Values(5)
                    }
                    EventKind::ModuleInserted(value) | EventKind::ModuleRemoved(value) => {
                        c.push_str(if matches!(e.kind, EventKind::ModuleInserted(_)) {
                            "ModuleInserted"
                        } else {
                            "ModuleRemoved"
                        });
                        c.push_str(&format!("{value:?}"));
                        Ret::Values(5)
                    }
                    EventKind::Activity(value) => {
                        c.push_str("Activity");
                        c.push_str(&format!("{value:?}"));
                        Ret::Values(5)
                    }
                    unit => {
                        c.push_str(&format!("{unit:?}"));
                        Ret::Values(4)
                    }
                }
            }
        }
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
local module=require('brain_source')(safe)()
assert(type(module)=='table','brain must return a table')
local app=module.create and module.create(START_MS) or module
assert(type(app.on_event)=='function' and type(app.tick)=='function','brain needs on_event and tick')
app.tick(START_MS)
local engines={}
host.complete('null')
while true do
    local raw,a,b,kind,d,e,f=host.next()
    if raw then
        local c
        if type(raw)=='string' then c=json.decode(raw)
        elseif raw==1 then c={op='tick',input=a}
        elseif raw==2 then c={op='event',input={type='PointerSample',at_ms=a,x=b,y=kind,pressed=d}}
        elseif raw==3 or raw==4 then c={op='event',input={type=raw==3 and 'Interaction' or 'IgnoreTouch',at_ms=a}}
        else
            local k=kind
            if kind=='Accel' then k={type=kind,ax_mg=d,ay_mg=e,az_mg=f}
            elseif kind=='Gyro' then k={type=kind,gx_dps=d,gy_dps=e,gz_dps=f}
            elseif kind=='MagneticEdge' then k={type=kind,confidence=d}
            elseif d~=nil then k={type=kind,args={d}} end
            c={op='event',input={type='SensorEvent',at_ms=a,source=b,kind=k}}
        end
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
        let prepare_started = clock.map(|clock| clock());
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
        let wrapped = format!("local START_MS={now}\n{DISPATCH}");
        let load_started = clock.map(|clock| clock());
        // Compile the standalone user chunk once, before the execution budget.
        // Concatenating source into a function wrapper would allow `end` to
        // escape its lexical sandbox and would shift user error line numbers.
        vm.prepare_chunk("brain_source", "brain", source.as_bytes())?;
        vm.load("brain-dispatch", wrapped.as_bytes())?;
        let loaded_at = clock.map(|clock| clock());
        // Registration and both chunk compilations are preparation, not callback
        // execution. Arm once here; slices and pcall retain this same deadline.
        vm.set_callback_budget(CALLBACK_HOOKS, CALLBACK_US);
        let mut brain = Self { vm };
        let result = brain.finish();
        if let (Some(start), Some(load), Some(loaded), Some(clock)) =
            (prepare_started, load_started, loaded_at, clock)
        {
            log::info!(
                "brain startup: prepare={}us compile={}us first_finish={}us ok={}",
                load.saturating_sub(start),
                loaded.saturating_sub(load),
                clock().saturating_sub(loaded),
                result.is_ok(),
            );
        }
        result?;
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
        self.rpc_inbox(Inbox::Raw(raw), now)
    }
    #[cfg(test)]
    fn rpc_typed(&mut self, input: Input, now: u64) -> Result<String, String> {
        self.rpc_inbox(Inbox::Typed(input), now)
    }
    fn rpc_inbox(&mut self, input: Inbox, now: u64) -> Result<String, String> {
        let h = self.vm.host_mut::<BrainHost>().unwrap();
        h.now = now;
        h.output.haptic = None;
        h.output.command = None;
        if h.output.play.is_some_and(|(_, end)| now >= end) {
            h.output.play = None;
        }
        h.inbox = Some(input);
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
    fn source_compilation_latency_does_not_spend_callback_wall_budget() {
        use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        static NOW: AtomicU64 = AtomicU64::new(0);
        static DELAYED: AtomicBool = AtomicBool::new(false);
        fn clock() -> u64 {
            NOW.load(Ordering::SeqCst)
        }
        unsafe fn allocate(layout: core::alloc::Layout) -> *mut u8 {
            // A real user-chunk string constant forces parser allocations.
            // Charge EVERY large allocation, so reparsing the source during
            // execution would consume a fresh >20ms and fail this test.
            if layout.size() > 16_000 {
                DELAYED.store(true, Ordering::SeqCst);
                NOW.fetch_add(CALLBACK_US + 1, Ordering::SeqCst);
            }
            unsafe { (mosaico_lua::GLOBAL_ALLOCATOR.alloc)(layout) }
        }
        unsafe fn deallocate(ptr: *mut u8, layout: core::alloc::Layout) {
            unsafe { (mosaico_lua::GLOBAL_ALLOCATOR.dealloc)(ptr, layout) }
        }
        static ALLOCATOR: RawAllocator = RawAllocator {
            alloc: allocate,
            dealloc: deallocate,
        };
        NOW.store(0, Ordering::SeqCst);
        DELAYED.store(false, Ordering::SeqCst);
        let source = format!(
            "local text='{}'\nassert(#text==24000)\nreturn {{on_event=function()end,tick=function()fox.set_expression('Happy')end}}",
            "x".repeat(24_000)
        );
        let mut brain = BrainVm::new(&source, 0, &ALLOCATOR, Some(clock)).unwrap();
        assert!(DELAYED.load(Ordering::SeqCst));
        assert!(NOW.load(Ordering::SeqCst) > CALLBACK_US);
        assert_eq!(brain.output().expression, Expression::Happy);

        // Negative control: the old runtime load(source) reparses the same
        // constant after arming, and must fail with this same allocator/clock.
        let mut legacy = Vm::new(
            Box::new(BrainHost {
                inbox: None,
                response: None,
                output: Output::default(),
                now: 0,
            }),
            Limits {
                clock: Some(clock),
                hook_interval: 100,
                ..Limits::default()
            },
            &ALLOCATOR,
            1,
        )
        .unwrap();
        legacy
            .load(
                "runtime-reparse-control",
                format!(
                    "local source={};assert(load(source))();for i=1,1000 do end",
                    serde_json::to_string(&source).unwrap(),
                )
                .as_bytes(),
            )
            .unwrap();
        legacy.set_callback_budget(CALLBACK_HOOKS, CALLBACK_US);
        let error = loop {
            match legacy.resume() {
                Step::Slice => continue,
                Step::Error(error) => break error,
                other => panic!("runtime reparse escaped its budget: {other:?}"),
            }
        };
        assert!(
            error.contains("callback budget exhausted (wall;"),
            "{error}"
        );

        // Large allocations caused by USER EXECUTION remain inside the same
        // wall budget, even though compilation now occurs before it.
        let error = BrainVm::new(
            "local t={};for i=1,3000 do t[i]=i end;while true do end",
            0,
            &ALLOCATOR,
            Some(clock),
        )
        .err()
        .unwrap();
        assert!(
            error.contains("callback budget exhausted (wall;"),
            "{error}"
        );
    }

    #[test]
    fn standalone_source_keeps_lexical_isolation_lines_and_size_boundary() {
        let allocator = &mosaico_lua::GLOBAL_ALLOCATOR;
        let isolated = r#"
          assert(SOURCE==nil and START_MS==nil and module==nil and host==nil and safe==nil and json==nil)
          assert(load==nil and require==nil and debug==nil and package==nil)
          assert(_G==_ENV)
          return {on_event=function()end,tick=function()fox.set_expression('Happy')end}
        "#;
        let mut brain = BrainVm::new(isolated, 123, allocator, None).unwrap();
        assert_eq!(brain.output().expression, Expression::Happy);
        for source in [
            "end; fox.set_expression('Angry'); local x=function()",
            "local ok=true\nerror('line receipt')",
        ] {
            let err = BrainVm::new(source, 0, allocator, None).err().unwrap();
            assert!(
                err.contains(if source.starts_with("end") {
                    "brain:1:"
                } else {
                    "brain:2:"
                }),
                "{err}"
            );
        }
        let mut maximum = String::from("return {on_event=function()end,tick=function()end}\n--");
        maximum.push_str(&"x".repeat(SOURCE_MAX - maximum.len()));
        assert_eq!(maximum.len(), SOURCE_MAX);
        assert!(BrainVm::new(&maximum, 0, allocator, None).is_ok());
        maximum.push('x');
        assert_eq!(
            BrainVm::new(&maximum, 0, allocator, None).err().unwrap(),
            "brain source exceeds 64 KiB"
        );
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

    fn raw_command(input: Input) -> (String, u64) {
        match input {
            Input::Tick(now) => (format!("{{\"op\":\"tick\",\"input\":{now}}}"), now),
            Input::Sensor(e) => (format!("{{\"op\":\"event\",\"input\":{}}}", sensor_json(e)), e.at_ms),
            Input::Pointer(p) => (format!("{{\"op\":\"event\",\"input\":{{\"type\":\"PointerSample\",\"at_ms\":{},\"x\":{},\"y\":{},\"pressed\":{}}}}}",p.at_ms,p.x,p.y,p.pressed), p.at_ms),
            Input::Interaction(now) => (format!("{{\"op\":\"event\",\"input\":{{\"type\":\"Interaction\",\"at_ms\":{now}}}}}"), now),
            Input::IgnoreTouch(until) => (format!("{{\"op\":\"event\",\"input\":{{\"type\":\"IgnoreTouch\",\"at_ms\":{until}}}}}"), 0),
        }
    }
    fn inputs() -> alloc::vec::Vec<Input> {
        use super::super::affect::{EventSource::*, ModuleKind};
        use EventKind::*;
        let mut kinds = alloc::vec![
            Accel {
                ax_mg: i16::MIN,
                ay_mg: 0,
                az_mg: i16::MAX
            },
            Gyro {
                gx_dps: i16::MAX,
                gy_dps: i16::MIN,
                gz_dps: -1
            },
            TouchPet,
            TouchPoke,
            LightLevel(0),
            LightLevel(255),
            Presence(false),
            Presence(true),
            KeyDown,
            KeyUp,
            ButtonShort,
            ButtonLong,
            ButtonDouble,
            MagneticApproach,
            MagneticContact,
            MagneticRelease,
            MagneticOrbit,
            MagneticEdge { confidence: 255 },
            ModuleInserted(ModuleKind::Interaction),
            ModuleInserted(ModuleKind::Camera),
            ModuleRemoved(ModuleKind::Interaction),
            ModuleRemoved(ModuleKind::Camera),
            BatteryLevel(0),
            BatteryLevel(255),
        ];
        kinds.extend(Expression::ALL.into_iter().map(Activity));
        let sources = [
            Imu,
            Touch,
            Light,
            Pir,
            KeyLeft,
            KeyRight,
            ButtonPower,
            ButtonBoot,
            Bmm150,
            ModuleRight,
            ModuleLeft,
            Battery,
            System,
        ];
        let mut commands = alloc::vec::Vec::new();
        for (index, kind) in kinds.into_iter().enumerate() {
            commands.push(Input::Sensor(SensorEvent {
                at_ms: index as u64 * 100,
                source: sources[index % sources.len()],
                kind,
            }));
        }
        // Include JSON number rounding boundaries, beyond i64 and at u64 max.
        for now in [
            0,
            1,
            1_000,
            (1u64 << 53) - 1,
            1u64 << 53,
            (1u64 << 53) + 1,
            (1u64 << 63) - 1,
            1u64 << 63,
            u64::MAX,
        ] {
            commands.extend([
                Input::Tick(now),
                Input::Interaction(now),
                Input::IgnoreTouch(now),
            ]);
            for pressed in [false, true] {
                commands.push(Input::Pointer(super::super::personality::PointerSample {
                    at_ms: now,
                    x: i32::MIN,
                    y: i32::MAX,
                    pressed,
                }));
            }
        }
        commands
    }
    #[test]
    fn typed_inputs_match_raw_structure_and_number_observations() {
        // A sandboxed user brain observes exact structure, array key types,
        // numeric subtypes and tostring; read it back via unchanged raw oracle.
        let source = r#"
local last
local function signature(v)
 local t=type(v)
 if t=='number' then return t..':'..math.type(v)..':'..tostring(v) end
 if t~='table' then return t..':'..tostring(v) end
 local keys={};for k in pairs(v) do keys[#keys+1]=k end
 table.sort(keys,function(a,b)return tostring(a)<tostring(b) end)
 local out={'table['}
 for _,k in ipairs(keys) do out[#out+1]=signature(k)..'='..signature(v[k])..';' end
 return table.concat(out)..']'
end
return {on_event=function(e)last=signature(e);return e.pressed end,
 tick=function(n)last=signature(n) end,
 Affect={new=function()return {read=function()return last end}end}}
"#;
        let mut raw = BrainVm::new(source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        let mut typed = BrainVm::new(source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        for brain in [&mut raw, &mut typed] {
            brain
                .rpc(
                    r#"{"op":"oracle","method":"affect.new","engine":1,"input":0}"#.into(),
                    0,
                )
                .unwrap();
        }
        for input in inputs() {
            let (json, now) = raw_command(input);
            assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now), "{input:?}");
            assert_eq!(raw.output(), typed.output(), "{input:?}");
            let read = r#"{"op":"oracle","method":"affect.read","engine":1,"input":null}"#;
            assert_eq!(
                raw.rpc(read.into(), now),
                typed.rpc(read.into(), now),
                "{input:?}"
            );
        }
    }
    #[test]
    fn typed_default_brain_replays_raw_outputs_and_expiry() {
        let mut raw =
            BrainVm::new(DEFAULT_SOURCE, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        let mut typed =
            BrainVm::new(DEFAULT_SOURCE, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        for input in inputs()
            .into_iter()
            .chain((0..120).map(|n| Input::Tick(n * 100)))
        {
            let (json, now) = raw_command(input);
            assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now), "{input:?}");
            assert_eq!(raw.output(), typed.output(), "{input:?}");
        }
        // Custom callback makes reset order, side-effect order and host now
        // observable; IgnoreTouch deliberately uses host now=0, not until.
        let source = r#"local n=0
return {tick=function()end,on_event=function(e)
n=n+1
if n==1 then fox.play('Happy',10);haptic.play('Happy');fox.command('Hub');fox.set_expression('Happy');fox.set_expression('Angry');led_strip.set(1,1,2,3)
elseif e.type=='IgnoreTouch' then fox.play('Curious',7) end
end}"#;
        raw = BrainVm::new(source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        typed = BrainVm::new(source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
        for input in [
            Input::Interaction(100),
            Input::Tick(109),
            Input::Tick(110),
            Input::IgnoreTouch(999),
            Input::Tick(6),
            Input::Tick(7),
        ] {
            let (json, now) = raw_command(input);
            assert_eq!(raw.rpc(json, now), typed.rpc_typed(input, now));
            assert_eq!(raw.output(), typed.output());
            match input {
                Input::Interaction(_) => {
                    assert_eq!(typed.output().play, Some((FoxPose::Happy, 110)))
                }
                Input::IgnoreTouch(_) => {
                    assert_eq!(typed.output().play, Some((FoxPose::Curious, 7)))
                }
                Input::Tick(109) => {
                    assert!(typed.output().haptic.is_none());
                    assert!(typed.output().command.is_none());
                    assert!(typed.output().play.is_some());
                }
                Input::Tick(110) | Input::Tick(7) => assert!(typed.output().play.is_none()),
                _ => {}
            }
        }
    }
    #[test]
    fn typed_controller_acceptance_and_failures_match_raw_fallback() {
        for result in [
            "nil",
            "false",
            "true",
            "42",
            "'invalid'",
            "error('failure')",
            "(function()while true do end end)()",
            "(function()local t={} while true do t[#t+1]={1,2,3,4} end end)()",
        ] {
            let source=format!("return {{tick=function()end,on_event=function()fox.set_expression('Happy');return {result} end}}");
            let mut raw =
                Controller::new(&source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
            let mut typed =
                Controller::new(&source, 0, &mosaico_lua::GLOBAL_ALLOCATOR, None).unwrap();
            let p = super::super::personality::PointerSample {
                at_ms: 123,
                x: 1,
                y: 2,
                pressed: true,
            };
            let (json, now) = raw_command(Input::Pointer(p));
            let expected = raw.dispatch(json, now).is_some_and(|v| v != "false");
            assert_eq!(typed.pointer(p), expected, "{result}");
            assert_eq!(raw.active(), typed.active(), "{result}");
            assert_eq!(raw.output(), typed.output(), "{result}");
            assert_eq!(raw.error().is_some(), typed.error().is_some(), "{result}");
            if !raw.active() {
                raw.tick(200);
                typed.tick(200);
                assert!(!typed.active());
                assert!(typed.output().is_none());
            }
        }
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
        self.dispatch_inbox(Inbox::Raw(input), now)
    }
    fn dispatch_typed(&mut self, input: Input, now: u64) -> Option<String> {
        self.dispatch_inbox(Inbox::Typed(input), now)
    }
    fn dispatch_inbox(&mut self, input: Inbox, now: u64) -> Option<String> {
        #[cfg(all(feature = "touch-latency-diagnostics", target_os = "none"))]
        let trace_started = crate::telemetry::touch_trace::now();
        let result = self.active.as_mut()?.rpc_inbox(input, now);
        #[cfg(all(feature = "touch-latency-diagnostics", target_os = "none"))]
        crate::telemetry::touch_trace::with(|t| {
            t.rpc(crate::telemetry::touch_trace::now().saturating_sub(trace_started))
        });
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.active = None;
                self.error = Some(e);
                // Report the transition once, after releasing the failed VM.
                // Later callbacks return above while inactive, without logging.
                log::warn!(
                    "brain: runtime fallback: {}",
                    crate::hub::brain_store::error_line(self.error.as_deref().unwrap_or(""))
                );
                None
            }
        }
    }
    pub fn tick(&mut self, now: u64) {
        self.dispatch_typed(Input::Tick(now), now);
    }
    pub fn event(&mut self, event: SensorEvent) {
        self.dispatch_typed(Input::Sensor(event), event.at_ms);
    }
    pub fn pointer(&mut self, p: super::personality::PointerSample) -> bool {
        self.dispatch_typed(Input::Pointer(p), p.at_ms)
            .is_some_and(|v| v != "false")
    }
    pub fn interaction(&mut self, now: u64) {
        self.dispatch_typed(Input::Interaction(now), now);
    }
    pub fn ignore_touch(&mut self, until: u64) {
        self.dispatch_typed(Input::IgnoreTouch(until), 0);
    }
}
use super::affect::{EventKind, SensorEvent};
#[cfg(test)]
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
