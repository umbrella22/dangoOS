do
  -- This board-specific extension has no official upstream haptic module.
  local haptic = {}
  local presets = {
    "panic",
    "angry",
    "surprised",
    "puzzled",
    "happy",
    "satisfied",
    "confused",
    "listening",
    "thinking",
    "error",
    "module_insert",
    "module_remove",
    "magnetic_contact",
  }
  function haptic.play(...)
    exact(1, ...)
    local name = ...
    check(type(name) == "string", "expected preset name")
    snapshot(32)
    for i, p in ipairs(presets) do
      if p == name then
        host.request(0, i - 1)
        return
      end
    end
    error("unknown haptic preset")
  end
  function haptic.presets(...)
    exact(0, ...)
    return { table.unpack(presets) }
  end
  modules.haptic = haptic
end
