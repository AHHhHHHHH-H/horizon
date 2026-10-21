//! The Speedrunner role's route, overworld part (docs/companion-speedrun.md,
//! "The route"): wood → stone tools → food → iron ×10 → an iron pickaxe, two
//! buckets and flint and steel → a nether portal cast from a lava pool, lit.
//! Each stage names goals; the tech-tree planner (companion/techtree.rs)
//! turns them into work skills from what it carries and knows; a raw
//! material it knows no source of sends it exploring (the surface for
//! trees and animals, a staircase down for stone, strip mining at y 16 for
//! iron, deep for lava). Every failure plans again from the inventory and
//! memory; splits are kept for the Watch HUD and the harness.
use super::{
    Game,
    companion::Body,
    companion_body::{Steer, mob_at},
    companion_work::{eye, feet},
    companion_walk::throwaway_slot,
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c, CompanionReport, Skill, gear,
        moves::Goal,
        perception::{Keep, PerceptionLevel},
        techtree::{self, Action, Knowledge, Mined, Stations},
        work::CraftRecipe,
    },
    items::ItemStack,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

/// The route's stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Wood,
    Stone,
    Food,
    Iron,
    Kit,
    Portal,
    Done,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Wood => "wood",
            Stage::Stone => "stone tools",
            Stage::Food => "food",
            Stage::Iron => "iron",
            Stage::Kit => "iron kit",
            Stage::Portal => "nether portal",
            Stage::Done => "overworld done",
        }
    }
    fn next(self) -> Self {
        match self {
            Stage::Wood => Stage::Stone,
            Stage::Stone => Stage::Food,
            Stage::Food => Stage::Iron,
            Stage::Iron => Stage::Kit,
            Stage::Kit => Stage::Portal,
            Stage::Portal | Stage::Done => Stage::Done,
        }
    }
}

/// Ticks a stage may take before it is called a failure (and, for food,
/// skipped).
fn budget(stage: Stage) -> u64 {
    match stage {
        Stage::Wood => 4800,
        Stage::Stone => 4800,
        Stage::Food => 2400,
        Stage::Iron => 24_000,
        Stage::Kit => 14_400,
        Stage::Portal => 24_000,
        Stage::Done => u64::MAX,
    }
}

/// Ticks under water before it goes up for air (of the 300 a player has).
const AIR_TICKS: u32 = 140;

/// Ticks work may go without its inventory changing before the run calls
/// it stuck.
const STALL: u64 = 3000;

/// How it looks for what it doesn't know of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Search {
    /// Walk the surface in long legs, turning (trees, animals, water, gravel).
    Surface,
    /// A staircase down a few blocks (stone).
    Down,
    /// Down to y 16, then tunnels (iron, coal, gravel).
    Mine(i32),
}

struct Explore {
    search: Search,
    heading: f64,
    goal: Option<Goal>,
    legs: u32,
    since: u64,
}

/// A speedrun under way (`Body::run`).
pub(super) struct Run {
    pub stage: Stage,
    /// The game tick it started at, and when the stage began.
    pub start: u64,
    stage_start: u64,
    /// Milestones reached: name, ticks since the start.
    pub splits: Vec<(String, u64)>,
    /// What it does now, for the HUD.
    pub note: String,
    explore: Option<Explore>,
    /// The last work it was given, the inventory then, and how often.
    last: Option<(String, u64, u32)>,
    /// Blocks it gave up on near here (none it can reach): (block, where).
    exhausted: Vec<(String, [i32; 3])>,
    /// Why work failed, last.
    pub failure: Option<String>,
    /// Stages that ran out of time.
    pub failed: Vec<String>,
    cast: Option<Box<Cast>>,
    /// The block or mob of the work handed out last.
    source: Option<String>,
    /// The inventory's signature, and since when it hasn't changed.
    progress: (u64, u64),
    /// Where it was (and what it carried) and since when, for the route's
    /// own moves.
    moving: (u64, u64),
    health: f32,
    /// Ticks its eyes have been under water.
    under: u32,
    /// Going up for air, until it has its breath back.
    surfacing: bool,
    /// When it set off back for its things.
    recover_since: u64,
    /// A smelt of its meat stalled: the meat stays raw.
    no_cook: bool,
    /// Where it stood last tick, and where it died (its things lie there)
    /// with when it got back there.
    at: [f64; 3],
    death: Option<([f64; 3], Option<u64>)>,
    /// The pit it spends the night in (where its feet go), and whether
    /// the night is given up on (no way to dig one here).
    shelter: Option<[i32; 3]>,
    no_shelter: Option<i64>,
    pub deaths: u32,
    night: Option<i64>,
    died_at_night: bool,
    turn: u32,
}

impl Run {
    fn new(now: u64) -> Self {
        Self {
            stage: Stage::Wood,
            start: now,
            stage_start: now,
            splits: Vec::new(),
            note: "starting".into(),
            explore: None,
            last: None,
            exhausted: Vec::new(),
            failure: None,
            failed: Vec::new(),
            cast: None,
            source: None,
            progress: (0, now),
            moving: (0, now),
            health: 20.0,
            under: 0,
            surfacing: false,
            recover_since: 0,
            no_cook: false,
            at: [0.0; 3],
            death: None,
            shelter: None,
            no_shelter: None,
            deaths: 0,
            night: None,
            died_at_night: false,
            turn: 0,
        }
    }
    fn split(&mut self, name: &str, ticks: u64) -> bool {
        if self.splits.iter().any(|(n, _)| n == name) {
            return false;
        }
        self.splits.push((name.to_owned(), ticks));
        true
    }
    /// Start at a later stage (`run all from portal`): wood, stone, food,
    /// iron, kit, portal.
    pub fn skip_to(&mut self, stage: &str) {
        let mut s = Stage::Wood;
        while s != Stage::Done && !s.name().starts_with(stage) && !format!("{s:?}").eq_ignore_ascii_case(stage) {
            s = s.next();
        }
        self.stage = s;
    }
    /// A report the body made: a failure is kept.
    pub fn heard(&mut self, report: &CompanionReport) {
        match report {
            CompanionReport::SkillFailed { reason, .. } => self.failure = Some(reason.clone()),
            CompanionReport::Died { .. } => {
                self.deaths += 1;
                self.death = Some((self.at, None));
                self.explore = None;
                self.cast = None;
                if self.night.is_some() {
                    self.died_at_night = true;
                }
            }
            _ => {}
        }
    }
}

// ---- what the planner knows -------------------------------------------------------

struct GameKnowledge<'a, 'g> {
    game: &'a Game<'g>,
    book: Arc<super::companion_craft::BookIndex>,
    mined: Arc<HashMap<String, Vec<Mined>>>,
    smelted: Arc<HashMap<String, Vec<String>>>,
    /// Tool, block → whether the tool harvests it (asked for every item
    /// it carries at every tool the plan needs).
    harvests: std::cell::RefCell<HashMap<(String, String), bool>>,
    /// Blocks it remembers: the nearest of each, how far.
    known: HashMap<String, f64>,
    mobs: HashMap<String, f64>,
    level: PerceptionLevel,
    exhausted: Vec<String>,
}

const PREY: [&str; 5] = [
    "minecraft:cow",
    "minecraft:pig",
    "minecraft:sheep",
    "minecraft:chicken",
    "minecraft:rabbit",
];

impl Knowledge for GameKnowledge<'_, '_> {
    fn recipes(&self, item: &str) -> &[Arc<CraftRecipe>] {
        use horizon_client::companion::work::RecipeBook;
        self.book.recipes_for(item)
    }
    fn mined_from(&self, item: &str) -> Vec<Mined> {
        self.mined.get(item).cloned().unwrap_or_default()
    }
    fn hunted_from(&self, item: &str) -> Vec<String> {
        let n = c::short_id(item);
        let mob = match n {
            "beef" | "leather" => "cow",
            "porkchop" => "pig",
            "mutton" | "white_wool" => "sheep",
            "chicken" | "feather" => "chicken",
            "rabbit" | "rabbit_hide" => "rabbit",
            _ => return Vec::new(),
        };
        vec![c::namespaced(mob)]
    }
    fn smelted_from(&self, item: &str) -> Vec<String> {
        self.smelted.get(item).cloned().unwrap_or_default()
    }
    fn harvests(&self, tool: &str, block: &str) -> bool {
        let key = (tool.to_owned(), block.to_owned());
        if let Some(&h) = self.harvests.borrow().get(&key) {
            return h;
        }
        let h = self
            .game
            .res
            .items()
            .correct_for_drops(Some(&ItemStack::new(tool, 1)), block);
        self.harvests.borrow_mut().insert(key, h);
        h
    }
    fn gather_cost(&self, block: &str) -> Option<u32> {
        if self.exhausted.iter().any(|b| b == block) {
            return None;
        }
        match self.known.get(block) {
            Some(d) => Some(40 + (*d * 4.0) as u32),
            // Through walls it may find one within 48 blocks: what is
            // common underground.
            None if self.level == PerceptionLevel::AllKnowing
                && (common_underground(block) || natural_wood(block)) =>
            {
                Some(1500)
            }
            None if self.level == PerceptionLevel::Sharp && common_underground(block) => Some(1500),
            None => None,
        }
    }
    fn hunt_cost(&self, mob: &str) -> Option<u32> {
        self.mobs.get(mob).map(|d| 150 + (*d * 5.0) as u32)
    }
}

/// Blocks plentiful enough under the surface that a search through walls
/// will likely find one.
fn common_underground(block: &str) -> bool {
    let n = c::short_id(block);
    (n.ends_with("_ore") && !n.starts_with("nether_") && n != "ancient_debris")
        || matches!(
            n,
            "stone" | "deepslate" | "gravel" | "dirt" | "sand" | "granite" | "diorite" | "andesite" | "tuff"
        )
}

/// Logs and leaves as trees grow them (not stripped, not bark blocks).
fn natural_wood(block: &str) -> bool {
    let n = c::short_id(block);
    !n.starts_with("stripped_") && (n.ends_with("_log") || n.ends_with("_leaves"))
}

/// Items one block gives, on average, where it isn't one.
fn per_block(block: &str, item: &str) -> f32 {
    match (c::short_id(block), c::short_id(item)) {
        ("gravel", "flint") => 0.1,
        (_, "apple") => 0.005,
        (b, "stick") if b.ends_with("_leaves") => 0.02,
        (b, _) if b.ends_with("_leaves") => 0.05,
        _ => 1.0,
    }
}

fn count(inventory: &[Option<ItemStack>], item: &str) -> u32 {
    super::companion_body::carried(inventory, item)
}

fn count_any(inventory: &[Option<ItemStack>], items: &[&str]) -> u32 {
    items.iter().map(|i| count(inventory, i)).sum()
}

