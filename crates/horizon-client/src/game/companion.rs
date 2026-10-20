//! Companions (docs/companions.md): an AI teammate in a player's body. The
//! body is a LAN guest with no connection (`Guest::companion`): the hub
//! admits, saves and shows it like any player, so vanilla clients see a
//! player; the host's game moves it (game/companion_body.rs). A world that is
//! not open to LAN gets a hub that doesn't listen (`LanHost::private`), which
//! "Open to LAN" later makes listen.
//!
//! This file is the companion's life: spawning and dismissing it, saving it
//! with the world and bringing it back, the `/companion` command and the
//! control socket, and what the brain reads and sends (`companion_command`,
//! `companion_reports`, `companion_state_summary`).
use super::{Dim, Game, lan::LanHost};
use horizon_client::{
    companion::{
        self as c, CompanionCommand, CompanionReport, Skill,
        file::CompanionFile,
        summary::{self, Nearby, OwnerView, Perception},
    },
    lan::{self, hub::TickCtx, playerdata},
    mobs::path::Path,
    sprint::Keys,
    text::{self, Text},
};
use horizon_protocol::crypto::uuid_string;
use serde_json::{Value, json};
use std::collections::VecDeque;

/// Chunks kept loaded around a companion: it sees nothing, so only the
/// ground it walks and fights on.
pub(super) const LOADED_RADIUS: i32 = 3;
/// Reports kept for the brain to read before the oldest go.
const REPORTS: usize = 256;
/// Events a body remembers for its state summary.
const RECENT: usize = 8;

/// A companion's body in the hub (`LanHost::companions`) and the skill it runs.
pub(super) struct Body {
    pub conn: u32,
    pub uuid: u128,
    pub file: CompanionFile,
    pub skill: Skill,
    /// The skill to go back to once a fight it was told to start is over.
    pub resume: Skill,
    pub hold_fire: bool,
    pub commands: VecDeque<CompanionCommand>,
    /// Where Stay holds and GoTo goes.
    pub post: Option<[f64; 3]>,
    /// The entity it fights, and whether it was told to (not just defending).
    pub target: Option<u32>,
    pub ordered: bool,
    pub nav: Nav,
    /// Follow is on its way to the owner (`follow_step`'s hysteresis).
    pub moving: bool,
    /// What its "player" presses this tick, and where it looks.
    pub keys: Keys,
    pub yaw: f32,
    pub pitch: f32,
    pub crouch: bool,
    pub jump: bool,
    pub retreating: bool,
    /// Eating: its food level when it started, and the food.
    pub eating: Option<(u32, String)>,
    /// Ticks since it died.
    pub dead_ticks: Option<u32>,
    /// The level it is really in: the in-window hub has one level, the one
    /// in place, which changes under it when the owner changes dimension.
    pub dim: Dim,
    /// It was within `COME_ALONG` of the owner last tick.
    pub near_owner: bool,
    pub last_health: f32,
    /// The mob that hurt it last, and when.
    pub hurt_by: Option<(u32, i64)>,
    /// Hostiles already called out.
    pub seen: Vec<u32>,
    /// The dropped item it is walking to, and since when; items it gave up
    /// on (in a nook it cannot reach).
    pub reaching: Option<(u32, u64)>,
    pub out_of_reach: Vec<u32>,
    pub recent: VecDeque<String>,
    pub random: u64,
    pub ticks: u64,
}

/// What the hub costs per tick while companions are in it: its whole tick
/// (their skills, bodies, pickups) and the mobs' view of them.
#[derive(Default)]
pub(super) struct Cost {
    ticks: u32,
    total: std::time::Duration,
    max: std::time::Duration,
    views: std::time::Duration,
}

impl Cost {
    pub fn hub(&mut self, d: std::time::Duration) {
        self.ticks += 1;
        self.total += d;
        self.max = self.max.max(d);
    }
    pub fn views(&mut self, d: std::time::Duration) {
        self.total += d;
        self.views += d;
    }
    /// Mean and worst milliseconds a tick, then clears.
    fn take(&mut self) -> Value {
        let n = self.ticks.max(1) as f64;
        let v = json!({
            "ticks": self.ticks,
            "mean_ms": self.total.as_secs_f64() * 1000.0 / n,
            "max_ms": self.max.as_secs_f64() * 1000.0,
            "views_ms": self.views.as_secs_f64() * 1000.0 / n,
        });
        *self = Self::default();
        v
    }
}

