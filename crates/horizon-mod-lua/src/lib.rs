//! Horizon mods in Lua (`docs/mods.md`, "Lua"): a Lua 5.4 interpreter
//! ([piccolo](https://github.com/kyren/piccolo), pure Rust) compiled into the
//! mod's WebAssembly component with the mod's script, so a Lua mod runs in
//! the same sandbox, with the same capabilities and time budgets, as a Rust
//! one. The mod's crate is one line:
//!
//! ```ignore
//! horizon_mod_lua::lua_mod!("main.lua");
//! ```
//!
//! The script registers its commands with `horizon.command(name)` and
//! defines the hooks it wants as globals: `on_tick(tick)`,
//! `on_frame(seconds)`, `on_command(name, args)` (a string returned is the
//! feedback, `error(text)` the red error) and `on_chat(text)`. The
//! interfaces are tables of `horizon`: `log` always, and `game`, `chat`,
//! `hud`, `storage` with this crate's feature of the same capability
//! (`game-read` for `game`), as the mod's `horizon-mod.toml` lists them.
#[cfg(feature = "chat")]
use horizon_mod_sdk::chat;
#[cfg(feature = "game-read")]
use horizon_mod_sdk::game;
#[cfg(feature = "hud")]
use horizon_mod_sdk::hud;
#[cfg(feature = "storage")]
use horizon_mod_sdk::storage;
use horizon_mod_sdk::{Registration, log};
use piccolo::{
    Callback, CallbackReturn, Closure, Context, Error, Executor, Lua, StashedExecutor, StaticError,
    Table, Value, Variadic,
};
use std::cell::RefCell;

mod stdlib;

/// Seeds for piccolo's hash tables and `math.random`: a mod's sandbox has
/// no entropy source, so they come from a fixed sequence (SplitMix64).
/// `getrandom` 0.3 takes it with `--cfg getrandom_backend="custom"`, which
/// `cargo horizon-mod build` passes.
#[cfg(target_arch = "wasm32")]
mod entropy {
    use std::cell::Cell;

    std::thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0x9E37_79B9_7F4A_7C15) };
    }

    fn fill(dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let z = STATE.with(|s| {
                let z = s.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
                s.set(z);
                z
            });
            let mut z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_le_bytes()[..chunk.len()]);
        }
    }

    fn fill02(dest: &mut [u8]) -> Result<(), getrandom02::Error> {
        fill(dest);
        Ok(())
    }
    getrandom02::register_custom_getrandom!(fill02);

    /// getrandom 0.3's custom backend.
    ///
    /// # Safety
    /// `dest` points to `len` writable bytes (getrandom's contract).
    #[unsafe(no_mangle)]
    unsafe extern "Rust" fn __getrandom_v03_custom(
        dest: *mut u8,
        len: usize,
    ) -> Result<(), getrandom03::Error> {
        // SAFETY: getrandom passes a buffer of `len` bytes it owns.
        fill(unsafe { std::slice::from_raw_parts_mut(dest, len) });
        Ok(())
    }
}

#[doc(hidden)]
pub use horizon_mod_sdk as __sdk;