const COOKED: [&str; 4] = [
    "minecraft:cooked_beef",
    "minecraft:cooked_porkchop",
    "minecraft:cooked_mutton",
    "minecraft:cooked_chicken",
];
/// The same meat raw, in the same order.
const RAW: [&str; 4] = [
    "minecraft:beef",
    "minecraft:porkchop",
    "minecraft:mutton",
    "minecraft:chicken",
];
/// Pieces of meat a run takes along.
const FOOD: u32 = 4;

/// Iron it has, counting what its kit holds.
fn iron(inventory: &[Option<ItemStack>]) -> u32 {
    count_any(inventory, &["minecraft:raw_iron", "minecraft:iron_ingot"])
        + 3 * count(inventory, "minecraft:iron_pickaxe")
        + 3 * count_any(
            inventory,
            &["minecraft:bucket", "minecraft:water_bucket", "minecraft:lava_bucket"],
        )
        + count(inventory, "minecraft:flint_and_steel")
}

fn buckets(inventory: &[Option<ItemStack>]) -> u32 {
    count_any(
        inventory,
        &["minecraft:bucket", "minecraft:water_bucket", "minecraft:lava_bucket"],
    )
}

fn has_pickaxe(inventory: &[Option<ItemStack>], tiers: &[&str]) -> bool {
    tiers
        .iter()
        .any(|t| count(inventory, &format!("minecraft:{t}_pickaxe")) > 0)
}

