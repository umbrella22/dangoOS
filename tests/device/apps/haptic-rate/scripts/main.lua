-- Finite request stimulus for later logic-analyzer measurement, not a timing oracle.
local delay = require("delay")
local haptic = require("haptic")
print("[N1HR] BEGIN: wait 5s, then 20 thinking requests at nominal 40ms intervals")
print("[N1HR] thinking preset: 1x30ms, 35% duty; API requests are NOT actual pattern starts")
delay.delay_ms(5000)
for i = 1, 20 do
  print("[N1HR] REQUEST thinking " .. i .. "/20; request receipt only")
  haptic.play("thinking")
  delay.delay_ms(40)
end
print("[N1HR] REQUESTS END: wait 1s for pending work; requests may coalesce")
delay.delay_ms(1000)
print("[N1HR] END: script completion does NOT prove the 150ms pattern-start physical limit")
print("[N1HR] Measure actual motor waveform start spacing with a logic analyzer separately")
