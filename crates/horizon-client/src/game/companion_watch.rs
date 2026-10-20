//! Watch mode (docs/companion-speedrun.md): the owner watches one companion
//! play, as a spectator camera behind it.
//!
//! The window holds one level at a time, so the owner's view is what links
//! the companion to the levels: when the body goes through a portal (its own
//! portal process, as a server's player's), the owner goes through the same
//! portal from where the body stands, and the body comes along onto the exit.
//! A body that dies away from here and comes back in another level takes its
//! owner there too. The HUD shows what the planner feeds it: the route step,
//! the run timer and the perception level.
//!
//! `companion watch [name]`, `companion watch off` (the chat command and the
//! control socket), plus `companion strike [name] <id>`, a dev blow at any
//! tracked entity (dragon parts and crystals included) for tests.
use super::{
    Dim, Game,
    companion::{Body, companion_index},
    lan::LanHost,
    nether_portal::PLAYER_DELAY,
};
use horizon_client::{
    command::args::GameMode, companion::CompanionCommand, hud::HudBatch, lan::playerdata,
};
use horizon_core::{geometry::Vec3, position::BlockPos};
use serde_json::{Value, json};

/// The camera behind the body and above its feet.
const BEHIND: f64 = 4.0;
const ABOVE: f64 = 1.5;
/// How far the camera closes on where it should be each tick; past
/// `SNAP` it jumps there.
const EASE: f64 = 0.35;
const SNAP: f64 = 16.0;
/// A player's box: Player's standing dimensions.
const WIDTH: f64 = 0.6;
const HEIGHT: f64 = 1.8;

/// The owner watching a companion.
pub(super) struct Watch {
    pub name: String,
    /// The owner's game mode before, given back when Watch ends.
    mode: GameMode,
    /// The level the body was in last tick.
    dim: Dim,
    /// The run timer, in ticks.
    ticks: u64,
    /// What the planner says it is doing, and how much it sees.
    pub step: String,
    pub perception: String,
}

/// Where the body goes through: what its portal process decided.
enum Through {
    Nether(BlockPos),
    End,
}

