-- {name}: a Horizon mod in Lua (https://playhorizon.dev/docs/mod-code).
-- Commands come from horizon.command; hooks are global functions:
-- on_tick(tick), on_frame(seconds), on_command(name, args), on_chat(text).

local greeted = 0

horizon.command("hello")

function on_command(name, args)
  greeted = greeted + 1
  horizon.chat.show(string.format("Hello %s! (%d so far)", args, greeted))
end