impl Game<'_> {
    /// `run [level]`: the Speedrunner sets off on its route.
    pub(super) fn companion_start_run(&mut self, host: &mut LanHost, b: &mut Body, level: Option<PerceptionLevel>) {
        self.companion_cancel_work(host, b, None);
        if let Some(l) = level {
            b.senses.level = l;
        }
        b.target = None;
        b.post = None;
        b.walk.reset();
        b.run = Some(Box::new(Run::new(self.world.game_time.max(0) as u64)));
        b.skill = Skill::Run;
        b.nav.reset();
        self.companion_report(b, CompanionReport::SkillStarted(Skill::Run));
    }

    /// What a run looks like for the status line and the harness.
    pub(super) fn companion_run_status(&self, b: &Body) -> Value {
        let Some(run) = b.run.as_ref() else {
            return Value::Null;
        };
        let now = self.world.game_time.max(0) as u64;
        json!({
            "stage": run.stage.name(),
            "note": run.note,
            "ticks": now.saturating_sub(run.start),
            "level": b.senses.level.id(),
            "level_label": b.senses.level.label(),
            "splits": run.splits.iter().map(|(n, t)| json!({ "name": n, "ticks": t })).collect::<Vec<_>>(),
            "failed": run.failed,
            "failure": run.failure,
            "deaths": run.deaths,
            "placed": b.walk.placed,
            "dug": b.walk.dug,
            "plans": b.walk.plans,
            "plan_ms": b.walk.plan_time.as_secs_f64() * 1000.0,
            "plan_max_ms": b.walk.plan_max.as_secs_f64() * 1000.0,
            "known": b.senses.memory().len(),
            "cast": run.cast.as_ref().map(|c| json!({
                "pool": c.pool,
                "base": c.site.map(|s| s.base),
                "front": c.site.map(|s| s.front),
                "slot": c.slot,
                "phase": format!("{:?}", c.phase),
                "mould": c.mould_done,
            })),
            "seen": b.senses.seen,
        })
    }

    /// The route's tick (`companion_move` for Skill::Run).
    pub(super) fn companion_run_tick(&mut self, host: &mut LanHost, b: &mut Body, pos: [f64; 3]) {
        let Some(mut run) = b.run.take() else {
            b.skill = Skill::Follow;
            return;
        };
        self.companion_run_inner(host, b, &mut run, pos);
        b.run = Some(run);
    }

    /// Every tick of a run, whatever its work has on (`companion_move`):
    /// splits, creepers, the HUD, and work that stopped getting anywhere.
    /// True when it took the tick over.
    pub(super) fn companion_run_watch(&mut self, host: &mut LanHost, b: &mut Body, pos: [f64; 3]) -> bool {
        let Some(mut run) = b.run.take() else {
            return false;
        };
        let took = self.companion_run_supervise(host, b, &mut run, pos);
        // The Watch HUD (game/companion_watch.rs) shows the run.
        if self.watched(b)
            && let Some(w) = self.companions.watch.as_mut()
        {
            w.step = format!("{}: {}", run.stage.name(), run.note);
            w.perception = b.senses.level.label().to_owned();
            w.ticks = (self.world.game_time.max(0) as u64).saturating_sub(run.start);
        }
        b.run = Some(run);
        took
    }

    fn watery(&self, p: [i32; 3]) -> bool {
        self.res.facts().get(self.world.block(p[0], p[1], p[2])).fluid == Fluid::Water
    }

    fn companion_run_supervise(&mut self, host: &mut LanHost, b: &mut Body, run: &mut Run, pos: [f64; 3]) -> bool {
        let _slow = super::companion::SlowLog::new("run supervise");
        let now = self.world.game_time.max(0) as u64;
        let Some(g) = host.hub.guest(b.conn) else {
            return false;
        };
        let inventory = g.inventory.clone();
        let health = g.health;
        // Hurt by nothing it fights: what it stands in, for the log.
        if health < run.health - 0.5 && b.hurt_by.is_none_or(|(_, at)| self.world.game_time - at > 5) {
            let f = feet(pos);
            log::info!(
                "[companion] {} run: hurt {:.1} at {f:?} in {} / {} on {} (in a wall: {})",
                b.file.name,
                run.health - health,
                self.block_name(f),
                self.block_name([f[0], f[1] + 1, f[2]]),
                self.block_name([f[0], f[1] - 1, f[2]]),
                self.in_wall(pos, 1.62),
            );
        }
        run.health = health;
        run.at = pos;
        self.companion_run_splits(b, run, &inventory, now.saturating_sub(run.start));
        // Dropped out of the run's work somehow: back to the route.
        if matches!(b.skill, Skill::Follow | Skill::Idle | Skill::Stay | Skill::Come) && b.work.tasks.is_empty() {
            b.skill = Skill::Run;
        }
        // Under water too long (a player has 15 s of air, 300 ticks, one
        // a tick under water and four back a tick out of it): up for air,
        // through what is above if it must, and it stays up until its air
        // is back.
        // Its eyes as the game has them: low when a ceiling a block over
        // its feet makes it swim (or crawl) flat.
        let low = {
            let f = feet(pos);
            let over = [f[0], f[1] + 1, f[2]];
            !self.passable(over) && !self.watery(over)
        };
        let eye_height = if low { 0.4 } else { host.hub.guest(b.conn).map_or(1.62, |g| g.eye_height()) };
        let head = feet([pos[0], pos[1] + eye_height, pos[2]]);
        let under = self.watery(head);
        run.under = if under { run.under + 1 } else { run.under.saturating_sub(4) };
        if run.under > AIR_TICKS {
            run.surfacing = true;
        } else if run.under < 20 {
            run.surfacing = false;
        }
        if run.surfacing {
            run.note = "up for air".into();
            b.walk.reset();
            // Nothing left over from the walk: sneaking sinks it.
            b.keys = Default::default();
            if !under {
                // Out: it treads water (or stands) where it is.
                self.companion_stop_dig(host, b);
                b.keys.jump = self.watery(feet(pos));
                return true;
            }
            let above = [head[0], head[1] + 1, head[2]];
            if !self.passable(above) && !self.watery(above) {
                let _ = self.companion_dig_step(host, b, above, 0);
                return true;
            }
            self.companion_stop_dig(host, b);
            b.keys.jump = true;
            b.pitch = -30.0;
            // Air nearest to the side (a flooded tunnel's way out) when the
            // way straight up ends under a ceiling.
            let open_up = (1..12)
                .map(|dy| [head[0], head[1] + dy, head[2]])
                .take_while(|p| self.passable(*p) || self.watery(*p))
                .any(|p| !self.watery(p));
            if !open_up {
                let air = (-4..=4)
                    .flat_map(|dx| (-4..=4).flat_map(move |dz| (0..=4).map(move |dy| [dx, dy, dz])))
                    .map(|d| [head[0] + d[0], head[1] + d[1], head[2] + d[2]])
                    .filter(|p| self.passable(*p) && !self.watery(*p) && self.passable([p[0], p[1] - 1, p[2]]))
                    .min_by_key(|p| (p[0] - head[0]).pow(2) + (p[2] - head[2]).pow(2) + (p[1] - head[1]).pow(2));
                if let Some(a) = air {
                    let to = [f64::from(a[0]) + 0.5, pos[1], f64::from(a[2]) + 0.5];
                    if c::horizontal(pos, to) > 0.3 {
                        b.yaw = c::yaw_toward(pos, to) as f32;
                        b.keys.forward = true;
                    }
                }
            }
            return true;
        }
        // Night under the open sky: into a capped pit until morning.
        if self.companion_night(host, b, run, pos) {
            return true;
        }
        // A creeper close, or the crossbows of a pillager outpost: away
        // from it, whatever it was doing.
        if let Some(creeper) = self
            .mobs
            .iter()
            .filter(|m| m.alive())
            .filter_map(|m| match &*m.kind.id {
                "minecraft:creeper" => Some((mob_at(m), 5.0)),
                "minecraft:pillager" => Some((mob_at(m), 20.0)),
                _ => None,
            })
            .find(|(at, near)| c::distance(*at, pos) < *near)
            .map(|(at, _)| at)
        {
            self.companion_stop_dig(host, b);
            let away = [
                pos[0] + (pos[0] - creeper[0]) * 3.0,
                pos[1],
                pos[2] + (pos[2] - creeper[2]) * 3.0,
            ];
            run.note = "backing off".into();
            let _ = self.companion_steer(host, b, away, 0.5, true);
            return true;
        }
        // Work under way: it has to get somewhere (items change), and the
        // stage has its time.
        if b.skill != Skill::Run && !b.work.tasks.is_empty() {
            let sig: u64 = inventory
                .iter()
                .flatten()
                .map(|s| u64::from(s.count) * 31 + s.item.len() as u64)
                .sum();
            if run.progress.0 != sig {
                run.progress = (sig, now);
            }
            let stalled = now.saturating_sub(run.progress.1) > STALL;
            // A smelt under way finishes (what is in the furnace would be
            // lost to it); it doesn't go long without items changing.
            let late = now.saturating_sub(run.stage_start) > budget(run.stage) && b.skill != Skill::Smelt;
            if stalled || late {
                log::info!(
                    "[companion] {} run: {} ({})",
                    b.file.name,
                    if stalled { "work stalled" } else { "stage out of time" },
                    run.note
                );
                self.companion_cancel_work(host, b, None);
                b.target = None;
                b.skill = Skill::Run;
                b.walk.reset();
                if run.note.starts_with("smelt") && COOKED.iter().any(|m| run.note.contains(&c::short_id(m).replace('_', " "))) {
                    run.no_cook = true;
                }
                if stalled && let Some(s) = run.source.take() {
                    run.exhausted.push((s, feet(pos)));
                }
                run.progress = (sig, now);
            }
        } else {
            run.progress.1 = now;
            // The route's own moves (exploring, the cast): it has to get
            // somewhere, or its inventory change, while it isn't sheltering.
            let sig: u64 = inventory
                .iter()
                .flatten()
                .map(|s| u64::from(s.count) * 31 + s.item.len() as u64)
                .sum();
            let f = feet(pos);
            let here = (f[0].div_euclid(3), f[1].div_euclid(3), f[2].div_euclid(3));
            let key = sig ^ (here.0 as u64).wrapping_mul(0x9e37) ^ (here.1 as u64).wrapping_mul(0x85eb) ^ (here.2 as u64).wrapping_mul(0xc2b2);
            if run.moving.0 != key || run.shelter.is_some() || run.death.is_some() {
                run.moving = (key, now);
            } else if now.saturating_sub(run.moving.1) > STALL {
                log::info!("[companion] {} run: stuck at {f:?} ({}), trying elsewhere", b.file.name, run.note);
                run.moving = (key, now);
                b.walk.reset();
                if let Some(ex) = run.explore.as_mut() {
                    ex.goal = None;
                    ex.heading += 2.1;
                }
                if let Some(cast) = run.cast.as_mut()
                    && cast.slot < SLOTS.len()
                {
                    if let Some(ruin) = cast.ruin.take() {
                        cast.ruins_seen.push(ruin);
                    }
                    if !cast.mould_done && let Some(pool) = cast.pool.take() {
                        cast.bad.push(pool);
                        cast.site = None;
                    }
                }
            }
        }
        false
    }

    fn companion_run_inner(&mut self, host: &mut LanHost, b: &mut Body, run: &mut Run, pos: [f64; 3]) {
        let _slow = super::companion::SlowLog::new("run");
        let now = self.world.game_time.max(0) as u64;
        let ticks = now.saturating_sub(run.start);
        let Some(g) = host.hub.guest(b.conn) else {
            return;
        };
        let inventory = g.inventory.clone();
        let _ = ticks;
        // Died: back for its things (they lie there 5 minutes).
        if let Some((at, back)) = run.death {
            let far = c::distance(pos, at);
            match back {
                _ if far > 400.0 => run.death = None,
                Some(t) if now > t + 60 => {
                    log::info!("[companion] {} run: back with what was left", b.file.name);
                    run.death = None;
                }
                Some(_) => {
                    // There: a moment to pick everything up.
                    run.note = "picking my things up".into();
                    let _ = self.companion_steer(host, b, at, 0.5, false);
                }
                None if far < 2.0 => run.death = Some((at, Some(now))),
                None => {
                    if run.note != "going back for my things" {
                        log::info!("[companion] {} run: going back for my things at {at:?}", b.file.name);
                        run.recover_since = now;
                    }
                    run.note = "going back for my things".into();
                    let goal = Goal { at: feet(at), near: 1.5, vertical: 2 };
                    match self.companion_travel(host, b, goal, pos) {
                        // Two minutes on the way: the things are given up.
                        Steer::Moving if now > run.recover_since + 2400 => {
                            log::info!("[companion] {} run: my things are out of reach", b.file.name);
                            b.walk.reset();
                            run.death = None;
                        }
                        Steer::Moving => {}
                        Steer::Arrived => run.death = Some((at, Some(now))),
                        Steer::Stuck => run.death = None,
                    }
                }
            }
            if run.death.is_some() {
                return;
            }
        }
        if run.stage == Stage::Done {
            run.note = "portal lit: ready for the Nether".into();
            return;
        }
        // Out of time for this stage: classified, then on (food is optional).
        if now.saturating_sub(run.stage_start) > budget(run.stage) {
            let what = format!("{} ({})", run.stage.name(), run.failure.clone().unwrap_or_else(|| "out of time".into()));
            log::info!("[companion] run: out of time: {what}");
            run.failed.push(what);
            if run.stage == Stage::Food {
                self.companion_run_next(run, now);
            } else {
                run.stage_start = now;
            }
        }
        if run.stage == Stage::Portal {
            self.companion_cast_tick(host, b, run, pos, &inventory);
            return;
        }
        let goals = self.companion_stage_goals(run.stage, &inventory, pos, !run.no_cook);
        if goals.is_empty() {
            self.companion_run_next(run, now);
            return;
        }
        // Work that found none of its block: that source is used up here.
        if let Some(f) = run.failure.take() {
            if (f.starts_with("no ") || f.contains("can't get")) && let Some(s) = run.source.take() {
                run.exhausted.push((s, feet(pos)));
            }
            run.failure = Some(f);
        }
        // Plan from what it carries and knows.
        if run.explore.is_none() || b.ticks.is_multiple_of(40) {
            let plan = self.companion_run_plan(b, run, &inventory, &goals, pos);
            if let Some(action) = plan.actions.first() {
                run.explore = None;
                self.companion_run_act(host, b, run, action.clone(), &inventory);
                return;
            }
            if run.explore.is_none() {
                let lack = plan
                    .lacks
                    .first()
                    .cloned()
                    .unwrap_or_else(|| goals[0].0.clone());
                let search = search_for(&lack, feet(pos));
                run.note = format!("looking for {}", c::short_id(&lack).replace('_', " "));
                log::info!("[companion] {} run: {} ({search:?}, plan lacks {:?}, missing {:?})", b.file.name, run.note, plan.lacks, plan.missing);
                let heading = self.companion_heading(b, run);
                run.explore = Some(Explore {
                    search,
                    heading,
                    goal: None,
                    legs: 0,
                    since: now,
                });
            }
        }
        self.companion_explore(host, b, run, pos);
    }

    fn companion_run_next(&mut self, run: &mut Run, now: u64) {
        log::info!("[companion] run: {} done at {}", run.stage.name(), now - run.start);
        run.stage = run.stage.next();
        run.stage_start = now;
        run.explore = None;
        run.last = None;
        run.failure = None;
    }

    /// A heading to explore along: away from where it started, turning.
    fn companion_heading(&self, b: &mut Body, run: &mut Run) -> f64 {
        run.turn += 1;
        let base = (b.next_float() * std::f64::consts::TAU).rem_euclid(std::f64::consts::TAU);
        base + f64::from(run.turn) * 1.3
    }

    /// Milestones: first time each holds.
    fn companion_run_splits(&mut self, b: &mut Body, run: &mut Run, inventory: &[Option<ItemStack>], ticks: u64) {
        let mut hit = Vec::new();
        if has_pickaxe(inventory, &["wooden", "stone", "iron", "diamond"]) {
            hit.push("wooden_pickaxe");
        }
        if has_pickaxe(inventory, &["stone", "iron", "diamond"]) {
            hit.push("stone_tools");
        }
        if count_any(inventory, &COOKED) >= FOOD {
            hit.push("food");
        }
        if iron(inventory) >= 10 {
            hit.push("iron_10");
        }
        if has_pickaxe(inventory, &["iron", "diamond"]) {
            hit.push("iron_pickaxe");
        }
        if buckets(inventory) >= 2 {
            hit.push("buckets");
        }
        // The first night: from dusk until morning without dying.
        let day = self.world.day_time.div_euclid(24000);
        let time = self.world.day_time.rem_euclid(24000);
        if (13000..23000).contains(&time) {
            if run.night.is_none() {
                run.night = Some(day);
                run.died_at_night = false;
            }
        } else if let Some(n) = run.night
            && (day > n || time >= 23000)
        {
            if !run.died_at_night {
                hit.push("first_night");
            }
            run.night = None;
        }
        for h in hit {
            if run.split(h, ticks) {
                self.companion_report(
                    b,
                    CompanionReport::Progress {
                        skill: Skill::Run,
                        item: h.into(),
                        done: run.splits.len() as u32,
                        total: 9,
                    },
                );
            }
        }
    }

    /// What a stage still wants (item, count), from what it carries.
    fn companion_stage_goals(&self, stage: Stage, inv: &[Option<ItemStack>], pos: [f64; 3], cook: bool) -> Vec<(String, u32)> {
        let m = |s: &str| format!("minecraft:{s}");
        let mut goals = Vec::new();
        match stage {
            Stage::Wood => {
                if !has_pickaxe(inv, &["wooden", "stone", "iron", "diamond"]) {
                    goals.push((m("wooden_pickaxe"), 1));
                }
            }
            Stage::Stone => {
                if !has_pickaxe(inv, &["stone", "iron", "diamond"]) {
                    goals.push((m("stone_pickaxe"), 1));
                }
                if count_any(inv, &["minecraft:stone_sword", "minecraft:iron_sword", "minecraft:diamond_sword"]) == 0 {
                    goals.push((m("stone_sword"), 1));
                }
            }
            Stage::Food => {
                // Raw meat from the animal it knows nearest (cooked later
                // with the iron); none near: no food stage.
                let have = count_any(inv, &RAW) + count_any(inv, &COOKED);
                if have < FOOD {
                    let nearest = RAW
                        .iter()
                        .filter_map(|meat| {
                            let mob = match c::short_id(meat) {
                                "beef" => "minecraft:cow",
                                "porkchop" => "minecraft:pig",
                                "mutton" => "minecraft:sheep",
                                _ => "minecraft:chicken",
                            };
                            self.mobs
                                .iter()
                                .filter(|x| x.alive() && !x.is_baby() && &*x.kind.id == mob)
                                .map(|x| c::distance(mob_at(x), pos))
                                .filter(|d| *d < 48.0)
                                .min_by(f64::total_cmp)
                                .map(|d| (d, *meat))
                        })
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    if let Some((_, meat)) = nearest {
                        goals.push((meat.to_string(), FOOD - have));
                    }
                }
            }
            Stage::Iron => {
                let have = iron(inv);
                if have < 10 {
                    goals.push((m("raw_iron"), 10 - have));
                }
            }
            Stage::Kit => {
                if !has_pickaxe(inv, &["iron", "diamond"]) {
                    goals.push((m("iron_pickaxe"), 1));
                }
                let have = buckets(inv);
                if have < 2 {
                    goals.push((m("bucket"), 2 - have));
                }
                if count(inv, "minecraft:flint_and_steel") == 0 {
                    goals.push((m("flint_and_steel"), 1));
                }
                // The meat it hunted, cooked in the same furnace (raw it
                // is food too, when cooking went wrong once).
                for (raw, cooked) in RAW.iter().zip(COOKED).filter(|_| cook) {
                    let n = count(inv, raw);
                    if n > 0 {
                        goals.push((cooked.to_string(), count(inv, cooked) + n));
                    }
                }
                // Blocks to bridge, pillar and build the mould with.
                let blocks = Self::companion_blocks(inv);
                if blocks < 24 {
                    goals.push((m("cobblestone"), 32 - blocks));
                }
            }
            Stage::Portal | Stage::Done => {}
        }
        goals
    }

    /// Plan the stage's goals from what it carries and knows.
    fn companion_run_plan(
        &mut self,
        b: &Body,
        run: &mut Run,
        inventory: &[Option<ItemStack>],
        goals: &[(String, u32)],
        pos: [f64; 3],
    ) -> techtree::TechPlan {
        let _slow = super::companion::SlowLog::new("tech plan");
        let here = feet(pos);
        // What it gave up on far behind it is worth a look again.
        // Stone and the like are everywhere: a few blocks on is elsewhere.
        run.exhausted.retain(|(block, at)| {
            let far = (0..3).map(|k| (at[k] - here[k]).abs()).max().unwrap_or(0);
            far < if common_underground(block) && !block.ends_with("_ore") { 6 } else { 48 }
        });
        let book = self.companion_book();
        let mined = self.companion_mined_index();
        let smelted = self.companion_smelted_index();
        let facts = self.res.facts();
        // The nearest of each state first (integers, no names), then of
        // each block: memory holds tens of thousands of blocks.
        let mut by_state: HashMap<u32, i64> = HashMap::new();
        for (p, s) in b.senses.memory().iter() {
            let d2: i64 = (0..3).map(|k| i64::from(p[k] - here[k]).pow(2)).sum();
            let e = by_state.entry(s).or_insert(d2);
            *e = (*e).min(d2);
        }
        let mut known: HashMap<String, f64> = HashMap::new();
        let mut stations = Stations::default();
        for (s, d2) in by_state {
            let d = (d2 as f64).sqrt();
            let name = &*facts.get(horizon_world::registry::BlockStateId(s)).name;
            if d <= 16.0 && name == "minecraft:crafting_table" {
                stations.crafting_table = true;
            }
            if d <= 16.0 && name == "minecraft:furnace" {
                stations.furnace = true;
            }
            match known.get_mut(name) {
                Some(best) if *best <= d => {}
                Some(best) => *best = d,
                None => {
                    known.insert(name.to_owned(), d);
                }
            }
        }
        let mut mobs: HashMap<String, f64> = HashMap::new();
        for m in self.mobs.iter().filter(|m| m.alive() && !m.is_baby()) {
            if !PREY.contains(&&*m.kind.id) {
                continue;
            }
            let d = c::distance(mob_at(m), pos);
            if d < 48.0 {
                let e = mobs.entry(m.kind.id.to_string()).or_insert(d);
                *e = e.min(d);
            }
        }
        let mut stock: HashMap<String, u32> = HashMap::new();
        for s in inventory.iter().flatten() {
            *stock.entry(s.item.to_string()).or_default() += s.count;
        }
        let k = GameKnowledge {
            game: self,
            book,
            mined,
            smelted,
            harvests: Default::default(),
            known,
            mobs,
            level: b.senses.level,
            exhausted: run.exhausted.iter().map(|(b, _)| b.clone()).collect(),
        };
        let goals: Vec<(&str, u32)> = goals.iter().map(|(i, n)| (i.as_str(), *n)).collect();
        techtree::plan(&k, stock, stations, &goals)
    }

    /// Item → the blocks that drop it, and the tool each needs (once).
    /// Item → the inputs that smelt into it (two of each recipe's options).
    fn companion_smelted_index(&mut self) -> Arc<HashMap<String, Vec<String>>> {
        if let Some(m) = &self.companions.smelted {
            return m.clone();
        }
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        for r in self.res.recipes().cooking.iter().filter(|r| r.kind.ends_with("smelting")) {
            index
                .entry(r.result.item.to_string())
                .or_default()
                .extend(r.ingredient.1.iter().take(2).cloned());
        }
        let index = Arc::new(index);
        self.companions.smelted = Some(index.clone());
        index
    }

    pub(super) fn companion_mined_index(&mut self) -> Arc<HashMap<String, Vec<Mined>>> {
        if let Some(m) = &self.companions.mined {
            return m.clone();
        }
        let drops = self.block_drops();
        let items = self.res.items();
        let facts = self.res.facts();
        let mut index: HashMap<String, Vec<Mined>> = HashMap::new();
        let mut needs: HashMap<String, bool> = HashMap::new();
        for f in &facts.states {
            needs.entry(f.name.to_string()).or_insert(f.requires_tool);
        }
        for (block, got) in drops.iter() {
            for item in got {
                let tool = needs
                    .get(block)
                    .copied()
                    .unwrap_or(false)
                    .then(|| gear::tool_needed(items, block).map(str::to_owned))
                    .flatten();
                index.entry(item.clone()).or_default().push(Mined {
                    block: block.to_string(),
                    tool,
                    per_block: per_block(block, item),
                });
            }
        }
        let index = Arc::new(index);
        self.companions.mined = Some(index.clone());
        index
    }

    /// Hand the first action of the plan to the work skills (they come back
    /// to the run when done).
    fn companion_run_act(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut Run,
        action: Action,
        inventory: &[Option<ItemStack>],
    ) {
        // A craft gathers what it lacks (`make`): the work skills' own
        // planner may see the stations differently.
        let words = match action.words().strip_prefix("craft ") {
            Some(rest) => format!("make {rest}"),
            None => action.words(),
        };
        // The same work again with nothing changed: that source is
        // exhausted here; it looks elsewhere.
        let sig = inventory
            .iter()
            .flatten()
            .fold(0u64, |h, s| h.wrapping_mul(31).wrapping_add(u64::from(s.count) ^ s.item.len() as u64));
        let repeats = match &run.last {
            Some((w, s, n)) if *w == words && *s == sig => n + 1,
            _ => 0,
        };
        run.last = Some((words.clone(), sig, repeats));
        if repeats >= 2 {
            let here = host.hub.guest(b.conn).map_or([0; 3], |g| feet(g.position));
            if let Action::Mine { block, .. } | Action::Hunt { mob: block, .. } = &action {
                run.exhausted.push((block.clone(), here));
            } else {
                // Work that keeps failing here: somewhere else may do.
                run.note = format!("{} keeps failing: moving on", words.replace('_', " "));
                log::info!("[companion] {} run: {}", b.file.name, run.note);
                let heading = self.companion_heading(b, run);
                run.explore = Some(Explore {
                    search: Search::Surface,
                    heading,
                    goal: None,
                    legs: 0,
                    since: self.world.game_time.max(0) as u64,
                });
            }
            run.last = None;
            return;
        }
        run.note = words.replace('_', " ");
        log::info!("[companion] {} run: {words}", b.file.name);
        run.source = match &action {
            Action::Mine { block, .. } => Some(block.clone()),
            Action::Hunt { mob, .. } => Some(mob.clone()),
            _ => None,
        };
        run.failure = None;
        let parts: Vec<&str> = words.split_whitespace().collect();
        let Ok(command) = c::parse_command(&parts) else {
            return;
        };
        match self.companion_task(host, b, command, Skill::Run) {
            Ok(task) => {
                let skill = task.skill;
                b.work.tasks.push(task);
                b.skill = skill;
                b.nav.reset();
                b.walk.reset();
                self.companion_report(b, CompanionReport::SkillStarted(skill));
            }
            Err(e) => run.failure = Some(e),
        }
    }

    /// Walk out looking for what it lacks.
    fn companion_explore(&mut self, host: &mut LanHost, b: &mut Body, run: &mut Run, pos: [f64; 3]) {
        let Some(ex) = run.explore.as_mut() else {
            return;
        };
        let f = feet(pos);
        let goal = match ex.goal {
            Some(g) => g,
            None => {
                let (dx, dz) = (ex.heading.cos(), ex.heading.sin());
                let g = match ex.search {
                    Search::Surface => Goal::column(
                        [f[0] + (dx * 48.0) as i32, f[1], f[2] + (dz * 48.0) as i32],
                        4.0,
                    ),
                    Search::Down => Goal {
                        at: [f[0] + (dx * 3.0) as i32, f[1] - 6, f[2] + (dz * 3.0) as i32],
                        near: 2.0,
                        vertical: 1,
                    },
                    Search::Mine(y) if f[1] > y + 2 => Goal {
                        at: [f[0] + (dx * 12.0) as i32, y, f[2] + (dz * 12.0) as i32],
                        near: 3.0,
                        vertical: 2,
                    },
                    Search::Mine(_) => Goal {
                        at: [f[0] + (dx * 24.0) as i32, f[1], f[2] + (dz * 24.0) as i32],
                        near: 1.0,
                        vertical: 0,
                    },
                };
                ex.goal = Some(g);
                g
            }
        };
        let step = self.companion_travel(host, b, goal, pos);
        let Some(ex) = run.explore.as_mut() else {
            return;
        };
        match step {
            Steer::Moving => {}
            Steer::Arrived => {
                ex.legs += 1;
                ex.goal = None;
                ex.heading += match ex.search {
                    Search::Mine(_) => std::f64::consts::FRAC_PI_2 * if ex.legs % 2 == 0 { 1.0 } else { -1.0 },
                    _ => 0.4,
                };
            }
            Steer::Stuck => {
                ex.goal = None;
                ex.heading += 2.1;
                b.walk.reset();
            }
        }
        let _ = ex.since;
    }

    // ---- the portal, cast from a lava pool ---------------------------------------------

    fn companion_cast_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut Run,
        pos: [f64; 3],
        inventory: &[Option<ItemStack>],
    ) {
        let _slow = super::companion::SlowLog::new("cast");
        let have = |i: &str| count(inventory, i);
        if run.cast.as_ref().is_none_or(|c| c.site.is_none()) {
            b.walk.keep.clear();
        }
        // The kit first: two buckets, flint and steel, blocks.
        if buckets(inventory) < 2 || have("minecraft:flint_and_steel") == 0 {
            run.stage = Stage::Kit;
            return;
        }
        if Self::companion_blocks(inventory) < 30 {
            let words = "collect 16 cobblestone".to_owned();
            run.note = "blocks for the mould".into();
            if run.last.as_ref().is_none_or(|(w, _, _)| *w != words) {
                self.companion_run_act(
                    host,
                    b,
                    run,
                    Action::Mine {
                        item: "minecraft:cobblestone".into(),
                        count: 16,
                        block: "minecraft:stone".into(),
                    },
                    inventory,
                );
                return;
            }
        }
        let cast = run.cast.get_or_insert_with(|| Box::new(Cast::default()));
        // A full bucket: the clock for getting it stops.
        if have("minecraft:water_bucket") > 0 {
            cast.trying_water = None;
        }
        if have("minecraft:lava_bucket") > 0 {
            cast.trying = None;
        }
        let water_known = b
            .senses
            .memory()
            .recall(|s| self.is_source(s, Fluid::Water), feet(pos), 192)
            .into_iter()
            .find(|w| !cast.bad.contains(w));
        // Water in a bucket first.
        if have("minecraft:water_bucket") == 0 && cast.site.is_none() {
            let Some(w) = water_known else {
                self.companion_cast_explore(host, b, run, pos, Search::Surface, "looking for water");
                return;
            };
            run.note = "filling a bucket with water".into();
            let reach = super::companion_work::to_block(eye(pos), w);
            if reach > 3.8 {
                let goal = Goal::near([w[0], w[1] + 1, w[2]], 2.0);
                if self.companion_travel(host, b, goal, pos) == Steer::Stuck {
                    // No way to that water: another.
                    log::info!("[companion] {} run: water at {w:?} out of reach", b.file.name);
                    if let Some(cast) = run.cast.as_mut() {
                        cast.bad.push(w);
                    }
                }
                return;
            }
            let now = self.world.game_time.max(0) as u64;
            if let Some(cast) = run.cast.as_mut()
                && Cast::stuck_at(&mut cast.trying_water, w, now)
            {
                log::info!("[companion] {} run: water at {w:?} out of reach", b.file.name);
                cast.bad.push(w);
                return;
            }
            self.companion_fill_bucket(host, b, w, pos);
            return;
        }
        // Through walls (Sharp senses, All-knowing): lava it can't see yet
        // is known too, as far as its senses reach.
        if b.senses.level.xray_radius(48).is_some() {
            let here = feet(pos);
            let cast = run.cast.get_or_insert_with(|| Box::new(Cast::default()));
            let stale = cast
                .scanned
                .is_none_or(|s| (0..3).map(|k| (s[k] - here[k]).abs()).max().unwrap_or(0) > 24);
            if stale {
                let lava = self.states_where(|n| n == "minecraft:lava");
                let mut search = cast.search.take();
                match self.companion_known(b, &lava, here, 48, &mut search, 6 * 4096) {
                    None => {
                        cast.search = search;
                        return;
                    }
                    Some(found) => {
                        for p in found {
                            let s = self.world.block(p[0], p[1], p[2]).0;
                            if self.is_source(s, Fluid::Lava) {
                                b.senses.memory_mut().remember(p, s, Keep::Notable);
                            }
                        }
                        cast.scanned = Some(here);
                    }
                }
            }
        }
        // A lava pool.
        let pools = b.senses.memory().pools(|s| self.is_source(s, Fluid::Lava), feet(pos), 160);
        let given_up = run.cast.as_ref().map_or(Vec::new(), |c| c.bad.clone());
        let near_bad = |p: &[i32; 3]| given_up.iter().any(|q| (0..3).all(|k| (p[k] - q[k]).abs() <= 6));
        let Some((pool, size)) = pools.iter().copied().find(|(p, n)| *n >= 4 && !near_bad(p)) else {
            // Scout and All-knowing know where ruined portals are: lava
            // and obsidian lie by most of them.
            let cast = run.cast.get_or_insert_with(|| Box::new(Cast::default()));
            if cast.ruin.is_none()
                && let Some(ruin) = self.companion_structure(b, "ruined_portal", pos)
                && !cast.ruins_seen.contains(&ruin)
                && c::horizontal(pos, [f64::from(ruin[0]), pos[1], f64::from(ruin[2])]) < 400.0
            {
                cast.ruin = Some(ruin);
            }
            if let Some(ruin) = cast.ruin {
                run.note = "going to a ruined portal for its lava".into();
                // There (it sees what lava lies by it), or no way there:
                // the search goes on from here.
                let there = c::horizontal(pos, [f64::from(ruin[0]), pos[1], f64::from(ruin[2])]) < 16.0;
                if there || self.companion_travel(host, b, Goal::column(ruin, 12.0), pos) == Steer::Stuck {
                    cast.ruins_seen.push(ruin);
                    cast.ruin = None;
                    cast.scanned = None;
                    b.walk.reset();
                }
                return;
            }
            let search = if run.explore.as_ref().is_some_and(|e| e.legs > 6) {
                Search::Mine(-50)
            } else {
                Search::Surface
            };
            self.companion_cast_explore(host, b, run, pos, search, "looking for a lava pool");
            return;
        };
        run.explore = None;
        let cast = run.cast.get_or_insert_with(|| Box::new(Cast::default()));
        // Another pool than the one its site was built by: a new site there.
        if let Some(site) = cast.site
            && (0..3).map(|k| (site.base[k] - pool[k]).abs()).max().unwrap_or(0) > 12
        {
            cast.site = None;
            cast.mould_done = false;
            cast.slot = 0;
            cast.phase = Phase::Lava;
            b.walk.keep.clear();
        }
        cast.pool = Some(pool);
        let _ = size;
        self.companion_cast_portal(host, b, run, pos, inventory);
    }

    fn companion_cast_explore(&mut self, host: &mut LanHost, b: &mut Body, run: &mut Run, pos: [f64; 3], search: Search, note: &str) {
        run.note = note.into();
        if run.explore.as_ref().is_none_or(|e| e.search != search) {
            let heading = self.companion_heading(b, run);
            run.explore = Some(Explore {
                search,
                heading,
                goal: None,
                legs: 0,
                since: self.world.game_time.max(0) as u64,
            });
        }
        self.companion_explore(host, b, run, pos);
    }

    /// A water source in the frame's plane or just before it.
    fn frame_water(&self, site: &Site) -> Option<[i32; 3]> {
        (0..=3)
            .flat_map(|x| (0..=5).flat_map(move |y| [site.at([x, y]), site.front_cell(x, y)]))
            .find(|q| self.is_source(self.world.block(q[0], q[1], q[2]).0, Fluid::Water))
    }

    fn is_source(&self, s: u32, fluid: Fluid) -> bool {
        let f = self.res.facts().get(horizon_world::registry::BlockStateId(s));
        f.fluid == fluid && f.fluid_level == 8 && !f.fluid_falling
    }

    /// Fill an empty bucket from fluid source `w`: walk within reach, look
    /// at it, use the bucket.
    fn companion_fill_bucket(&mut self, host: &mut LanHost, b: &mut Body, w: [i32; 3], pos: [f64; 3]) -> bool {
        let reach = super::companion_work::to_block(eye(pos), w);
        if reach > 3.8 {
            let _ = self.companion_travel(host, b, Goal::near([w[0], w[1] + 1, w[2]], 2.0), pos);
            return false;
        }
        let Some(slot) = host
            .hub
            .guest(b.conn)
            .and_then(|g| g.inventory.iter().position(|s| s.as_ref().is_some_and(|s| &*s.item == "minecraft:bucket")))
        else {
            return false;
        };
        let to = [f64::from(w[0]) + 0.5, f64::from(w[1]) + 0.85, f64::from(w[2]) + 0.5];
        // A natural block between its eyes and the water (leaf litter, the
        // bank): dug first.
        let e = eye(pos);
        let d = c::distance(e, to).max(1e-6);
        let dir = [(to[0] - e[0]) / d, (to[1] - e[1]) / d, (to[2] - e[2]) / d];
        if let Some(h) = self.clip_ray(e, dir, d)
            && h.pos != w
            && self.natural(h.pos)
        {
            let face = super::companion_work::face_index(h.face);
            let _ = self.companion_dig_step(host, b, h.pos, face);
            return false;
        }
        self.companion_stop_dig(host, b);
        self.companion_hold_slot(host, b, slot);
        self.companion_use_toward(host, b, to, pos);
        // The source is gone from where it was (the bucket took it).
        let gone = !self.is_source(self.world.block(w[0], w[1], w[2]).0, Fluid::Water)
            && !self.is_source(self.world.block(w[0], w[1], w[2]).0, Fluid::Lava);
        if gone {
            b.senses.memory_mut().forget(w);
        }
        gone
    }

    /// Use the held item looking at point `to` (a bucket's fill or pour).
    fn companion_use_toward(&mut self, host: &mut LanHost, b: &mut Body, to: [f64; 3], pos: [f64; 3]) {
        b.yaw = c::yaw_toward(pos, to) as f32;
        b.pitch = super::companion_body::look_pitch(eye(pos), to);
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = (b.yaw, b.pitch);
        }
        let (yaw, pitch) = (b.yaw, b.pitch);
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
    }

    /// Hold the first stack of `item`; false when it carries none.
    fn companion_hold(&mut self, host: &mut LanHost, b: &Body, item: &str) -> bool {
        let Some(slot) = host
            .hub
            .guest(b.conn)
            .and_then(|g| g.inventory.iter().position(|s| s.as_ref().is_some_and(|s| &*s.item == item)))
        else {
            return false;
        };
        self.companion_hold_slot(host, b, slot);
        true
    }

    /// The cast itself: a site beside the pool, the mould (a floor, a back
    /// wall, two corners), then each frame block: lava in, water above it,
    /// the water back; then the flint and steel.
    fn companion_cast_portal(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut Run,
        pos: [f64; 3],
        inventory: &[Option<ItemStack>],
    ) {
        let Some(cast) = run.cast.as_mut() else {
            return;
        };
        let Some(pool) = cast.pool else {
            return;
        };
        let f = feet(pos);
        // A site: where it stands once within 6 blocks of the pool.
        let Some(site) = cast.site else {
            let near = c::horizontal(pos, [f64::from(pool[0]), pos[1], f64::from(pool[2])]) < 7.0
                && (f[1] - pool[1]).abs() <= 3;
            if !near {
                run.note = "going to the lava pool".into();
                if self.companion_travel(host, b, Goal::near([pool[0], pool[1] + 1, pool[2]], 5.0), pos) == Steer::Stuck {
                    // Can't get there: another pool.
                    log::info!("[companion] {} run: no way to the lava pool at {pool:?}", b.file.name);
                    cast.bad.push(pool);
                    cast.pool = None;
                }
                return;
            }
            // The frame's plane runs along x, two blocks from where it
            // stands, on the side away from the pool.
            let dz = if pool[2] >= f[2] { -1 } else { 1 };
            cast.site = Some(Site {
                base: [f[0] - 1, f[1], f[2] + 2 * dz],
                front: -dz.signum(),
            });
            cast.slot = 0;
            run.note = "building the mould".into();
            return;
        };
        let _ = inventory;
        // Its blocks stay where they are whatever way it walks.
        if b.walk.keep.is_empty() {
            b.walk.keep = site.mould();
        }
        // The mould: the floor under the frame and the front, the back wall,
        // the corners; the frame's own cells and the front cleared.
        if !cast.mould_done {
            match self.companion_build_mould(host, b, &site, pos) {
                Build::Busy => {
                    run.note = format!("building the mould ({})", b.walk.note);
                    return;
                }
                Build::Done => cast.mould_done = true,
                Build::Failed => {
                    // Somewhere else then.
                    cast.site = None;
                    cast.pool = None;
                    cast.fails += 1;
                    return;
                }
            }
        }
        // Each frame block.
        while cast.slot < SLOTS.len() {
            let p = site.at(SLOTS[cast.slot]);
            if &*self.block_name(p) == "minecraft:obsidian" {
                cast.slot += 1;
                cast.phase = Phase::Lava;
                continue;
            }
            break;
        }
        // Five minutes on one frame block: this site (or the lava by it)
        // won't do; another pool.
        let now = self.world.game_time.max(0) as u64;
        match cast.slot_since {
            Some((s, t)) if s == cast.slot => {
                if now > t + 6000 && cast.slot >= SLOTS.len() {
                    // A whole frame isn't given up: only the way to it.
                    cast.slot_since = Some((cast.slot, now));
                    b.walk.reset();
                } else if now > t + 6000 {
                    log::info!("[companion] {} run: frame block {} won't cast here", b.file.name, cast.slot + 1);
                    cast.bad.push(pool);
                    cast.site = None;
                    cast.pool = None;
                    cast.mould_done = false;
                    cast.slot_since = None;
                    cast.phase = Phase::Lava;
                    b.walk.reset();
                    return;
                }
            }
            _ => {
                cast.slot_since = Some((cast.slot, now));
                cast.trying = None;
                cast.trying_water = None;
            }
        }
        if cast.slot >= SLOTS.len() {
            let now = self.world.game_time.max(0) as u64;
            run.split("obsidian_10", now - run.start);
            // Light it on the top face of a bottom block.
            let interior = site.at([1, 1]);
            if &*self.block_name(interior) == "minecraft:nether_portal" {
                run.split("portal_lit", now - run.start);
                b.senses.memory_mut().places.insert("portal".into(), interior);
                log::info!("[companion] {} run: portal lit at {interior:?}", b.file.name);
                self.companion_run_next(run, now);
                self.companion_report(b, CompanionReport::SkillFinished(Skill::Run));
                return;
            }
            run.note = "lighting the portal".into();
            let stand = site.stand_at(1, 0);
            // The inside clear first: the water left from the cast taken
            // back (a source in or before the frame keeps it flowing in),
            // what flowed in and set (cobblestone) dug out.
            let mut near_frame: Vec<[i32; 3]> = Vec::new();
            for x in 0..=3 {
                for y in 0..=5 {
                    near_frame.extend([site.at([x, y]), site.front_cell(x, y)]);
                }
            }
            let source = near_frame.iter().copied().find(|q| {
                let s = self.world.block(q[0], q[1], q[2]).0;
                self.is_source(s, Fluid::Water) || self.is_source(s, Fluid::Lava)
            });
            if let Some(w) = source {
                run.note = "draining the portal".into();
                let rc = [(w[0] - site.base[0]).clamp(0, 3), (w[1] - site.base[1]).clamp(0, 4)];
                if !self.companion_at_stand(host, b, site.stand_for(rc), pos) {
                    return;
                }
                let inv = self.companion_inventory(host, b);
                if count(&inv, "minecraft:bucket") > 0 && self.companion_hold(host, b, "minecraft:bucket") {
                    self.companion_use_toward(host, b, site.near_point(w), pos);
                } else {
                    let _ = self.companion_place_block(host, b, w, pos);
                }
                return;
            }
            if !self.companion_at_stand(host, b, stand, pos) {
                return;
            }
            for y in 1..=3 {
                for x in 1..=2 {
                    let q = site.at([x, y]);
                    if !self.passable(q) && &*self.block_name(q) != "minecraft:nether_portal" {
                        let _ = self.companion_dig_step(host, b, q, site.front_face());
                        return;
                    }
                    if self.res.facts().get(self.world.block(q[0], q[1], q[2])).fluid != Fluid::None {
                        // Flowing in from no source it can see: stopped
                        // with a block, dug out next.
                        let _ = self.companion_place_block(host, b, q, pos);
                        return;
                    }
                }
            }
            self.companion_stop_dig(host, b);
            if self.companion_hold(host, b, "minecraft:flint_and_steel") {
                let bottom = site.at([1, 0]);
                self.companion_use_block(host, b, bottom, 1, pos);
            }
            return;
        }
        let rc = SLOTS[cast.slot];
        let slot = site.at(rc);
        let stand = site.stand_for(rc);
        let water = site.at(water_cell(rc));
        match cast.phase {
            Phase::Lava => {
                // Something fell or flowed in (cobblestone): out with it.
                if !self.passable(slot) && &*self.block_name(slot) != "minecraft:obsidian" {
                    if self.companion_at_stand(host, b, stand, pos) {
                        let _ = self.companion_dig_step(host, b, slot, site.front_face());
                    }
                    return;
                }
                // (No stop to the digging here: the way to the water or the
                // lava may be dug.)
                let inv = self.companion_inventory(host, b);
                // Water in hand before any lava: it sets the lava at once.
                if count(&inv, "minecraft:water_bucket") == 0 {
                    // Water left in or before the frame from the last block:
                    // taken back from here first.
                    if let Some(w) = self.frame_water(&site)
                        && count(&inv, "minecraft:bucket") > 0
                    {
                        run.note = "taking the water back".into();
                        if Cast::stuck_at(&mut cast.trying_water, w, self.world.game_time.max(0) as u64) {
                            // Out of its reach from here: a block on it.
                            let _ = self.companion_place_block(host, b, w, pos);
                            return;
                        }
                        let rc = [(w[0] - site.base[0]).clamp(0, 3), (w[1] - site.base[1]).clamp(0, 4)];
                        if self.companion_at_stand(host, b, site.stand_for(rc), pos)
                            && self.companion_hold(host, b, "minecraft:bucket")
                        {
                            self.companion_use_toward(host, b, site.near_point(w), pos);
                        }
                        return;
                    }
                    run.note = "water for the cast".into();
                    let Some(w) = b
                        .senses
                        .memory()
                        .recall(|s| self.is_source(s, Fluid::Water), feet(pos), 192)
                        .into_iter()
                        .find(|w| !cast.bad.contains(w))
                    else {
                        self.companion_cast_explore(host, b, run, pos, Search::Surface, "looking for water");
                        return;
                    };
                    let reach = super::companion_work::to_block(eye(pos), w);
                    if reach > 3.8 {
                        let goal = Goal::near([w[0], w[1] + 1, w[2]], 2.0);
                        if self.companion_travel(host, b, goal, pos) == Steer::Stuck {
                            log::info!("[companion] {} run: water at {w:?} out of reach", b.file.name);
                            cast.bad.push(w);
                        }
                        return;
                    }
                    if Cast::stuck_at(&mut cast.trying_water, w, self.world.game_time.max(0) as u64) {
                        log::info!("[companion] {} run: water at {w:?} out of reach", b.file.name);
                        cast.bad.push(w);
                        return;
                    }
                    self.companion_fill_bucket(host, b, w, pos);
                    return;
                }
                if count(&inv, "minecraft:lava_bucket") == 0 {
                    run.note = format!("lava for frame block {}", cast.slot + 1);
                    let sources: Vec<[i32; 3]> = b
                        .senses
                        .memory()
                        .recall(|s| self.is_source(s, Fluid::Lava), pool, 12);
                    let Some(src) = sources.iter().copied().find(|s| site.away_from(*s) && !cast.bad.contains(s)) else {
                        // None left it can use there: another pool.
                        cast.bad.push(pool);
                        cast.pool = None;
                        cast.site = None;
                        return;
                    };
                    // Half a minute at one source (out of reach, the ray
                    // taking something else): another one.
                    let late = Cast::stuck_at(&mut cast.trying, src, self.world.game_time.max(0) as u64);
                    let reached = self.companion_fill_lava(host, b, src, pos);
                    if !reached || late {
                        log::info!("[companion] {} run: lava at {src:?} out of reach", b.file.name);
                        // No way there: none to the ones around it either.
                        if !reached {
                            cast.bad.extend(sources.iter().filter(|s| (0..3).all(|k| (s[k] - src[k]).abs() <= 1)));
                        }
                        cast.bad.push(src);
                        cast.trying = None;
                        b.walk.reset();
                    }
                    return;
                }
                run.note = format!("casting frame block {} of 10", cast.slot + 1);
                if !self.companion_at_stand(host, b, stand, pos) {
                    return;
                }
                self.companion_stop_dig(host, b);
                // Pour against the back wall's face: the lava lands in the slot.
                if self.companion_hold(host, b, "minecraft:lava_bucket") {
                    let to = site.face_point(site.back(rc));
                    self.companion_use_toward(host, b, to, pos);
                    if self.res.facts().get(self.world.block(slot[0], slot[1], slot[2])).fluid == Fluid::Lava {
                        cast.phase = Phase::Water;
                        cast.since = b.ticks;
                    }
                }
            }
            Phase::Water => {
                if &*self.block_name(slot) == "minecraft:obsidian" {
                    cast.phase = Phase::Pickup;
                    cast.since = b.ticks;
                    return;
                }
                if b.ticks - cast.since > 20 {
                    // The water missed: the lava back into the bucket
                    // before it spreads (30 ticks), then again.
                    cast.phase = Phase::Scoop;
                    cast.since = b.ticks;
                    return;
                }
                if !self.companion_at_stand(host, b, stand, pos) {
                    return;
                }
                // Water beside or above the lava (never below: that sets
                // nothing) turns the source to obsidian.
                if self.res.facts().get(self.world.block(water[0], water[1], water[2])).fluid != Fluid::Water
                    && self.companion_hold(host, b, "minecraft:water_bucket")
                {
                    let to = site.face_point(site.back(water_cell(rc)));
                    self.companion_use_toward(host, b, to, pos);
                }
            }
            Phase::Scoop => {
                let lava = self.is_source(self.world.block(slot[0], slot[1], slot[2]).0, Fluid::Lava);
                if !lava || b.ticks - cast.since > 40 {
                    cast.phase = Phase::Pickup;
                    cast.since = b.ticks;
                    return;
                }
                if self.companion_hold(host, b, "minecraft:bucket") {
                    let to = site.near_point(slot);
                    self.companion_use_toward(host, b, to, pos);
                }
            }
            Phase::Pickup => {
                // The water back into a bucket: where it was poured, or
                // wherever in the frame or before it a source is left (two
                // sources side by side there make more: it would fill the
                // frame for good).
                let inv = self.companion_inventory(host, b);
                let mut cells = vec![
                    water,
                    site.front_of(slot),
                    site.front_of(water),
                    [slot[0], slot[1] + 1, slot[2]],
                    [slot[0] - 1, slot[1], slot[2]],
                    [slot[0] + 1, slot[1], slot[2]],
                ];
                for x in 0..=3 {
                    for y in 0..=4 {
                        cells.extend([site.at([x, y]), site.front_cell(x, y)]);
                    }
                }
                let e = eye(pos);
                let source = cells
                    .into_iter()
                    .filter(|p| self.is_source(self.world.block(p[0], p[1], p[2]).0, Fluid::Water))
                    .min_by(|a, b| {
                        super::companion_work::to_block(e, *a).total_cmp(&super::companion_work::to_block(e, *b))
                    });
                let Some(w) = source else {
                    cast.phase = Phase::Lava;
                    cast.since = b.ticks;
                    return;
                };
                if b.ticks - cast.since > 200 {
                    // It can't get at it: the water stays, the run goes on.
                    cast.phase = Phase::Lava;
                    cast.since = b.ticks;
                    return;
                }
                if !self.companion_at_stand(host, b, stand, pos) {
                    return;
                }
                if count(&inv, "minecraft:bucket") > 0 && self.companion_hold(host, b, "minecraft:bucket") {
                    // The near half of its face: the ray reaches it past
                    // the frame's corners.
                    let to = site.near_point(w);
                    self.companion_use_toward(host, b, to, pos);
                } else {
                    // Both buckets full: a block in its place.
                    let _ = self.companion_place_block(host, b, w, pos);
                }
            }
        }
    }

    /// The night: under the open sky it digs two blocks down, caps the pit
    /// and waits there for the morning (zombies and skeletons can't reach
    /// it); under a roof (mining) it works on. True while it shelters.
    fn companion_night(&mut self, host: &mut LanHost, b: &mut Body, run: &mut Run, pos: [f64; 3]) -> bool {
        let time = self.world.day_time.rem_euclid(24000);
        let day = self.world.day_time.div_euclid(24000);
        let night = (12900..23200).contains(&time);
        if !night {
            if run.shelter.take().is_some() {
                log::info!("[companion] {} run: morning, out of the pit", b.file.name);
                b.walk.reset();
            }
            return false;
        }
        let f = feet(pos);
        // In its capped pit nothing reaches it: it stays there, whatever
        // growls above (out of it, it fights what comes at it first).
        let sheltered = run.shelter == Some(f) && self.floor([f[0], f[1] + 2, f[2]]);
        if sheltered {
            b.target = None;
        }
        if run.no_shelter == Some(day) || (b.target.is_some() && !sheltered) || run.stage == Stage::Done {
            return false;
        }
        let pit = match run.shelter {
            Some(p) => p,
            None => {
                // Under a roof already: it works on.
                let roofed = (2..26).any(|dy| !self.passable([f[0], f[1] + dy, f[2]]));
                if roofed {
                    return false;
                }
                log::info!("[companion] {} run: night, digging a pit at {f:?}", b.file.name);
                self.companion_cancel_work(host, b, None);
                b.skill = Skill::Run;
                b.walk.reset();
                let p = [f[0], f[1] - 2, f[2]];
                run.shelter = Some(p);
                p
            }
        };
        run.note = "sheltering for the night".into();
        if f != pit {
            match self.companion_travel(host, b, Goal::exact(pit), pos) {
                Steer::Stuck => {
                    log::info!("[companion] {} run: no pit here tonight", b.file.name);
                    run.shelter = None;
                    run.no_shelter = Some(day);
                    return false;
                }
                _ => return true,
            }
        }
        // In it: the cap over its head.
        let cap = [pit[0], pit[1] + 2, pit[2]];
        if !self.floor(cap) {
            let _ = self.companion_place_block(host, b, cap, pos);
        }
        b.keys = Default::default();
        true
    }

    /// Place a throwaway block at `p` against a solid neighbour in reach;
    /// false when it can't (no blocks, nothing to place against).
    fn companion_place_block(&mut self, host: &mut LanHost, b: &mut Body, p: [i32; 3], pos: [f64; 3]) -> bool {
        let Some(slot) = host.hub.guest(b.conn).and_then(|g| throwaway_slot(&g.inventory)) else {
            return false;
        };
        let against = [[0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0], [0, -1, 0]]
            .iter()
            .map(|d| [p[0] + d[0], p[1] + d[1], p[2] + d[2]])
            .find(|n| self.floor(*n) && super::companion_work::to_block(eye(pos), *n) < 4.4);
        let Some(n) = against else {
            return false;
        };
        let face = match [p[0] - n[0], p[1] - n[1], p[2] - n[2]] {
            [0, -1, 0] => 0,
            [0, 1, 0] => 1,
            [0, 0, -1] => 2,
            [0, 0, 1] => 3,
            [-1, 0, 0] => 4,
            _ => 5,
        };
        self.companion_hold_slot(host, b, slot);
        self.companion_use_block(host, b, n, face, pos);
        true
    }

    fn companion_inventory(&self, host: &LanHost, b: &Body) -> Vec<Option<ItemStack>> {
        host.hub.guest(b.conn).map(|g| g.inventory.clone()).unwrap_or_default()
    }

    /// Walk to a standing spot (exact block); true once there and still.
    fn companion_at_stand(&mut self, host: &mut LanHost, b: &mut Body, stand: [i32; 3], pos: [f64; 3]) -> bool {
        let f = feet(pos);
        let at = [f64::from(stand[0]) + 0.5, f64::from(stand[1]), f64::from(stand[2]) + 0.5];
        if f == stand && c::horizontal(pos, at) < 0.35 {
            return true;
        }
        if f == stand {
            b.yaw = c::yaw_toward(pos, at) as f32;
            b.keys.forward = true;
            b.keys.shift = true;
            return false;
        }
        // A raised stand with nothing under it yet, right beside it: the
        // floor first (from where it is), then a step over.
        let under = [stand[0], stand[1] - 1, stand[2]];
        let beside = (f[0] - stand[0]).abs() + (f[2] - stand[2]).abs() == 1 && f[1] == stand[1];
        if beside && !self.floor(under) && self.passable(under) {
            b.walk.reset();
            b.keys.shift = true;
            let _ = self.companion_place_block(host, b, under, pos);
            return false;
        }
        let _ = self.companion_travel(host, b, Goal::exact(stand), pos);
        false
    }

    /// Fill the empty bucket from lava source `src`.
    /// False when it can't get there.
    fn companion_fill_lava(&mut self, host: &mut LanHost, b: &mut Body, src: [i32; 3], pos: [f64; 3]) -> bool {
        let reach = super::companion_work::to_block(eye(pos), src);
        if reach > 3.6 {
            // A floor beside the lava, not in it.
            let goal = Goal::near([src[0], src[1] + 1, src[2]], 2.5);
            return self.companion_travel(host, b, goal, pos) != Steer::Stuck;
        }
        let to = [f64::from(src[0]) + 0.5, f64::from(src[1]) + 0.85, f64::from(src[2]) + 0.5];
        // A natural block between its eyes and the lava (a pool under the
        // ground): dug first, if it is above the lava (one level with it
        // would let the lava out).
        let e = eye(pos);
        let d = c::distance(e, to).max(1e-6);
        let dir = [(to[0] - e[0]) / d, (to[1] - e[1]) / d, (to[2] - e[2]) / d];
        if let Some(h) = self.clip_ray(e, dir, d)
            && h.pos != src
        {
            if self.natural(h.pos) && h.pos[1] > src[1] {
                let face = super::companion_work::face_index(h.face);
                let _ = self.companion_dig_step(host, b, h.pos, face);
                return true;
            }
            return false;
        }
        self.companion_stop_dig(host, b);
        if !self.companion_hold(host, b, "minecraft:bucket") {
            return true;
        }
        self.companion_use_toward(host, b, to, pos);
        // Taken (or the ray took another source): what is there now.
        let now = self.world.block(src[0], src[1], src[2]).0;
        if !self.is_source(now, Fluid::Lava) {
            b.senses.memory_mut().forget(src);
        }
        true
    }

    /// Build the mould around a portal site, one block a tick.
    fn companion_build_mould(&mut self, host: &mut LanHost, b: &mut Body, site: &Site, pos: [f64; 3]) -> Build {
        // Clear: the frame plane (all but the corners we fill) and the front.
        for y in 0..=5 {
            for x in 0..=3 {
                let corner = matches!((x, y), (0, 0) | (3, 0) | (0, 4));
                let p = site.at([x, y]);
                if !corner && !self.passable(p) && &*self.block_name(p) != "minecraft:obsidian" {
                    {
                    b.walk.note = format!("clear {p:?}");
                    return self.companion_build_op(host, b, site, p, false, pos);
                }
                }
            }
        }
        for x in 0..=3 {
            for y in 0..=4 {
                let p = site.front_cell(x, y);
                if !self.passable(p) {
                    {
                    b.walk.note = format!("clear {p:?}");
                    return self.companion_build_op(host, b, site, p, false, pos);
                }
                }
            }
        }
        // Floors: under the frame, the front and the back wall.
        for x in 0..=3 {
            for p in [site.at([x, -1]), site.front_cell(x, -1), site.back([x, -1])] {
                if !self.floor(p) {
                    {
                    b.walk.note = format!("place {p:?}");
                    return self.companion_build_op(host, b, site, p, true, pos);
                }
                }
            }
        }
        // The back wall bottom up, then the corners.
        for y in 0..=5 {
            for x in 0..=3 {
                let p = site.back([x, y]);
                if !self.floor(p) {
                    {
                    b.walk.note = format!("place {p:?}");
                    return self.companion_build_op(host, b, site, p, true, pos);
                }
                }
            }
        }
        for rc in [[0, 0], [3, 0], [0, 4]] {
            let p = site.at(rc);
            if !self.floor(p) {
                {
                    b.walk.note = format!("place {p:?}");
                    return self.companion_build_op(host, b, site, p, true, pos);
                }
            }
        }
        Build::Done
    }

    /// One mould block: dug out or placed, from the front.
    fn companion_build_op(&mut self, host: &mut LanHost, b: &mut Body, site: &Site, p: [i32; 3], place: bool, pos: [f64; 3]) -> Build {
        let stand = site.stand((p[0] - site.base[0]).clamp(0, 3));
        // Its own feet's cell or head: from beside it.
        let stand = if p == stand || p == [stand[0], stand[1] + 1, stand[2]] {
            site.stand(if stand[0] - site.base[0] < 2 { 3 } else { 0 })
        } else {
            stand
        };
        if !self.companion_at_stand(host, b, stand, pos) {
            if b.walk.left() == 0 && b.ticks.is_multiple_of(400) {
                return Build::Failed;
            }
            return Build::Busy;
        }
        if super::companion_work::to_block(eye(pos), p) > 4.4 {
            return Build::Failed;
        }
        if !place {
            let face = site.front_face();
            if self.companion_dig_step(host, b, p, face).is_err() {
                return Build::Failed;
            }
            return Build::Busy;
        }
        self.companion_stop_dig(host, b);
        let Some(slot) = host.hub.guest(b.conn).and_then(|g| throwaway_slot(&g.inventory)) else {
            return Build::Failed;
        };
        let against = [[0, -1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0], [0, 1, 0]]
            .iter()
            .map(|d| [p[0] + d[0], p[1] + d[1], p[2] + d[2]])
            .find(|n| self.floor(*n) && super::companion_work::to_block(eye(pos), *n) < 4.4);
        let Some(n) = against else {
            // Nothing to place it against: the block under it first.
            let below = [p[0], p[1] - 1, p[2]];
            if below[1] > site.base[1] - 4 && !self.floor(below) {
                return self.companion_build_op(host, b, site, below, true, pos);
            }
            return Build::Busy;
        };
        let d = [p[0] - n[0], p[1] - n[1], p[2] - n[2]];
        let face = match d {
            [0, -1, 0] => 0,
            [0, 1, 0] => 1,
            [0, 0, -1] => 2,
            [0, 0, 1] => 3,
            [-1, 0, 0] => 4,
            _ => 5,
        };
        self.companion_hold_slot(host, b, slot);
        self.companion_use_block(host, b, n, face, pos);
        Build::Busy
    }
}

