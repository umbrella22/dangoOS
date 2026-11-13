do
  local led = {}
  local lm = {}
  function led.new(...)
    exact(2, ...)
    local pin, count = ...
    int(pin, 38, 38)
    int(count, 1, 6)
    local s = snapshot(16)
    local o = open("led_strip", "right_interaction", lm)
    o.count = count
    o.generation = s.led_generation
    o.frame = {}
    for i = 1, 18 do
      o.frame[i] = 0
    end
    return o
  end
  function lm:set_pixel(...)
    exact(4, ...)
    live(self, "led_strip")
    local i, r, g, b = ...
    int(i, 0, self.count - 1)
    int(r, 0, 255)
    int(g, 0, 255)
    int(b, 0, 255)
    self.frame[i * 3 + 1] = r
    self.frame[i * 3 + 2] = g
    self.frame[i * 3 + 3] = b
  end
  function lm:set_pixel_hsv(...)
    exact(4, ...)
    live(self, "led_strip")
    local i, h, s, v = ...
    int(i, 0, self.count - 1)
    int(h, 0, 359)
    int(s, 0, 255)
    int(v, 0, 255)
    local c = v * s / 255
    local x = c * (1 - math.abs((h / 60) % 2 - 1))
    local m = v - c
    local r, g, b
    if h < 60 then
      r, g, b = c, x, 0
    elseif h < 120 then
      r, g, b = x, c, 0
    elseif h < 180 then
      r, g, b = 0, c, x
    elseif h < 240 then
      r, g, b = 0, x, c
    elseif h < 300 then
      r, g, b = x, 0, c
    else
      r, g, b = c, 0, x
    end
    self:set_pixel(i, math.floor(r + m), math.floor(g + m), math.floor(b + m))
  end
  function lm:refresh(...)
    exact(0, ...)
    live(self, "led_strip")
    local s = snapshot(16)
    check(s.led_generation == self.generation, "LED board changed")
    host.request(1, string.char(table.unpack(self.frame)))
  end
  function lm:clear(...)
    exact(0, ...)
    live(self, "led_strip")
    for i = 1, 18 do
      self.frame[i] = 0
    end
    self:refresh()
  end
  function lm:close(...)
    exact(0, ...)
    if not self._open then
      return
    end
    check(type(self) == "table" and self._kind == "led_strip", "invalid LED handle")
    -- Weak leases are cleared before Lua runs table finalizers. Do not use
    -- live() here, and do not release a newer handle opened by another finalizer.
    if leases.led_strip == nil or leases.led_strip == self then
      host.request(2)
    end
    close(self)
  end
  modules.led_strip = led
end
