-- Mosaico default behavior. A real Lua port of behavior/affect.rs and
-- personality.rs; Rust only supplies events, clocks and rendering/output APIs.
local min,max,abs = math.min,math.max,math.abs
local function sub(a,b) return max(0,a-b) end
local function priority(p) return p=='Error' and 3 or p=='Panic' and 2 or 1 end
local intensity={Idle=0,Sleep=0,Curious=40,Happy=50,Puzzled=60,Confused=45,Surprised=70,Panic=100,Angry=90,Satisfied=55,Listening=60,Thinking=35,Speaking=55,Error=100,Tired=25,Dying=15,Peering=40,Dazzled=65}
local function window(times,at,span,capacity)
    times[#times+1]=at
    while #times>capacity or (#times>0 and sub(at,times[1])>span) do table.remove(times,1) end
end
local A={}; A.__index=A
function A.new(now)
    return setmetatable({now_ms=now,last_expression='Idle',fallback_since_ms=now,idle_expression='Idle',error_active=false,error_since_ms=0,
        angry_cooldown_until=0,poke_times={},r_key_times={},key_left_down=false,key_right_down=false,
        light_history={},dawn_ref_level=0,pir_raw=false,pir_raw_since=now,pir_debounced=false,pir_sustained_fired=false,pir_rise_ms=0,
        magnetic_contacted=false,module_left_present=false,module_right_present=false,
        shake={violent_peaks={},violent_above=false,dizzy=0,dizzy_at_ms=0,burst_active=false,burst_first_motion_ms=0,burst_last_motion_ms=0,
            burst_above_small=false,burst_small_peaks=0,burst_large=false,burst_confused_emitted=false,small_burst_count=0,burst_window_start_ms=0}},A)
end
function A:queue_haptic(p)
    if self.error_active and p~='Error' then return end
    if not self.pending_haptic or priority(p)>=priority(self.pending_haptic) then self.pending_haptic=p end
end
function A:take_haptic(now)
    local p=self.pending_haptic
    if not p or (self.last_haptic_ms and now<self.last_haptic_ms+150) then return nil end
    self.pending_haptic=nil;self.last_haptic_ms=now;return p
end
function A:try_start_reaction(e,r,p,at,duration,h)
    if self.error_active then return false end
    local c=self.reaction
    if c and at<c.ends_ms then
        if p<c.priority or (p==c.priority and p>=3) then return false end
        if p==c.priority and (r=='Light' or r=='Presence' or r=='Magnetic') and r~=c.reason and at<c.started_ms+1200 then return false end
    end
    if self.module_sequence and p<=2 then return false end
    if p>=3 then self.module_sequence=nil end
    self.reaction={expression=e,reason=r,priority=p,started_ms=at,ends_ms=at+duration}
    if h then self:queue_haptic(h) end
    return true
end
function A:is_panic_active(at) local r=self.reaction;return r~=nil and r.priority==4 and at<r.ends_ms end
function A:is_angry_active(at) local r=self.reaction;return r~=nil and r.priority==3 and at<r.ends_ms end
function A:wake(at)
    self.dark_since=nil
    if self.idle_expression=='Sleep' then self.idle_expression='Idle';self.fallback_since_ms=at end
end
function A:keep_awake(at) self.awake_until=at+300000;self:wake(at) end
function A:rouse(at,how)
    local visible=self.idle_expression=='Sleep' and not self.error_active and not self.activity and not self.battery_expression and not self.reaction and not self.module_sequence
    local r=self.reaction
    local drowsy=r and r.reason=='Wake' and r.expression=='Tired' and at<r.ends_ms
    self:keep_awake(at)
    local grumpy
    if visible then grumpy=how=='Rough'
    elseif drowsy and (how=='Poke' or how=='Rough') then grumpy=true
    else return false end
    if grumpy then self.reaction=nil;return self:try_start_reaction('Angry','Wake',2,at,2500,'Angry') end
    return self:try_start_reaction('Tired','Wake',2,at,3000,nil)
end
function A:expire_reaction(now)
    if self.reaction and now>=self.reaction.ends_ms then self.fallback_since_ms=self.reaction.ends_ms;self.reaction=nil end
end
function A:advance_panic(now)
    if self.error_active then self.panic_followup=nil;return end
    local f=self.panic_followup;if not f or now<f.panic_end_ms then return end
    if not f.confused_started then
        local start=max(self.shake.burst_last_motion_ms,f.panic_end_ms)+5000;local ending=start+3000
        if now<start then return end
        if now>=ending then self.panic_followup=nil;self.fallback_since_ms=ending;return end
        self:try_start_reaction('Confused','ShakeLarge',2,start,3000,'Confused')
        f.confused_started=true;f.confused_end_ms=ending;return
    end
    if now>=f.confused_end_ms then
        local r=self.reaction
        if r and r.expression=='Confused' and r.reason=='ShakeLarge' and r.ends_ms==f.confused_end_ms then self.reaction=nil end
        self.fallback_since_ms=f.confused_end_ms;self.panic_followup=nil
    end
end
function A:advance_module_sequence(now)
    local s=self.module_sequence;if not s then return end
    while true do
        local ending=s.stage_started_ms+s.steps[s.index].duration_ms
        if now<ending then break end
        s.stage_started_ms=ending;s.index=s.index+1
        if s.index>#s.steps then self.module_sequence=nil;self.fallback_since_ms=ending;return end
    end
end
function A:advance_pir(now)
    if self.pir_raw~=self.pir_debounced then
        local at=self.pir_raw_since+200
        if now>=at then
            self.pir_debounced=self.pir_raw
            if self.pir_raw then
                local same=self.pir_absence_since and at<self.pir_absence_since+30000 or not self.pir_absence_since and self.pir_presence_since~=nil
                self.pir_rise_ms=at;self.pir_absence_since=nil;self:wake(at)
                if not same then self.pir_presence_since=at;self.pir_sustained_fired=false;self.fallback_since_ms=at;self:try_start_reaction('Curious','Presence',1,at,4000,nil) end
            else self.pir_absence_since=at end
        end
    end
    if self.pir_debounced and self.pir_presence_since then
        local fire=max(self.pir_presence_since+10000,self.pir_rise_ms)
        if now>=fire and not self.pir_sustained_fired then
            local allowed=not self.pir_last_satisfied_ms or fire>=self.pir_last_satisfied_ms+30000
            if allowed and self:try_start_reaction('Satisfied','Presence',1,fire,3000,'Satisfied') then self.pir_last_satisfied_ms=fire end
            self.pir_sustained_fired=true
        end
    end
    if self.pir_absence_since then
        local since=self.pir_absence_since
        if now>=since+30000 then self.pir_presence_since=nil;self.pir_sustained_fired=false end
        local sleep=max(since+60000,self.awake_until or 0)
        if not self.pir_debounced and now>=sleep and self.idle_expression~='Sleep' then self.idle_expression='Sleep';self.fallback_since_ms=sleep end
    end
end
function A:advance_dawn(now)
    if not self.light_level or not self.dawn_ref_at or now<self.dawn_ref_at+30000 then return end
    if self.light_level>=self.dawn_ref_level+20 then
        self:try_start_reaction('Curious','Light',1,now,3000,nil);self.dawn_ref_level=self.light_level;self.dawn_ref_at=now
    elseif self.light_level<self.dawn_ref_level then self.dawn_ref_level=self.light_level;self.dawn_ref_at=now end
end
function A:advance(now)
    now=max(now,self.now_ms);self.now_ms=now
    local r=self.reaction
    if self.key_left_down and r and r.reason=='KeyLeft' and r.expression=='Happy' and r.priority==2 then r.ends_ms=now+3000 end
    self:update_burst(now);self:expire_reaction(now);self:advance_panic(now);self:expire_reaction(now)
    self:advance_module_sequence(now);self:advance_pir(now);self:advance_dawn(now)
    if self.dark_since then local sleep=max(self.dark_since+60000,self.awake_until or 0)
        if now>=sleep and self.idle_expression~='Sleep' then self.idle_expression='Sleep';self.fallback_since_ms=sleep end end
    self:expire_reaction(now)
end
function A:resolve(skip_reaction)
    if self.error_active then return 'Error','Error',self.error_since_ms end
    local r=self.reaction;if not skip_reaction and r then return r.expression,r.reason,r.started_ms end
    local s=self.module_sequence;if s then local step=s.steps[s.index];return step.expression,step.reason,s.stage_started_ms end
    if self.activity then
        if not skip_reaction and self.panic_followup then return self.idle_expression,'Idle',self.fallback_since_ms end
        return self.activity,'Activity',self.fallback_since_ms
    end
    if self.battery_expression then return self.battery_expression,'Battery',self.fallback_since_ms end
    return self.idle_expression,'Idle',self.fallback_since_ms
end
function A:snapshot_at(now)
    local e,r,s=self:resolve(self.reaction and now>=self.reaction.ends_ms)
    return {type='AffectState',expression=e,intensity=intensity[e],since_ms=s,reason=r}
end
function A:tick(now)
    now=max(now,self.now_ms);self:advance(now)
    local e,r,s=self:resolve();local changed=e~=self.last_expression;self.last_expression=e
    local command=self.pending_command;self.pending_command=nil
    return {type='AffectTick',state={type='AffectState',expression=e,intensity=intensity[e],since_ms=s,reason=r},changed=changed,haptic=self:take_haptic(now),command=command}
end
function A:on_screen_touch(at) at=max(at,self.now_ms);self:advance(at);return self:rouse(at,'Poke') end
function A:on_interaction(at) at=max(at,self.now_ms);self:advance(at);self:keep_awake(at) end
function A:is_asleep() return self.idle_expression=='Sleep' end
function A:trigger_panic(at)
    if self.error_active then return end
    self.reaction={expression='Panic',reason='ShakeLarge',priority=4,started_ms=at,ends_ms=at+1500}
    self.panic_followup={panic_end_ms=at+1500,confused_started=false,confused_end_ms=0};self.module_sequence=nil
    local s=self.shake;s.violent_peaks={};s.dizzy=0;s.small_burst_count=0;s.burst_window_start_ms=at;s.prev_burst_last_motion_ms=nil;s.burst_small_peaks=0;s.burst_confused_emitted=true
    self:queue_haptic('Panic')
end
function A:start_angry(at,reason)
    if self.error_active then return end
    self.reaction={expression='Angry',reason=reason,priority=3,started_ms=at,ends_ms=at+10000}
    self.angry_cooldown_until=at+40000;self.module_sequence=nil;self.shake.small_burst_count=0;self.shake.burst_window_start_ms=at;self:queue_haptic('Angry')
end
function A:drain_dizzy(at)
    local s=self.shake;s.dizzy=sub(s.dizzy,min(sub(at,s.dizzy_at_ms),60000)*500//1000);s.dizzy_at_ms=max(s.dizzy_at_ms,at)
end
function A:add_dizzy(at,gain) self:drain_dizzy(at);self.shake.dizzy=min(self.shake.dizzy+gain,3000);if self.shake.dizzy>=1500 then self:on_dizzy(at) end end
function A:note_motion(at)
    local s=self.shake
    if not s.burst_active then s.burst_active=true;s.burst_first_motion_ms=at;s.burst_above_small=false;s.burst_small_peaks=0;s.burst_large=false;s.burst_confused_emitted=false end
    s.burst_last_motion_ms=at
    local r=self.reaction;if r and r.reason=='ShakeSmall' and r.expression=='Confused' and at<r.ends_ms then r.ends_ms=at+2500 end
end
function A:on_dizzy(at)
    local r=self.reaction
    if r then
        if r.reason=='ShakeSmall' and r.expression=='Confused' and at<r.ends_ms then r.ends_ms=at+2500;self.shake.burst_confused_emitted=true;return end
        if r.priority>=2 and at<r.ends_ms then return end
    end
    if self.module_sequence then return end
    local h=not self.shake.burst_confused_emitted and 'Confused' or nil
    if self:try_start_reaction('Confused','ShakeSmall',2,at,2500,h) then self.shake.burst_confused_emitted=true end
end
function A:on_violent_peak(at)
    local s=self.shake;window(s.violent_peaks,at,math.maxinteger,5)
    if #s.violent_peaks==5 and sub(at,s.violent_peaks[1])<=1000 then
        s.violent_peaks={};if not self:is_panic_active(at) then s.burst_confused_emitted=true;self:trigger_panic(at) end
    end
end
local function isqrt(n)
    if n<2 then return n end
    local x=n;local y=(x+1)//2
    while y<x do x=y;y=(x+n//x)//2 end
    return x
end
function A:on_accel(at,x,y,z)
    local d=abs(isqrt(x*x+y*y+z*z)-1000);local s=self.shake
    if d<80 then s.burst_above_small=false;s.violent_above=false;self:drain_dizzy(at);return end
    self:note_motion(at);self:rouse(at,d>=950 and 'Rough' or 'Gentle')
    if d>1200 then s.burst_large=true end
    if d>=300 then
        if not s.burst_above_small then s.burst_above_small=true;s.burst_small_peaks=min(255,s.burst_small_peaks+1) end
        self:add_dizzy(at,min(d,1000)//10)
    else s.burst_above_small=false;self:drain_dizzy(at) end
    if d>=950 then if not s.violent_above then s.violent_above=true;self:on_violent_peak(at) end
    elseif d<600 then s.violent_above=false end
end
function A:on_gyro(at,x,y,z)
    local d=max(abs(x),abs(y),abs(z))
    if d<20 then self:drain_dizzy(at);return end
    self:note_motion(at);self:rouse(at,'Gentle')
    if d>=150 then self:add_dizzy(at,min(d,500)//10) else self:drain_dizzy(at) end
end
function A:update_burst(now)
    local s=self.shake;if not s.burst_active then return end
    local close=s.burst_last_motion_ms+300;if now<close then return end
    s.burst_active=false;local small=s.burst_small_peaks>=1;local large=s.burst_large;local first,last=s.burst_first_motion_ms,s.burst_last_motion_ms
    s.burst_above_small=false;s.burst_small_peaks=0;s.burst_large=false;s.burst_confused_emitted=false
    if large or not small then return end
    local gap=s.prev_burst_last_motion_ms and sub(first,s.prev_burst_last_motion_ms)
    local valid=gap and gap>=300 and gap<=800
    if valid and s.small_burst_count>0 and sub(first,s.burst_window_start_ms)<=15000 then s.small_burst_count=min(255,s.small_burst_count+1)
    else s.small_burst_count=1;s.burst_window_start_ms=first end
    s.prev_burst_last_motion_ms=last
    if s.small_burst_count>=5 then s.small_burst_count=0;s.burst_window_start_ms=close
        if close>=self.angry_cooldown_until and not self:is_panic_active(close) and not self:is_angry_active(close) then self:start_angry(close,'ShakeBursts') end end
end
function A:on_touch_pet(at)
    if self:rouse(at,'Gentle') then return end
    local r=self.reaction;if r and r.reason=='TouchPet' and r.expression=='Happy' and at<r.ends_ms then r.ends_ms=at+1500;return end
    self:try_start_reaction('Happy','TouchPet',2,at,1500,'Happy')
end
function A:on_touch_poke(at)
    if self:rouse(at,'Poke') then return end
    window(self.poke_times,at,1500,3)
    if #self.poke_times>=3 then self.poke_times={}
        if at>=self.angry_cooldown_until and not self:is_panic_active(at) and not self:is_angry_active(at) then self:start_angry(at,'TouchPoke');return end end
    self:try_start_reaction('Puzzled','TouchPoke',2,at,1500,'Puzzled')
end
function A:reanchor_dawn(at,level) self.dawn_ref_level=level;self.dawn_ref_at=at end
function A:hold_cover(at,level)
    local c=self.light_cover;if not c then return end
    local threshold=max(8,c.before*40//100)
    if level>c.before-threshold//2 then self.light_cover=nil;return end
    local r=self.reaction
    if r and r.expression=='Peering' and at<r.ends_ms then r.ends_ms=max(r.ends_ms,min(at+3000,c.since_ms+15000)) end
end
function A:on_light_level(at,level)
    level=min(level,100);local h=self.light_history;h[#h+1]={level=level,at_ms=at};if #h>16 then table.remove(h,1) end
    if level>=20 then self:wake(at);self.dark_since=nil
    elseif level>=10 then self.dark_since=nil
    elseif not self.dark_since then self.dark_since=at end
    local oldest
    for _,sample in ipairs(h) do if sub(at,sample.at_ms)<=400 then oldest=sample;break end end
    if oldest and oldest.at_ms<at then
        local before=oldest.level;local threshold=max(8,max(before,level)*40//100)
        if level-before>=threshold then
            self.light_cover=nil;self:try_start_reaction('Dazzled','Light',1,at,3000,'Surprised')
            self.light_history={{level=level,at_ms=at}};self:reanchor_dawn(at,level)
        elseif before-level>=threshold and not self.light_cover then
            self.light_cover={before=before,since_ms=at};self:try_start_reaction('Peering','Light',1,at,3000,nil)
            self.light_history={{level=level,at_ms=at}};self:reanchor_dawn(at,level)
        end
    end
    self:hold_cover(at,level);if not self.dawn_ref_at then self:reanchor_dawn(at,level) end;self.light_level=level
end
function A:on_presence(at,present) if present~=self.pir_raw then self.pir_raw=present;self.pir_raw_since=at end end
function A:start_hug(at) self.r_key_times={};self.poke_times={};self:try_start_reaction('Satisfied','BothKeys',2,at,3500,'Satisfied') end
function A:on_key_down(at,source)
    if (source=='KeyLeft' or source=='KeyRight') and self:rouse(at,'Gentle') then return end
    if source=='KeyLeft' then
        if self.key_left_down then return end
        self.key_left_down=true;self.key_left_down_at=at
        if self.key_right_down and self.key_right_down_at and sub(at,self.key_right_down_at)<=300 then self:start_hug(at);return end
        self:try_start_reaction('Happy','KeyLeft',2,at,3000,'Happy')
    elseif source=='KeyRight' then
        if self.key_right_down then return end
        self.key_right_down=true;self.key_right_down_at=at
        if self.key_left_down and self.key_left_down_at and sub(at,self.key_left_down_at)<=300 then self:start_hug(at);return end
        window(self.r_key_times,at,1500,3)
        if #self.r_key_times>=3 then self.r_key_times={}
            if at>=self.angry_cooldown_until and not self:is_panic_active(at) and not self:is_angry_active(at) then self:start_angry(at,'KeyRight');return end end
        self:try_start_reaction('Puzzled','KeyRight',2,at,3000,'Puzzled')
    end
end
function A:on_key_up(at,source)
    if source=='KeyLeft' then
        if not self.key_left_down then return end
        self.key_left_down=false;self.key_left_down_at=nil
        local r=self.reaction;if r and r.reason=='KeyLeft' and r.expression=='Happy' and r.priority==2 then r.ends_ms=at+3000 end
    elseif source=='KeyRight' then self.key_right_down=false;self.key_right_down_at=nil end
end
function A:on_button_short(at,source)
    if source~='ButtonPower' then return end
    self:keep_awake(at)
    if not self.error_active then self.activity='Listening';self.fallback_since_ms=at end
    self.pending_command='StartListening';self:queue_haptic('Listening')
end
function A:on_button_long(at,source) if source=='ButtonPower' then self.pending_command='Hub' end end
function A:on_button_double(at,source) if source=='ButtonPower' then self.activity=nil;self.fallback_since_ms=at;self.pending_command='StopActivity' end end
function A:enter_error(at)
    if self.error_active then return end
    self.error_active=true;self.error_since_ms=at;self.fallback_since_ms=at;self.activity=nil;self.reaction=nil;self.panic_followup=nil;self.module_sequence=nil;self:queue_haptic('Error')
end
function A:on_activity(at,e)
    if e=='Listening' or e=='Thinking' or e=='Speaking' then
        if not self.error_active then self.activity=e;self.fallback_since_ms=at end
    elseif e=='Error' then self:enter_error(at)
    elseif e=='Idle' then self.activity=nil
        if self.error_active then self.error_active=false;self.error_since_ms=0;if self.pending_haptic=='Error' then self.pending_haptic=nil end end
        self.fallback_since_ms=at
    end
end
function A:on_battery_level(at,p)
    p=min(p,100);local old=self.battery_expression;local next
    if old=='Dying' then if p<8 then next='Dying' elseif p<=15 then next='Tired' end
    elseif old=='Tired' then if p>=18 then next=nil elseif p<=5 then next='Dying' else next='Tired' end
    else if p<=5 then next='Dying' elseif p<=15 then next='Tired' end end
    if next~=old then self.battery_expression=next;self.fallback_since_ms=at end
end
function A:on_magnetic_approach(at) if not self:rouse(at,'Gentle') then self:try_start_reaction('Curious','Magnetic',2,at,2000,nil) end end
function A:on_magnetic_contact(at)
    self:keep_awake(at);if self.magnetic_contacted then return end
    self.magnetic_contacted=true;self:try_start_reaction('Satisfied','Magnetic',2,at,1500,'MagneticContact')
end
function A:on_magnetic_release(at)
    if not self.magnetic_contacted then return end
    self.magnetic_contacted=false;self:try_start_reaction('Confused','Magnetic',2,at,2000,'Confused')
end
function A:on_magnetic_orbit(at) self:try_start_reaction('Surprised','Magnetic',2,at,1200,'Surprised') end
function A:on_magnetic_edge(at,c) if c>=50 then self:try_start_reaction('Puzzled','Magnetic',2,at,1500,'Puzzled') end end
function A:start_module_sequence(at,steps,h)
    if self.error_active or self.module_sequence or (self.reaction and at<self.reaction.ends_ms and self.reaction.priority>2) then return end
    self.reaction=nil;self.module_sequence={steps=steps,index=1,stage_started_ms=at};self:queue_haptic(h)
end
local function step(e,r,d) return {expression=e,reason=r,duration_ms=d} end
function A:on_module_inserted(at,source,kind)
    self:keep_awake(at);self.magnetic_contacted=false
    local key=source=='ModuleLeft' and 'module_left_present' or source=='ModuleRight' and 'module_right_present' or nil
    if not key or self[key] then return end;self[key]=true
    if kind=='Interaction' then self:start_module_sequence(at,{step('Surprised','ModuleInsert',800),step('Curious','ModuleInsert',2000),step('Happy','ModuleInsert',2000)},'ModuleInsert')
    elseif kind=='Camera' then self:start_module_sequence(at,{step('Curious','ModuleInsert',2500)},'ModuleInsert') end
end
function A:on_module_removed(at,source,kind)
    self.magnetic_contacted=false
    local key=source=='ModuleLeft' and 'module_left_present' or source=='ModuleRight' and 'module_right_present' or nil
    if not key or not self[key] then return end;self[key]=false
    if kind=='Interaction' then self:start_module_sequence(at,{step('Surprised','ModuleRemove',800),step('Confused','ModuleRemove',2500)},'ModuleRemove')
    elseif kind=='Camera' then self:start_module_sequence(at,{step('Confused','ModuleRemove',2500)},'ModuleRemove') end
end
function A:on_event(event)
    local at=max(event.at_ms,self.now_ms);self:advance(at)
    local k=event.kind;local t=type(k)=="string" and k or k.type;local args=type(k)=="table" and k.args or {}
    if t=='Accel' then self:on_accel(at,k.ax_mg,k.ay_mg,k.az_mg)
    elseif t=='Gyro' then self:on_gyro(at,k.gx_dps,k.gy_dps,k.gz_dps)
    elseif t=='MagneticEdge' then self:on_magnetic_edge(at,k.confidence)
    elseif t=='KeyDown' then self:on_key_down(at,event.source)
    elseif t=='KeyUp' then self:on_key_up(at,event.source)
    elseif t=='ButtonShort' then self:on_button_short(at,event.source)
    elseif t=='ButtonLong' then self:on_button_long(at,event.source)
    elseif t=='ButtonDouble' then self:on_button_double(at,event.source)
    elseif t=='ModuleInserted' then self:on_module_inserted(at,event.source,args[1])
    elseif t=='ModuleRemoved' then self:on_module_removed(at,event.source,args[1])
    else
        local names={TouchPet='on_touch_pet',TouchPoke='on_touch_poke',LightLevel='on_light_level',Presence='on_presence',MagneticApproach='on_magnetic_approach',MagneticContact='on_magnetic_contact',MagneticRelease='on_magnetic_release',MagneticOrbit='on_magnetic_orbit',BatteryLevel='on_battery_level',Activity='on_activity'}
        local method=names[t];if method then self[method](self,at,args[1]) else error('unknown event '..t) end
    end
end

local P={};P.__index=P
local holds={Curious=1500,Puzzled=1800,Annoyed=2000,Angry=4000,Petted=1800}
local faces={Curious='Curious',Puzzled='Puzzled',Annoyed='Confused',Angry='Angry',Petted='Satisfied'}
local buzz={Puzzled='Puzzled',Annoyed='Confused',Angry='Angry',Petted='Satisfied'}
local function half(dt,ms)
    local x,scale=dt/ms,1
    while x>1 do scale=scale*0.5;x=x-1 end
    local y=x*0.6931471805599453
    return scale/(1+y+y*y*0.5)
end
function P.new(now)
    return setmetatable({now_ms=now,phase='Idle',press_start_ms=0,press_x=0,press_y=0,last_x=0,last_y=0,press_travel=0,look_x=0,
        petting=false,stroke=0,pet=0,lean_x=0,eased_ms=now,tap_times={},angry_cooldown_until=0,ignore_until_ms=0},P)
end
function P:ignore_until(at)
    self.ignore_until_ms=max(self.ignore_until_ms,at)
    if self.phase~='Idle' then self.phase='Idle';self.pending=nil;self.petting=false end
end
function P:accepts(at) return at>=self.ignore_until_ms end
function P:start_petting()
    self.petting=true;self.pending=nil;self.tap_times={};self.active={reaction='Petted',ends_ms=math.maxinteger};self.haptic_pending='Satisfied'
end
function P:expire(now)
    local p=self.pending
    if p and now>=p.at_ms then
        self.pending=nil;self.active={reaction=p.reaction,ends_ms=now+holds[p.reaction]};self.haptic_pending=buzz[p.reaction]
        if p.reaction=='Angry' then self.angry_cooldown_until=now+6000 end
    end
    if self.active and now>=self.active.ends_ms then self.active=nil end
end
function P:track_press_move(x,y)
    local moved=abs(x-self.last_x)+abs(y-self.last_y)
    if moved>2 then
        self.press_travel=min(2147483647,self.press_travel+moved)
        if self.petting then self.stroke=min(1,self.stroke+moved*0.02) end
        self.last_x=x;self.last_y=y
    end
end
function P:register_tap(at)
    window(self.tap_times,at,2500,8)
    local count=#self.tap_times;local frantic=0
    for _,t in ipairs(self.tap_times) do if sub(at,t)<=1200 then frantic=frantic+1 end end
    local r
    if frantic>=5 then r=at>=self.angry_cooldown_until and 'Angry' or 'Annoyed'
    else r=count==1 and 'Curious' or count==2 and 'Puzzled' or 'Annoyed' end
    if self.active and self.active.reaction=='Angry' then return end
    self.pending={reaction=r,at_ms=at+(count==1 and 220 or 380)};self.pulse_start_ms=at
end
function P:on_pointer(s)
    local at=max(s.at_ms,self.now_ms);self.now_ms=at
    if at<self.ignore_until_ms then return self:output() end
    self:expire(at)
    if s.pressed then
        if self.phase=='Idle' then
            self.phase='Pressed';self.press_start_ms=at;self.press_x=s.x;self.press_y=s.y;self.last_x=s.x;self.last_y=s.y;self.press_travel=0;self.petting=false;self.pulse_start_ms=at
        else self:track_press_move(s.x,s.y)
            if not self.petting and (self.press_travel>=40 or sub(at,self.press_start_ms)>=450) then self:start_petting() end
        end
    elseif self.phase=='Pressed' then
        self:track_press_move(s.x,s.y);self.last_x=s.x;self.last_y=s.y;local held=sub(at,self.press_start_ms);self.phase='Idle'
        if self.petting then self.petting=false;self.active={reaction='Petted',ends_ms=at+1800}
        elseif held<=320 and self.press_travel<=14 then self:register_tap(at) end
    end
    return self:output()
end
function P:tick(now)
    now=max(now,self.now_ms);self.now_ms=now;self:expire(now)
    local target=0
    if self.phase~='Idle' then
        local dx,dy=self.last_x-240,self.last_y-240;local a,b=abs(dx),abs(dy)
        local len=max(a,b)+min(a,b)//2;target=dx/max(1,len)*7
    end
    self.look_x=self.look_x+(target-self.look_x)*0.25
    if self.phase=='Pressed' and not self.petting and sub(now,self.press_start_ms)>=450 then self:start_petting() end
    local dt=min(sub(now,self.eased_ms),200);self.eased_ms=now;self.stroke=self.stroke*half(dt,180)
    local squash,lean=0,0
    if self.petting and self.phase=='Pressed' then squash=min(1,0.35+self.stroke);lean=max(-6,min(6,(self.last_x-self.press_x)*0.06)) end
    local k=1-half(dt,60);self.pet=self.pet+(squash-self.pet)*k;self.lean_x=self.lean_x+(lean-self.lean_x)*k
    if self.pet<0.01 and squash==0 then self.pet=0 end
    if abs(self.lean_x)<0.05 and lean==0 then self.lean_x=0 end
    return self:output()
end
function P:output()
    local pulse=0
    if self.pulse_start_ms then local elapsed=sub(self.now_ms,self.pulse_start_ms)
        if elapsed<280 then local t=elapsed/280;pulse=t<0.15 and t/0.15 or (1-t)/0.85 end end
    local r=self.active and self.active.reaction;local h=self.haptic_pending;self.haptic_pending=nil
    return {type='PersonaOutput',expression=r and faces[r],reaction=r,haptic=h,pulse=pulse,look_x=self.look_x,pet=self.pet,lean_x=self.lean_x}
end
local function merge(persona,affect)
    if affect=='Error' or affect=='Panic' then return affect end
    if persona then return persona end
    return affect
end
return {Affect=A,Personality=P,create=function(now)
    local a,p=A.new(now),P.new(now)
    local pointer_down=false
    local haptics={}
    local persona=p:output()
    local function add(h) if h then haptics[#haptics+1]=h;if #haptics>2 then table.remove(haptics,2) end end end
    local function publish(at)
        local state=a:snapshot_at(at)
        fox.set_expression(merge(persona.expression,state.expression))
        fox.visual(persona.look_x,persona.pulse,persona.pet,persona.lean_x)
    end
    return {
        on_event=function(e)
            if e.type=='PointerSample' then
                local press=e.pressed and not pointer_down;pointer_down=e.pressed
                if press and a:on_screen_touch(e.at_ms) then p:ignore_until(e.at_ms+3000);publish(e.at_ms);return true end
                if not p:accepts(e.at_ms) then return false end
                persona=p:on_pointer(e);add(persona.haptic);publish(e.at_ms);return true
            elseif e.type=='Interaction' then pointer_down=false;a:on_interaction(e.at_ms);publish(e.at_ms)
            elseif e.type=='IgnoreTouch' then p:ignore_until(e.at_ms)
            else a:on_event(e);publish(e.at_ms) end
        end,
        tick=function(now)
            persona=p:tick(now);add(persona.haptic);local t=a:tick(now);add(t.haptic);publish(now)
            local best
            for _,h in ipairs(haptics) do if not best or priority(h)>priority(best) then best=h end end
            haptics={};if best then haptic.play(best) end
            if t.command then fox.command(t.command) end
        end
    }
end}