std::thread_local! {
    /// What `horizon.command` registered while the script's chunk ran.
    static COMMANDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A hook's argument.
pub enum Arg {
    Integer(i64),
    Number(f64),
    Text(String),
}

/// A running Lua mod: the interpreter with the script loaded.
pub struct LuaMod {
    lua: Lua,
    /// One executor, restarted for each call (a new one per call costs a
    /// thread's allocation).
    executor: StashedExecutor,
}

/// A Lua error as the player reads it: the message alone for `error(text)`.
fn message(e: StaticError) -> String {
    match e {
        StaticError::Lua(e) => e.to_string(),
        StaticError::Runtime(e) => e.to_string(),
    }
}

impl LuaMod {
    /// Runs the script's chunk; the hooks it defined and the commands it
    /// registered make the [`Registration`].
    pub fn start(name: &str, source: &str) -> Result<(Self, Registration), String> {
        let mut lua = Lua::core();
        lua.try_enter(|ctx| {
            stdlib::install(ctx)?;
            install(ctx)
        })
        .map_err(message)?;
        COMMANDS.with(|c| c.borrow_mut().clear());
        let chunk = lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, Some(name), source.as_bytes())?;
                Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
            })
            .map_err(message)?;
        lua.execute::<()>(&chunk).map_err(message)?;
        let mut registration = Registration::new();
        lua.enter(|ctx| {
            let defined = |hook: &'static str| matches!(ctx.get_global(hook), Value::Function(_));
            registration.tick = defined("on_tick");
            registration.frame = defined("on_frame");
            registration.chat = defined("on_chat");
        });
        registration.commands = COMMANDS.with(|c| std::mem::take(&mut *c.borrow_mut()));
        let executor = lua.enter(|ctx| ctx.stash(Executor::new(ctx)));
        Ok((LuaMod { lua, executor }, registration))
    }

    /// Calls the global function `hook` with `args`: what it returned, as
    /// text (`None` for nothing or nil), or the error it raised. A hook the
    /// script didn't define returns `None`.
    pub fn call(&mut self, hook: &'static str, args: Vec<Arg>) -> Result<Option<String>, String> {
        let executor = &self.executor;
        let found = self
            .lua
            .try_enter(|ctx| {
                let Value::Function(f) = ctx.get_global(hook) else {
                    return Ok(false);
                };
                let args: Vec<Value> = args
                    .into_iter()
                    .map(|a| match a {
                        Arg::Integer(i) => Value::Integer(i),
                        Arg::Number(n) => Value::Number(n),
                        Arg::Text(s) => ctx.intern(s.as_bytes()).into(),
                    })
                    .collect();
                ctx.fetch(executor).restart(ctx, f, Variadic(args));
                Ok(true)
            })
            .map_err(message)?;
        if !found {
            return Ok(None);
        }
        self.lua.finish(executor);
        self.lua
            .try_enter(|ctx| {
                let value = ctx.fetch(executor).take_result::<Value>(ctx)??;
                Ok(match value {
                    Value::Nil => None,
                    Value::String(s) => Some(s.to_str_lossy().into_owned()),
                    other => Some(other.to_string()),
                })
            })
            .map_err(message)
    }

    /// A hook that returns nothing: an error goes to the mod's log.
    pub fn run(&mut self, hook: &'static str, args: Vec<Arg>) {
        if let Err(e) = self.call(hook, args) {
            log::write(log::Level::Error, &format!("{hook}: {e}"));
        }
    }
}

/// `fn(args) -> results` as a Lua function.
fn function<'gc>(
    ctx: Context<'gc>,
    f: impl Fn(Context<'gc>, Vec<Value<'gc>>) -> Result<Vec<Value<'gc>>, Error<'gc>> + 'static,
) -> Callback<'gc> {
    Callback::from_fn(&ctx, move |ctx, _, mut stack| {
        let args: Vec<Value> = stack.drain(..).collect();
        let out = f(ctx, args)?;
        stack.extend(out);
        Ok(CallbackReturn::Return)
    })
}

fn text<'gc>(ctx: Context<'gc>, s: &str) -> Value<'gc> {
    ctx.intern(s.as_bytes()).into()
}

/// The argument at `i` as text (numbers convert, as Lua's `tostring`).
fn arg_text(args: &[Value], i: usize, what: &str) -> Result<String, String> {
    match args.get(i).copied().unwrap_or(Value::Nil) {
        Value::String(s) => Ok(s.to_str_lossy().into_owned()),
        Value::Integer(n) => Ok(n.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(format!(
            "{what} must be a string, not {}",
            other.type_name()
        )),
    }
}

fn lua_error<'gc>(ctx: Context<'gc>, message: String) -> Error<'gc> {
    text(ctx, &message).into()
}

#[cfg(feature = "game-read")]
fn table<'gc>(
    ctx: Context<'gc>,
    fields: &[(&'static str, Value<'gc>)],
) -> Result<Table<'gc>, Error<'gc>> {
    let t = Table::new(&ctx);
    for &(k, v) in fields {
        t.set(ctx, k, v)?;
    }
    Ok(t)
}

