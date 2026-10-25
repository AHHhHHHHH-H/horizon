# Write it in Lua

A mod's code can be Lua 5.4 instead of Rust. The Lua interpreter is built into the mod with your script, so a Lua mod is an `.hmod` like any other: it runs in the same sandbox, asks for the same capabilities, has the same time budget per call and goes through the same checks to be published. You write Lua; `cargo horizon-mod` builds the rest.

Lua mods are **experimental**: the `horizon` table may still change in any release ([API stability](https://github.com/ludovic111/horizon/blob/main/docs/api/stability.md)). The mod API under it is the same one Rust mods use.

## Start one

You need the same tools as for a Rust mod ([Make a mod](make-a-mod.md)): Rust with the `wasm32-unknown-unknown` target, and `cargo horizon-mod`.

```sh
cargo horizon-mod new hello-lua --template lua
cd hello-lua
cargo horizon-mod build
```

The first build compiles the interpreter, which takes about 30 seconds; after that only your script changes. The mod is about 240 KB (the Rust coords-hud is 25 KB).

| File | What it is |
| --- | --- |
| `main.lua` | your mod |
| `horizon-mod.toml` | the manifest ([Manifest](mod-manifest.md)): `api = "0.1"`, your capabilities |
| `Cargo.toml` | builds the interpreter with your script; its features list your capabilities |
| `src/lib.rs` | one line, `horizon_mod_lua::lua_mod!("main.lua");` |

## The shape of a mod

```lua
local greeted = 0

horizon.command("hello")

function on_command(name, args)
  greeted = greeted + 1
  horizon.chat.show(string.format("Hello %s! (%d so far)", args, greeted))
end
```

- **The script runs once, when the mod loads.** Register your commands there with `horizon.command(name)`, and keep your state in local variables.
- **Hooks are global functions.** The game calls the ones your script defines:

| Hook | When |
| --- | --- |
| `on_tick(tick)` | every game tick, 20 a second |
| `on_frame(seconds)` | every frame drawn |
| `on_command(name, args)` | the player typed one of your commands: return a string to answer, or `error("text")` to answer in red |
| `on_chat(text)` | a chat line reached the player |

## What the script can call

Each capability your manifest lists is a table of `horizon`:

| Capability | Lua |
| --- | --- |
| always | `horizon.command(name)`, `horizon.log.info(text)` (also `debug`, `warn`, `error`), `print(...)` to the mod's log |
| `game-read` | `horizon.game.player()`: `{x, y, z, yaw, pitch, health, food, dimension}` or nil; `horizon.game.clock()`: `{game_time, day_time}` or nil |
| `chat` | `horizon.chat.show(text)` |
| `hud` | `horizon.hud.set_panel(corner, lines)`: `"top-left"`, `"top-right"`, `"bottom-left"` or `"bottom-right"`, and lines as strings or `{text = "...", color = 0xRRGGBB}`; `horizon.hud.clear()` |
| `storage` | `horizon.storage.get(key)`: a string or nil; `horizon.storage.set(key, value)`: `true`, or `nil` and why; `horizon.storage.delete(key)` |

List the same capabilities in two places: in `horizon-mod.toml`, and as features of `horizon-mod-lua` in `Cargo.toml` (`features = ["game-read", "hud", "chat"]`). Only the tables you list as features are built into the mod. A feature the manifest doesn't list makes the game (and `cargo horizon-mod check`) refuse the mod; a capability in the manifest but not in the features leaves its table out, and the script fails with `attempt to index a nil value`.

## Lua's standard library

You get base Lua, `math`, `coroutine`, and parts of `string` and `table`:

- **`string`:** `len`, `sub`, `lower`, `upper`, `reverse`, `rep`, and `format` (`%d %i %s %f %x %X %o %c %e %g %q %%`, with flags, width and precision).
- **`table`:** `insert`, `remove`, `concat`, `pack`, `unpack`.

There is no `io` or `os`: a mod can't touch files, the network or the clock, in Lua as in Rust. Patterns (`string.find`, `string.gsub`, `string.match`) aren't there yet. `math.random` works, but it is seeded the same way every time, because the sandbox has no source of randomness.

## A complete example

[`coords-hud-lua`](https://github.com/ludovic111/horizon/blob/main/crates/horizon-mod-sdk/examples/coords-hud-lua/main.lua) is the Coordinates HUD example rewritten in Lua, line for line. It shows your position, facing and the time of day in a corner, `/where` puts them in chat, and `/coords` hides the panel. The game's tests run the same checks on the Rust and the Lua version.

It costs about 60 µs per tick with a new panel every tick, against about 25 to 60 µs for the Rust one. That is well under a tick's budget, but a Lua mod doing heavy work every tick is slower than the same mod in Rust.

## Test and publish

A Lua mod is tested, checked, signed and published exactly like a Rust one: see [Test a mod](test-a-mod.md) and [Publish a mod](publish-a-mod.md).
