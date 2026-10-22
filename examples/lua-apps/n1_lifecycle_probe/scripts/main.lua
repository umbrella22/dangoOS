-- Temporary manual lifecycle probe; all hardware access uses public N1 APIs.
local delay = require("delay")
local led = require("led_strip")
local button = require("button")
local haptic = require("haptic")
local cfg = type(args) == "table" and args or {}
local totals = { PASS = 0, FAIL = 0, SKIP = 0 }
local strip, old_strip
local keys, old_keys = {}, {}
local haptic_requests = 0
local events = 0
local observed = { [11] = {}, [10] = {} }
local seen_down = false
local seen_up = false
local old_led_detached_rejected = false
local old_led_reinsert_rejected = false
local old_buttons_rejected = false
local restored = false
local function short(value)
  return tostring(value):gsub("[\r\n]+", " "):sub(1, 85)
end
local function log(kind, label, detail)
  if totals[kind] ~= nil then
    totals[kind] = totals[kind] + 1
  end
  print("[N1L] " .. kind .. " " .. label .. (detail and (" | " .. short(detail)) or ""))
end
local function finite(n)
  return type(n) == "number" and n == n and n ~= math.huge and n ~= -math.huge
end
local function release_keys(entries)
  for _, entry in ipairs(entries) do
    local ok, err = pcall(button.off, entry.handle)
    -- dispatch invalidates detached handles automatically. off() then rejects,
    -- whereas close() is idempotent; this is an expected detached state.
    if not ok then
      log("INFO", "off GPIO" .. entry.pin, err)
    end
    local closed, result = pcall(button.close, entry.handle)
    if not closed or result ~= true then
      log("FAIL", "close GPIO" .. entry.pin, result)
    end
  end
end
local function release_strip(handle, label, clear_first)
  if not handle then
    return
  end
  if clear_first then
    local cleared, err = pcall(handle.clear, handle)
    if not cleared then
      log("INFO", label .. " clear unavailable", err)
    end
  end
  local closed, err = pcall(handle.close, handle)
  log(closed and "PASS" or "FAIL", label .. " close", err)
end
local function cleanup()
  log("INFO", "cleanup begin", "clear/release outputs; physical darkness needs observation")
  release_keys(keys)
  release_keys(old_keys)
  release_strip(strip, "current LED", true)
  if old_strip ~= strip then
    release_strip(old_strip, "old LED", false)
  end
  strip, old_strip = nil, nil
  log("INFO", "cleanup end", "normal probe exits; pending preset cannot be measured here")
end
local function new_strip(recovered)
  local handle = led.new(38, 6)
  -- Caller retains ownership before refresh can fail.
  strip = handle
  handle:clear()
  if recovered then
    handle:set_pixel(0, 0, 16, 0)
    handle:set_pixel(5, 0, 16, 0)
  else
    handle:set_pixel(0, 16, 0, 0)
    handle:set_pixel(5, 0, 16, 0)
  end
  handle:refresh()
  log(
    "PASS",
    recovered and "LED reopened" or "LED initial API",
    recovered and "both endpoints GREEN" or "endpoint0 RED endpoint5 GREEN"
  )
  log("SKIP", "LED physical measurement", "API acceptance only; ask observer to confirm colors")