/// The `horizon` table and `print` (to the mod's log).
fn install(ctx: Context<'_>) -> Result<(), Error<'_>> {
    let horizon = Table::new(&ctx);
    horizon.set(
        ctx,
        "command",
        function(ctx, |ctx, args| {
            let name = arg_text(&args, 0, "the command's name").map_err(|e| lua_error(ctx, e))?;
            COMMANDS.with(|c| c.borrow_mut().push(name));
            Ok(vec![])
        }),
    )?;
    let log_table = Table::new(&ctx);
    for (name, level) in [
        ("debug", log::Level::Debug),
        ("info", log::Level::Info),
        ("warn", log::Level::Warn),
        ("error", log::Level::Error),
    ] {
        log_table.set(
            ctx,
            name,
            function(ctx, move |ctx, args| {
                let message = arg_text(&args, 0, "the message").map_err(|e| lua_error(ctx, e))?;
                log::write(level, &message);
                Ok(vec![])
            }),
        )?;
    }
    horizon.set(ctx, "log", log_table)?;
    ctx.set_global(
        "print",
        function(ctx, |_, args| {
            let line: Vec<String> = args
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.to_str_lossy().into_owned(),
                    other => other.to_string(),
                })
                .collect();
            log::write(log::Level::Info, &line.join("\t"));
            Ok(vec![])
        }),
    )?;

    #[cfg(feature = "game-read")]
    {
        let game_table = Table::new(&ctx);
        game_table.set(
            ctx,
            "player",
            function(ctx, |ctx, _| {
                let Some(p) = game::get_player() else {
                    return Ok(vec![Value::Nil]);
                };
                let t = table(
                    ctx,
                    &[
                        ("x", Value::Number(p.x)),
                        ("y", Value::Number(p.y)),
                        ("z", Value::Number(p.z)),
                        ("yaw", Value::Number(p.yaw.into())),
                        ("pitch", Value::Number(p.pitch.into())),
                        ("health", Value::Number(p.health.into())),
                        ("food", Value::Integer(p.food.into())),
                        ("dimension", text(ctx, &p.dimension)),
                    ],
                )?;
                Ok(vec![t.into()])
            }),
        )?;
        game_table.set(
            ctx,
            "clock",
            function(ctx, |ctx, _| {
                let Some(c) = game::get_clock() else {
                    return Ok(vec![Value::Nil]);
                };
                let t = table(
                    ctx,
                    &[
                        ("game_time", Value::Integer(c.game_time as i64)),
                        ("day_time", Value::Integer(c.day_time as i64)),
                    ],
                )?;
                Ok(vec![t.into()])
            }),
        )?;
        horizon.set(ctx, "game", game_table)?;
    }

    #[cfg(feature = "chat")]
    {
        let chat_table = Table::new(&ctx);
        chat_table.set(
            ctx,
            "show",
            function(ctx, |ctx, args| {
                let line = arg_text(&args, 0, "the line").map_err(|e| lua_error(ctx, e))?;
                chat::show(&line);
                Ok(vec![])
            }),
        )?;
        horizon.set(ctx, "chat", chat_table)?;
    }

    #[cfg(feature = "hud")]
    {
        let hud_table = Table::new(&ctx);
        hud_table.set(
        ctx,
        "set_panel",
        function(ctx, |ctx, args| {
            let corner = match arg_text(&args, 0, "the corner").map_err(|e| lua_error(ctx, e))?.as_str() {
                "top-left" => hud::Corner::TopLeft,
                "top-right" => hud::Corner::TopRight,
                "bottom-left" => hud::Corner::BottomLeft,
                "bottom-right" => hud::Corner::BottomRight,
                other => {
                    return Err(lua_error(
                        ctx,
                        format!("no corner {other:?}: top-left, top-right, bottom-left or bottom-right"),
                    ));
                }
            };
            let Some(Value::Table(rows)) = args.get(1).copied() else {
                return Err(lua_error(ctx, "the lines must be a table".into()));
            };
            let mut lines = Vec::new();
            for i in 1..=rows.length() {
                let line = match rows.get(ctx, i) {
                    Value::Table(row) => hud::Line {
                        text: arg_text(&[row.get(ctx, "text")], 0, "a line's text")
                            .map_err(|e| lua_error(ctx, e))?,
                        color: row.get(ctx, "color").to_integer().unwrap_or(0xFFFFFF) as u32,
                    },
                    v => hud::Line {
                        text: arg_text(&[v], 0, "a line").map_err(|e| lua_error(ctx, e))?,
                        color: 0xFFFFFF,
                    },
                };
                lines.push(line);
            }
            hud::set_panel(corner, &lines);
            Ok(vec![])
        }),
    )?;
        hud_table.set(
            ctx,
            "clear",
            function(ctx, |_, _| {
                hud::clear();
                Ok(vec![])
            }),
        )?;
        horizon.set(ctx, "hud", hud_table)?;
    }

    #[cfg(feature = "storage")]
    {
        let storage_table = Table::new(&ctx);
        storage_table.set(
            ctx,
            "get",
            function(ctx, |ctx, args| {
                let key = arg_text(&args, 0, "the key").map_err(|e| lua_error(ctx, e))?;
                Ok(vec![match storage::get(&key) {
                    Some(bytes) => ctx.intern(&bytes).into(),
                    None => Value::Nil,
                }])
            }),
        )?;
        storage_table.set(
            ctx,
            "set",
            function(ctx, |ctx, args| {
                let key = arg_text(&args, 0, "the key").map_err(|e| lua_error(ctx, e))?;
                let value = match args.get(1).copied().unwrap_or(Value::Nil) {
                    Value::String(s) => s.as_bytes().to_vec(),
                    v => arg_text(&[v], 0, "the value")
                        .map_err(|e| lua_error(ctx, e))?
                        .into_bytes(),
                };
                // Lua's way: true, or nil and why.
                Ok(match storage::set(&key, &value) {
                    Ok(()) => vec![Value::Boolean(true)],
                    Err(e) => vec![Value::Nil, text(ctx, &e)],
                })
            }),
        )?;
        storage_table.set(
            ctx,
            "delete",
            function(ctx, |ctx, args| {
                let key = arg_text(&args, 0, "the key").map_err(|e| lua_error(ctx, e))?;
                storage::delete(&key);
                Ok(vec![])
            }),
        )?;
        horizon.set(ctx, "storage", storage_table)?;
    }
    ctx.set_global("horizon", horizon)?;
    Ok(())
}

