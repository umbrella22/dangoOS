assert(PARENT_SECRET == nil)
assert(args.nested.value == 42 and args.flag == true)
for _,module in ipairs({'io','os','storage','image','lvgl','display','package','debug','imu'}) do
    assert(not pcall(require,module), module)
end
assert(io == nil and os == nil and package == nil and debug == nil and loadfile == nil)
local ok,err=require('thread').start('/tmp/forbidden.lua',{})
assert(ok==nil and err:find('nested workers',1,true))
local thread=require('thread')
assert(thread.sync.lock('owner_lock',1000))
assert(thread.sync.queue_send('ready','ready\0binary',1000))
setmetatable({}, {__gc=function() while true do end end})
while true do require('delay').delay_ms(100) end
