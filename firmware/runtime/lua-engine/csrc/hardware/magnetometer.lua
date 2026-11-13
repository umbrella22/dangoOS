do
  local mag = {}
  local mm = {}
  local function identity()
    return { { 1, 0, 0 }, { 0, 1, 0 }, { 0, 0, 1 } }
  end
  local function calcopy(o)
    local c =
      { hard_iron = {}, soft_iron = {}, calibrated = o.calibrated, collecting = o.collecting, sample_count = o.count }
    for i = 1, 3 do
      c.hard_iron[i] = o.hard[i]
      c.soft_iron[i] = { table.unpack(o.soft[i]) }
    end
    return c
  end
  function mag.new(...)
    exact(1, ...)
    local cfg = ...
    options(cfg, { device = true })
    check(cfg.device == "magnetometer_sensor", "unavailable board magnetometer")
    snapshot(2)
    local o = open("magnetometer", cfg.device, mm)
    o.hard = { 0, 0, 0 }
    o.soft = identity()
    o.calibrated = false
    o.collecting = false
    o.count = 0
    return o
  end
  function mm:read(...)
    exact(0, ...)
    live(self, "magnetometer")
    local s = snapshot(2, 16)
    local out = {}
    for i = 1, 3 do
      local n = 0
      for j = 1, 3 do
        n = n + self.soft[i][j] * (s.magnetic[j] - self.hard[j])
      end
      out[i] = n
    end
    return {
      magnetic = xyz(out),
      raw_magnetic = xyz(s.magnetic),
      status = s.magnet_status,
      calibrated = self.calibrated,
      proximity = ({ "away", "near", "contact" })[s.magnet_state + 1],
    }
  end
  function mm:read_temperature(...)
    exact(0, ...)
    live(self, "magnetometer")
    error("BMM150 has no temperature read")
  end
  function mm:read_int_status(...)
    exact(0, ...)
    live(self, "magnetometer")
    error("interrupt reads are not supported by snapshot adapter")
  end
  function mm:chip_id(...)
    exact(0, ...)
    live(self, "magnetometer")
    snapshot(2)
    return 0x32
  end
  function mm:name(...)
    exact(0, ...)
    live(self, "magnetometer")
    return self._name
  end
  function mm:close(...)
    exact(0, ...)
    close(self)
  end
  function mm:calibration_get(...)
    exact(0, ...)
    live(self, "magnetometer")
    return calcopy(self)
  end
  function mm:calibration_clear(...)
    exact(0, ...)
    live(self, "magnetometer")
    self.hard = { 0, 0, 0 }
    self.soft = identity()
    self.calibrated = false
    self.collecting = false
    self.count = 0
    self.min = nil
    self.max = nil
    return calcopy(self)
  end
  function mm:calibration_reset(...)
    exact(0, ...)
    self:calibration_clear()
    self.collecting = true
  end
  function mm:calibration_add_sample(...)
    check(select("#", ...) <= 1, "wrong number of arguments")
    live(self, "magnetometer")
    self.collecting = true
    local v = ...
    if v == nil then
      v = snapshot(2, 16).magnetic
    end
    check(type(v) == "table" and #v == 3, "expected three-number sample")
    local sample = { finite(v[1]), finite(v[2]), finite(v[3]) }
    if not self.min then
      self.min = { table.unpack(sample) }
      self.max = { table.unpack(sample) }
    end
    for i = 1, 3 do
      self.min[i] = math.min(self.min[i], sample[i])
      self.max[i] = math.max(self.max[i], sample[i])
    end
    self.count = self.count + 1
    return self.count
  end
  function mm:calibration_set(...)
    exact(1, ...)
    live(self, "magnetometer")
    local cfg = ...
    options(cfg, { hard_iron = true, soft_iron = true, calibrated = true, collecting = true, sample_count = true })
    check(
      type(cfg.hard_iron) == "table" and #cfg.hard_iron == 3 and type(cfg.soft_iron) == "table" and #cfg.soft_iron == 3,
      "invalid calibration"
    )
    local h, m = {}, {}
    for i = 1, 3 do
      h[i] = finite(cfg.hard_iron[i])
      check(type(cfg.soft_iron[i]) == "table" and #cfg.soft_iron[i] == 3, "invalid matrix row")
      m[i] = {}
      for j = 1, 3 do
        m[i][j] = finite(cfg.soft_iron[i][j])
      end
    end
    self.hard = h
    self.soft = m
    self.calibrated = true
    self.collecting = false
    return calcopy(self)
  end
  function mm:calibration_finish(...)
    exact(0, ...)
    live(self, "magnetometer")
    check(self.count >= 16, "at least 16 samples required")
    local h, m, r = {}, identity(), {}
    for i = 1, 3 do
      r[i] = (self.max[i] - self.min[i]) / 2
      check(r[i] > 0, "nonzero span required on every axis")
      h[i] = (self.max[i] + self.min[i]) / 2
    end
    local average = (r[1] + r[2] + r[3]) / 3
    for i = 1, 3 do
      m[i][i] = finite(average / r[i])
      finite(h[i])
    end
    return self:calibration_set({ hard_iron = h, soft_iron = m })
  end
  modules.magnetometer = mag
end