/// Where to look for a raw item it lacks.
fn search_for(item: &str, f: [i32; 3]) -> Search {
    let n = c::short_id(item);
    if n.ends_with("_log") || n.ends_with("_planks") || n == "stick" || n == "apple" {
        Search::Surface
    } else if n == "cobblestone" || n == "stone" || n == "cobbled_deepslate" {
        Search::Down
    } else if n.starts_with("raw_iron") || n.contains("iron") || n == "coal" || n == "flint" || n == "gravel" {
        Search::Mine(if f[1] > 40 { 16 } else { f[1].min(16) })
    } else {
        Search::Surface
    }
}

// ---- the cast's state -------------------------------------------------------------------

/// Where the water goes to set a frame cell's lava: above the bottom and
/// top rows, beside the side columns (inside the frame).
fn water_cell(rc: [i32; 2]) -> [i32; 2] {
    match rc {
        [0, y] => [1, y],
        [3, y] => [2, y],
        [x, y] => [x, y + 1],
    }
}

/// The frame's cells to fill, in order: (column, row) in the frame's plane.
/// The bottom two, the left column, the right column, the top two; the
/// corners are cobblestone (any block does).
const SLOTS: [[i32; 2]; 10] = [
    [1, 0],
    [2, 0],
    [0, 1],
    [0, 2],
    [0, 3],
    [3, 1],
    [3, 2],
    [3, 3],
    [1, 4],
    [2, 4],
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Lava,
    Water,
    Scoop,
    Pickup,
}

