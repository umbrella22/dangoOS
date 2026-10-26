-- Temporary on-device N1 acceptance probe. No bus access, arbitrary GPIO,
-- persistent files, shutdown or reboot. All output requests are bounded.
local delay = require("delay")
local imu = require("imu")
local magnetometer = require("magnetometer")
local fuel = require("fuel_gauge")
local fuel_alias = require("lib_fuel_gauge")
local haptic = require("haptic")
local led = require("led_strip")
local button = require("button")
local totals = { PASS = 0, FAIL = 0, SKIP = 0 }
local cfg = type(args) == "table" and args or {}
local sensors, keys = {}, {}
local strip
local callback_counts = {}
local function report(kind, name, detail)
  totals[kind] = totals[kind] + 1
  print("[N1] " .. kind .. " " .. name .. (detail and (" | " .. tostring(detail)) or ""))
end
local function integer(value, low, high)
  return type(value) == "number" and value == math.floor(value) and value >= low and value <= high
end
local function finite(value)
  return type(value) == "number" and value == value and value ~= math.huge and value ~= -math.huge
end
local function vector(v, raw)
  assert(type(v) == "table", "missing XYZ vector")
  for _, axis in ipairs({ "x", "y", "z" }) do
    assert(finite(v[axis]), "invalid axis " .. axis)
    if raw then
      assert(integer(v[axis], -32768, 32767), "axis outside signed raw range")
    end
  end
end
local function test(name, fn)
  local ok, detail = xpcall(fn, debug.traceback)
  report(ok and "PASS" or "FAIL", name, detail)
  return ok
