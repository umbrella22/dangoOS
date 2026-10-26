do
  local imu = {}
  local im = {}
  function imu.new(...)
    exact(1, ...)
    local cfg = ...
    options(cfg, { device = true })
    check(cfg.device == "imu_sensor", "unavailable board IMU")
    snapshot(1)
    return open("imu", cfg.device, im)
  end
  function im:read(...)
    exact(0, ...)
    live(self, "imu")
    local s = snapshot(1, 11)
    local sample = { accel = xyz(s.accel), gyro = xyz(s.gyro) }
    sample.sens_time = s.sensor_time
    sample.status = s.imu_status
    return sample
  end
  function im:read_temperature(...)
    exact(0, ...)
    live(self, "imu")
    return snapshot(1, 4).temperature
  end
  function im:read_int_status(...)
    exact(0, ...)
    live(self, "imu")
    error("interrupt reads are not supported by snapshot adapter")
  end
  function im:name(...)
    exact(0, ...)
    live(self, "imu")
    return self._name
  end
  function im:close(...)
    exact(0, ...)
    close(self)
  end
  modules.imu = imu
end