enum Build {
    Busy,
    Done,
    Failed,
}

/// A portal frame's place: the bottom left frame cell, the frame along x,
/// and which way (z) its front faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Site {
    base: [i32; 3],
    front: i32,
}

impl Site {
    /// The frame cell at column x, row y.
    /// The blocks of the mould: floors, the back wall, the corners.
    fn mould(&self) -> Vec<[i32; 3]> {
        let mut cells = Vec::new();
        for x in 0..=3 {
            cells.extend([self.at([x, -1]), self.front_cell(x, -1)]);
            cells.extend((-1..=5).map(|y| self.back([x, y])));
        }
        cells.extend([[0, 0], [3, 0], [0, 4]].map(|rc| self.at(rc)));
        cells
    }

    fn at(&self, rc: [i32; 2]) -> [i32; 3] {
        [self.base[0] + rc[0], self.base[1] + rc[1], self.base[2]]
    }
    /// The back wall's block behind a frame cell.
    fn back(&self, rc: [i32; 2]) -> [i32; 3] {
        let p = self.at(rc);
        [p[0], p[1], p[2] - self.front]
    }
    /// The front's cell at column x, row y (where it stands at row 0).
    fn front_cell(&self, x: i32, y: i32) -> [i32; 3] {
        [self.base[0] + x, self.base[1] + y, self.base[2] + self.front]
    }
    fn stand(&self, x: i32) -> [i32; 3] {
        self.front_cell(x.clamp(0, 3), 0)
    }
    /// Where it stands in the front row: column x, `h` blocks up.
    fn stand_at(&self, x: i32, h: i32) -> [i32; 3] {
        self.front_cell(x.clamp(0, 3), h)
    }
    /// Where it stands to pour into a frame cell: in front of its column,
    /// high enough that its eye sees the back wall behind the cell past
    /// the obsidian already cast below (rows 0-1 from the ground, row 2
    /// one block up, rows 3-4 two).
    fn stand_for(&self, rc: [i32; 2]) -> [i32; 3] {
        let h = match rc[1] {
            0 | 1 => 0,
            2 => 1,
            _ => 2,
        };
        self.stand_at(rc[0], h)
    }
    /// The cell in front of `p` (toward where it stands).
    fn front_of(&self, p: [i32; 3]) -> [i32; 3] {
        [p[0], p[1], p[2] + self.front]
    }
    /// A point on the front half of a frame cell's face toward the front.
    fn near_point(&self, p: [i32; 3]) -> [f64; 3] {
        [
            f64::from(p[0]) + 0.5,
            f64::from(p[1]) + 0.5,
            f64::from(p[2]) + 0.5 + 0.45 * f64::from(self.front),
        ]
    }
    /// The face of a back wall block that looks at the front.
    fn front_face(&self) -> u8 {
        if self.front > 0 { 3 } else { 2 }
    }
    /// The middle of a back wall block's front face.
    fn face_point(&self, wall: [i32; 3]) -> [f64; 3] {
        [
            f64::from(wall[0]) + 0.5,
            f64::from(wall[1]) + 0.5,
            f64::from(wall[2]) + 0.5 + 0.5 * f64::from(self.front),
        ]
    }
    /// A lava source not in the site's own blocks.
    fn away_from(&self, p: [i32; 3]) -> bool {
        let dz = (p[2] - self.base[2]) * self.front;
        !((-1..=1).contains(&dz) && (self.base[0] - 1..=self.base[0] + 4).contains(&p[0]))
    }
}

