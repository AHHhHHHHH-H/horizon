-- Example Horizon mod in Lua (docs/mods.md, "Lua"): coordinates, facing and
-- the time of day in the top-left corner, /where to put them in chat and
-- /coords to hide or show the panel. The same mod as examples/coords-hud
-- in Rust, and the same test checks both.

local shown = true
-- The panel last sent, so an unchanged one costs no call.
local last = ""

horizon.command("where")
horizon.command("coords")

-- Direction.fromYRot: yaw 0 faces south, 90 west.
local FACING = { "South (+Z)", "West (-X)", "North (-Z)", "East (+X)" }
local function facing(yaw)
  return FACING[(math.floor(yaw / 90 + 0.5) & 3) + 1]
end

-- "Day 3, 14:05": day time 0 is 6:00 on day 1.
local function time_of_day(clock)
  local day = clock.day_time // 24000 + 1
  local ticks = clock.day_time % 24000
  local hours = (ticks // 1000 + 6) % 24
  local minutes = ticks % 1000 * 60 // 1000
  return string.format("Day %d, %02d:%02d", day, hours, minutes)
end

local function lines(p, clock)
  local out = {
    string.format("XYZ %.1f / %.1f / %.1f", p.x, p.y, p.z),
    "Facing " .. facing(p.yaw),
  }
  if clock then
    table.insert(out, time_of_day(clock))
  end
  return out
end

function on_tick(tick)
  local p = shown and horizon.game.player()
  if not p then
    if last ~= "" then
      horizon.hud.clear()
      last = ""
    end
    return
  end
  local now = lines(p, horizon.game.clock())
  local key = table.concat(now, "\n")
  if key ~= last then
    horizon.hud.set_panel("top-left", now)
    last = key
  end
end

function on_command(name, args)
  if name == "coords" then
    shown = not shown
    return "Coordinates panel " .. (shown and "shown" or "hidden")
  end
  local p = horizon.game.player()
  if not p then
    error("Not in a world")
  end
  horizon.chat.show(string.format("%.0f, %.0f, %.0f in %s",
    math.floor(p.x), math.floor(p.y), math.floor(p.z), p.dimension))
end
