//! Original assets and effects; deliberately not a claimed mod adaptation.
use horizon_mod_sdk::{Mod, Registration, chat, game, render, storage, ui};

struct Lab {
    enabled: bool,
    dimension: Option<String>,
}
impl Lab {
    fn settings(&self) {
        ui::set_settings(&[ui::Widget::Button(ui::ButtonWidget {
            id: 1,
            label: format!("Effects: {}", if self.enabled { "ON" } else { "OFF" }),
        })]);
    }
    fn clear(&self) {
        let _ = render::set_colored_lights(&[]);
        let _ = render::set_emissive_materials(&[], false);
        let _ = render::set_post_effects(&[]);
    }
}
impl Mod for Lab {
    fn init() -> Result<(Self, Registration), String> {
        let enabled = storage::get("enabled").as_deref() == Some(b"true");
        let lab = Lab {
            enabled,
            dimension: None,
        };
        lab.settings();
        Ok((lab, Registration::new().command("effect_lab").on_tick()))
    }
    fn tick(&mut self, tick: u64) {
        let Some(p) = game::get_player() else {
            self.dimension = None;
            return;
        };
        if !self.enabled {
            return;
        }
        if self.dimension.as_deref() != Some(&p.dimension) {
            let _ = render::set_emissive_materials(
                &[render::EmissiveMaterial {
                    base: "minecraft:block/stone".into(),
                    overlay: "effect_lab:emissive/stone".into(),
                }],
                true,
            );
            let _ = render::set_post_effects(&[render::PostEffect::Bloom(render::Bloom {
                strength: 0.15,
                threshold: 0.8,
                radius: 2.0,
            })]);
            self.dimension = Some(p.dimension.clone());
        }
        let _ = render::set_colored_lights(&[render::ColoredLight {
            x: p.x,
            y: p.y + 0.5,
            z: p.z,
            color: 0x4488FF,
            radius: 5.0,
            intensity: 0.4,
        }]);
        if tick.is_multiple_of(4) {
            let yaw = f64::from(p.yaw).to_radians();
            render::emit_sprite_particles(&[render::SpriteParticle {
                sprite: "effect_lab:particle/spark".into(),
                particle: render::ConfiguredParticle {
                    particle: render::WorldParticle {
                        kind: render::AmbientParticle::EndRod,
                        x: p.x - yaw.sin() * 2.0,
                        y: p.y + 1.0,
                        z: p.z + yaw.cos() * 2.0,
                        velocity_x: 0.0,
                        velocity_y: 0.025,
                        velocity_z: 0.0,
                    },
                    color: 0xFFFFFF,
                    alpha: 0.7,
                    size: 0.08,
                    lifetime: 30,
                    gravity: 0.0,
                    friction: 0.99,
                    collision: false,
                    full_bright: true,
                },
            }]);
        }
    }
    fn command(&mut self, _name: &str, args: &str) -> Result<Option<String>, String> {
        self.enabled = match args.trim() {
            "on" => true,
            "off" => false,
            _ => !self.enabled,
        };
        storage::set("enabled", if self.enabled { b"true" } else { b"false" })?;
        self.dimension = None;
        if !self.enabled {
            self.clear();
        }
        self.settings();
        chat::show(if self.enabled {
            "Effect Lab on"
        } else {
            "Effect Lab off"
        });
        Ok(None)
    }
    fn ui_select(&mut self, id: u32, _row: u32) {
        if id == 1 {
            let _ = self.command("effect_lab", "");
        }
    }
    fn ui_settings_opened(&mut self) {
        self.settings();
    }
}
horizon_mod_sdk::export_mod!(Lab);