#[derive(Default)]
struct Cast {
    /// Where it last searched for lava through walls, and the search.
    scanned: Option<[i32; 3]>,
    search: Option<super::companion_work::BlockSearch>,
    pool: Option<[i32; 3]>,
    site: Option<Site>,
    mould_done: bool,
    slot: usize,
    phase: Phase,
    since: u64,
    fails: u32,
    /// The ruined portal it is going to, and those it went to.
    ruin: Option<[i32; 3]>,
    ruins_seen: Vec<[i32; 3]>,
    /// The frame block it is on and since when.
    slot_since: Option<(usize, u64)>,
    /// The lava source it is after and since when; sources it gave up on.
    trying: Option<([i32; 3], u64)>,
    trying_water: Option<([i32; 3], u64)>,
    bad: Vec<[i32; 3]>,
}

impl Cast {
    /// Half a minute after one source (out of reach, the ray taking
    /// something else): time for another.
    /// (The clock runs for the frame block, whichever source it is after:
    /// two it can't get at would otherwise take turns for ever.)
    fn stuck_at(timer: &mut Option<([i32; 3], u64)>, src: [i32; 3], now: u64) -> bool {
        match *timer {
            Some((_, t)) if now > t + 600 => {
                *timer = None;
                true
            }
            Some(_) => false,
            None => {
                *timer = Some((src, now));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_run_in_order() {
        let mut s = Stage::Wood;
        let mut names = vec![s.name()];
        while s != Stage::Done {
            s = s.next();
            names.push(s.name());
        }
        assert_eq!(names.len(), 7);
        assert_eq!(names[5], "nether portal");
    }

    #[test]
    fn a_site_frames_ten_obsidian_and_a_wall_behind() {
        let site = Site {
            base: [10, 64, 20],
            front: 1,
        };
        assert_eq!(site.at([0, 0]), [10, 64, 20]);
        assert_eq!(site.back([1, 4]), [11, 68, 19]);
        assert_eq!(site.stand(2), [12, 64, 21]);
        assert_eq!(site.face_point([11, 68, 19]), [11.5, 68.5, 20.0]);
        let mut cells: Vec<[i32; 2]> = SLOTS.to_vec();
        cells.sort();
        cells.dedup();
        assert_eq!(cells.len(), 10, "ten frame blocks, no corner");
        assert!(!SLOTS.contains(&[0, 0]) && !SLOTS.contains(&[3, 4]));
        assert!(!site.away_from([11, 64, 20]));
        // Water sets each cell from above or beside, never from below.
        assert_eq!(water_cell([1, 0]), [1, 1]);
        assert_eq!(water_cell([0, 3]), [1, 3]);
        assert_eq!(water_cell([3, 2]), [2, 2]);
        assert_eq!(water_cell([2, 4]), [2, 5]);
        // Higher cells are poured from higher up the front row.
        assert_eq!(site.stand_for([0, 1]), [10, 64, 21]);
        assert_eq!(site.stand_for([0, 2]), [10, 65, 21]);
        assert_eq!(site.stand_for([1, 4]), [11, 66, 21]);
        assert!(site.away_from([11, 64, 26]));
    }

    #[test]
    fn what_is_looked_for_where() {
        assert_eq!(search_for("minecraft:oak_log", [0, 70, 0]), Search::Surface);
        assert_eq!(search_for("minecraft:cobblestone", [0, 70, 0]), Search::Down);
        assert_eq!(search_for("minecraft:raw_iron", [0, 70, 0]), Search::Mine(16));
        assert_eq!(search_for("minecraft:raw_iron", [0, 10, 0]), Search::Mine(10));
        assert_eq!(search_for("minecraft:beef", [0, 70, 0]), Search::Surface);
    }
}
