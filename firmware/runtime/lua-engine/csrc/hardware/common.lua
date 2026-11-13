-- Snapshot adapters. Public names follow esp-claw 345073de; unsupported bus,
-- temperature/status and arbitrary GPIO operations raise explicit errors.
local host = ...
local int, finite = host.integer, host.finite
local modules = {}
local leases = setmetatable({}, { __mode = "v" })
local function check(ok, message)
  if not ok then
    error(message, 3)
  end
end
local function exact(n, ...)
  check(select("#", ...) == n, "wrong number of arguments")
end
local function options(t, allowed)
  check(type(t) == "table", "expected options table")
  for k in pairs(t) do
    check(allowed[k], "unsupported option: " .. tostring(k))
  end
end
local function snapshot(bit, valid)
  local s = host.snapshot()
  check(s.available & bit ~= 0, "hardware unavailable")
  if valid then
    check(s.valid & valid == valid, "sample unavailable")
  end
  return s
end
local function open(kind, name, methods)
  check(not leases[kind], "device already open")
  local o = setmetatable({ _kind = kind, _name = name, _open = true }, {
    __index = methods,
    __gc = function(h)
      pcall(methods.close, h)
    end,
    __close = function(h)
      methods.close(h)
    end,
  })
  leases[kind] = o
  return o
end
local function live(o, kind)
  check(type(o) == "table" and o._open and o._kind == kind and leases[kind] == o, "closed or invalid handle")
end
local function close(o)
  o._open = false
  if leases[o._kind] == o then
    leases[o._kind] = nil
  end
end
local function xyz(v)
  return { x = v[1], y = v[2], z = v[3] }
end