/// A Lua mod: the crate's `src/lib.rs` is `horizon_mod_lua::lua_mod!("main.lua");`,
/// the script's path from the crate's folder.
#[macro_export]
macro_rules! lua_mod {
    ($script:literal) => {
        const _: () = {
            // The SDK's export macro names the SDK's crate.
            use $crate::__sdk as horizon_mod_sdk;
            struct LuaMod($crate::LuaMod);
            impl horizon_mod_sdk::Mod for LuaMod {
                fn init() -> Result<(Self, horizon_mod_sdk::Registration), String> {
                    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $script));
                    let (m, registration) = $crate::LuaMod::start($script, source)?;
                    Ok((LuaMod(m), registration))
                }
                fn tick(&mut self, tick: u64) {
                    self.0
                        .run("on_tick", vec![$crate::Arg::Integer(tick as i64)]);
                }
                fn frame(&mut self, seconds: f32) {
                    self.0
                        .run("on_frame", vec![$crate::Arg::Number(seconds.into())]);
                }
                fn command(&mut self, name: &str, args: &str) -> Result<Option<String>, String> {
                    self.0.call(
                        "on_command",
                        vec![
                            $crate::Arg::Text(name.into()),
                            $crate::Arg::Text(args.into()),
                        ],
                    )
                }
                fn chat(&mut self, text: &str) {
                    self.0.run("on_chat", vec![$crate::Arg::Text(text.into())]);
                }
            }
            horizon_mod_sdk::export_mod!(LuaMod);
        };
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loads `source` and calls its global `f`.
    fn run(source: &str) -> Result<Option<String>, String> {
        let (mut m, _) = LuaMod::start("test.lua", source)?;
        m.call("on_command", vec![])
    }

    #[test]
    fn the_script_makes_the_registration() {
        let (_, r) = LuaMod::start(
            "test.lua",
            r#"horizon.command("where") horizon.command("coords")
               function on_tick(t) end
               function on_command(name, args) end"#,
        )
        .unwrap();
        assert_eq!(r.commands, ["where", "coords"]);
        assert!(r.tick && !r.frame && !r.chat);
    }

    #[test]
    fn a_command_answers_or_errs() {
        assert_eq!(
            run("function on_command() return 'done' end"),
            Ok(Some("done".into()))
        );
        assert_eq!(run("function on_command() end"), Ok(None));
        assert_eq!(
            run("function on_command() error('Not in a world') end"),
            Err("Not in a world".into())
        );
        assert!(run("function on_command() return nil + 1 end").is_err());
        assert!(LuaMod::start("test.lua", "this is not lua").is_err());
    }

    #[test]
    fn string_format_as_c_prints() {
        let f = |args: &str| {
            run(&format!(
                "function on_command() return string.format({args}) end"
            ))
        };
        assert_eq!(
            f("'XYZ %.1f / %.1f', 12.5, -3.25"),
            Ok(Some("XYZ 12.5 / -3.2".into()))
        );
        assert_eq!(
            f("'Day %d, %02d:%02d', 2, 6, 5"),
            Ok(Some("Day 2, 06:05".into()))
        );
        assert_eq!(
            f("'[%5s|%-4d|%x|%X|%+d]', 'ab', 7, 255, 255, 3"),
            Ok(Some("[   ab|7   |ff|FF|+3]".into()))
        );
        assert_eq!(
            f("'%g %g %e', 0.0001, 100000, 1234.5"),
            Ok(Some("0.0001 100000 1.234500e+03".into()))
        );
        assert_eq!(f("'%.0f%%', 99.6"), Ok(Some("100%".into())));
        assert_eq!(f("'%s %s', 1.0, nil"), Ok(Some("1.0 nil".into())));
        assert!(
            f("'%d', 1.5")
                .unwrap_err()
                .contains("no integer representation")
        );
    }

    #[test]
    fn table_insert_remove_concat() {
        let script = r#"function on_command()
            local t = {}
            table.insert(t, "b")
            table.insert(t, "c")
            table.insert(t, 1, "a")
            local last = table.remove(t)
            return table.concat(t, ",") .. "|" .. last .. "|" .. string.rep("ab", 3, "-")
        end"#;
        assert_eq!(run(script), Ok(Some("a,b|c|ab-ab-ab".into())));
    }
}
