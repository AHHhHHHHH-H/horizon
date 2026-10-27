//! Example Horizon mod (`docs/mods.md`, "Shaders from a mod"): a post pass of
//! its own. `assets/vignette/shaders/vignette.wgsl` darkens the edges of the
//! world's picture and reddens them as the player's health falls; the mod
//! sets its two numbers every tick. `/vignette <0 to 1>` sets how strong it
//! is, and 0 removes the pass, so it costs nothing while off.
use horizon_mod_sdk::{
    Mod, Registration, game,
    render::{self, ShaderKind},
};

struct Vignette {
    /// The registered pass, while it is on.
    shader: Option<u32>,
    strength: f32,
    /// The values last sent, so an unchanged frame costs no call.
    sent: [f32; 2],
}

/// Registers the pass: its numbers are `params.strength` and `params.low_health`.
fn register() -> Result<u32, String> {
    let params = ["strength".to_owned(), "low_health".to_owned()];
    render::register_shader("vignette", ShaderKind::PostAfterTranslucent, &params)
}

impl Mod for Vignette {
    fn init() -> Result<(Self, Registration), String> {
        // A shader the game refuses stops only the pass: the mod runs on.
        let state = Vignette {
            shader: register().inspect_err(|e| horizon_mod_sdk::warn(e)).ok(),
            strength: 0.6,
            sent: [-1.0; 2],
        };
        Ok((state, Registration::new().command("vignette").on_tick()))
    }

    fn tick(&mut self, _tick: u64) {
        let Some(shader) = self.shader else {
            return;
        };
        let low = game::get_player().map_or(0.0, |p| (1.0 - p.health / 20.0).clamp(0.0, 1.0));
        let values = [self.strength, low];
        if values != self.sent {
            render::set_shader_params(shader, &values);
            self.sent = values;
        }
    }

    fn command(&mut self, _name: &str, args: &str) -> Result<Option<String>, String> {
        let strength: f32 = args
            .trim()
            .parse()
            .ok()
            .filter(|s: &f32| (0.0..=1.0).contains(s))
            .ok_or("Give a strength from 0 to 1")?;
        self.strength = strength;
        match (self.shader, strength > 0.0) {
            (Some(shader), false) => {
                render::remove_shader(shader);
                self.shader = None;
            }
            (None, true) => {
                self.shader = Some(register()?);
                self.sent = [-1.0; 2];
            }
            _ => {}
        }
        Ok(Some(format!("Vignette at {strength}")))
    }
}

horizon_mod_sdk::export_mod!(Vignette);