/// Path following (game/companion_body.rs).
#[derive(Default)]
pub(super) struct Nav {
    pub path: Option<Path>,
    /// What the path was planned to.
    pub goal: [f64; 3],
    pub planned_at: u64,
    /// The nearest it got to the goal, and when.
    pub best: f64,
    pub best_at: u64,
    pub jump_until: u64,
    pub fails: u32,
    /// Plans in a row that fell short of a goal that barely moved.
    pub misses: u32,
}

impl Nav {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Body {
    fn new(conn: u32, uuid: u128, file: CompanionFile, dim: Dim) -> Self {
        let skill = match Skill::from_name(&file.skill) {
            Some(s @ (Skill::Idle | Skill::Stay | Skill::Defend)) => s,
            _ => Skill::Follow,
        };
        let post = file
            .post
            .filter(|_| matches!(skill, Skill::Stay | Skill::Defend));
        let hold_fire = file.hold_fire;
        Self {
            conn,
            uuid,
            file,
            skill,
            resume: Skill::Follow,
            hold_fire,
            commands: VecDeque::new(),
            post,
            target: None,
            ordered: false,
            nav: Nav::default(),
            moving: false,
            keys: Keys::default(),
            yaw: 0.0,
            pitch: 0.0,
            crouch: false,
            jump: false,
            retreating: false,
            eating: None,
            dead_ticks: None,
            dim,
            near_owner: true,
            last_health: 20.0,
            hurt_by: None,
            seen: Vec::new(),
            reaching: None,
            out_of_reach: Vec::new(),
            recent: VecDeque::new(),
            random: uuid as u64 ^ (uuid >> 64) as u64 | 1,
            ticks: 0,
        }
    }
    /// Put what it is doing in its file: a fight or a retreat resumes what
    /// came before; arriving somewhere it stays.
    pub fn keep_skill(&mut self) {
        let skill = match self.skill {
            Skill::Attack | Skill::Retreat => self.resume,
            Skill::Come => Skill::Follow,
            Skill::GoTo => Skill::Stay,
            s => s,
        };
        let skill = match skill {
            s @ (Skill::Idle | Skill::Stay | Skill::Defend) => s,
            _ => Skill::Follow,
        };
        self.file.skill = skill.name().to_owned();
        self.file.post = self
            .post
            .filter(|_| matches!(skill, Skill::Stay | Skill::Defend));
        self.file.hold_fire = self.hold_fire;
    }
    /// xorshift64*: the body's own randomness (teleport spots, item
    /// scatter), so that a companion never draws from the level's random.
    pub fn next_random(&mut self) -> u64 {
        let mut x = self.random;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.random = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// A random integer in `0..bound`.
    pub fn next_int(&mut self, bound: u32) -> i32 {
        (self.next_random() >> 33) as i32 % bound as i32
    }
    pub fn next_float(&mut self) -> f64 {
        (self.next_random() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The companions of the open world that are not in its hub.
#[derive(Default)]
pub(super) struct Companions {
    /// Those saved with the world, to come back once its player stands in it.
    pub pending: Vec<CompanionFile>,
    /// Those that stayed in another dimension.
    pub parked: Vec<(CompanionFile, Dim)>,
    /// What the bodies told the brain: the companion's name and the report.
    pub reports: VecDeque<(String, CompanionReport)>,
    /// Mobs a companion hit, and when: their deaths are its kills.
    pub struck: std::collections::HashMap<u32, i64>,
    /// A companion's body is swapped in (`enter_guest`): its advancements
    /// are not announced.
    pub acting: bool,
    /// Path searches since the last `companion cost`, and their time.
    pub plans: (u32, std::time::Duration, std::time::Duration),
}

impl Companions {
    /// Whether mob `id` died of a companion's blow: it hit last, after the
    /// owner did.
    pub fn killed(&mut self, id: u32, owner_hit: Option<(u32, i64)>) -> bool {
        if self.struck.is_empty() {
            return false;
        }
        self.struck
            .remove(&id)
            .is_some_and(|at| owner_hit.is_none_or(|(mob, when)| mob != id || when < at))
    }
}

impl Game<'_> {
    /// A report from a body: kept for the brain, and in its recent events.
    pub(super) fn companion_report(&mut self, b: &mut Body, report: CompanionReport) {
        let line = report.text();
        log::info!("[companion] {}: {line}", b.file.name);
        if b.recent.len() == RECENT {
            b.recent.pop_front();
        }
        b.recent.push_back(line);
        if self.companions.reports.len() == REPORTS {
            self.companions.reports.pop_front();
        }
        self.companions
            .reports
            .push_back((b.file.name.clone(), report));
    }

    /// Whether companions can play here: a world this window runs in its
    /// frame, with its player in it.
    fn companions_supported(&self) -> std::result::Result<(), String> {
        if self.remote.is_some() || self.integrated.is_some() {
            return Err("companions play in your own world, not on a server".into());
        }
        if self.serving() || self.options.dedicated.is_some() || self.title.is_some() {
            return Err("companions need a world with you in it".into());
        }
        if self.player.is_none() {
            return Err("you are not in the world yet".into());
        }
        Ok(())
    }

    /// `/companion spawn [name]`: a companion joins beside its owner. A name
    /// this world's companions already have brings that one back.
    pub(super) fn companion_spawn(
        &mut self,
        name: Option<&str>,
    ) -> std::result::Result<String, String> {
        self.companions_supported()?;
        let wanted = match name {
            Some(n) => Some(c::valid_name(n).ok_or("a name is 3 to 16 letters, digits or _")?),
            None => None,
        };
        let files = CompanionFile::load_all(&self.world_root);
        let known = wanted
            .as_ref()
            .and_then(|n| files.iter().find(|f| f.name.eq_ignore_ascii_case(n)))
            .cloned();
        if self.lan.is_none() {
            self.lan_start_private().map_err(|e| e.to_string())?;
        }
        let host = self.lan.as_ref().ok_or("no hub")?;
        let taken = |n: &str| {
            host.hub
                .guests
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(n))
                || host.host_name.eq_ignore_ascii_case(n)
        };
        let file = match known {
            Some(f) => {
                if host.companions.iter().any(|b| b.file.uuid == f.uuid) {
                    return Err(format!("{} is already here", f.name));
                }
                self.companions.parked.retain(|(p, _)| p.uuid != f.uuid);
                self.companions.pending.retain(|p| p.uuid != f.uuid);
                f
            }
            None => {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(7, |d| d.as_nanos() as u64)
                    ^ files.len() as u64;
                let uuid = c::uuid_for_skin(seed, None);
                let name = match wanted {
                    Some(n) => n,
                    None => {
                        // Named after the default skin it wears.
                        let skin = horizon_client::tab_list::default_skin(uuid);
                        let base = skin.rsplit('/').next().unwrap_or("Ari");
                        let mut name = format!("{}{}", base[..1].to_uppercase(), &base[1..]);
                        let mut n = 2;
                        while taken(&name)
                            || files.iter().any(|f| f.name.eq_ignore_ascii_case(&name))
                        {
                            name = format!("{}{n}", &name[..base.len()]);
                            n += 1;
                        }
                        name
                    }
                };
                CompanionFile {
                    uuid: uuid_string(uuid),
                    name,
                    owner: uuid_string(host.host_uuid),
                    ..Default::default()
                }
            }
        };
        if taken(&file.name) {
            return Err(format!("a player called {} is here", file.name));
        }
        let name = file.name.clone();
        self.companion_join(file, true)?;
        Ok(name)
    }

    /// Bring a companion's body into the hub: admitted and placed like a
    /// network player (its saved body loaded), then, `beside_owner`, moved
    /// next to its owner.
    fn companion_join(
        &mut self,
        mut file: CompanionFile,
        beside_owner: bool,
    ) -> std::result::Result<(), String> {
        let uuid = file.uuid().ok_or("the companion's file has no UUID")?;
        let Some(owner) = self
            .player
            .as_ref()
            .map(|p| [p.position.x, p.position.y, p.position.z])
        else {
            return Err("you are not in the world yet".into());
        };
        let mut host = self.lan.take().ok_or("no hub")?;
        if host.hub.guests.iter().any(|g| g.uuid == uuid) {
            self.lan = Some(host);
            return Err(format!("{} is already here", file.name));
        }
        let saved = playerdata::exists(&self.world_root, uuid);
        let conn = host.hub.lan.next_conn();
        host.hub.lan.reserve(uuid);
        let joined = lan::Joined {
            conn,
            uuid,
            name: file.name.clone(),
            properties: Vec::new(),
            info: horizon_protocol::packets::configuration::ClientInformation::new(2),
            brand: "horizon-companion".into(),
            sender: lan::PlayerSender::sink(),
            latency: Default::default(),
            address: std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
        };
        {
            let clock_rate = self.network_clock_rate();
            let mut ctx = TickCtx {
                world: &mut self.world,
                host: None,
                entities: &[],
                next_entity: &mut self.next_entity,
                spawn: owner,
                difficulty: i32::from(self.ui.difficulty),
                difficulty_locked: self.level.difficulty_locked,
                clock_rate,
                tick_rate: self.clock.rate(),
                tick_frozen: self.clock.frozen(),
                collision: Some(self.collision),
            };
            host.hub.admit_companion(joined, &mut ctx);
        }
        let state = self.lan_host_state(&host);
        self.lan_guest_joined(&mut host, conn, state.as_ref(), &[], owner);
        let Some(g) = host.hub.guest_mut(conn) else {
            host.hub.lan.unreserve(uuid);
            self.lan = Some(host);
            return Err("the body could not join".into());
        };
        // It plays survival whatever the world's mode: mobs see it, it can be hurt.
        g.loaded = true;
        let health = g.health;
        if g.game_mode != 0 {
            host.hub.set_game_mode(conn, 0);
        }
        let mut b = Body::new(conn, uuid, file.clone(), self.dim);
        b.last_health = health;
        if beside_owner {
            // Called back by name: with its owner, whatever it did before.
            (b.skill, b.post) = (Skill::Follow, None);
        }
        if (beside_owner || !saved) && !self.companion_place_near(&mut host, &mut b, owner) {
            self.companion_move_to(&mut host, &mut b, owner);
        }
        file.active = true;
        if let Err(e) = file.save(&self.world_root) {
            log::warn!("[companion] could not save {}: {e}", file.name);
        }
        let skill = b.skill;
        self.companion_report(&mut b, CompanionReport::SkillStarted(skill));
        host.companions.push(b);
        self.lan = Some(host);
        Ok(())
    }

    /// `/companion dismiss [name]`: the body leaves the world (saved, so the
    /// same name brings it back). The hub goes with the last companion when
    /// the world is not open to LAN.
    pub(super) fn companion_dismiss(
        &mut self,
        name: Option<&str>,
    ) -> std::result::Result<String, String> {
        let Some(mut host) = self.lan.take() else {
            return Err("no companion here".into());
        };
        let Some(bi) = companion_index(&host, name) else {
            self.lan = Some(host);
            return Err("no companion by that name".into());
        };
        let b = self.companion_leave(&mut host, bi, false);
        let close = host.companions.is_empty() && host.private;
        self.lan = Some(host);
        if close {
            self.lan_close(false);
        }
        Ok(b.file.name)
    }

    /// Take a body out of the hub, saved: its player data like a guest's,
    /// and its file (`active`: it comes back with the world).
    fn companion_leave(&mut self, host: &mut LanHost, bi: usize, active: bool) -> Body {
        let mut b = host.companions.remove(bi);
        if let Some(g) = host.hub.guest_mut(b.conn) {
            // Saved in the level it is really in.
            g.dimension = b.dim.id().to_owned();
        }
        if let Some(g) = host.hub.guest(b.conn) {
            self.save_guest(host, g);
        }
        host.hub.remove_companion(b.conn);
        host.hub.lan.unreserve(b.uuid);
        host.models.remove(&b.conn);
        host.play.remove(&b.conn);
        host.mob_players.retain(|_, conn| *conn != b.conn);
        b.file.active = active;
        b.keep_skill();
        if let Err(e) = b.file.save(&self.world_root) {
            log::warn!("[companion] could not save {}: {e}", b.file.name);
        }
        b
    }

    /// Leave a body in the level it is in while its owner goes elsewhere.
    pub(super) fn companion_park(&mut self, host: &mut LanHost, bi: usize) {
        let dim = host.companions[bi].dim;
        let mut b = self.companion_leave(host, bi, true);
        self.companion_report(
            &mut b,
            CompanionReport::StayedBehind {
                dimension: dim.id().to_owned(),
            },
        );
        self.companions.parked.push((b.file, dim));
    }

    /// The world closes: every companion is saved and leaves, to come back
    /// with it (before the hub closes, so that it closes for good).
    pub(super) fn companions_leave_world(&mut self) {
        let pending = std::mem::take(&mut self.companions);
        // Those not in the hub keep their files as they are.
        drop(pending);
        let Some(mut host) = self.lan.take() else {
            return;
        };
        while !host.companions.is_empty() {
            self.companion_leave(&mut host, 0, true);
        }
        self.lan = Some(host);
    }

    /// `save_game`: each body's player data and file.
    pub(super) fn save_companions(&mut self) {
        let Some(mut host) = self.lan.take() else {
            return;
        };
        let in_place = host.hub.config.dimension.clone();
        for bi in 0..host.companions.len() {
            let (conn, dim) = (host.companions[bi].conn, host.companions[bi].dim);
            // Saved in the level it is really in.
            if let Some(g) = host.hub.guest_mut(conn) {
                g.dimension = dim.id().to_owned();
            }
            if let Some(g) = host.hub.guest(conn) {
                self.save_guest(&host, g);
            }
            if let Some(g) = host.hub.guest_mut(conn) {
                g.dimension = in_place.clone();
            }
            host.companions[bi].keep_skill();
            let file = &host.companions[bi].file;
            if let Err(e) = file.save(&self.world_root) {
                log::warn!("[companion] could not save {}: {e}", file.name);
            }
        }
        self.lan = Some(host);
    }

    /// A world opened: its active companions come back once its player is in it.
    pub(super) fn companions_world_opened(&mut self) {
        self.companions = Default::default();
        if self.remote.is_some() || self.options.dedicated.is_some() || self.options.integrated {
            return;
        }
        self.companions.pending = CompanionFile::load_all(&self.world_root)
            .into_iter()
            .filter(|f| f.active)
            .collect();
    }

    /// Per tick before the hub's: the companions saved with the world come
    /// back, and those left in a dimension the owner is back in rejoin.
    pub(super) fn tick_companion_arrivals(&mut self) {
        if self.companions.pending.is_empty() && self.companions.parked.is_empty() {
            return;
        }
        let Some(p) = self.player.as_ref() else {
            return;
        };
        // Once the ground under the owner is there.
        let (x, z) = (p.position.x.floor() as i32, p.position.z.floor() as i32);
        if !self.world.loaded(x, z) || self.companions_supported().is_err() {
            return;
        }
        for file in std::mem::take(&mut self.companions.pending) {
            let Some(uuid) = file.uuid() else {
                continue;
            };
            let dim = playerdata::saved_dimension(&self.world_root, uuid)
                .map_or(self.dim, |d| Dim::from_id(&d));
            if dim != self.dim {
                self.companions.parked.push((file, dim));
                continue;
            }
            if self.lan.is_none()
                && let Err(e) = self.lan_start_private()
            {
                log::warn!("[companion] no hub for {}: {e}", file.name);
                continue;
            }
            if let Err(e) = self.companion_join(file, false) {
                log::warn!("[companion] could not bring back a companion: {e}");
            }
        }
        let here = self.dim;
        let (back, away): (Vec<_>, Vec<_>) = std::mem::take(&mut self.companions.parked)
            .into_iter()
            .partition(|(_, d)| *d == here);
        self.companions.parked = away;
        for (file, _) in back {
            if self.lan.is_none() && self.lan_start_private().is_err() {
                continue;
            }
            let name = file.name.clone();
            match self.companion_join(file, false) {
                Ok(()) => {
                    if let Some(mut host) = self.lan.take() {
                        if let Some(bi) = companion_index(&host, Some(&name)) {
                            let mut b = host.companions.remove(bi);
                            self.companion_report(
                                &mut b,
                                CompanionReport::ChangedDimension {
                                    dimension: here.id().to_owned(),
                                },
                            );
                            host.companions.insert(bi, b);
                        }
                        self.lan = Some(host);
                    }
                }
                Err(e) => log::warn!("[companion] {name} could not rejoin: {e}"),
            }
        }
    }

    // ---- the brain's side -------------------------------------------------

    /// Queue a command for a companion (the first one when `name` is none);
    /// it applies at the start of its next tick.
    pub(super) fn companion_command(
        &mut self,
        name: Option<&str>,
        command: CompanionCommand,
    ) -> std::result::Result<String, String> {
        let host = self.lan.as_mut().ok_or("no companion here")?;
        let bi = companion_index(host, name).ok_or("no companion by that name")?;
        let b = &mut host.companions[bi];
        b.commands.push_back(command);
        Ok(b.file.name.clone())
    }

    /// The reports since the last call, oldest first.
    pub(super) fn companion_reports(&mut self) -> Vec<(String, CompanionReport)> {
        self.companions.reports.drain(..).collect()
    }

    /// The compact state text the brain reads (`summary::state_summary`).
    pub(super) fn companion_state_summary(&self, name: Option<&str>) -> Option<String> {
        self.companion_perception(name)
            .map(|p| summary::state_summary(&p))
    }

    fn companion_perception(&self, name: Option<&str>) -> Option<Perception> {
        let host = self.lan.as_ref()?;
        let b = &host.companions[companion_index(host, name)?];
        let g = host.hub.guest(b.conn)?;
        let items = self.res.items();
        let mut inventory: Vec<(String, u32)> = Vec::new();
        for s in g
            .inventory
            .iter()
            .chain(g.armor.iter())
            .chain([&g.offhand])
            .flatten()
        {
            match inventory.iter_mut().find(|(id, _)| **id == *s.item) {
                Some(e) => e.1 += s.count,
                None => inventory.push((s.item.to_string(), s.count)),
            }
        }
        inventory.sort_by_key(|a| std::cmp::Reverse(a.1));
        let armor = g
            .armor
            .iter()
            .flatten()
            .filter_map(|s| items.get(&s.item))
            .flat_map(|i| i.modifiers.iter())
            .filter(|m| m.0 == "minecraft:armor")
            .map(|m| m.2)
            // An empty f64 sum is -0, which prints "-0".
            .fold(0.0, |a, b| a + b);
        let owner_at = self
            .player
            .as_ref()
            .map(|p| [p.position.x, p.position.y, p.position.z]);
        let from = owner_at.unwrap_or(g.position);
        let yaw = if owner_at.is_some() { self.yaw } else { g.yaw };
        let mut entities: Vec<Nearby> = self
            .mobs
            .iter()
            .filter(|m| m.alive())
            .map(|m| (m, [m.position.x, m.position.y, m.position.z]))
            .filter(|(_, at)| c::distance(from, *at) <= 24.0)
            .map(|(m, at)| Nearby {
                id: m.kind.id.to_string(),
                hostile: m.hostile(),
                bearing: c::clock_bearing(from, yaw, at),
                distance: c::distance(from, at),
            })
            .collect();
        for other in host
            .hub
            .guests
            .iter()
            .filter(|o| o.conn != b.conn && o.placed && !o.dead)
        {
            if c::distance(from, other.position) <= 24.0 {
                entities.push(Nearby {
                    id: format!("player {}", other.name),
                    hostile: false,
                    bearing: c::clock_bearing(from, yaw, other.position),
                    distance: c::distance(from, other.position),
                });
            }
        }
        entities.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        let owner = owner_at.zip(self.player.as_ref()).map(|(at, p)| OwnerView {
            name: host.host_name.clone(),
            health: p.health.health(),
            food: p.health.food.food,
            distance: c::distance(at, g.position),
            bearing: c::clock_bearing(at, self.yaw, g.position),
            looking_at: self.owner_looking_at(c::CROSSHAIR_RANGE),
        });
        let (rain, thunder) = self.world.weather.levels(1.0);
        Some(Perception {
            name: b.file.name.clone(),
            position: g.position,
            dimension: b.dim.id().to_owned(),
            biome: self.companion_biome(g.position),
            day_time: self.world.day_time,
            raining: rain > 0.2,
            thundering: thunder > 0.9,
            health: g.health,
            food: g.food,
            armor: armor as f32,
            held: g
                .inventory
                .get(g.selected)
                .and_then(|s| s.as_ref())
                .map(|s| s.item.to_string()),
            inventory,
            owner,
            entities,
            blocks: self.notable_blocks(from, yaw),
            skill: b.skill.name().to_owned(),
            hold_fire: b.hold_fire,
            events: b.recent.iter().cloned().collect(),
        })
    }

    fn companion_biome(&self, p: [f64; 3]) -> String {
        let (x, y, z) = (
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        );
        self.world.chunks.get(&(x >> 4, z >> 4)).map_or_else(
            || "unknown".to_owned(),
            |ch| {
                self.res.biomes().list[ch.biome(x, y, z) as usize]
                    .name
                    .clone()
            },
        )
    }

    /// Ores, containers, beds and lava around the owner: the nearest of each.
    fn notable_blocks(&self, from: [f64; 3], yaw: f32) -> Vec<Nearby> {
        const R: i32 = 6;
        let notable = |name: &str| {
            name.ends_with("_ore")
                || name.ends_with("chest")
                || name.ends_with("_bed")
                || name.ends_with("shulker_box")
                || matches!(
                    name,
                    "minecraft:lava"
                        | "minecraft:spawner"
                        | "minecraft:trial_spawner"
                        | "minecraft:crafting_table"
                        | "minecraft:furnace"
                        | "minecraft:ancient_debris"
                        | "minecraft:barrel"
                )
        };
        let base = from.map(|v| v.floor() as i32);
        let mut found: Vec<Nearby> = Vec::new();
        let facts = self.res.facts();
        for y in -R..=R {
            for z in -R..=R {
                for x in -R..=R {
                    let p = [base[0] + x, base[1] + y, base[2] + z];
                    let f = facts.get(self.world.block(p[0], p[1], p[2]));
                    if !notable(&f.name) {
                        continue;
                    }
                    let at = [
                        f64::from(p[0]) + 0.5,
                        f64::from(p[1]) + 0.5,
                        f64::from(p[2]) + 0.5,
                    ];
                    let d = c::distance(from, at);
                    match found.iter_mut().find(|n| *n.id == *f.name) {
                        Some(n) if n.distance <= d => {}
                        Some(n) => {
                            n.distance = d;
                            n.bearing = c::clock_bearing(from, yaw, at);
                        }
                        None => found.push(Nearby {
                            id: f.name.to_string(),
                            hostile: false,
                            bearing: c::clock_bearing(from, yaw, at),
                            distance: d,
                        }),
                    }
                }
            }
        }
        found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        found
    }

    /// What the owner's crosshair rests on within `range`: a mob or a block,
    /// and how far.
    pub(super) fn owner_looking_at(&self, range: f64) -> Option<(String, f64)> {
        let eye = self.eye_position()?;
        let dir = super::view_vector(self.yaw, self.pitch);
        let block = self.clip_ray(eye, dir, range).map(|h| {
            let d = c::distance(eye, h.point);
            let name = self
                .res
                .facts()
                .get(self.world.block(h.pos[0], h.pos[1], h.pos[2]))
                .name
                .to_string();
            (name, d)
        });
        let limit = block.as_ref().map_or(range, |b| b.1);
        let mob = self
            .mobs
            .iter()
            .filter(|m| m.alive())
            .filter_map(|m| {
                super::ray_box(eye, dir, limit, m.bounds()).map(|d| (m.kind.id.to_string(), d))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        mob.or(block)
    }

    // ---- commands -----------------------------------------------------------

    /// `/companion ...` typed in chat (a dev command): true when it was one.
    pub(super) fn companion_chat_command(&mut self, command: &str) -> bool {
        let words: Vec<&str> = command.split_whitespace().collect();
        if words.first() != Some(&"companion") || self.lan_acting.is_some() {
            return false;
        }
        let reply = match self.companion_words(&words[1..]) {
            Ok(v) => Ok(v
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| v.to_string(), str::to_owned)),
            Err(e) => Err(e),
        };
        match reply {
            Ok(text) => self.system_message(Text::literal(text)),
            Err(e) => self.system_message(Text::literal(e).with_color(text::RED)),
        }
        true
    }

    /// The control socket's `companion ...`.
    pub(super) fn companion_control(
        &mut self,
        words: &[&str],
    ) -> horizon_client::Result<Option<Value>> {
        if words.first() != Some(&"companion") {
            return Ok(None);
        }
        Ok(Some(match self.companion_words(&words[1..]) {
            Ok(v) => v,
            Err(e) => json!({ "error": e }),
        }))
    }

    fn companion_words(&mut self, words: &[&str]) -> std::result::Result<Value, String> {
        let usage = "companion spawn [name] | dismiss [name] | do [name] <command> | list | summary [name] | reports";
        match words.first().copied() {
            Some("spawn") => {
                let name = self.companion_spawn(words.get(1).copied())?;
                Ok(json!({ "ok": true, "name": name, "message": format!("{name} joined you") }))
            }
            Some("dismiss") => {
                let name = self.companion_dismiss(words.get(1).copied())?;
                Ok(json!({ "ok": true, "name": name, "message": format!("{name} left") }))
            }
            Some("do") => {
                let rest = &words[1..];
                // An optional companion name first.
                let named = rest.first().copied().filter(|n| {
                    self.lan.as_ref().is_some_and(|h| {
                        h.companions
                            .iter()
                            .any(|b| b.file.name.eq_ignore_ascii_case(n))
                    })
                });
                let rest = &rest[usize::from(named.is_some())..];
                let command = c::parse_command(rest)?;
                let text = format!("{command:?}");
                let name = self.companion_command(named, command)?;
                Ok(
                    json!({ "ok": true, "name": name, "command": text, "message": format!("{name}: {}", rest.join(" ")) }),
                )
            }
            Some("list" | "status") => {
                let list = self.companion_list();
                let message = if list.is_empty() {
                    "no companions".to_owned()
                } else {
                    list.iter()
                        .map(|v| {
                            format!(
                                "{} ({})",
                                v["name"].as_str().unwrap_or("?"),
                                v["skill"].as_str().unwrap_or("?")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                Ok(json!({ "ok": true, "companions": list, "message": message }))
            }
            Some("summary") => {
                let text = self
                    .companion_state_summary(words.get(1).copied())
                    .ok_or("no companion by that name")?;
                Ok(json!({ "ok": true, "text": text, "message": text }))
            }
            Some("cost") => {
                let host = self.lan.as_mut().ok_or("no companion here")?;
                let mut cost = host.cost.take();
                let (plans, total, max) = std::mem::take(&mut self.companions.plans);
                cost["plans"] = json!(plans);
                cost["plans_ms"] = json!(total.as_secs_f64() * 1000.0);
                cost["plan_max_ms"] = json!(max.as_secs_f64() * 1000.0);
                Ok(json!({ "ok": true, "cost": cost }))
            }
            Some("reports") => {
                let reports: Vec<Value> = self
                    .companion_reports()
                    .into_iter()
                    .map(|(name, r)| json!({ "name": name, "report": format!("{r:?}"), "text": r.text() }))
                    .collect();
                Ok(json!({ "ok": true, "reports": reports }))
            }
            _ => Err(usage.into()),
        }
    }

    /// Each companion's state, for tests and the debug socket.
    fn companion_list(&self) -> Vec<Value> {
        let Some(host) = self.lan.as_ref() else {
            return self
                .companions
                .parked
                .iter()
                .map(|(f, d)| json!({ "name": f.name, "uuid": f.uuid, "parked": d.id() }))
                .collect();
        };
        let mut out: Vec<Value> = host
            .companions
            .iter()
            .filter_map(|b| {
                let g = host.hub.guest(b.conn)?;
                let items: u32 = g.inventory.iter().flatten().map(|s| s.count).sum();
                Some(json!({
                    "name": b.file.name,
                    "uuid": b.file.uuid,
                    "entity_id": g.entity_id,
                    "skin": horizon_client::tab_list::default_skin(b.uuid),
                    "position": g.position,
                    "dimension": b.dim.id(),
                    "health": g.health,
                    "food": g.food,
                    "dead": g.dead,
                    "skill": b.skill.name(),
                    "target": b.target,
                    "hold_fire": b.hold_fire,
                    "retreating": b.retreating,
                    "eating": b.eating.is_some(),
                    "items": items,
                    "held": g.inventory.get(g.selected).and_then(|s| s.as_ref()).map(|s| s.item.to_string()),
                    "armor": g.armor.iter().map(|s| s.as_ref().map(|s| s.item.to_string())).collect::<Vec<_>>(),
                    "game_mode": g.game_mode,
                }))
            })
            .collect();
        out.extend(
            self.companions
                .parked
                .iter()
                .map(|(f, d)| json!({ "name": f.name, "uuid": f.uuid, "parked": d.id() })),
        );
        out
    }
}

/// The body called `name` (any case), or the first one.
pub(super) fn companion_index(host: &LanHost, name: Option<&str>) -> Option<usize> {
    match name {
        Some(n) => host
            .companions
            .iter()
            .position(|b| b.file.name.eq_ignore_ascii_case(n)),
        None => (!host.companions.is_empty()).then_some(0),
    }
}
