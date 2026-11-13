-- Board adaptation: six LEDs on GPIO38; capacitive keys right 11/10 or left
-- 13/12. This is not an upstream esp-claw example. Set args.outputs=true to
-- request light/motor feedback; default run only reads snapshots.
local delay = require("delay")
local display = require("display")
local imu = require("imu")
local magnetometer = require("magnetometer")
local fuel_gauge = require("lib_fuel_gauge")
local button = require("button")
local led_strip = require("led_strip")
local haptic = require("haptic")
local capabilities = require("capabilities")
local system = require("system")
local cfg = type(args) == "table" and args or {}
local screen, motion, magnet, gauge, key, strip
local lines = {}
local function line(text)
  print(text)
  lines[#lines + 1] = text
  if #lines > 12 then
    table.remove(lines, 1)
  end
end
local function attempt(label, fn)
  local ok, result = pcall(fn)
  if not ok then
    line(label .. ": " .. tostring(result))
  end
  return ok and result or nil
end
local function cleanup()
  if key then
    pcall(button.close, key)
  end
  for _, handle in pairs({ motion, magnet, gauge, strip, screen }) do
    pcall(handle.close, handle)
  end
end
local function diagnostics()
  local c = capabilities.get()
  line(string.format("API %d / Lua heap %d", c.api_version, c.limits.lua_heap_bytes))
  for _, name in ipairs({ "audio", "camera", "network" }) do
    local status = c.modules[name]
    line(string.format("%s: supported=%s available=%s", name, tostring(status.supported), tostring(status.available)))
    if status.supported then
      attempt(name, function()
        local s = require(name).status()
        line(name .. " state: " .. (s.state or s.wifi_state))
      end)
    end
  end
  local heap = system.heap
  for _, region in ipairs({ { "Internal", heap.caps.INTERNAL, 32768 }, { "PSRAM", heap.caps.SPIRAM, 262144 } }) do
    local h = heap.get_info(region[2])
    line(string.format("%s free=%s largest=%s", region[1], tostring(h.free_size), tostring(h.largest_free_block)))
    line("Sampled min: " .. tostring(h.sampled_minimum_free_size))
    if cfg.probe_heap == true then
      local ok, code = heap.probe(region[2], region[3], 128)
      line(string.format("%s probe %d/128: %s %s", region[1], region[3], tostring(ok), tostring(code or "")))
    end
  end
end
local function run()
  screen = display.open()
  diagnostics()
  motion = attempt("IMU", function()
    return imu.new({ device = "imu_sensor" })
  end)
  magnet = attempt("Magnet", function()
    return magnetometer.new({ device = "magnetometer_sensor" })
  end)
  gauge = attempt("Battery", function()
    return fuel_gauge.new({ borrow = true, port = 0, chip = "bq27220" })
  end)
  local err
  key, err = button.new(cfg.pin or 11)
  if not key then
    line("Key: " .. tostring(err))
  else
    button.on(key, "single_click", function()
      line("Key clicked")
      if cfg.outputs == true then
        attempt("Haptic", function()
          haptic.play("happy")
        end)
      end
    end)
  end
  if cfg.outputs == true then
    strip = attempt("LED", function()
      return led_strip.new(38, 6)
    end)
    if strip then
      strip:set_pixel_hsv(0, 120, 255, 32)
      strip:refresh()
    end
  end
  local ticks = math.max(1, math.floor((cfg.duration_ms or 30000) / 50))
  for i = 1, ticks do
    if i % 20 == 1 then
      if motion then
        attempt("IMU sample", function()
          local s = motion:read()
          line(string.format("IMU raw: %d %d %d", s.accel.x, s.accel.y, s.accel.z))
        end)
      end
      if magnet then
        attempt("Mag sample", function()
          local s = magnet:read()
          line(string.format("Mag: %.1f %.1f %.1f %s", s.magnetic.x, s.magnetic.y, s.magnetic.z, s.proximity))
        end)
      end
      if gauge then
        attempt("Battery sample", function()
          local s = gauge:read()
          line(string.format("Battery: %d%% %dmV %dmA", s.soc, s.voltage_mv, s.current_ma))
        end)
      end
    end
    button.dispatch()
    screen:begin({ clear = 0xff080809 })
    screen:text(16, 16, "Hardware Probe", { color = 0xffffffff, font_size = 28 })
    for n, text in ipairs(lines) do
      screen:text(16, 60 + (n - 1) * 30, text, { color = 0xff00ff00, font_size = 16 })
    end
    screen:present()
    delay.delay_ms(50)
  end
end
local ok, err = xpcall(run, debug.traceback)
cleanup()
if not ok then
  error(err)
end