end
local function track(handle)
  sensors[#sensors + 1] = handle
  return handle
end
local function ready_read(handle)
  for attempt = 1, 10 do
    local ok, sample = pcall(handle.read, handle)
    if ok then
      return sample
    end
    if not tostring(sample):find("sample unavailable", 1, true) then
      error(sample)
    end
    if attempt == 10 then
      error("sample unavailable after bounded 1s startup retry")
    end
    delay.delay_ms(100)
  end
end
local function cleanup()
  for _, entry in ipairs(keys) do
    local ok, err = pcall(function()
      assert(button.off(entry.handle))
      assert(button.close(entry.handle))
    end)
    if not ok then
      report("FAIL", "button cleanup GPIO" .. entry.pin, err)
    end
  end
  if strip then
    print(
      "[N1] LED CLEANUP BEGIN expected: turn both endpoint pixels off, then release strip; confirm darkness independently"
    )
    -- Close still runs if clear fails after a board is removed.
    local cleared, clear_err = pcall(strip.clear, strip)
    local closed, close_err = pcall(strip.close, strip)
    if not cleared then
      report("FAIL", "LED cleanup clear", clear_err)
    end
    report(closed and "PASS" or "FAIL", "LED cleanup close", close_err)
    print(
      "[N1] LED CLEANUP END clear_accepted="
        .. tostring(cleared)
        .. " close_accepted="
        .. tostring(closed)
        .. "; physical darkness not measured"
    )
    strip = nil
  end
  for _, handle in ipairs(sensors) do
    local ok, err = pcall(handle.close, handle)
    if not ok then
      report("FAIL", "sensor cleanup", err)
    end
  end
end
local function run()
  print("[N1] BEGIN n1_contract_probe; API receipts only; physical output timing is not measured")
  test("IMU complete raw snapshot", function()
    local sensor = track(imu.new({ device = "imu_sensor" }))
    local sample = ready_read(sensor)
    vector(sample.accel, true)
    vector(sample.gyro, true)
    assert(integer(sample.sens_time, 0, 0xffffff), "invalid hardware sensor counter")
    assert(integer(sample.status, 0, 255), "invalid BMI270 status byte")
    local temperature = sensor:read_temperature()
    assert(integer(temperature, -32768, 32767), "invalid raw temperature")
    return string.format(
      "accel=%d,%d,%d gyro=%d,%d,%d temp_raw=%d sens_time=%d status=0x%02X",
      sample.accel.x,
      sample.accel.y,
      sample.accel.z,
      sample.gyro.x,
      sample.gyro.y,
      sample.gyro.z,
      temperature,
      sample.sens_time,
      sample.status
    )
  end)
  local magnetic_sensor
  test("magnetometer snapshot/status", function()
    magnetic_sensor = track(magnetometer.new({ device = "magnetometer_sensor" }))
    local sample = ready_read(magnetic_sensor)
    vector(sample.magnetic)
    vector(sample.raw_magnetic)
    assert(integer(sample.status, 0, 255), "invalid BMM150 status byte")
    assert(sample.temperature == nil, "BMM150 must omit temperature")
    assert(magnetic_sensor:chip_id() == 0x32)
    assert(sample.proximity == "away" or sample.proximity == "near" or sample.proximity == "contact")
    return string.format(
      "uT=%.3f,%.3f,%.3f status=0x%02X proximity=%s; status-byte receipt, not independent register measurement",
      sample.magnetic.x,
      sample.magnetic.y,
      sample.magnetic.z,
      sample.status,
      sample.proximity
    )
  end)
  if magnetic_sensor then
    test("BMM150 temperature explicitly unsupported", function()
      local ok, err = pcall(magnetic_sensor.read_temperature, magnetic_sensor)
      assert(not ok and tostring(err):find("BMM150 has no temperature read", 1, true), tostring(err))
      return err
    end)
    test("magnetometer calibration roundtrip", function()
      local before = magnetic_sensor:calibration_get()
      local roundtrip = magnetic_sensor:calibration_set(before)
      assert(roundtrip.calibrated and not roundtrip.collecting)
      for i = 1, 3 do
        assert(roundtrip.hard_iron[i] == before.hard_iron[i])
        for j = 1, 3 do
          assert(roundtrip.soft_iron[i][j] == before.soft_iron[i][j])
        end
      end
      magnetic_sensor:calibration_set({ hard_iron = { 1, 2, 3 }, soft_iron = { { 2, 0, 0 }, { 0, 3, 0 }, { 0, 0, 4 } } })
      local sample = ready_read(magnetic_sensor)
      for i, axis in ipairs({ "x", "y", "z" }) do
        local expected = (sample.raw_magnetic[axis] - i) * (i + 1)
        assert(math.abs(sample.magnetic[axis] - expected) < 0.00001, "calibration arithmetic mismatch")
      end
      if before.calibrated then
        magnetic_sensor:calibration_set(before)
      else
        magnetic_sensor:calibration_clear()
      end
      return "get->set preserves coefficients; nonidentity correction checked against same-read raw vector; local calibration restored"
    end)
  else
    report("SKIP", "magnetometer temperature/calibration", "no handle after failed initialization")
  end
  test("fuel_gauge alias/read/charging", function()
    assert(fuel == fuel_alias, "require aliases must resolve to same module table")
    local gauge = track(fuel_alias.new({ borrow = true, port = 0, chip = "bq27220" }))
    local sample = ready_read(gauge)
    assert(gauge:chip() == "bq27220" and gauge:address() == 0x55)
    assert(sample.chip == "bq27220" and integer(sample.soc, 0, 100))
    assert(integer(sample.voltage_mv, 0, 65535) and integer(sample.current_ma, -32768, 32767))
    assert(
      type(sample.charging) == "boolean" and sample.charging == (sample.current_ma > 0),
      "charging/current inconsistency"
    )
    assert(integer(gauge:read_soc(), 0, 100) and integer(gauge:read_voltage_mv(), 0, 65535))
    assert(integer(gauge:read_current_ma(), -32768, 32767))
    return string.format(
      "soc=%d voltage_mv=%d current_ma=%d charging=%s (current>0 convention)",
      sample.soc,
      sample.voltage_mv,
      sample.current_ma,
      tostring(sample.charging)
    )
  end)
  test("haptic bounded request accepted", function()
    haptic.play("thinking") -- existing preset: one short 30ms burst
    delay.delay_ms(200) -- exceed existing 150ms pattern-start spacing
    haptic.play("happy") -- existing preset: one short 60ms burst
    return "two finite presets accepted; physical vibration amplitude/timing not measured"
  end)
  local led_ok, handle = pcall(led.new, 38, 6)
  if led_ok then
    strip = handle -- record before any subsequent operation can fail
    test("right interaction LED staged/refresh", function()
      strip:clear()
      strip:set_pixel(0, 16, 0, 0)
      strip:set_pixel_hsv(5, 120, 255, 16)
      strip:refresh()
      print(
        "[N1] LED EXPECT pixel0=low red RGB(16,0,0), pixel5=low green RGB(0,16,0), middle four off; retained until cleanup"
      )
      return "six-pixel API accepted and retained; physical color not measured; cleanup will clear/close"
    end)
  elseif tostring(handle):find("hardware unavailable", 1, true) then
    report("PASS", "LED unavailable branch", handle)
    report("SKIP", "LED physical exercise", "no supported active right interaction board")
  else
    report("FAIL", "LED open", handle)
  end
  local unavailable, rejected_gpio = 0, 0
  for _, pin in ipairs({ 11, 10, 13, 12 }) do
    local ok, key, err = pcall(button.new, pin, 0, 0, 0)
    if ok and key then
      local entry = { pin = pin, handle = key }
      keys[#keys + 1] = entry
      callback_counts[pin] = 0
      test("button register GPIO" .. pin, function()
        local level, level_err = button.get_key_level(key)
        assert(level == 0 or level == 1, tostring(level_err))
        for _, event in ipairs({
          "press_down",
          "press_up",
          "single_click",
          "double_click",
          "long_press_start",
          "long_press_up",
        }) do
          local subscribed_event = event
          assert(button.on(key, subscribed_event, function(info)
            assert(info.handle == key and info.event == subscribed_event)
            assert(integer(info.pressed_time_ms, 0, 0xffffffff))
            callback_counts[pin] = callback_counts[pin] + 1
            print(
              string.format(
                "[N1] EVENT GPIO%d %s repeat=%d pressed_ms=%d",
                pin,
                info.event,
                info.repeat_count,
                info.pressed_time_ms
              )
            )
          end))
        end
        return "initial_level=" .. level
      end)
    elseif ok and tostring(err):find("interaction board unavailable", 1, true) then
      unavailable = unavailable + 1
    elseif ok and tostring(err):find("GPIO is not an active interaction key", 1, true) then
      rejected_gpio = rejected_gpio + 1
    else
      report("FAIL", "button open GPIO" .. pin, ok and err or key)
    end
  end
  if unavailable == 4 then
    report("PASS", "button unavailable branch", "all four board key GPIOs returned nil + interaction board unavailable")
    report("SKIP", "button callback observation", "no interaction board; no events claimed")
  elseif #keys == 2 and rejected_gpio == 2 then
    report("PASS", "button active-slot selection", "two active keys opened; other slot GPIOs explicitly rejected")
  else
    report(
      "FAIL",
      "button active-slot selection",
      "opened=" .. #keys .. " unavailable=" .. unavailable .. " rejected_other_slot=" .. rejected_gpio
    )
  end
  if #keys > 0 then
    local duration_ms = cfg.duration_ms == nil and 60000 or cfg.duration_ms
    assert(finite(duration_ms) and duration_ms >= 1, "duration_ms must be a finite positive number")
    duration_ms = math.min(120000, math.floor(duration_ms))
    print("[N1] WINDOW BEGIN nominal_ms=" .. duration_ms .. "; tap/double-tap/hold BOTH active keys now")
    print(
      "[N1] WINDOW LED EXPECT opposite strip endpoints: low red pixel0 and low green pixel5; middle four off; API acceptance is not visual confirmation"
    )
    local dispatched, waited_ms, polls = 0, 0, 0
    while waited_ms < duration_ms do
      dispatched = dispatched + button.dispatch()
      local sleep_ms = math.min(20, duration_ms - waited_ms)
      delay.delay_ms(sleep_ms)
      waited_ms = waited_ms + sleep_ms
      polls = polls + 1
    end
    print(
      "[N1] WINDOW END before cleanup: red/green endpoints should still be visible if LED API succeeded; cleanup follows"
    )
    report(
      "PASS",
      "button dispatch finite window",
      "nominal_ms=" .. duration_ms .. " polls=" .. polls .. " callbacks=" .. dispatched
    )
    for _, entry in ipairs(keys) do
      local count = callback_counts[entry.pin]
      report(
        count > 0 and "PASS" or "SKIP",
        "button observed GPIO" .. entry.pin,
        count > 0 and ("callback events=" .. count)
          or "no human key activity observed; callback behavior not physically verified"
      )
    end
  else
    assert(button.dispatch() == 0)
  end
end
local ok, err = xpcall(run, debug.traceback)
if not ok then
  report("FAIL", "unexpected probe error", err)
end
cleanup()
print(string.format("[N1] SUMMARY PASS=%d FAIL=%d SKIP=%d", totals.PASS, totals.FAIL, totals.SKIP))
print("[N1] END n1_contract_probe; resources released; physical checks require independent observation")
if totals.FAIL > 0 then
  error("N1 contract probe recorded failures; see tagged logs")
end
