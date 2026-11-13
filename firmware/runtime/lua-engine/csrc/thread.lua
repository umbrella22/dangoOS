-- Independent worker states are scheduled by the owning VM group.
local native=require('_thread')
local json=require('json')
local function call(op,args,bytes)
    native.request(op,json.encode(args or {}),bytes or '')
    local response,blob=native.reply()
    local result=json.decode(response)
    if result.error then error(result.error,2) end
    if result.binary then return blob end
    return table.unpack(result.values,1,result.count)
end
local thread={sync={}}
local function integer(v) return type(v)=='number' and v%1==0 end
local function job(op,path,args,opts)
    assert(type(path)=='string','path must be a string')
    assert(args==nil or type(args)=='table','args must be a table or nil')
    assert(opts==nil or type(opts)=='table','opts must be a table or nil')
    opts=opts or {}
    assert(opts.name==nil or type(opts.name)=='string' and #opts.name>0 and #opts.name<=31,'invalid job name')
    assert(opts.timeout_ms==nil or integer(opts.timeout_ms) and opts.timeout_ms>=0,'invalid timeout_ms')
    assert(opts.replace==nil or type(opts.replace)=='boolean','replace must be boolean')
    assert(opts.exclusive==nil or type(opts.exclusive)=='string','exclusive must be a string')
    return call(op,{path,args or {},opts})
end
function thread.start(...)return job('start',...)end
function thread.run(...)return job('run',...)end
function thread.list(status)return call('list',{status or 'all'})end
function thread.get(id)return call('get',{id})end
function thread.stop(id,wait)return call('stop',{id,wait or 0})end
for _,name in ipairs({'queue_create','queue_recv','queue_delete','sem_create','sem_give','sem_take','sem_delete','lock_create','lock','unlock','lock_delete'}) do
    thread.sync[name]=function(key,...)
        assert(type(key)=='string' and #key>0 and #key<=31,'name length must be 1..31')
        assert(key:match('^[%w_.%-]+$'),'invalid character in sync name')
        local params={...}
        if name=='queue_recv' or name=='sem_take' or name=='lock' then
            assert(params[1]==nil or integer(params[1]) and params[1]>=0 and params[1]<=4294967295,'invalid timeout_ms')
        elseif name=='queue_create' or name=='sem_create' then
            assert(params[1]==nil or type(params[1])=='table','options must be a table')
            local o=params[1] or {}
            local function bound(v,lo,hi) return integer(v) and v>=lo and v<=hi end
            if name=='queue_create' then
                assert(o.depth==nil or bound(o.depth,1,32),'invalid depth')
                assert(o.item_size==nil or bound(o.item_size,1,4096),'invalid item_size')
            else
                assert(o.max==nil or bound(o.max,1,255),'invalid max')
                assert(o.initial==nil or bound(o.initial,0,o.max or 1),'invalid initial')
            end
        end
        return call(name,{key,...})
    end
end
function thread.sync.queue_send(name,bytes,timeout)
    assert(type(name)=='string' and #name>0 and #name<=31,'name length must be 1..31')
    assert(name:match('^[%w_.%-]+$'),'invalid character in sync name')
    assert(type(bytes)=='string','queue value must be a string')
    assert(timeout==nil or integer(timeout) and timeout>=0 and timeout<=4294967295,'invalid timeout_ms')
    return call('queue_send',{name,timeout or 0},bytes)
end
return thread
