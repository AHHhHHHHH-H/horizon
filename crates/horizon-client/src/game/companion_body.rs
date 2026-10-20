//! A companion's body on the server, each tick (docs/companions.md, "The
//! body"): its commands, then what it notices (getting hurt, hostiles, its
//! gear, its hunger), then the fight it picks and where it goes, along
//! vanilla's path finder sized for a player. What it decides is pressed as a
//! player's keys: its body moves with the player's own physics
//! (`companion_body_tick`), and it attacks, eats and picks up items as a
//! guest's packets would make it.
use super::{
    Dim, Game,
    companion::{Body, Nav},
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c, AttackTarget, CompanionCommand, CompanionReport, FollowStep, GoTarget, Skill,
        gear,
    },
    items::ItemStack,
    mobs::{
        Mob, Who,
        path::{Evaluator, Finder, PATH_TYPES, Path, PathMob},
    },
    motion::{MotionEvent, MotionWorld},
    sprint::Keys,
};
use horizon_core::geometry::{Aabb, Vec3};
use horizon_protocol::packets::play::{self, Serverbound as S};

/// The armour slots by `EquipmentSlot` name, head first.
const ARMOR_SLOTS: [&str; 4] = ["head", "chest", "legs", "feet"];
/// A player's entity interaction range, less a margin.
const REACH: f64 = 2.9;
/// How long after a hit the companion still answers it (`getLastHurtByMob`).
const HURT_MEMORY: i64 = 100;
/// A fight that is not ordered ends this far from the owner.
const LEASH: f64 = 24.0;

/// What steering toward a goal came to this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Steer {
    Arrived,
    Moving,
    /// No way there yet (no path, or stuck too long).
    Stuck,
}

/// Hostiles a player would leave alone until they turn on someone: neutral
/// mobs of the monster category, and the warden.
fn leave_alone(m: &Mob) -> bool {
    matches!(
        &*m.kind.id,
        "minecraft:enderman"
            | "minecraft:zombified_piglin"
            | "minecraft:piglin"
            | "minecraft:spider"
            | "minecraft:cave_spider"
            | "minecraft:warden"
    ) && m.ai.target.is_none()
}

/// A mob worth fighting or calling out.
pub(super) fn threat(m: &Mob) -> bool {
    m.alive() && m.hostile() && !leave_alone(m)
}

pub(super) fn mob_at(m: &Mob) -> [f64; 3] {
    [m.position.x, m.position.y, m.position.z]
}

/// The distance from `eye` to the nearest point of `b`.
fn distance_to_box(eye: [f64; 3], b: Aabb) -> f64 {
    let (lo, hi) = (b.min_position(), b.max_position());
    let clamp = |v: f64, l: f64, h: f64| v.clamp(l, h);
    let p = [
        clamp(eye[0], lo.x, hi.x),
        clamp(eye[1], lo.y, hi.y),
        clamp(eye[2], lo.z, hi.z),
    ];
    c::distance(eye, p)
}

/// Ticks it tries to reach a dropped item before leaving it.
const ITEM_PATIENCE: u64 = 100;

