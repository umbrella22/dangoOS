local delay = require("delay")
local haptic = require("haptic")
print("[N1H] BEGIN: in 5s, four Angry requests; each 2x160ms, 90% duty")
delay.delay_ms(5000)
for i=1,4 do
  print("[N1H] REQUEST Angry " .. i)
  haptic.play("angry")
  delay.delay_ms(3000)
end
print("[N1H] END: API requests complete; physical tactile evidence separate")