impl Game<'_> {
    pub(super) fn watching(&self) -> bool {
        self.companions.watch.is_some()
    }

    /// Whether `b` is the companion its owner watches.
    pub(super) fn watched(&self, b: &Body) -> bool {
        self.companions
            .watch
            .as_ref()
            .is_some_and(|w| w.name.eq_ignore_ascii_case(&b.file.name))
    }

    /// `companion watch ...`.
    pub(super) fn companion_watch_words(
        &mut self,
        words: &[&str],
    ) -> std::result::Result<Value, String> {
        match words {
            ["off" | "stop"] => self.companion_unwatch(),
            ["status"] => Ok(self.watch_status()),
            ["step", text @ ..] => self.watch_set(|w| w.step = text.join(" ")),
            ["perception", text @ ..] => self.watch_set(|w| w.perception = text.join(" ")),
            ["timer", "reset"] => self.watch_set(|w| w.ticks = 0),
            [] | [_] => self.companion_watch(words.first().copied()),
            _ => Err("companion watch [name] | off | status | step TEXT | perception LABEL | timer reset".into()),
        }
    }

    fn watch_set(&mut self, f: impl FnOnce(&mut Watch)) -> std::result::Result<Value, String> {
        f(self.companions.watch.as_mut().ok_or("not watching")?);
        Ok(self.watch_status())
    }

    /// The owner becomes the camera on companion `name` (the first one).
    fn companion_watch(&mut self, name: Option<&str>) -> std::result::Result<Value, String> {
        if self.player.is_none() || self.travelling.is_some() {
            return Err("you are not in the world yet".into());
        }
        let host = self.lan.as_ref().ok_or("no companion here")?;
        let bi = companion_index(host, name).ok_or("no companion by that name")?;
        let b = &host.companions[bi];
        if b.dim != self.dim {
            return Err(format!("{} is in another dimension", b.file.name));
        }
        let name = b.file.name.clone();
        // Watching another one keeps the mode from before the first.
        let mode = self.companions.watch.take().map_or(self.mode, |w| w.mode);
        self.companions.watch = Some(Watch {
            name: name.clone(),
            mode,
            dim: self.dim,
            ticks: 0,
            step: String::new(),
            perception: String::new(),
        });
        self.set_mode(GameMode::Spectator);
        self.watch_camera(true);
        let mut v = self.watch_status();
        v["message"] = json!(format!("watching {name}"));
        Ok(v)
    }

    /// Watch ends: the owner stands where the body is (players do not
    /// collide), in the mode they had.
    fn companion_unwatch(&mut self) -> std::result::Result<Value, String> {
        let name = self
            .companions
            .watch
            .as_ref()
            .ok_or("not watching")?
            .name
            .clone();
        self.watch_end();
        Ok(json!({ "ok": true, "watching": null, "message": format!("stopped watching {name}") }))
    }

    pub(super) fn watch_end(&mut self) {
        let Some(w) = self.companions.watch.take() else {
            return;
        };
        let at = self.lan.as_ref().and_then(|host| {
            let b = &host.companions[companion_index(host, Some(&w.name))?];
            let g = host.hub.guest(b.conn)?;
            (b.dim == self.dim && !g.dead).then_some(g.position)
        });
        if let (Some(at), Some(p)) = (at, self.player.as_mut()) {
            p.position = Vec3::new(at[0], at[1], at[2]);
            p.velocity = Vec3::ZERO;
            p.fall_distance = 0.0;
        }
        self.set_mode(w.mode);
    }

    fn watch_status(&self) -> Value {
        let Some(w) = self.companions.watch.as_ref() else {
            return json!({ "ok": true, "watching": null });
        };
        json!({
            "ok": true,
            "watching": w.name,
            "step": w.step,
            "perception": w.perception,
            "timer_ticks": w.ticks,
            "hud": self.watch_lines(w),
            "camera": self.player.as_ref().map(|p| [p.position.x, p.position.y, p.position.z]),
        })
    }

    /// Per tick, after the hub's: the camera, the body's portals, and the
    /// owner after a body that came back elsewhere.
    pub(super) fn tick_companion_watch(&mut self) {
        let Some(w) = self.companions.watch.as_mut() else {
            return;
        };
        if self.player.is_none() || self.travelling.is_some() {
            return;
        }
        w.ticks += 1;
        let name = w.name.clone();
        if !self.spectator() {
            self.set_mode(GameMode::Spectator);
        }
        let Some(mut host) = self.lan.take() else {
            self.watch_end();
            return;
        };
        let Some(bi) = companion_index(&host, Some(&name)) else {
            self.lan = Some(host);
            self.watch_follow_parked(&name);
            return;
        };
        let mut b = host.companions.remove(bi);
        let through = self.watch_body(&mut host, &mut b);
        host.companions.insert(bi, b);
        self.lan = Some(host);
        self.watch_camera(false);
        if let Some(through) = through {
            self.watch_go_through(through);
        }
    }

    /// The watched body this tick: onto the owner's exit when it just came
    /// through with them, then its portal process.
    fn watch_body(&mut self, host: &mut LanHost, b: &mut Body) -> Option<Through> {
        let w = self.companions.watch.as_mut()?;
        if w.dim != self.dim && b.dim == self.dim {
            // Through the portal with its owner: on the exit itself (where the
            // owner went through from where it stood), holding there until told.
            w.dim = self.dim;
            if let Some(p) = self.player.as_ref() {
                let at = [p.position.x, p.position.y, p.position.z];
                self.companion_move_to(host, b, at);
            }
            b.commands.push_back(CompanionCommand::Stay);
            return None;
        }
        let g = host.hub.guest(b.conn)?;
        if b.dim != self.dim || g.dead || g.spectator() {
            return None;
        }
        let at = Vec3::new(g.position[0], g.position[1], g.position[2]);
        let (sleeping, creative) = (g.sleeping, g.creative());
        // Entity.portalProcess and portalCooldown are the body's own.
        let mut portal = host.play.entry(b.conn).or_default().portal;
        let mut through = None;
        // Entity.checkInsideBlocks: the Nether portal blocks the box is in.
        if let Some(pos) = self.nether_portal_inside(at) {
            portal.set_inside(pos, PLAYER_DELAY);
        }
        let (end_portal, _) = self.inside_end_portals(at, WIDTH, HEIGHT);
        if end_portal {
            if portal.cooldown == 0 {
                through = Some(Through::End);
            }
            // Entity.setAsInsidePortal while cooling down restarts the cooldown.
            portal.cooldown = PLAYER_DELAY;
        } else {
            let rule = if creative {
                "players_nether_portal_creative_delay"
            } else {
                "players_nether_portal_default_delay"
            };
            let transition = self.ui.rules.int(rule).max(0);
            through = portal
                .handle(!sleeping, transition, PLAYER_DELAY)
                .map(Through::Nether);
        }
        host.play.entry(b.conn).or_default().portal = portal;
        through
    }

    /// The owner goes through the body's portal from where the body stands,
    /// which brings the body along (game/companion_body.rs) onto the exit.
    fn watch_go_through(&mut self, through: Through) {
        let at = self.lan.as_ref().and_then(|host| {
            let name = &self.companions.watch.as_ref()?.name;
            let g = host
                .hub
                .guest(host.companions[companion_index(host, Some(name))?].conn)?;
            Some((g.position, g.yaw))
        });
        let Some(([x, y, z], yaw)) = at else {
            return;
        };
        if let Some(p) = self.player.as_mut() {
            p.position = Vec3::new(x, y, z);
            p.velocity = Vec3::ZERO;
            p.yaw = yaw;
            p.head_yaw = yaw;
        }
        self.yaw = yaw;
        let result = match through {
            Through::Nether(entry) => self.player_through_nether_portal(entry),
            Through::End => {
                let target = if self.dim == Dim::End {
                    Dim::Overworld
                } else {
                    Dim::End
                };
                self.end_portal_travel = true;
                let result = self.enter_dimension(target);
                self.end_portal_travel = false;
                result
            }
        };
        if let Err(e) = result {
            log::error!("[companion] the watched body's portal: {e}");
        }
    }

    /// The body is parked in another level (it came back there after dying
    /// away from here): the owner goes there, where it rejoins.
    fn watch_follow_parked(&mut self, name: &str) {
        let parked = self
            .companions
            .parked
            .iter()
            .find(|(f, _)| f.name.eq_ignore_ascii_case(name))
            .map(|(f, d)| (f.uuid(), *d));
        let Some((uuid, dim)) = parked else {
            // Dismissed: nothing left to watch.
            log::info!("[companion] {name} left: Watch ends");
            self.watch_end();
            return;
        };
        if dim == self.dim {
            // It rejoins here once the ground is in (tick_companion_arrivals).
            return;
        }
        let at = uuid
            .and_then(|u| playerdata::saved_position(&self.world_root, u))
            .unwrap_or([0.5, 80.0, 0.5]);
        if let Err(e) = self.teleport_player_to_dimension(dim, at, None) {
            log::error!("[companion] could not follow {name} to {}: {e:?}", dim.id());
            return;
        }
        if let Some(w) = self.companions.watch.as_mut() {
            w.dim = dim;
        }
    }

    /// The owner's view: behind the body and a little above, looking at its
    /// head, closing in smoothly (at once on `snap`).
    fn watch_camera(&mut self, snap: bool) {
        let Some(name) = self.companions.watch.as_ref().map(|w| w.name.clone()) else {
            return;
        };
        let body = self.lan.as_ref().and_then(|host| {
            let b = &host.companions[companion_index(host, Some(&name))?];
            let g = host.hub.guest(b.conn)?;
            (b.dim == self.dim).then_some((g.position, g.yaw))
        });
        let Some((at, yaw)) = body else {
            return;
        };
        let eye = f64::from(self.eye);
        let r = f64::from(yaw).to_radians();
        // Facing (-sin yaw, cos yaw): the camera stands behind that.
        let want = Vec3::new(
            at[0] + r.sin() * BEHIND,
            at[1] + ABOVE,
            at[2] - r.cos() * BEHIND,
        );
        let Some(p) = self.player.as_mut() else {
            return;
        };
        let d = [
            want.x - p.position.x,
            want.y - p.position.y,
            want.z - p.position.z,
        ];
        p.position = if snap || (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > SNAP {
            want
        } else {
            Vec3::new(
                p.position.x + d[0] * EASE,
                p.position.y + d[1] * EASE,
                p.position.z + d[2] * EASE,
            )
        };
        p.velocity = Vec3::ZERO;
        p.fall_distance = 0.0;
        let (dx, dy, dz) = (
            at[0] - p.position.x,
            at[1] + 1.62 - (p.position.y + eye),
            at[2] - p.position.z,
        );
        let look_yaw = (-dx).atan2(dz).to_degrees() as f32;
        let look_pitch = (-dy.atan2(dx.hypot(dz))).to_degrees() as f32;
        p.yaw = look_yaw;
        p.head_yaw = look_yaw;
        p.pitch = look_pitch;
        self.yaw = look_yaw;
        self.pitch = look_pitch;
        if snap {
            self.previous_position = p.position;
        }
    }

    fn watch_lines(&self, w: &Watch) -> Vec<String> {
        let seconds = w.ticks / 20;
        let or_dash = |s: &str| {
            if s.is_empty() {
                "-".to_owned()
            } else {
                s.to_owned()
            }
        };
        vec![
            format!(
                "Watching {}  {:02}:{:02}:{:02}",
                w.name,
                seconds / 3600,
                seconds / 60 % 60,
                seconds % 60
            ),
            format!("Step: {}", or_dash(&w.step)),
            format!("Perception: {}", or_dash(&w.perception)),
        ]
    }

    /// The Watch HUD, top right: who, the run timer, the route step and the
    /// perception level.
    pub(super) fn draw_companion_watch(&self, batch: &mut HudBatch) {
        let Some(w) = self.companions.watch.as_ref() else {
            return;
        };
        let atlas = &self.gui.atlas;
        let background = self.settings.text_background(0.5);
        let mut y = 4.0;
        for line in self.watch_lines(w) {
            let tw = HudBatch::text_width(atlas, &line);
            let x = batch.width - tw - 4.0;
            batch.fill(atlas, x - 2.0, y - 1.0, tw + 4.0, 10.0, background);
            batch.text(atlas, &line, x, y, u32::MAX, true);
            y += 10.0;
        }
    }

    /// `companion strike [name] <id>`: one blow at entity `id` (a mob, a
    /// dragon part or a crystal, as tracked to players) once its attack is
    /// charged, whatever its skill.
    pub(super) fn companion_strike_words(
        &mut self,
        words: &[&str],
    ) -> std::result::Result<Value, String> {
        let (name, id) = match words {
            [id] => (None, *id),
            [name, id] => (Some(*name), *id),
            _ => return Err("companion strike [name] <entity id>".into()),
        };
        let id: u32 = id.parse().map_err(|_| format!("not an entity id: {id}"))?;
        let mut host = self.lan.take().ok_or("no companion here")?;
        let Some(bi) = companion_index(&host, name) else {
            self.lan = Some(host);
            return Err("no companion by that name".into());
        };
        let mut b = host.companions.remove(bi);
        self.companion_strike(&mut host, &mut b, id);
        let name = b.file.name.clone();
        host.companions.insert(bi, b);
        self.lan = Some(host);
        Ok(json!({ "ok": true, "name": name, "message": format!("{name} struck {id}") }))
    }
}
