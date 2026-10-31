//! Compile once, then execute bounded callbacks in the restricted resident VM.
use super::{interface::*, CALLBACK_HOOKS, CALLBACK_US, HEAP_MAX, SOURCE_MAX};
use alloc::{boxed::Box, format, string::String};
use lua_engine::{Limits, RawAllocator, Request, Step, Vm};

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
                        .ok_or_else(|| "missing callback receipt".into());
                }
                Step::Slice => {}
                Step::Request(Request::Host(IDLE)) => {
                    return Err("callback did not complete".into());
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
    pub(super) fn rpc_typed(&mut self, input: Input, now: u64) -> Result<String, String> {
        self.rpc_inbox(Inbox::Typed(input), now)
    }
    pub(super) fn rpc_inbox(&mut self, input: Inbox, now: u64) -> Result<String, String> {
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