impl Game<'_> {
    /// Per hub tick, before the guests': each companion decides what it does.
    pub(super) fn tick_companions(&mut self, host: &mut LanHost) {
        let owner = self
            .player
            .as_ref()
            .filter(|p| !p.health.dead_or_dying())
            .map(|p| [p.position.x, p.position.y, p.position.z]);
        if self.world.game_time % 200 == 0 && !self.companions.struck.is_empty() {
            let mobs = &self.mobs;
            self.companions
                .struck
                .retain(|id, _| mobs.iter().any(|m| m.id == *id && m.alive()));
        }
        let mut bi = 0;
        while bi < host.companions.len() {
            let mut b = host.companions.remove(bi);
            let keep = match owner {
                Some(owner) => self.tick_companion(host, &mut b, owner),
                // The owner is travelling or dead: the bodies wait.
                None => {
                    b.keys = Keys::default();
                    true
                }
            };
            host.companions.insert(bi, b);
            if keep {
                bi += 1;
            } else {
                self.companion_park(host, bi);
            }
        }
    }

    /// One companion's decisions; false when it stays behind in the level
    /// its owner left.
    fn tick_companion(&mut self, host: &mut LanHost, b: &mut Body, owner: [f64; 3]) -> bool {
        b.ticks += 1;
        b.keys = Keys::default();
        if host.hub.guest(b.conn).is_none() {
            return true;
        }
        if b.dim != self.dim {
            if !b.near_owner || host.hub.guest(b.conn).is_some_and(|g| g.dead) {
                return false;
            }
            // It came through with its owner.
            b.dim = self.dim;
            b.target = None;
            if matches!(b.skill, Skill::Stay | Skill::GoTo | Skill::Come) {
                b.skill = Skill::Follow;
                b.post = None;
            }
            if !self.companion_place_near(host, b, owner) {
                self.companion_move_to(host, b, owner);
            }
            self.companion_report(
                b,
                CompanionReport::ChangedDimension {
                    dimension: self.dim.id().to_owned(),
                },
            );
        }
        while let Some(command) = b.commands.pop_front() {
            horizon_client::spans::time("companion_apply", || {
                self.companion_apply(host, b, command, owner);
            });
        }
        let Some(g) = host.hub.guest(b.conn) else {
            return true;
        };
        if g.dead {
            // A watched body's owner is only its camera.
            let with_owner = c::distance(g.position, owner) <= c::COME_ALONG && !self.watched(b);
            let t = b.dead_ticks.get_or_insert(0);
            *t += 1;
            if *t >= c::RESPAWN_TICKS {
                // With its owner (where it fell) it comes back beside them;
                // alone, where a player would.
                if with_owner {
                    self.companion_respawn(host, b, owner);
                } else {
                    return self.companion_respawn_alone(host, b);
                }
            }
            return true;
        }
        let (pos, health, food) = (g.position, g.health, g.food);
        b.near_owner = c::distance(pos, owner) <= c::COME_ALONG;
        self.companion_notice_hurt(host, b, pos, health);
        if b.ticks.is_multiple_of(10) {
            horizon_client::spans::time("companion_callouts", || {
                self.companion_callouts(b, pos, owner);
            });
        }
        // Eating ends when the item is used up (or the use stops).
        if let Some((before, item)) = b.eating.clone() {
            let using = host
                .play
                .get(&b.conn)
                .and_then(super::lan_actions::GuestPlay::using)
                .is_some();
            if !using {
                b.eating = None;
                let after = host
                    .hub
                    .guest(b.conn)
                    .map_or(before, |g| carried(&g.inventory, &item));
                if after < before {
                    self.companion_report(b, CompanionReport::Ate { item });
                }
                self.companion_gear(host, b);
            }
        } else if b.ticks.is_multiple_of(20) {
            horizon_client::spans::time("companion_gear", || self.companion_gear(host, b));
        }
        // Self-preservation: it backs off when badly hurt.
        if !b.retreating && health > 0.0 && health < c::RETREAT_BELOW {
            b.retreating = true;
            b.target = None;
            self.companion_report(b, CompanionReport::SkillStarted(Skill::Retreat));
        } else if b.retreating && health >= c::BRAVE_AGAIN {
            b.retreating = false;
            self.companion_report(b, CompanionReport::SkillFinished(Skill::Retreat));
        }
        let danger_close = self
            .mobs
            .iter()
            .any(|m| threat(m) && c::distance(mob_at(m), pos) < 5.0);
        let hungry = food < c::EAT_BELOW || b.retreating && food < 20 && !danger_close;
        if b.eating.is_none() && hungry && (b.target.is_none() || !danger_close) {
            self.companion_start_eating(host, b);
        }
        horizon_client::spans::time("companion_reflexes", || {
            self.companion_work_reflexes(host, b);
        });
        horizon_client::spans::time("companion_target", || {
            self.companion_pick_target(host, b, pos, owner);
        });
        horizon_client::spans::time("companion_move", || {
            self.companion_move(host, b, pos, owner);
        });
        if b.jump {
            b.keys.jump = true;
            b.jump = false;
        }
        b.keys.shift |= b.crouch;
        true
    }

    /// A command from the brain or the dev command.
    fn companion_apply(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        command: CompanionCommand,
        owner: [f64; 3],
    ) {
        let pos = host.hub.guest(b.conn).map_or(owner, |g| g.position);
        // A new order for where it goes ends the work it had on.
        if !b.work.tasks.is_empty()
            && matches!(
                command,
                CompanionCommand::Follow
                    | CompanionCommand::Stay
                    | CompanionCommand::Come
                    | CompanionCommand::GoTo(_)
                    | CompanionCommand::Stop
                    | CompanionCommand::Defend
            )
        {
            self.companion_cancel_work(host, b, Some("stopped"));
        }
        let start = |game: &mut Self, b: &mut Body, skill: Skill| {
            if b.skill != skill {
                b.skill = skill;
                game.companion_report(b, CompanionReport::SkillStarted(skill));
            }
            b.nav.reset();
        };
        match command {
            CompanionCommand::Follow => {
                b.target = None;
                b.post = None;
                start(self, b, Skill::Follow);
            }
            CompanionCommand::Stay => {
                b.post = Some(pos);
                start(self, b, Skill::Stay);
            }
            CompanionCommand::Come => {
                b.target = None;
                start(self, b, Skill::Come);
            }
            CompanionCommand::GoTo(to) => {
                let block = match to {
                    GoTarget::Block(p) => Some(p),
                    GoTarget::Crosshair => self.owner_crosshair_block(),
                };
                match block.map(|p| self.standing_spot(p)) {
                    Some(at) => {
                        b.target = None;
                        b.post = Some(at);
                        start(self, b, Skill::GoTo);
                    }
                    None => self.companion_report(
                        b,
                        CompanionReport::SkillFailed {
                            skill: Skill::GoTo,
                            reason: "nothing in sight".into(),
                        },
                    ),
                }
            }
            CompanionCommand::Stop => {
                b.target = None;
                b.post = None;
                b.eating = None;
                start(self, b, Skill::Idle);
            }
            CompanionCommand::Attack(who) => {
                let id = match who {
                    AttackTarget::Entity(id) => self
                        .mobs
                        .iter()
                        .any(|m| m.id == id && m.alive())
                        .then_some(id),
                    AttackTarget::NearestHostile => self
                        .mobs
                        .iter()
                        .filter(|m| threat(m) && c::distance(mob_at(m), pos) <= c::NOTICE_RADIUS)
                        .min_by(|x, y| {
                            c::distance(mob_at(x), pos).total_cmp(&c::distance(mob_at(y), pos))
                        })
                        .map(|m| m.id),
                };
                match id {
                    Some(id) => {
                        if b.skill != Skill::Attack {
                            b.resume = if b.skill == Skill::Idle {
                                Skill::Follow
                            } else {
                                b.skill
                            };
                        }
                        b.target = Some(id);
                        b.ordered = true;
                        b.hold_fire = false;
                        start(self, b, Skill::Attack);
                    }
                    None => self.companion_report(
                        b,
                        CompanionReport::SkillFailed {
                            skill: Skill::Attack,
                            reason: "no target".into(),
                        },
                    ),
                }
            }
            CompanionCommand::Defend => {
                b.post = None;
                start(self, b, Skill::Defend);
            }
            CompanionCommand::HoldFire(hold) => {
                b.hold_fire = hold;
                if hold {
                    b.target = None;
                }
            }
            CompanionCommand::Jump => b.jump = true,
            CompanionCommand::Crouch(on) => b.crouch = on,
            work @ (CompanionCommand::Collect { .. }
            | CompanionCommand::Give { .. }
            | CompanionCommand::Craft { .. }
            | CompanionCommand::Smelt { .. }
            | CompanionCommand::Sleep) => self.companion_start_work(host, b, work),
            CompanionCommand::Eat => {
                if !self.companion_start_eating(host, b) {
                    self.companion_report(
                        b,
                        CompanionReport::SkillFailed {
                            skill: b.skill,
                            reason: "nothing to eat".into(),
                        },
                    );
                }
            }
        }
    }

    /// The block the owner's crosshair hits, up to 128 blocks away.
    fn owner_crosshair_block(&self) -> Option<[i32; 3]> {
        let eye = self.eye_position()?;
        let hit = self.clip_ray(
            eye,
            super::view_vector(self.yaw, self.pitch),
            c::CROSSHAIR_RANGE,
        )?;
        Some([
            hit.pos[0] + hit.face[0],
            hit.pos[1] + hit.face[1],
            hit.pos[2] + hit.face[2],
        ])
    }

    /// Where a player stands to be at block `p`: on top of it when it is
    /// solid, else on the first floor below (within 16 blocks).
    fn standing_spot(&self, p: [i32; 3]) -> [f64; 3] {
        let solid = |q: [i32; 3]| {
            let f = self.res.facts().get(self.world.block(q[0], q[1], q[2]));
            f.full_collision || f.legacy_solid
        };
        let mut q = p;
        if solid(q) {
            q[1] += 1;
        } else {
            for _ in 0..16 {
                if solid([q[0], q[1] - 1, q[2]]) {
                    break;
                }
                q[1] -= 1;
            }
        }
        [
            f64::from(q[0]) + 0.5,
            f64::from(q[1]),
            f64::from(q[2]) + 0.5,
        ]
    }

    /// Health lost since last tick: a Hurt report, and who did it (the mob
    /// that sees it as its player, nearest first) to answer.
    fn companion_notice_hurt(&mut self, host: &LanHost, b: &mut Body, pos: [f64; 3], health: f32) {
        // Death itself is the guest tick's to report.
        if health > 0.0 && health < b.last_health - 0.01 {
            let amount = b.last_health - health;
            let by = self
                .mobs
                .iter()
                .filter(|m| m.alive() && host.mob_players.get(&m.id) == Some(&b.conn))
                .filter(|m| c::distance(mob_at(m), pos) < 16.0)
                .min_by(|x, y| c::distance(mob_at(x), pos).total_cmp(&c::distance(mob_at(y), pos)))
                .map(|m| (m.id, m.kind.id.to_string()));
            if let Some((id, _)) = &by {
                b.hurt_by = Some((*id, self.world.game_time));
            }
            self.companion_report(
                b,
                CompanionReport::Hurt {
                    amount,
                    by: by.map(|(_, kind)| kind),
                },
            );
        }
        b.last_health = health;
    }

    /// Hostiles new in view near the owner or the companion are called out
    /// with their clock bearing from the owner's facing.
    fn companion_callouts(&mut self, b: &mut Body, pos: [f64; 3], owner: [f64; 3]) {
        let mut seen = Vec::new();
        let mut new = Vec::new();
        for m in self.mobs.iter().filter(|m| threat(m)) {
            let at = mob_at(m);
            let (to_owner, to_body) = (c::distance(at, owner), c::distance(at, pos));
            if to_owner.min(to_body) > c::NOTICE_RADIUS {
                continue;
            }
            seen.push(m.id);
            if !b.seen.contains(&m.id) {
                new.push(CompanionReport::SawHostile {
                    kind: m.kind.id.to_string(),
                    bearing: c::clock_bearing(owner, self.yaw, at),
                    distance: to_owner as f32,
                });
            }
        }
        b.seen = seen;
        for r in new {
            self.companion_report(b, r);
        }
    }

    /// The fight it is in: kept while it lasts, else one it should start.
    fn companion_pick_target(
        &mut self,
        host: &LanHost,
        b: &mut Body,
        pos: [f64; 3],
        owner: [f64; 3],
    ) {
        if b.hold_fire || b.retreating || b.skill == Skill::Idle {
            if b.skill != Skill::Attack {
                b.target = None;
            }
            if b.hold_fire || b.retreating {
                return;
            }
        }
        // The current target, while it is alive and near enough.
        if let Some(id) = b.target {
            let keep = self
                .mobs
                .iter()
                .find(|m| m.id == id && m.alive())
                .is_some_and(|m| {
                    let at = mob_at(m);
                    if b.ordered {
                        c::distance(at, pos) <= 48.0
                    } else {
                        match (b.skill, b.post) {
                            (Skill::Stay, Some(post)) => {
                                c::distance(at, post) <= c::GUARD_RADIUS + 8.0
                            }
                            _ => c::distance(at, owner) <= LEASH,
                        }
                    }
                });
            if keep {
                return;
            }
            b.target = None;
            b.nav.reset();
            if b.ordered {
                b.ordered = false;
                self.companion_report(b, CompanionReport::SkillFinished(Skill::Attack));
                if b.skill == Skill::Attack {
                    b.skill = b.resume;
                }
            }
        }
        if b.skill == Skill::Idle || b.skill == Skill::Attack {
            return;
        }
        let now = self.world.game_time;
        let alive = |id: u32| self.mobs.iter().find(|m| m.id == id && m.alive());
        let near_body = |m: &Mob| c::distance(mob_at(m), pos) <= c::NOTICE_RADIUS;
        // Whoever hurt it lately (HurtByTargetGoal).
        let mut pick = b
            .hurt_by
            .filter(|(_, at)| now - at <= HURT_MEMORY)
            .and_then(|(id, _)| alive(id))
            .map(|m| m.id);
        // Whoever hurt its owner, then whatever its owner attacked
        // (OwnerHurtByTargetGoal, OwnerHurtTargetGoal).
        if pick.is_none()
            && let Some((Who::Mob(id), _)) = self.player_hurt_by()
        {
            pick = alive(id).filter(|m| near_body(m)).map(|m| m.id);
        }
        if pick.is_none()
            && let Some((id, at)) = self.combat.last_hurt_mob
            && now - at <= HURT_MEMORY
        {
            pick = alive(id).filter(|m| near_body(m)).map(|m| m.id);
        }
        // Anything on it now, close: it defends itself.
        if pick.is_none() {
            pick = self
                .mobs
                .iter()
                .filter(|m| threat(m) && m.ai.target == Some(Who::Player))
                .filter(|m| host.mob_players.get(&m.id) == Some(&b.conn))
                .filter(|m| c::distance(mob_at(m), pos) <= 8.0)
                .min_by(|x, y| c::distance(mob_at(x), pos).total_cmp(&c::distance(mob_at(y), pos)))
                .map(|m| m.id);
        }
        // Defend: anything after the owner; Stay: anything near its post.
        if pick.is_none() {
            let (centre, radius, only_hunting) = match (b.skill, b.post) {
                (Skill::Defend, _) => (owner, c::GUARD_RADIUS, true),
                (Skill::Stay, Some(post)) => (post, c::GUARD_RADIUS, false),
                _ => (owner, 0.0, true),
            };
            if radius > 0.0 {
                pick = self
                    .mobs
                    .iter()
                    .filter(|m| threat(m) && (!only_hunting || m.ai.target == Some(Who::Player)))
                    .filter(|m| c::distance(mob_at(m), centre) <= radius)
                    .min_by(|x, y| {
                        c::distance(mob_at(x), pos).total_cmp(&c::distance(mob_at(y), pos))
                    })
                    .map(|m| m.id);
            }
        }
        if let Some(id) = pick {
            b.target = Some(id);
            b.ordered = false;
            b.nav.reset();
        }
    }

    /// Where it goes this tick, and the blow it strikes.
    fn companion_move(&mut self, host: &mut LanHost, b: &mut Body, pos: [f64; 3], owner: [f64; 3]) {
        let eye = [pos[0], pos[1] + 1.62, pos[2]];
        // Fighting.
        if let Some(id) = b.target {
            let Some(m) = self.mobs.iter().find(|m| m.id == id) else {
                return;
            };
            let (at, bounds, height) = (mob_at(m), m.bounds(), m.kind.height);
            let reach = distance_to_box(eye, bounds);
            // A fight stops the digging and the menus; at range, the bow.
            self.companion_stop_dig(host, b);
            self.companion_close_menu(host, b);
            if self.companion_bow(host, b, id, pos) {
                return;
            }
            if reach > REACH * 0.8 {
                let sprint = c::horizontal(pos, at) > 4.0;
                let _ = self.companion_steer(host, b, at, 1.2, sprint);
            }
            let centre = [at[0], at[1] + height * 0.5, at[2]];
            b.yaw = c::yaw_toward(pos, at) as f32;
            b.pitch = look_pitch(eye, centre);
            if reach <= REACH && b.eating.is_none() {
                self.companion_strike(host, b, id);
            }
            return;
        }
        // Backing off from what hurts it.
        if b.retreating {
            let nearest = self
                .mobs
                .iter()
                .filter(|m| threat(m) && c::distance(mob_at(m), pos) < 10.0)
                .min_by(|x, y| c::distance(mob_at(x), pos).total_cmp(&c::distance(mob_at(y), pos)))
                .map(mob_at);
            if let Some(danger) = nearest {
                let (dx, dz) = (pos[0] - danger[0], pos[2] - danger[2]);
                let len = (dx * dx + dz * dz).sqrt().max(0.01);
                // Away from it, leaning toward the owner.
                let (ox, oz) = (owner[0] - pos[0], owner[2] - pos[2]);
                let olen = (ox * ox + oz * oz).sqrt().max(0.01);
                let away = [
                    pos[0] + dx / len * 8.0 + ox / olen * 3.0,
                    pos[1],
                    pos[2] + dz / len * 8.0 + oz / olen * 3.0,
                ];
                let _ = self.companion_steer(host, b, away, 1.0, true);
                return;
            }
        }
        match b.skill {
            Skill::Idle | Skill::Attack => self.companion_look_at_owner(b, pos, owner),
            Skill::Collect | Skill::Give | Skill::Craft | Skill::Smelt | Skill::Sleep => {
                self.companion_work(host, b, pos, owner);
            }
            Skill::Follow | Skill::Defend | Skill::Retreat => {
                // TamableAnimal.tryToTeleportToOwner wants a floor by the
                // owner: none while they fall or fly, when it heads for the
                // ground below them.
                let grounded = self.player.as_ref().is_some_and(|p| p.ground.on_ground);
                let owner = if grounded {
                    owner
                } else {
                    self.floor_below(owner)
                };
                let d = c::distance(pos, owner);
                match c::follow_step(d, b.moving) {
                    FollowStep::Teleport => {
                        if grounded && self.companion_place_near(host, b, owner) {
                            b.moving = false;
                            self.companion_report(b, CompanionReport::Teleported);
                        } else {
                            // Nowhere to stand by the owner yet: run after them.
                            b.moving = true;
                            let _ = self.companion_steer(host, b, owner, c::FOLLOW_NEAR, true);
                        }
                    }
                    step @ (FollowStep::Walk | FollowStep::Sprint) => {
                        b.moving = true;
                        let sprint = step == FollowStep::Sprint || b.retreating;
                        if self.companion_steer(host, b, owner, c::FOLLOW_NEAR - 0.5, sprint)
                            == Steer::Stuck
                        {
                            // Like a tamed wolf that cannot reach its owner.
                            if d > 12.0 && grounded && self.companion_place_near(host, b, owner) {
                                self.companion_report(b, CompanionReport::Teleported);
                            }
                            b.nav.reset();
                        }
                    }
                    FollowStep::Hold => {
                        b.moving = false;
                        // Close to the owner: pick up what lies around.
                        match self.companion_item_nearby(&b.out_of_reach, pos, owner) {
                            Some((id, item)) => {
                                let since = match b.reaching {
                                    Some((i, t)) if i == id => t,
                                    _ => {
                                        b.reaching = Some((id, b.ticks));
                                        b.ticks
                                    }
                                };
                                if b.ticks - since > ITEM_PATIENCE {
                                    // Out of reach: it leaves that one.
                                    if b.out_of_reach.len() >= 16 {
                                        b.out_of_reach.remove(0);
                                    }
                                    b.out_of_reach.push(id);
                                    b.reaching = None;
                                } else {
                                    let _ = self.companion_steer(host, b, item, 0.4, false);
                                }
                            }
                            None => {
                                b.reaching = None;
                                self.companion_look_at_owner(b, pos, owner);
                            }
                        }
                    }
                }
            }
            Skill::Stay => match b.post {
                Some(post) if c::horizontal(pos, post) > 1.5 => {
                    let _ = self.companion_steer(host, b, post, 1.0, false);
                }
                _ => self.companion_look_at_owner(b, pos, owner),
            },
            Skill::Come => {
                match self.companion_steer(host, b, owner, 2.5, c::distance(pos, owner) > 8.0) {
                    Steer::Arrived => {
                        b.skill = Skill::Stay;
                        b.post = Some(pos);
                        self.companion_report(b, CompanionReport::SkillFinished(Skill::Come));
                    }
                    Steer::Stuck => {
                        if self.companion_place_near(host, b, owner) {
                            self.companion_report(b, CompanionReport::Teleported);
                        }
                    }
                    Steer::Moving => {}
                }
            }
            Skill::GoTo => {
                let Some(goal) = b.post else {
                    b.skill = Skill::Follow;
                    return;
                };
                match self.companion_steer(host, b, goal, 0.7, c::distance(pos, goal) > 8.0) {
                    Steer::Arrived => {
                        b.skill = Skill::Stay;
                        self.companion_report(b, CompanionReport::SkillFinished(Skill::GoTo));
                    }
                    Steer::Stuck => {
                        b.skill = Skill::Stay;
                        b.post = Some(pos);
                        self.companion_report(
                            b,
                            CompanionReport::SkillFailed {
                                skill: Skill::GoTo,
                                reason: "no way there".into(),
                            },
                        );
                    }
                    Steer::Moving => {}
                }
            }
        }
    }

    pub(super) fn companion_look_at_owner(&self, b: &mut Body, pos: [f64; 3], owner: [f64; 3]) {
        b.yaw = c::yaw_toward(pos, owner) as f32;
        let eye_height = self
            .player
            .as_ref()
            .map_or(1.62, |p| f64::from(p.dimensions.eye_height));
        b.pitch = look_pitch(
            [pos[0], pos[1] + 1.62, pos[2]],
            [owner[0], owner[1] + eye_height, owner[2]],
        );
    }

    /// A dropped item it can take near its owner, the nearest first.
    fn companion_item_nearby(
        &self,
        skip: &[u32],
        pos: [f64; 3],
        owner: [f64; 3],
    ) -> Option<(u32, [f64; 3])> {
        self.dropped
            .iter()
            .filter(|d| d.pickup_delay < 10 && !skip.contains(&d.id))
            .map(|d| (d.id, [d.position.x, d.position.y, d.position.z]))
            .filter(|(_, at)| c::distance(*at, pos) <= 8.0 && c::distance(*at, owner) <= 12.0)
            .filter(|(_, at)| (at[1] - pos[1]).abs() < 2.5)
            .min_by(|(_, x), (_, y)| c::distance(*x, pos).total_cmp(&c::distance(*y, pos)))
    }

    /// Steer toward `goal` along a vanilla path for a player-sized walker:
    /// sets its keys and facing for the physics tick.
    pub(super) fn companion_steer(
        &mut self,
        host: &LanHost,
        b: &mut Body,
        goal: [f64; 3],
        near: f64,
        sprint: bool,
    ) -> Steer {
        let Some(g) = host.hub.guest(b.conn) else {
            return Steer::Stuck;
        };
        let (pos, on_ground, food) = (g.position, g.on_ground, g.food);
        let h = c::horizontal(pos, goal);
        let dy = goal[1] - pos[1];
        if h <= near && dy.abs() < 2.5 {
            b.nav.path = None;
            return Steer::Arrived;
        }
        let ticks = b.ticks;
        let nav = &mut b.nav;
        let d = c::distance(pos, goal);
        if nav.best_at == 0 || c::distance(nav.goal, goal) > 4.0 {
            nav.best = d;
            nav.best_at = ticks;
        }
        if d < nav.best - 0.3 {
            nav.best = d;
            nav.best_at = ticks;
        }
        let stalled = ticks - nav.best_at;
        if stalled > 30 && on_ground {
            nav.jump_until = ticks + 3;
        }
        if stalled > 60 {
            nav.path = None;
            nav.fails += 1;
            nav.best = d;
            nav.best_at = ticks;
        }
        if nav.fails > 4 {
            nav.fails = 0;
            return Steer::Stuck;
        }
        let direct = h < 2.5 && dy.abs() < 1.0;
        let in_water = self.res.facts().get(self.world.block(
            pos[0].floor() as i32,
            pos[1].floor() as i32,
            pos[2].floor() as i32,
        ));
        let in_water = in_water.fluid == Fluid::Water;
        let stale =
            nav.path.as_ref().is_none_or(Path::is_done) || c::distance(nav.goal, goal) > 2.0;
        // FollowOwnerGoal and MeleeAttackGoal plan again every 10 ticks or
        // so; a goal out of reach (an owner in the air) costs a full search
        // every time, so a plan that fell short waits longer.
        if c::distance(nav.goal, goal) > 8.0 {
            nav.misses = 0;
        }
        let gap = match &nav.path {
            _ if nav.planned_at == 0 => 0,
            Some(p) if p.reached => 10,
            _ => 15 << nav.misses.min(2),
        };
        if !direct && stale && ticks - nav.planned_at >= gap {
            let started = std::time::Instant::now();
            nav.path = self.companion_path(pos, on_ground, in_water, goal, near);
            let took = started.elapsed();
            let plans = &mut self.companions.plans;
            *plans = (plans.0 + 1, plans.1 + took, plans.2.max(took));
            nav.misses = if nav.path.as_ref().is_some_and(|p| p.reached) {
                0
            } else {
                nav.misses + 1
            };
            nav.goal = goal;
            nav.planned_at = ticks;
            if nav.path.is_none() {
                nav.fails += 1;
            }
        }
        let mut aim = goal;
        if direct {
            nav.path = None;
        } else if let Some(path) = nav.path.as_mut() {
            while !path.is_done() {
                let n = path.next_node();
                let at = [f64::from(n.x) + 0.5, f64::from(n.y), f64::from(n.z) + 0.5];
                if c::horizontal(pos, at) < 0.45 && (pos[1] - at[1]).abs() < 1.1 {
                    path.advance();
                    continue;
                }
                aim = at;
                break;
            }
            if path.is_done() && h > 2.5 {
                // A partial path ran out: plan again from here next tick.
                nav.path = None;
                return Steer::Moving;
            }
        } else if h > 3.0 {
            // No way known: never walk blind toward a far goal.
            return Steer::Moving;
        }
        let jump_until = nav.jump_until;
        b.yaw = c::yaw_toward(pos, aim) as f32;
        b.pitch = 0.0;
        b.keys.forward = true;
        b.keys.sprint = sprint && food > 6;
        let climb = aim[1] > pos[1] + 0.5 && c::horizontal(pos, aim) < 1.5 && on_ground;
        b.keys.jump = climb || in_water || ticks < jump_until;
        // Self-preservation: no step into lava, fire or a big fall.
        if !self.companion_safe_ahead(pos, b.yaw, b.keys.sprint) {
            b.keys.forward = false;
            b.keys.sprint = false;
            b.keys.jump = in_water;
        }
        Steer::Moving
    }

    /// `PathFinder.findPath` for a player-sized walker (vanilla's
    /// WalkNodeEvaluator: no lava, fire or falls past 3 blocks).
    fn companion_path(
        &self,
        from: [f64; 3],
        on_ground: bool,
        in_water: bool,
        goal: [f64; 3],
        near: f64,
    ) -> Option<Path> {
        let w = self.mob_world_base(&[]);
        let mob = PathMob {
            position: Vec3::new(from[0], from[1], from[2]),
            width: 0.6,
            height: 1.8,
            on_ground,
            in_water,
            max_up_step: 0.6,
            max_fall_distance: 3,
            malus: [None; PATH_TYPES],
            can_float: true,
            can_open_doors: false,
            can_pass_doors: true,
            can_walk_over_fences: false,
            stands_on_lava: false,
        };
        let target = goal.map(|v| v.floor() as i32);
        // PathNavigation's budget for a follow range of 32.
        Finder::new(&w, &mob, Evaluator::Walk).find(&[target], 48.0, near.floor() as i32, 512)
    }

    /// The first floor under `at` (an owner in the air), or `at` itself.
    fn floor_below(&self, at: [f64; 3]) -> [f64; 3] {
        let (x, z) = (at[0].floor() as i32, at[2].floor() as i32);
        let top = at[1].floor() as i32;
        let facts = self.res.facts();
        (0..96)
            .map(|dy| top - dy)
            .find(|&y| {
                let f = facts.get(self.world.block(x, y - 1, z));
                f.full_collision || f.fluid != Fluid::None
            })
            .map_or(at, |y| [at[0], f64::from(y), at[2]])
    }

    /// Whether the steps ahead are safe: no lava or fire at the feet or head,
    /// and ground within a 3-block fall that is not lava or fire. A sprint
    /// carries on after the keys let go, so it looks further ahead.
    fn companion_safe_ahead(&self, pos: [f64; 3], yaw: f32, sprinting: bool) -> bool {
        let reach: &[f64] = if sprinting { &[0.7, 1.4] } else { &[0.7] };
        reach.iter().all(|&d| self.companion_safe_at(pos, yaw, d))
    }

    pub(super) fn companion_safe_at(&self, pos: [f64; 3], yaw: f32, ahead: f64) -> bool {
        let r = f64::from(yaw).to_radians();
        let (x, z) = (
            (pos[0] - r.sin() * ahead).floor() as i32,
            (pos[2] + r.cos() * ahead).floor() as i32,
        );
        let y = pos[1].floor() as i32;
        let facts = self.res.facts();
        let danger = |name: &str| {
            matches!(
                name,
                "minecraft:lava"
                    | "minecraft:fire"
                    | "minecraft:soul_fire"
                    | "minecraft:magma_block"
                    | "minecraft:campfire"
                    | "minecraft:soul_campfire"
                    | "minecraft:cactus"
                    | "minecraft:sweet_berry_bush"
                    | "minecraft:wither_rose"
                    | "minecraft:powder_snow"
            )
        };
        for dy in 0..=2 {
            if danger(&facts.get(self.world.block(x, y + dy, z)).name) {
                return false;
            }
        }
        for dy in 0..=4 {
            let f = facts.get(self.world.block(x, y - dy, z));
            if danger(&f.name) {
                return false;
            }
            if f.full_collision || f.legacy_solid || f.fluid == Fluid::Water {
                return true;
            }
        }
        false
    }

    /// A blow at mob `id` once the attack is charged (`getAttackStrengthScale`
    /// near 1): the guest's attack, aimed at it.
    pub(super) fn companion_strike(&mut self, host: &mut LanHost, b: &mut Body, id: u32) {
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return;
        };
        let Ok(entity) = i32::try_from(id) else {
            return;
        };
        if self.look_at_entity(host, gi, entity).is_none() {
            return;
        }
        // The host's own "last hurt mob" is its owner's (wolves follow it).
        let owner_hurt = self.combat.last_hurt_mob;
        let hit = self
            .as_guest(host, b.conn, |g| {
                g.attack_strength_scale() >= 0.95 && g.attack()
            })
            .unwrap_or(false);
        self.combat.last_hurt_mob = owner_hurt;
        if hit {
            host.mob_players.insert(id, b.conn);
            // Its kill is its own, not its owner's (game/creatures.rs mob_died).
            self.companions.struck.insert(id, self.world.game_time);
        }
        if let Some(g) = host.hub.guest(b.conn) {
            (b.yaw, b.pitch) = (g.yaw, g.pitch);
        }
    }

    /// Its best armour on and its best weapon in hand.
    pub(super) fn companion_gear(&mut self, host: &mut LanHost, b: &mut Body) {
        if b.eating.is_some() || b.work.busy_hands() {
            return;
        }
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return;
        };
        let items = self.res.items();
        let g = &mut host.hub.guests[gi];
        let mut armor: Vec<(usize, Option<ItemStack>, Option<ItemStack>)> = Vec::new();
        for slot in 0..4 {
            let worn = g.armor[slot]
                .as_ref()
                .and_then(|s| gear::armor_score(items, s))
                .map_or(0.0, |(_, v)| v);
            let best = gear::best(&g.inventory, |s| {
                gear::armor_score(items, s)
                    .filter(|(at, _)| *at == slot)
                    .map(|(_, v)| v)
            });
            if let Some((i, v)) = best
                && v > worn
            {
                let new = g.inventory[i].take();
                let old = std::mem::replace(&mut g.armor[slot], new.clone());
                g.inventory[i] = old.clone();
                armor.push((slot, old, new));
            }
        }
        let held = g
            .inventory
            .get(g.selected)
            .and_then(|s| s.as_ref())
            .and_then(|s| gear::weapon_score(items, s));
        let mut weapon = None;
        if let Some((i, v)) = gear::best(&g.inventory, |s| gear::weapon_score(items, s))
            && held.is_none_or(|h| v > h)
        {
            if i < 9 {
                g.selected = i;
            } else {
                let selected = g.selected;
                g.inventory.swap(i, selected);
            }
            weapon = g.inventory[g.selected].as_ref().map(|s| s.item.to_string());
        }
        for (slot, old, new) in armor {
            let name = new.as_ref().map(|s| s.item.to_string()).unwrap_or_default();
            self.as_guest(host, b.conn, |game| {
                game.player_on_equip(ARMOR_SLOTS[slot], old.as_ref(), new.as_ref());
            });
            self.companion_report(b, CompanionReport::Equipped { item: name });
        }
        if let Some(item) = weapon {
            self.companion_report(b, CompanionReport::Equipped { item });
        }
    }

    /// Start eating its best food: into the hotbar, then used as a client's
    /// use-item packet would. False when it carries nothing worth eating.
    fn companion_start_eating(&mut self, host: &mut LanHost, b: &mut Body) -> bool {
        if b.eating.is_some() {
            return true;
        }
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return false;
        };
        let items = self.res.items();
        let g = &mut host.hub.guests[gi];
        let Some((i, _)) = gear::best(&g.inventory, |s| gear::food_score(items, s)) else {
            return false;
        };
        let slot = if i < 9 {
            i
        } else {
            g.inventory.swap(i, 8);
            8
        };
        let item = g.inventory[slot]
            .as_ref()
            .map(|s| s.item.to_string())
            .unwrap_or_default();
        let (count, yaw, pitch) = (carried(&g.inventory, &item), g.yaw, g.pitch);
        self.lan_guest_packet(
            host,
            b.conn,
            S::SetCarriedItem(play::SetCarriedItem { slot: slot as i16 }),
        );
        self.lan_guest_packet(
            host,
            b.conn,
            S::UseItem(play::UseItem {
                hand: 0,
                sequence: 0,
                yaw,
                pitch,
            }),
        );
        let started = host
            .play
            .get(&b.conn)
            .and_then(super::lan_actions::GuestPlay::using)
            .is_some();
        if started {
            b.eating = Some((count, item));
        }
        started
    }

    /// Pickups the guest tick made for a companion.
    pub(super) fn companion_picked_up(
        &mut self,
        host: &mut LanHost,
        conn: u32,
        picked: Vec<(String, u32)>,
    ) {
        let Some(bi) = host.companions.iter().position(|b| b.conn == conn) else {
            return;
        };
        let mut b = host.companions.remove(bi);
        for (item, count) in picked {
            self.companion_report(&mut b, CompanionReport::PickedUp { item, count });
        }
        host.companions.insert(bi, b);
    }

    /// Its body died (the guest tick saw it): what it carried drops where it
    /// fell unless the world keeps inventories, and it respawns after
    /// `RESPAWN_TICKS`: beside its owner if they are with it, else at its bed
    /// or the world spawn.
    pub(super) fn companion_died(&mut self, host: &mut LanHost, conn: u32) {
        let Some(bi) = host.companions.iter().position(|b| b.conn == conn) else {
            return;
        };
        let keep = self.ui.rules.bool("keep_inventory");
        let Some(g) = host.hub.guest_mut(conn) else {
            return;
        };
        let at = [g.position[0], g.position[1] + 1.62 - 0.3, g.position[2]];
        let mut stacks = Vec::new();
        if !keep {
            // Inventory.dropAll.
            stacks.extend(g.inventory.iter_mut().filter_map(Option::take));
            stacks.extend(g.armor.iter_mut().filter_map(Option::take));
            stacks.extend(g.offhand.take());
            stacks.extend(g.carried.take());
        }
        let mut b = host.companions.remove(bi);
        for stack in stacks {
            // Player.drop(stack, true): thrown in a random direction.
            let f = b.next_float() * 0.5;
            let angle = b.next_float() * std::f64::consts::TAU;
            self.spawn_item(stack, at, [-angle.sin() * f, 0.2, angle.cos() * f], 40);
        }
        let cause = b
            .hurt_by
            .and_then(|(id, _)| self.mobs.iter().find(|m| m.id == id))
            .map_or_else(|| "hurt".to_owned(), |m| c::short_id(&m.kind.id).to_owned());
        b.dead_ticks = Some(0);
        b.target = None;
        b.ordered = false;
        b.eating = None;
        b.retreating = false;
        b.nav.reset();
        self.companion_cancel_work(host, &mut b, Some("died"));
        self.companion_report(&mut b, CompanionReport::Died { cause });
        host.companions.insert(bi, b);
    }

    /// Back to life beside the owner.
    fn companion_respawn(&mut self, host: &mut LanHost, b: &mut Body, owner: [f64; 3]) {
        let at = self.companion_spot(b, owner).unwrap_or(owner);
        self.guest_respawn_at(host, b.conn, Some(at));
        self.companion_revived(host, b);
    }

    /// Back to life without its owner, as a player comes back
    /// (`PlayerList.respawn`): at its bed, else the world spawn, in the
    /// Overworld. False when another level is in place: its body waits there,
    /// parked, for its owner.
    fn companion_respawn_alone(&mut self, host: &mut LanHost, b: &mut Body) -> bool {
        self.guest_respawn_at(host, b.conn, None);
        self.companion_revived(host, b);
        if self.dim == Dim::Overworld {
            return true;
        }
        // A post it held belongs to the level it left.
        (b.dim, b.post) = (Dim::Overworld, None);
        false
    }

    /// What a respawn leaves the body with: loaded, no pending teleport, its
    /// fight and route forgotten.
    fn companion_revived(&mut self, host: &mut LanHost, b: &mut Body) {
        if let Some(g) = host.hub.guest_mut(b.conn) {
            g.loaded = true;
            g.awaiting_teleport = None;
            b.last_health = g.health;
        }
        b.dead_ticks = None;
        b.hurt_by = None;
        b.nav.reset();
        if b.skill == Skill::Attack {
            b.skill = b.resume;
        }
        self.companion_report(b, CompanionReport::Respawned);
    }

    /// `TamableAnimal.tryToTeleportToOwner`'s ten tries for a spot 2 to 3
    /// blocks from the owner: a full block below (not leaves), no fluid, room
    /// for a player.
    fn companion_spot(&self, b: &mut Body, at: [f64; 3]) -> Option<[f64; 3]> {
        let base = at.map(|v| v.floor() as i32);
        let w = self.mob_world_base(&[]);
        let fits = |p: [i32; 3]| {
            let below = w.facts.get(w.world.block(p[0], p[1] - 1, p[2]));
            if !below.full_collision || below.name.ends_with("_leaves") {
                return None;
            }
            if w.facts.get(w.world.block(p[0], p[1], p[2])).fluid != Fluid::None {
                return None;
            }
            let (x, y, z) = (
                f64::from(p[0]) + 0.5,
                f64::from(p[1]),
                f64::from(p[2]) + 0.5,
            );
            w.no_collision(Aabb::new(
                Vec3::new(x - 0.3, y, z - 0.3),
                Vec3::new(x + 0.3, y + 1.8, z + 0.3),
            ))
            .then_some([x, y, z])
        };
        for _ in 0..10 {
            let xd = b.next_int(7) - 3;
            let zd = b.next_int(7) - 3;
            if xd.abs() < 2 && zd.abs() < 2 {
                continue;
            }
            let yd = b.next_int(3) - 1;
            if let Some(spot) = fits([base[0] + xd, base[1] + yd, base[2] + zd]) {
                return Some(spot);
            }
        }
        // The owner on a tree or a ledge: the nearest floor a few blocks
        // around and below, so it never stays far behind for want of luck.
        let mut best: Option<(i32, [f64; 3])> = None;
        for xd in -3i32..=3 {
            for zd in -3i32..=3 {
                if xd.abs() < 2 && zd.abs() < 2 {
                    continue;
                }
                for yd in (-6..=2).rev() {
                    if let Some(spot) = fits([base[0] + xd, base[1] + yd, base[2] + zd]) {
                        let cost = xd * xd + zd * zd + yd * yd;
                        if best.is_none_or(|(c, _)| cost < c) {
                            best = Some((cost, spot));
                        }
                        break;
                    }
                }
            }
        }
        best.map(|(_, spot)| spot)
    }

    /// Teleport beside the owner, if a spot is free.
    pub(super) fn companion_place_near(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        at: [f64; 3],
    ) -> bool {
        match self.companion_spot(b, at) {
            Some(to) => {
                self.companion_move_to(host, b, to);
                true
            }
            None => false,
        }
    }

    /// Put the body at `to` at once.
    pub(super) fn companion_move_to(&mut self, host: &mut LanHost, b: &mut Body, to: [f64; 3]) {
        if let Some(g) = host.hub.guest_mut(b.conn) {
            g.position = to;
            g.previous_position = to;
            g.awaiting_teleport = None;
            if let Some(m) = g.motion.as_mut() {
                m.position = Vec3::new(to[0], to[1], to[2]);
                m.old_position = m.position;
                m.velocity = Vec3::ZERO;
                m.fall_distance = 0.0;
            }
        }
        b.nav = Nav::default();
    }

    /// The body's physics for the tick: the player's own movement
    /// (`LocalPlayer.aiStep` with the keys its skill pressed, then the
    /// server's hazards and effects), run as the guest.
    pub(super) fn companion_body_tick(&mut self, host: &mut LanHost, conn: u32) {
        let Some(b) = host.companions.iter().find(|b| b.conn == conn) else {
            return;
        };
        let (keys, yaw, pitch) = (b.keys, b.yaw, b.pitch);
        let Some(g) = host.hub.guest_mut(conn) else {
            return;
        };
        g.moves.clear();
        // Its ground must be there: a body in a chunk not loaded yet waits;
        // one in bed lies still.
        if g.spectator()
            || g.bed.is_some()
            || !self
                .world
                .loaded(g.position[0].floor() as i32, g.position[2].floor() as i32)
        {
            return;
        }
        (g.yaw, g.pitch, g.head_yaw) = (yaw, pitch.clamp(-90.0, 90.0), yaw);
        let before = g.position;
        let in_wall = !g.sleeping && self.in_wall(g.position, g.eye_height());
        let nether = self.dim == Dim::Nether;
        let out = self
            .as_guest(host, conn, |game| {
                let respiration = game.armor[0]
                    .as_ref()
                    .map_or(0, |s| s.enchantment_level("minecraft:respiration") as i32);
                let leather_boots = game.armor[3]
                    .as_ref()
                    .is_some_and(|s| &*s.item == "minecraft:leather_boots");
                let immune = game
                    .res
                    .items()
                    .item_tags
                    .get("minecraft:freeze_immune_wearables");
                let freeze_immune = game
                    .armor
                    .iter()
                    .flatten()
                    .any(|s| immune.is_some_and(|t| t.contains(&*s.item)));
                let use_speed = game
                    .using
                    .and_then(|(slot, _)| game.inventory.get(slot)?.as_ref())
                    .and_then(|s| game.res.items().get(&s.item))
                    .map(|i| i.use_speed);
                let (yaw, pitch) = (game.yaw, game.pitch);
                let player = game.player.as_mut()?;
                player.yaw = yaw;
                player.pitch = pitch;
                player.may_fly = false;
                player.invulnerable = false;
                player.spectator = false;
                player.auto_jump_enabled = true;
                player.respiration = respiration;
                player.glider = false;
                player.item_use_speed = use_speed;
                player.leather_boots = leather_boots;
                player.freeze_immune = freeze_immune;
                player.fluids.fast_lava = nether;
                let mut w = MotionWorld {
                    entities: &[],
                    player,
                    scene: &mut game.world,
                    assets: &game.pack.assets,
                    collision_binding: game.collision,
                    resources: game.motion,
                };
                let _ = w.controlled_simulation_tick(keys);
                let _ = w.server_hazards_tick(in_wall);
                let events = std::mem::take(&mut w.player.events);
                let p = &*w.player;
                let state = (
                    [p.position.x, p.position.y, p.position.z],
                    p.ground.on_ground,
                    p.sprinting,
                    p.pose == horizon_client::pose::Pose::Crouching,
                );
                game.tick_mob_effects();
                let sounds: Vec<_> = events
                    .into_iter()
                    .filter_map(|e| match e {
                        MotionEvent::HurtSound(sound) => Some(sound),
                        _ => None,
                    })
                    .collect();
                Some((state, sounds))
            })
            .flatten();
        let Some(((at, on_ground, sprinting, crouching), sounds)) = out else {
            return;
        };
        if let Some(g) = host.hub.guest_mut(conn) {
            // Moved by the server, not teleported: no confirmation to wait for.
            g.awaiting_teleport = None;
            g.previous_position = before;
            g.position = at;
            g.on_ground = on_ground;
            g.sprinting = sprinting;
            g.sneaking = crouching;
            g.moved = true;
        }
        for sound in sounds {
            self.player_hurt_sound(sound, at);
        }
    }
}

/// The pitch that looks from `eye` at `to`.
pub(super) fn look_pitch(eye: [f64; 3], to: [f64; 3]) -> f32 {
    let horizontal = c::horizontal(eye, to);
    (-(to[1] - eye[1]).atan2(horizontal)).to_degrees() as f32
}

/// How many of `item` it carries.
pub(super) fn carried(inventory: &[Option<ItemStack>], item: &str) -> u32 {
    inventory
        .iter()
        .flatten()
        .filter(|s| &*s.item == item)
        .map(|s| s.count)
        .sum()
}
