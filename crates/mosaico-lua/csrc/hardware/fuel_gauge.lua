do
  local gauge = {}
  local gm = {}
  function gauge.new(...)
    exact(1, ...)
    local cfg = ...
    options(cfg, { device = true, chip = true, borrow = true, port = true, addr = true })
    check(cfg.device == nil or cfg.device == "fuel_gauge", "unavailable fuel gauge")
    check(cfg.chip == nil or cfg.chip == "bq27220", "unsupported fuel gauge chip")
    check(cfg.borrow == nil or cfg.borrow == true, "only board snapshot supported")
    if cfg.port ~= nil then
      check(int(cfg.port, 0, 0) == 0, "unsupported bus")
    end
    if cfg.addr ~= nil then
      int(cfg.addr, 0x55, 0x55)
    end
    snapshot(4)
    return open("fuel_gauge", "fuel_gauge", gm)
  end
  function gm:read(...)
    exact(0, ...)
    live(self, "fuel_gauge")
    local s = snapshot(4, 32)
    return {
      chip = "bq27220",
      voltage_mv = s.voltage_mv,
      current_ma = s.current_ma,
      soc = s.soc,
      charging = s.charging ~= 0,
    }
  end
  function gm:read_voltage_mv(...)
    exact(0, ...)
    return self:read().voltage_mv
  end
  function gm:read_current_ma(...)
    exact(0, ...)
    return self:read().current_ma
  end
  function gm:read_soc(...)
    exact(0, ...)
    return self:read().soc
  end
  function gm:chip(...)
    exact(0, ...)
    live(self, "fuel_gauge")
    return "bq27220"
  end
  function gm:address(...)
    exact(0, ...)
    live(self, "fuel_gauge")
    return 0x55
  end
  function gm:close(...)
    exact(0, ...)
    close(self)
  end
  function gauge.supported_chips(...)
    exact(0, ...)
    return { "bq27220" }
  end
  modules.fuel_gauge = gauge
  modules.lib_fuel_gauge = gauge
end
