do
  local button = {}
  local buttons = {}
  local dispatching = false
  local INTERACTION_CLICK_GAP_MS = 400
  local ONBOARD_CLICK_GAP_MS = 180
  local events = {
    press_down = true,
    press_up = true,
    press_repeat = true,
    press_repeat_done = true,
    single_click = true,
    double_click = true,
    multiple_click = true,
    long_press_start = true,
    long_press_hold = true,
    long_press_up = true,
    press_end = true,
  }
  local function bvalid(h)
    check(type(h) == "table" and buttons[h.pin] == h and not h.closed, "closed or invalid button handle")
  end
  local function elapsed(a, b)
    return (a - b) & 0xffffffff
  end
  -- AI/BOOT have an independent edge ring; slot generation changes must not
  -- invalidate their handles. Interaction keys retain the original domain.
  local function domain(s, onboard)
    if not onboard then return s end
    return {
      available = s.onboard_ready ~= 0 and 8 or 0,
      valid = s.onboard_ready ~= 0 and 64 or 0,
      pins = {7, 61}, pressed = s.onboard_pressed, generation = 0,
      sequence = s.onboard_sequence, edges = s.onboard_edges,
    }
  end
  function button.new(...)
    local n = select("#", ...)
    check(n >= 1 and n <= 4, "wrong number of arguments")
    local pin, active, long, short = ...
    int(pin, 0, 63)
    active = active == nil and 0 or int(active, 0, 1)
    long = long == nil and 0 or int(long, 0, 65535)
    short = short == nil and 0 or int(short, 0, 65535)
    local onboard = pin == 7 or pin == 61
    -- Upstream uses uint16_t thresholds; zero selects driver defaults.
    if long == 0 then
      long = 1500
    end
    if short == 0 then
      -- Touch keys need a more forgiving gap; AI/BOOT match native buttons.
      short = onboard and ONBOARD_CLICK_GAP_MS or INTERACTION_CLICK_GAP_MS
    end
    local s = domain(host.snapshot(), onboard)
    if s.available & 8 == 0 then
      return nil, onboard and "onboard buttons unavailable" or "interaction board unavailable"
    end
    local key
    for i = 1, 2 do
      if s.pins[i] == pin then
        key = i
      end
    end
    if not key then
      return nil, "GPIO is not an active interaction key"
    end
    if buttons[pin] then
      return nil, "button already open"
    end
    if s.valid & 64 == 0 then
      return nil, "button sample unavailable"
    end
    local h = {
      pin = pin,
      onboard = onboard,
      key = key,
      active = active,
      long = long,
      short = short,
      generation = s.generation,
      sequence = s.sequence,
      callbacks = {},
      pressed = s.pressed[key] ~= 0,
      count = 0,
    }
    -- Opening while pressed must not invent its down timestamp.
    h.unknown_down = h.pressed
    buttons[pin] = h
    return h
  end
  function button.close(...)
    exact(1, ...)
    local h = ...
    if type(h) == "table" and h.closed then
      return true
    end
    bvalid(h)
    buttons[h.pin] = nil
    h.closed = true
    h.callbacks = {}
    return true
  end
  function button.on(...)
    exact(3, ...)
    local h, event, callback = ...
    bvalid(h)
    if not events[event] or type(callback) ~= "function" then
      return nil, "invalid button event or callback"
    end
    h.callbacks[event] = callback
    return true
  end
  function button.off(...)
    local n = select("#", ...)
    check(n == 1 or n == 2, "wrong number of arguments")
    local h, event = ...
    bvalid(h)
    if event == nil then
      h.callbacks = {}
    elseif events[event] then
      h.callbacks[event] = nil
    else
      return nil, "unknown event"
    end
    return true
  end
  function button.get_key_level(...)
    exact(1, ...)
    local h = ...
    bvalid(h)
    local s = domain(host.snapshot(), h.onboard)
    if s.available & 8 == 0 or s.generation ~= h.generation or s.valid & 64 == 0 then
      return nil, "button unavailable or board changed"
    end
    return s.pressed[h.key] ~= 0 and h.active or 1 - h.active
  end
  function button.dispatch(...)
    exact(0, ...)
    check(not dispatching, "button.dispatch is not reentrant")
    dispatching = true
    local function run()
      local snapshot = host.snapshot()
      local now = host.now() & 0xffffffff
      local pending = {}
      local function emit(h, event, duration)
        local callback = h.callbacks[event]
        if callback then
          pending[#pending + 1] =
            { h, callback, { handle = h, event = event, repeat_count = h.count, pressed_time_ms = duration or 0 } }
        end
      end
      local function end_click(h)
        if h.count == 1 then
          emit(h, "single_click")
        elseif h.count == 2 then
          emit(h, "double_click")
        elseif h.count > 2 then
          emit(h, "multiple_click")
        end
        if h.count > 1 then
          emit(h, "press_repeat_done")
        end
        if h.count > 0 then
          emit(h, "press_end")
        end
        h.count = 0
        h.released_at = nil
      end
      local function timers(h, at)
        if h.pressed and h.down_at then
          local duration = elapsed(at, h.down_at)
          if duration >= h.long and not h.long_started then
            h.long_started = true
            emit(h, "long_press_start", duration)
          end
        -- The next down must precede this release-to-down limit strictly.
        elseif h.released_at and elapsed(at, h.released_at) >= h.short then
          end_click(h)
        end
      end
      for _, h in pairs(buttons) do
        local s = domain(snapshot, h.onboard)
        if s.available & 8 == 0 or s.generation ~= h.generation or s.valid & 64 == 0 then
          h.callbacks = {}
          h.closed = true
          buttons[h.pin] = nil
        else
          local delta = elapsed(s.sequence, h.sequence)
          check(delta <= 16, "button event queue overflow")
          for offset = 1, delta do
            local seq = (h.sequence + offset) & 0xffffffff
            local edge = s.edges[seq % 16 + 1]
            check(edge.sequence == seq, "button event queue overwritten")
            if edge.key == h.key then
              timers(h, edge.at)
              if edge.pressed ~= 0 then
                h.pressed = true
                h.down_at = edge.at
                h.long_started = false
                h.unknown_down = false
                emit(h, "press_down")
                if h.count > 0 then
                  emit(h, "press_repeat")
                end
              else
                local duration = h.down_at and elapsed(edge.at, h.down_at) or 0
                h.pressed = false
                emit(h, "press_up", duration)
                if h.long_started then
                  emit(h, "long_press_up", duration)
                  emit(h, "press_end", duration)
                  h.count = 0
                  h.released_at = nil
                elseif not h.unknown_down then
                  h.count = h.count + 1
                  h.released_at = edge.at
                end
                h.down_at = nil
                h.unknown_down = false
              end
            end
          end
          h.sequence = s.sequence
          timers(h, now)
          if h.pressed and h.long_started then
            emit(h, "long_press_hold", elapsed(now, h.down_at))
          end
        end
      end
      local count = 0
      -- State is committed before callbacks; closing handles inside a callback
      -- cancels remaining callbacks for that handle. New handles wait until next dispatch.
      for _, event in ipairs(pending) do
        local h, callback, info = table.unpack(event)
        if not h.closed and h.callbacks[info.event] == callback then
          callback(info)
          count = count + 1
        end
      end
      return count
    end
    local ok, result = pcall(run)
    dispatching = false
    if not ok then
      error(result, 0)
    end
    return result
  end
  modules.button = button
end
