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
    collisions::{SceneCollisions, no_collision_empty},
    command::args::GameMode,
    companion::CompanionCommand,
    hud::HudBatch,
    lan::playerdata,
};
use horizon_core::{
    geometry::{Aabb, Vec3},
    position::BlockPos,
};
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
/// The camera is kept clear of blocks like a third-person camera: a box this far around it (a
/// little more than the near plane) may touch none, and the ray from the body's head is tested
/// every `CLEAR_STEP` blocks (less than the box, so no wall slips between two tests).
const CAMERA_HALF: f64 = 0.25;
const CLEAR_STEP: f64 = 0.2;
/// How much of the gap the camera backs out each tick once the way is clear again (it comes in at
/// once), so a branch edge sliding past the ray does not make it breathe in and out.
const REACH_OUT: f64 = 0.08;
/// Where the body's head is, above its feet.
const HEAD: f64 = 1.62;

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
    /// Where the owner stood when the view last took the body's level: the
    /// watched body follows, comes to and looks at this, not the camera
    /// behind it (which would turn it round to face the camera).
    at: [f64; 3],
    /// How far out along the view ray the camera may stand now, in blocks, eased: the ray
    /// stops at blocks that collide, leaves and tall plants (`watch_clear`).
    reach: f64,
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

    fn owner_dead(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|p| p.health.dead_or_dying())
    }

    /// Where a body's owner is for it: the owner, or for the watched body
    /// in the watched level the spot its owner left to watch it.
    pub(super) fn owner_for(&self, b: &Body, owner: [f64; 3]) -> [f64; 3] {
        match &self.companions.watch {
            Some(w) if w.name == b.file.name && w.dim == self.dim && b.dim == self.dim => w.at,
            _ => owner,
        }
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
        // The death screen is the dead owner's: Respawn first (a camera
        // started now would sit behind it).
        if self.owner_dead() {
            return Err("you are dead: respawn first".into());
        }
        let host = self.lan.as_ref().ok_or("no companion here")?;
        let bi = companion_index(host, name).ok_or("no companion by that name")?;
        let b = &host.companions[bi];
        if b.dim != self.dim {
            return Err(format!("{} is in another dimension", b.file.name));
        }
        let name = b.file.name.clone();
        // The HUD's perception line follows the companion's setting (`watch perception` overrides).
        let perception = self.perception_of(&name).label().to_owned();
        // Watching another one keeps the mode from before the first.
        let mode = self.companions.watch.take().map_or(self.mode, |w| w.mode);
        let at = self
            .player
            .as_ref()
            .map_or([0.0; 3], |p| [p.position.x, p.position.y, p.position.z]);
        self.companions.watch = Some(Watch {
            name: name.clone(),
            mode,
            dim: self.dim,
            ticks: 0,
            step: String::new(),
            perception,
            at,
            reach: f64::INFINITY,
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
        // A dead owner stays where they died.
        let at = self
            .lan
            .as_ref()
            .filter(|_| !self.owner_dead())
            .and_then(|host| {
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
            // How far behind the head the blocks let it stand (infinite until the first tick).
            "reach": w.reach.is_finite().then_some(w.reach),
        })
    }

    /// Per tick, after the hub's: the camera, the body's portals, and the
    /// owner after a body that came back elsewhere.
    pub(super) fn tick_companion_watch(&mut self) {
        if self.companions.watch.is_none() || self.player.is_none() || self.travelling.is_some() {
            return;
        }
        // A spectator takes no damage but what bypasses invulnerability
        // (`/kill`): an owner killed while watching gets their death screen
        // and their mode back, not a camera behind it.
        if self.owner_dead() {
            self.watch_end();
            return;
        }
        let Some(w) = self.companions.watch.as_mut() else {
            return;
        };
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
                w.at = at;
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
            (w.dim, w.at) = (dim, at);
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
        // Along the ray from the body's head to where the camera would stand, as far as the
        // blocks let it: at once closer, smoothly farther.
        let head = Vec3::new(at[0], at[1] + HEAD, at[2]);
        let full = Vec3::new(want.x, want.y + eye, want.z);
        let clear = self.watch_clear(head, full);
        let reach = match self.companions.watch.as_mut() {
            Some(w) => {
                w.reach = if snap || !w.reach.is_finite() || clear < w.reach {
                    clear
                } else {
                    w.reach + (clear - w.reach) * REACH_OUT
                };
                w.reach
            }
            None => clear,
        };
        let length = distance(head, full);
        let along = if length > 1e-6 {
            reach.min(length) / length
        } else {
            0.0
        };
        let want = Vec3::new(
            head.x + (full.x - head.x) * along,
            head.y + (full.y - head.y) * along - eye,
            head.z + (full.z - head.z) * along,
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
        // Easing from one side of a branch to the other can cut its corner: whatever the camera
        // is, it stays on the clear part of the ray from the head.
        let cam = Vec3::new(p.position.x, p.position.y + eye, p.position.z);
        let seen = distance(head, cam);
        let clear = self.watch_clear(head, cam);
        let Some(p) = self.player.as_mut() else {
            return;
        };
        if clear + 1e-3 < seen {
            let k = clear / seen;
            p.position = Vec3::new(
                head.x + (cam.x - head.x) * k,
                head.y + (cam.y - head.y) * k - eye,
                head.z + (cam.z - head.z) * k,
            );
        }
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

    /// How far from `from` (the body's head) along the ray to `to` the camera may stand, in blocks
    /// (at most the ray's length): short of the first block that collides (leaves and the ground
    /// included), then short of any tall plant it would stand in. Like a third-person camera.
    fn watch_clear(&self, from: Vec3, to: Vec3) -> f64 {
        let length = distance(from, to);
        if length < 1e-6 {
            return 0.0;
        }
        let at = |t: f64| {
            let k = t / length;
            Vec3::new(
                from.x + (to.x - from.x) * k,
                from.y + (to.y - from.y) * k,
                from.z + (to.z - from.z) * k,
            )
        };
        let around = |c: Vec3| {
            Aabb::new(
                Vec3::new(c.x - CAMERA_HALF, c.y - CAMERA_HALF, c.z - CAMERA_HALF),
                Vec3::new(c.x + CAMERA_HALF, c.y + CAMERA_HALF, c.z + CAMERA_HALF),
            )
        };
        let mut scene = SceneCollisions {
            world: &self.world,
            binding: self.collision,
            subject: None,
        };
        let mut free = 0.0;
        let mut t = CLEAR_STEP;
        loop {
            if t >= length {
                free = length;
                break;
            }
            if !no_collision_empty(&mut scene, around(at(t))) {
                break;
            }
            free = t;
            t += CLEAR_STEP;
        }
        // Leaves collide and are in that test; grass and ferns do not, but a camera inside one
        // sees only its texture.
        while free > 0.0 && self.watch_in_plant(at(free)) {
            free = (free - CLEAR_STEP).max(0.0);
        }
        free
    }

    /// Whether the camera's box, centred on `c`, touches leaves or a plant that fills the view
    /// without colliding.
    fn watch_in_plant(&self, c: Vec3) -> bool {
        let block = |v: f64, d: f64| (v + d).floor() as i32;
        for x in block(c.x, -CAMERA_HALF)..=block(c.x, CAMERA_HALF) {
            for y in block(c.y, -CAMERA_HALF)..=block(c.y, CAMERA_HALF) {
                for z in block(c.z, -CAMERA_HALF)..=block(c.z, CAMERA_HALF) {
                    if blocks_view(&self.block_name([x, y, z])) {
                        return true;
                    }
                }
            }
        }
        false
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

fn distance(a: Vec3, b: Vec3) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// Leaves and the plants that fill a block's view without colliding (grass, ferns, bushes, vines,
/// the tall flowers).
fn blocks_view(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name.ends_with("_leaves")
        || name.ends_with("_vines")
        || name.ends_with("_vines_plant")
        || matches!(
            name,
            "short_grass"
                | "tall_grass"
                | "fern"
                | "large_fern"
                | "dead_bush"
                | "bush"
                | "sweet_berry_bush"
                | "vine"
                | "sunflower"
                | "lilac"
                | "rose_bush"
                | "peony"
                | "short_dry_grass"
                | "tall_dry_grass"
                | "firefly_bush"
                | "seagrass"
                | "tall_seagrass"
                | "kelp"
                | "kelp_plant"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_and_tall_plants_fill_the_camera_but_stone_and_flowers_do_not() {
        for name in [
            "minecraft:oak_leaves",
            "minecraft:cherry_leaves",
            "minecraft:pale_oak_leaves",
            "minecraft:tall_grass",
            "minecraft:large_fern",
            "minecraft:short_grass",
            "minecraft:sweet_berry_bush",
            "minecraft:weeping_vines_plant",
            "minecraft:vine",
        ] {
            assert!(blocks_view(name), "{name}");
        }
        for name in [
            "minecraft:air",
            "minecraft:stone",
            "minecraft:poppy",
            "minecraft:oak_log",
            "minecraft:glass",
        ] {
            assert!(!blocks_view(name), "{name}");
        }
    }

    #[test]
    fn distance_is_euclidean() {
        let d = distance(Vec3::new(0.0, 0.0, 0.0), Vec3::new(3.0, 4.0, 12.0));
        assert!((d - 13.0).abs() < 1e-9);
    }
}
