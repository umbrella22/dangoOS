-- Hello Push: the smallest pushable app. Edit, push again with
-- `scripts/device/console.py push examples/apps/hello --run`, and the
-- running copy restarts with the new files.
local delay = require("delay")
local display = require("display")
local palette = require("palette")
local storage = require("storage")

-- Count launches in the data directory (/data on the device) with the
-- standard io library.
local COUNT_FILE = storage.join_path(storage.get_root_dir(), "hello_push.count")
local count = 0
local f = io.open(COUNT_FILE, "r")
if f then
  count = tonumber(f:read("l")) or 0
  f:close()
end
count = count + 1
f = assert(io.open(COUNT_FILE, "w"))
f:write(tostring(count), "\n")
f:close()
print("hello_push: launch " .. count)

local canvas = display.open()
local info = canvas:info()
local w, h = info.width, info.height
local r = 24
local x, y, dx, dy = w // 2, h // 2, 5, 3

while true do
  x, y = x + dx, y + dy
  if x < r or x > w - r then dx = -dx end
  if y < r or y > h - r then dy = -dy end
  canvas:begin({ clear = palette.background })
  canvas:fill_circle(x, y, r, palette.ball)
  canvas:text(150, 200, "Hello Push #" .. count, { color = palette.text, font_size = 24 })
  canvas:present()
  delay.delay_ms(33)
end
