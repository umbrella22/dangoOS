local native=require('_lvgl')
local json=require('json')
local methods={}
local callbacks={}
local next_callback=0
local function rpc(op,args,bytes)
    native.request(op,json.encode(args or {}),bytes or '')
    local response,blob=native.reply()
    local result=json.decode(response)
    for _,key in ipairs(result.retired or {}) do callbacks[key]=nil end
    return result.values,blob
end
local function call(op,args,bytes)
    local values=rpc(op,args,bytes)
    return table.unpack(values)
end
local function handle(id) return native.handle(id,methods) end
local function id(obj) return native.id(obj) end
local lvgl={PANEL_IF_IO=0,PANEL_IF_RGB=1,PANEL_IF_MIPI_DSI=2}
function lvgl.init(...) local a={...};return call('init',{type(a[1])=='table' and a[1] or a[6] or {}}) end
function lvgl.deinit() local result=call('deinit');callbacks={};return result end
function lvgl.create_screen()return handle(call('create',{'screen',0,{}}))end
function lvgl.screen()return handle(call('screen'))end
for _,kind in ipairs({'object','container','label','button','canvas','image','msgbox'}) do
    lvgl[kind]=function(parent,opts)return handle(call('create',{kind,parent and id(parent) or 0,opts or {}}))end
end
for _,name in ipairs({'set_size','set_pos','get_size','get_pos','align','is_valid','set_style','set_flex','set_scroll','set_text','load','clean','delete','close','close_async','add_flag','clear_flag','move_foreground','fill_bg','set_px','get_px','set_pivot','set_rotation'}) do
    methods[name]=function(self,...)return call(name,{id(self),...})end
end
function methods:set_rgb565_data(bytes,order)return call('set_rgb565_data',{id(self),order or 'le'},bytes)end
function methods:set_raw_rgb565(w,h,bytes,opts)return call('set_raw_rgb565',{id(self),w,h,opts or {}},bytes)end
function methods:add_footer_button(text)return handle(call('add_footer_button',{id(self),text}))end
function methods:on(event,callback)
    assert(type(callback)=='function','event callback must be a function')
    next_callback=next_callback+1
    call('on',{id(self),event,next_callback})
    callbacks[next_callback]={fn=callback,obj=self,event=event}
    return next_callback
end
function methods:off(selector)
    local count=call('off',{id(self),selector})
    for key,v in pairs(callbacks) do if v.obj==self and (selector==nil or selector==key or selector==v.event) then callbacks[key]=nil end end
    return count
end
function lvgl.process_events(timeout)
    local ids=rpc('events',{math.max(0,timeout or 0)})
    local count=0
    for _,key in ipairs(ids) do
        local c=callbacks[key]
        if c and c.obj:is_valid() then
            local ok,err=pcall(c.fn)
            if not ok then print('lvgl callback failed: '..tostring(err)) end
            count=count+1
        end
    end
    if timeout and timeout>0 then require('delay').delay_ms(timeout) end
    return count
end
function lvgl.run(opts)
    local count=0
    while true do count=count+lvgl.process_events(opts and opts.period_ms or 200) end
end
return lvgl