end
local function new_keys()
  for _, pin in ipairs({ 11, 10 }) do
    local handle, err = button.new(pin, 0, 0, 0)
    if not handle then
      error("button.new GPIO" .. pin .. ": " .. tostring(err))
    end
    keys[#keys + 1] = { pin = pin, handle = handle }
    for _, event_name in ipairs({ "single_click", "double_click", "long_press_start", "long_press_up" }) do
      local subscribed = event_name
      assert(button.on(handle, subscribed, function(info)
        assert(info.handle == handle and info.event == subscribed, "callback identity mismatch")
        events = events + 1
        observed[pin][subscribed] = (observed[pin][subscribed] or 0) + 1
        log("EVENT", "GPIO" .. pin .. " " .. subscribed, "pressed_ms=" .. info.pressed_time_ms)
        if subscribed == "single_click" then
          if haptic_requests < 10 then
            haptic_requests = haptic_requests + 1
            local ok, request_err = pcall(haptic.play, "error")
            log(
              ok and "PASS" or "FAIL",
              "haptic.error request " .. haptic_requests,
              ok and "API accepted; feel motor independently" or request_err
            )
          else
            log("INFO", "haptic cap reached", "10 requests maximum; no more motor requests")
          end
        end
      end))
    end
    local level, level_err = button.get_key_level(handle)
    assert(level == 0 or level == 1, level_err)
    log("PASS", "key opened GPIO" .. pin, "level=" .. level)
  end
end
local function key_unavailable(entry)
  local ok, level, err = pcall(button.get_key_level, entry.handle)
  if not ok then
    local text = tostring(level)
    if text:find("closed or invalid button handle", 1, true) then
      return true, text
    end
    return false, text
  end
  if level == nil and tostring(err):find("button unavailable or board changed", 1, true) then
    return true, err
  end
  return false, level
end
local function old_refresh_expected(label, expected)
  local ok, err = pcall(old_strip.refresh, old_strip)
  if ok then
    log("FAIL", label, "old handle unexpectedly refreshed")
    return false
  end
  if tostring(err):find(expected, 1, true) then
    log("PASS", label, err)
    return true
  end
  log("FAIL", label, "unexpected rejection: " .. tostring(err))
  return false
end
local function run()
  local duration_ms = cfg.duration_ms == nil and 90000 or cfg.duration_ms
  assert(finite(duration_ms) and duration_ms >= 1, "duration_ms must be finite and positive")
  duration_ms = math.min(120000, math.floor(duration_ms))
  assert(cfg.mode == nil or cfg.mode == "normal" or cfg.mode == "error", "unknown mode")
  log("INFO", "BEGIN", "nominal window_ms=" .. duration_ms .. " mode=" .. tostring(cfg.mode or "normal"))
  new_strip(false)
  if cfg.mode == "error" then
    log("INFO", "EXPECTED ERROR MODE", "red/green held 3s; no explicit cleanup; VM must release LED")
    delay.delay_ms(3000)
    log("INFO", "EXPECTED ERROR NOW", "N1_EXPECTED_FAILURE; root should rerun normal probe after launcher")
    error("N1_EXPECTED_FAILURE")
  end
  new_keys()
  log("INFO", "ACTION", "Hold device; single-click BOTH right keys; feel haptic.error (max 10)")
  log("INFO", "GESTURES", "Also double-click LEFT and long-hold RIGHT (>1.5s), release; no long_hold spam")
  log("INFO", "ACTION", "Then UNPLUG right board only; keep USB connected; wait for DETACHED log")
  log("INFO", "ACTION", "Then REINSERT right board; wait for both GREEN endpoints + REOPENED log")
  old_strip = strip
  local waited_ms = 0
  while waited_ms < duration_ms do
    if not seen_down then
      local absent1 = key_unavailable(keys[1])
      local absent2 = key_unavailable(keys[2])
      if absent1 and absent2 then
        seen_down = true
        old_keys = keys
        keys = {}
        log("INFO", "DETACHED observed", "both old right-key handles unavailable")
        old_buttons_rejected = true
        log("PASS", "old buttons rejected", "get_key_level unavailable after detach")
        old_led_detached_rejected = old_refresh_expected("old LED detached reject", "hardware unavailable")
        local ok, result = pcall(button.dispatch)
        log(ok and "PASS" or "FAIL", "dispatch after detach", ok and ("callbacks=" .. result) or result)
        log("INFO", "ACTION", "Reinsert right interaction board now; old LED remains open for generation check")
      end
    elseif not restored then
      -- Probe only a temporary right-key handle. Opening proves this slot is
      -- sampled again without closing the old LED before its generation test.
      local ok, candidate, err = pcall(button.new, 11, 0, 0, 0)
      if ok and candidate then
        keys[#keys + 1] = { pin = 11, handle = candidate }
        seen_up = true
        log("INFO", "REINSERT detected", "right key snapshot available")
        old_led_reinsert_rejected = old_refresh_expected("old LED reinsert reject", "LED board changed")
        for _, entry in ipairs(old_keys) do
          local unavailable = key_unavailable(entry)
          old_buttons_rejected = old_buttons_rejected and unavailable
          log(
            unavailable and "PASS" or "FAIL",
            "old button reinsert GPIO" .. entry.pin,
            unavailable and "old handle still rejected" or "old handle unexpectedly revived"
          )
        end
        assert(button.close(candidate))
        keys = {}
        release_keys(old_keys)
        old_keys = {}
        release_strip(old_strip, "old LED before reopen", false)
        old_strip = nil
        strip = nil
        new_strip(true)
        new_keys()
        restored = true
        log("PASS", "REOPENED", "new LED + two new keys; both endpoints GREEN")
        log("INFO", "ACTION", "Click BOTH reopened keys; confirm green endpoints and motor by observation")
      elseif not ok then
        error(candidate)
      elseif not tostring(err):find("unavailable", 1, true) and not tostring(err):find("not an active", 1, true) then
        error(err)
      end
    end
    if not seen_down or restored then
      local ok, result = pcall(button.dispatch)
      if not ok then
        log("FAIL", "dispatch unavailable/error", result)
        -- Continue only until the next state sample detects detach.
      end
    end
    local sleep_ms = math.min(20, duration_ms - waited_ms)
    delay.delay_ms(sleep_ms)
    waited_ms = waited_ms + sleep_ms
  end
  log("INFO", "WINDOW END", "callbacks=" .. events .. " haptic_requests=" .. haptic_requests)
  if not seen_down then
    log("SKIP", "detach/reinsert lifecycle", "no manual detach observed")
  end
  if seen_down and not seen_up then
    log("SKIP", "reinsert/reopen lifecycle", "no sampled right board returned in window")
  end
  for _, pin in ipairs({ 11, 10 }) do
    for _, event_name in ipairs({ "single_click", "double_click", "long_press_start", "long_press_up" }) do
      local count = observed[pin][event_name] or 0
      log(
        count > 0 and "PASS" or "SKIP",
        "GPIO" .. pin .. " " .. event_name,
        count > 0 and ("observed=" .. count) or "manual gesture not observed"
      )
    end
  end
  if haptic_requests == 0 then
    log("SKIP", "haptic request exercise", "no single_click; motor not requested")
  end
  if
    seen_down
    and seen_up
    and restored
    and old_led_detached_rejected
    and old_led_reinsert_rejected
    and old_buttons_rejected
  then
    log("PASS", "lifecycle sequence", "detach rejection + reinsert generation rejection + fresh handles")
  end
  log("SKIP", "motor physical measurement", "human tactile observation required; requests are not pulse measurements")
end
-- Error mode intentionally bypasses active cleanup. The expected error reaches
-- the runner, which drops the VM and must release the LED lease there.
if cfg.mode == "error" then
  run()
else
  local ok, err = xpcall(run, debug.traceback)
  if not ok then
    log("FAIL", "unexpected error", err)
  end
  cleanup()
  log("INFO", "SUMMARY", "PASS=" .. totals.PASS .. " FAIL=" .. totals.FAIL .. " SKIP=" .. totals.SKIP)
  log("INFO", "END", "API receipts only; physical color/touch/motor need observer confirmation")
  if totals.FAIL > 0 then
    error("N1 lifecycle probe recorded failures")
  end
end
