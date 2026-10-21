//! The End of the Speedrunner's route (docs/companion-speedrun.md, "The
//! End"): from standing in a stronghold's portal room to the dragon's death,
//! alone, as a player would do it. The rules and the geometry are in
//! companion/end.rs; this is the body's skill (`Skill::End`):
//!
//! 1. **Portal.** Eyes into the frames that lack one, then into the portal
//!    (Watch mode takes its owner's view along).
//! 2. **Arrive.** From the obsidian platform (often inside the island's
//!    edge, or beside it over the void) straight toward the fountain: a
//!    staircase dug up through end stone, a bridge over gaps, never a step
//!    into the void.
//! 3. **Scout and crystals.** Fair finds the crystals by looking round (an
//!    entity is seen when a ray from its eye reaches it through air, bars or
//!    glass, in front of it); All-knowing knows all ten. The open ones are
//!    shot from the ground with the arrow's arc computed and the path
//!    checked; for a caged one it pillars up beside the cage, breaks a
//!    window in the bars, comes down, pillars up again 12 blocks off and
//!    shoots through the window, where the blast barely reaches.
//! 4. **Dragon.** It waits 12 blocks from the fountain, out of the breath
//!    clouds, stepping aside from fireballs. When the dragon perches it
//!    stands on a block beside the head (out of the wings' knockback, the
//!    head's and neck's bite and the breath on the ground) and lands
//!    critical hits on the head. With `beds` it blows beds up under the
//!    perched head from a hole instead, the ground between it and the
//!    blast. It eats and backs off between perches.
//! 5. **Exit.** Into the exit portal; the run is over.
//!
//! The Watch HUD shows the step ("Destroying crystals 6/10"), and the
//! moments are spoken as [`CompanionReport::Milestone`].
//!
//! Movement here is the End's own few primitives (a straight staircase
//! and bridge, pillar up to a height, dig down a pillar); the general
//! Movement 2.0 walker can take their place once it is in.
use super::{
    Dim, Game,
    companion::Body,
    companion_body::{Steer, look_pitch},
    companion_work::{centre, eye, feet, to_block},
    dragon::DragonView,
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c, CompanionReport, Skill,
        end::{self as e, DragonParts, EndPhase, Spike},
        perception::{PerceptionLevel, walk_ray},
        work::FULL_DRAW,
    },
    items::ItemStack,
    mobs::Who,
    sprint::Keys,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use serde_json::{Value, json};

/// Blocks it builds with (pillars, bridges, the step by the dragon's head).
/// (In this order: end stone last, kept for walls and roofs.)
const BLOCKS: [&str; 10] = [
    "minecraft:cobblestone",
    "minecraft:cobbled_deepslate",
    "minecraft:netherrack",
    "minecraft:dirt",
    "minecraft:stone",
    "minecraft:deepslate",
    "minecraft:blackstone",
    "minecraft:andesite",
    "minecraft:oak_planks",
    "minecraft:end_stone",
];
/// Blocks the dragon can't fly through (`dragon_immune`) that it can dig
/// again with an iron pickaxe: its walls and roof up a pillar.
const STURDY: [&str; 1] = ["minecraft:end_stone"];
/// End stone it gathers before a caged crystal (walls and roof on two
/// pillars).
const STURDY_WANT: u32 = 20;
/// How far it sees entities (a render distance of 10 chunks).
const ENTITY_SIGHT: f64 = 160.0;
/// Half its field of view, to see by (degrees).
const VIEW: f64 = 60.0;
/// How far from a caged spike's centre the second pillar stands.
const CAGE_FAR: i32 = 12;
/// Where it waits for the dragon to perch: this far from the fountain.
const WAIT_RING: f64 = 12.0;
/// Its health below which it backs off to eat, and above which it goes back.
const LOW: f32 = 11.0;
const HEALED: f32 = 16.0;
/// A part of the route taking longer than this is a failure (ticks).
const BUDGET_PORTAL: u64 = 2400;
const BUDGET_ARRIVE: u64 = 4800;
const BUDGET_CRYSTALS: u64 = 24_000;
const BUDGET_DRAGON: u64 = 48_000;
const ARROWS: [&str; 3] = [
    "minecraft:arrow",
    "minecraft:spectral_arrow",
    "minecraft:tipped_arrow",
];
const SWORDS: [&str; 5] = [
    "minecraft:netherite_sword",
    "minecraft:diamond_sword",
    "minecraft:iron_sword",
    "minecraft:stone_sword",
    "minecraft:wooden_sword",
];

/// A crystal it knows of.
#[derive(Clone, Debug)]
struct Known {
    at: [f64; 3],
    spike: Spike,
    gone: bool,
    /// Given up on (no way to shoot it): the dragon heals from it.
    skipped: bool,
}

/// Shooting an open crystal.
#[derive(Default)]
struct Shot {
    crystal: usize,
    spot: Option<[f64; 3]>,
    tried: Vec<[f64; 3]>,
    /// The tick the arrow flew, waiting to see it land.
    fired: Option<u64>,
    /// How long the last arrow takes to get there.
    flight: u64,
    /// Times the walker got nowhere on the way to the spot.
    stuck: u32,
    misses: u32,
    since: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CageStep {
    GoNear,
    PillarNear,
    Window,
    DownNear,
    GoFar,
    PillarFar,
    Shoot,
    DownFar,
}

/// Breaking a caged crystal.
struct Cage {
    crystal: usize,
    plan: e::CagePlan,
    step: CageStep,
    ground: i32,
    since: u64,
    fired: Option<u64>,
    misses: u32,
    /// Times it came down its pillar out of a breath cloud.
    retreats: u32,
}

/// A step it stands on by the perched dragon's head.
#[derive(Clone, Copy, Debug)]
struct Perch {
    spot: [f64; 3],
    /// The block it puts under that spot first.
    step: [i32; 3],
    since: u64,
}

/// The bed one-cycle at a perch: a hole to stand in, the bed's two cells.
#[derive(Clone, Copy, Debug)]
struct Bed {
    hole: [i32; 2],
    foot: [i32; 2],
    head: [i32; 2],
    /// Feet height on the ground there.
    ground: i32,
    since: u64,
    placed: Option<u64>,
    tries: u32,
}

/// The End route under way (`Body::end`).
pub(super) struct EndRun {
    pub level: PerceptionLevel,
    pub beds: bool,
    pub phase: EndPhase,
    /// The game tick it started at and when the phase began.
    start: u64,
    since: u64,
    /// Milestones: name, ticks since the start.
    pub splits: Vec<(String, u64)>,
    /// What it does now, for the HUD.
    pub note: String,
    pub failure: Option<String>,
    pub deaths: Vec<String>,
    known: Vec<Known>,
    looked: u32,
    scouts: u32,
    frames: Vec<[i32; 3]>,
    tried: Vec<[f64; 3]>,
    shot: Option<Shot>,
    cage: Option<Cage>,
    perch: Option<Perch>,
    bed: Option<Bed>,
    /// Its hands are on something (the bow, blocks, eyes): gear stays put.
    pub hands: bool,
    /// What threatens it now, named for its death if it comes to that.
    danger: &'static str,
    retreating: bool,
    /// Counts for the run summary.
    arrows: u32,
    hits: u32,
    beds_used: u32,
    bed_damage: f32,
    /// Bed plans dropped (a side it couldn't reach or dig): a few, then the sword.
    bed_misses: u32,
    dragon_health: f32,
    last_strike: u64,
    /// When the arrival platform's ground came in.
    arrived: Option<u64>,
    walk_fails: u32,
    /// Where this scout looks from (fixed once picked: a strafe it runs
    /// from doesn't move it).
    lookout: Option<[f64; 3]>,
    /// Where and since when it stood still while walking.
    stall: Option<([f64; 3], u64)>,
    health: f32,
    /// The fight's crystals have been there (its arena is in).
    arena: bool,
    /// Blocks it tried to put round its pillar's top.
    fence_tries: u32,
    /// The highest its feet have been since it last stood on the ground (a
    /// death after a long drop is a fall).
    peak: f64,
    /// Passes made back over the crystals it gave up on.
    passes: u8,
    /// On its way out of a cloud: it goes well clear before anything else
    /// may walk it back (no dithering on the edge while the cloud grows).
    fleeing: bool,
    /// Lifted a block out of a cloud that came down on it: the height it
    /// climbs to, then holds while the cloud lies under it.
    raised: Option<i32>,
    /// Ticks the raise has made no headway (on the ground under its top),
    /// and till when no new raise starts after one gave up.
    raise_stuck: u32,
    no_raise_until: u64,
    /// Hurt, with something after it on the ground: three blocks up where
    /// an enderman can't reach or stand (top, ground, since).
    tower: Option<(i32, i32, u64)>,
    /// The mobs after it last tick (to tell what set a new one off).
    hunted_by: Vec<u32>,
    /// The armour modifiers put on its attributes (`end_wear_armor`).
    armor_mods: Vec<(String, String)>,
}

impl EndRun {
    fn new(now: u64, level: PerceptionLevel, beds: bool) -> Self {
        Self {
            level,
            beds,
            phase: EndPhase::Portal,
            start: now,
            since: now,
            splits: Vec::new(),
            note: "Opening the End portal".into(),
            failure: None,
            deaths: Vec::new(),
            known: Vec::new(),
            looked: 0,
            scouts: 0,
            frames: Vec::new(),
            tried: Vec::new(),
            shot: None,
            cage: None,
            perch: None,
            bed: None,
            hands: false,
            danger: "",
            retreating: false,
            arrows: 0,
            hits: 0,
            beds_used: 0,
            bed_damage: 0.0,
            bed_misses: 0,
            dragon_health: 200.0,
            last_strike: 0,
            arrived: None,
            walk_fails: 0,
            lookout: None,
            stall: None,
            health: 20.0,
            arena: false,
            armor_mods: Vec::new(),
            fence_tries: 0,
            peak: 0.0,
            passes: 0,
            fleeing: false,
            raised: None,
            raise_stuck: 0,
            no_raise_until: 0,
            tower: None,
            hunted_by: Vec::new(),
        }
    }
    /// Whether it is on its way through the portal (the Watch's Stay on
    /// the way through doesn't end it).
    pub fn active(&self) -> bool {
        !matches!(self.phase, EndPhase::Done | EndPhase::Failed)
    }
    pub fn arriving(&self) -> bool {
        matches!(self.phase, EndPhase::Portal | EndPhase::Arrive)
    }
    fn set_phase(&mut self, phase: EndPhase, now: u64) {
        self.phase = phase;
        self.since = now;
        self.tried.clear();
        self.walk_fails = 0;
    }
    fn split(&mut self, name: &str, now: u64) {
        if !self.splits.iter().any(|(n, _)| n == name) {
            self.splits.push((name.to_owned(), now - self.start));
        }
    }
    fn crystals_left(&self) -> usize {
        10usize.saturating_sub(self.destroyed())
    }
    fn destroyed(&self) -> usize {
        self.known.iter().filter(|k| k.gone).count()
    }
    pub fn status(&self, now: u64) -> Value {
        json!({
            "phase": self.phase.name(),
            "perception": self.level.id(),
            "beds": self.beds,
            "note": self.note,
            "ticks": now - self.start,
            "splits": self.splits.iter().map(|(n, t)| json!([n, t])).collect::<Vec<_>>(),
            "known": self.known.len(),
            "destroyed": self.destroyed(),
            "skipped": self.known.iter().filter(|k| k.skipped && !k.gone).count(),
            "failure": self.failure,
            "deaths": self.deaths,
            "arrows": self.arrows,
            "hits": self.hits,
            "beds_used": self.beds_used,
            "bed_misses": self.bed_misses,
            "bed_damage": self.bed_damage,
            "dragon_health": self.dragon_health,
            "danger": self.danger,
        })
    }
}

/// The bow's aim at a crystal: where to look, how long the arrow flies,
/// whether it is the high arc.
#[derive(Clone, Copy, Debug)]
struct Aim {
    yaw: f32,
    pitch: f32,
    ticks: u32,
    lob: bool,
}

/// What a primitive came to this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prog {
    Busy,
    Done,
    Failed,
}

/// The face of block `p` that looks toward `from` (Direction 3D data value).
fn face_toward(p: [i32; 3], from: [f64; 3]) -> u8 {
    let c = centre(p);
    let d = [from[0] - c[0], from[1] - c[1], from[2] - c[2]];
    let k = (0..3)
        .max_by(|a, b| d[*a].abs().total_cmp(&d[*b].abs()))
        .unwrap_or(1);
    match (k, d[k] >= 0.0) {
        (0, true) => 5,
        (0, false) => 4,
        (1, true) => 1,
        (1, false) => 0,
        (_, true) => 3,
        _ => 2,
    }
}

/// The face index of a unit normal.
fn face_of(n: [i32; 3]) -> u8 {
    match n {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        _ => 5,
    }
}

/// Whether a body in the air is going up (a jump, not a fall).
fn v_up(g: Option<(bool, [f64; 3])>) -> bool {
    g.is_some_and(|(_, v)| v[1] > -0.2)
}

fn add(p: [i32; 3], d: [i32; 3]) -> [i32; 3] {
    [p[0] + d[0], p[1] + d[1], p[2] + d[2]]
}

fn count_any(inventory: &[Option<ItemStack>], items: &[&str]) -> u32 {
    inventory
        .iter()
        .flatten()
        .filter(|s| items.contains(&&*s.item))
        .map(|s| s.count)
        .sum()
}

/// The slot of the first of `items` it carries: the hotbar first, so that
/// holding it moves nothing.
fn slot_of(inventory: &[Option<ItemStack>], items: &[&str]) -> Option<usize> {
    let has = |i: &usize| {
        inventory[*i]
            .as_ref()
            .is_some_and(|s| items.contains(&&*s.item))
    };
    (0..inventory.len().min(9))
        .find(has)
        .or_else(|| (9..inventory.len()).find(has))
}

fn yaw_to(from: [f64; 3], to: [f64; 3]) -> f32 {
    c::yaw_toward(from, to) as f32
}

/// The unit view vector of a yaw and pitch (Minecraft degrees).
fn view(yaw: f32, pitch: f32) -> [f64; 3] {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
}

fn now_of(game: &Game<'_>) -> u64 {
    game.world.game_time.max(0) as u64
}

impl Game<'_> {
    /// `companion do end [level] [beds]`: the End route from here.
    pub(super) fn companion_start_end(
        &mut self,
        b: &mut Body,
        level: Option<PerceptionLevel>,
        beds: bool,
    ) {
        let now = now_of(self);
        let mut run = EndRun::new(now, level.unwrap_or_default(), beds);
        if b.dim == Dim::End {
            run.set_phase(EndPhase::Arrive, now);
        }
        b.target = None;
        b.post = None;
        b.nav.reset();
        b.skill = Skill::End;
        let name = b.file.name.clone();
        self.companions.end_runs.retain(|(n, _)| *n != name);
        b.end = Some(Box::new(run));
        self.companion_report(b, CompanionReport::SkillStarted(Skill::End));
    }

    pub(super) fn companion_end_status(&self, b: &Body) -> Value {
        match &b.end {
            Some(r) => r.status(now_of(self)),
            None => self
                .companions
                .end_runs
                .iter()
                .rev()
                .find(|(n, _)| *n == b.file.name)
                .map_or(Value::Null, |(_, v)| v.clone()),
        }
    }

    /// A finished run's status, kept for `companion list`.
    fn end_keep_record(&mut self, b: &Body, run: &EndRun) {
        let status = run.status(now_of(self));
        let runs = &mut self.companions.end_runs;
        runs.retain(|(n, _)| *n != b.file.name);
        runs.push((b.file.name.clone(), status));
    }

    /// The body's End skill this tick: true when it decided what the body
    /// does (else the usual fight goes on).
    pub(super) fn companion_end_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        pos: [f64; 3],
    ) -> bool {
        let Some(mut run) = b.end.take() else {
            return false;
        };
        if b.ticks.is_multiple_of(20) {
            self.end_wear_armor(host, b, &mut run);
        }
        self.end_trace_hurt(host, b, &mut run, pos);
        let handled = self.end_step(host, b, &mut run, pos);
        if !matches!(run.phase, EndPhase::Done | EndPhase::Failed) {
            self.end_mind_the_endermen(b, pos);
            self.end_note_new_hunters(host, b, &mut run, pos);
        }
        if self.watched(b)
            && let Some(w) = self.companions.watch.as_mut()
        {
            w.step.clone_from(&run.note);
            w.perception = run.level.label().to_owned();
        }
        if !run.active() {
            self.end_keep_record(b, &run);
        }
        b.end = Some(run);
        handled
    }

    /// Its death in the middle of the route: what killed it, as best it can tell.
    pub(super) fn companion_end_died(&mut self, b: &mut Body, eye: [f64; 3]) {
        let at = [eye[0], eye[1] - 1.32, eye[2]];
        let near_blast = b.end.as_ref().is_some_and(|run| {
            run.known
                .iter()
                .filter(|k| !k.gone && c::distance(k.at, at) < 14.0)
                .any(|k| !self.end_crystal_there(k.at))
        });
        let in_cloud = self
            .breath_view()
            .iter()
            .any(|(cl, r, _)| e::in_breath(at, *cl, *r, 0.5));
        let near_dragon = self.dragon_view().is_some_and(|d| {
            d.parts.iter().any(|p| {
                let mid = [(p.0[0] + p.1[0]) / 2.0, p.0[1], (p.0[2] + p.1[2]) / 2.0];
                c::distance(at, mid) < 6.0
            })
        });
        let Some(run) = b.end.as_mut() else {
            return;
        };
        let fell = run.peak - at[1] > 6.0;
        let cause = if at[1] < 0.0 {
            "the void"
        } else if fell && run.danger == "an enderman" {
            "a fall (an enderman's hit)"
        } else if fell {
            "a fall"
        } else if near_blast {
            "a crystal's blast"
        } else if in_cloud {
            "dragon breath"
        } else if near_dragon {
            "the dragon"
        } else if !run.danger.is_empty() {
            run.danger
        } else {
            "hurt"
        };
        run.deaths.push(cause.to_owned());
        if !matches!(run.phase, EndPhase::Done | EndPhase::Failed) {
            run.failure = Some(format!("died in phase {} ({cause})", run.phase.name()));
            run.phase = EndPhase::Failed;
            run.note = format!("Died: {cause}");
        }
        if let Some(run) = b.end.take() {
            self.end_keep_record(b, &run);
            b.end = Some(run);
        }
    }

    /// Its worn armour protects it: the hub never puts a guest's armour
    /// modifiers on its attributes (`LivingEntity.collectEquipmentChanges`),
    /// so the dragon's head, its wings and the endermen hit as through none.
    /// The local player's `apply_equipment`, for the guest, with its own list.
    fn end_wear_armor(&mut self, host: &mut LanHost, b: &Body, run: &mut EndRun) {
        // A carved pumpkin it carries goes on its head (endermen don't mind
        // its gaze then), its helmet back in the bag.
        if let Some(g) = host.hub.guest_mut(b.conn)
            && !g.armor[0].as_ref().is_some_and(|s| &*s.item == PUMPKIN)
            && let Some(i) = g
                .inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| &*s.item == PUMPKIN))
        {
            let pumpkin = g.inventory[i].take();
            g.inventory[i] = std::mem::replace(&mut g.armor[0], pumpkin);
            log::info!("[end] {} puts on a carved pumpkin", b.file.name);
        }
        let mut mods = std::mem::take(&mut run.armor_mods);
        self.as_guest(host, b.conn, |g| {
            std::mem::swap(&mut g.applied_modifiers, &mut mods);
            g.apply_equipment();
            std::mem::swap(&mut g.applied_modifiers, &mut mods);
        });
        run.armor_mods = mods;
    }

    /// Each hurt in the log, with what was round it then (the harness's
    /// causes come from this).
    fn end_trace_hurt(&self, host: &LanHost, b: &Body, run: &mut EndRun, pos: [f64; 3]) {
        let Some(health) = host.hub.guest(b.conn).map(|g| g.health) else {
            return;
        };
        if health < run.health - 0.01 {
            let dragon = self.dragon_view().map(|d| {
                (
                    d.phase,
                    (c::distance(pos, d.position) * 10.0).round() / 10.0,
                )
            });
            let cloud = self
                .breath_view()
                .iter()
                .map(|(at, _, _)| c::distance(pos, *at))
                .fold(f64::INFINITY, f64::min);
            log::info!(
                "[end] {} hurt {:.1} -> {health:.1} at {:?} phase {} danger '{}' dragon {dragon:?} cloud {cloud:.1} crystals {} fireballs {} note '{}' raised {:?} perch {:?} blocks {}",
                b.file.name,
                run.health - health,
                pos.map(|v| (v * 10.0).round() / 10.0),
                run.phase.name(),
                run.danger,
                self.crystals.len(),
                self.dragon_fireball_view().len(),
                run.note,
                run.raised,
                run.perch.map(|p| (p.spot, p.step)),
                count_any(
                    &host
                        .hub
                        .guest(b.conn)
                        .map(|g| g.inventory.clone())
                        .unwrap_or_default(),
                    &BLOCKS
                ),
            );
            if std::env::var_os("HORIZON_END_TRACE").is_some() {
                let clouds: Vec<_> = self
                    .breath_view()
                    .iter()
                    .map(|(at, r, d)| (at.map(|v| (v * 10.0).round() / 10.0), *r, *d))
                    .collect();
                log::info!("[end] clouds then {clouds:?}");
            }
        }
        run.health = health;
        let grounded = host.hub.guest(b.conn).is_some_and(|g| g.on_ground);
        run.peak = if grounded {
            pos[1]
        } else {
            run.peak.max(pos[1])
        };
    }

    fn end_crystal_there(&self, at: [f64; 3]) -> bool {
        self.crystals
            .iter()
            .any(|x| (x.position.x - at[0]).abs() < 0.5 && (x.position.z - at[2]).abs() < 0.5)
    }

    fn end_fail(&mut self, b: &mut Body, run: &mut EndRun, why: &str) {
        log::info!("[companion] {}: the End run failed: {why}", b.file.name);
        run.failure = Some(why.to_owned());
        run.note = format!("Failed: {why}");
        run.phase = EndPhase::Failed;
        run.hands = false;
        self.companion_report(
            b,
            CompanionReport::SkillFailed {
                skill: Skill::End,
                reason: why.to_owned(),
            },
        );
    }

    fn end_say(&mut self, b: &mut Body, line: &str) {
        self.companion_report(
            b,
            CompanionReport::Milestone {
                line: line.to_owned(),
            },
        );
    }

    fn end_step(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        pos: [f64; 3],
    ) -> bool {
        let now = now_of(self);
        run.hands = false;
        run.danger = "";
        match run.phase {
            EndPhase::Done | EndPhase::Failed => return true,
            EndPhase::Portal if b.dim == Dim::End => {
                run.split("entered the End", now);
                run.set_phase(EndPhase::Arrive, now);
                self.end_say(b, "We're in the End.");
            }
            EndPhase::Portal => {}
            EndPhase::Exit if b.dim == Dim::Overworld => {
                run.split("exit portal", now);
                run.set_phase(EndPhase::Done, now);
                run.note = "Done: the dragon is dead".into();
                self.companion_report(b, CompanionReport::SkillFinished(Skill::End));
                self.end_say(b, "Back home. Run complete.");
                return true;
            }
            _ if b.dim != Dim::End => {
                let why = run
                    .deaths
                    .last()
                    .map_or_else(|| "left the End".to_owned(), |d| format!("died ({d})"));
                self.end_fail(b, run, &why);
                return true;
            }
            _ => {}
        }
        let budget = match run.phase {
            EndPhase::Portal => BUDGET_PORTAL,
            EndPhase::Arrive => BUDGET_ARRIVE,
            EndPhase::Scout | EndPhase::Crystals => BUDGET_CRYSTALS,
            _ => BUDGET_DRAGON,
        };
        if now - run.since > budget {
            let why = format!("{} took too long", run.phase.name());
            self.end_fail(b, run, &why);
            return true;
        }
        // Badly hurt by the dragon's head: away to heal first.
        let health = host.hub.guest(b.conn).map_or(20.0, |g| g.health);
        if health < LOW {
            run.retreating = true;
            if run.perch.take().is_some() || run.bed.take().is_some() {
                self.companion_stop_dig(host, b);
            }
        } else if health >= HEALED {
            run.retreating = false;
        }
        // The fight it is in, where it may fight: on the ground, not up a pillar.
        let critical = run
            .cage
            .as_ref()
            .is_some_and(|k| k.step != CageStep::GoNear && k.step != CageStep::GoFar)
            || run.perch.is_some()
            || run.bed.is_some()
            || run.phase == EndPhase::Portal;
        if critical {
            b.target = None;
            // Up a pillar, something on it within reach: hit back from where
            // it stands (an enderman follows it up).
            if run.cage.is_some()
                && let Some(id) = self.end_hunter(host, b, pos, 4.0)
                && self.end_strike_in_place(host, b, run, id, pos)
            {
                return true;
            }
        }
        // Out of the breath and the fireballs' way before any fight.
        if b.dim == Dim::End
            && run.phase != EndPhase::Arrive
            && self.end_keep_safe(host, b, run, pos, critical || b.target.is_some())
        {
            return true;
        }
        if b.dim == Dim::End && !critical && self.end_tower(host, b, run, health, pos) {
            return true;
        }
        if let Some(id) = b.target {
            if self.end_fight(host, b, run, id, pos) {
                return true;
            }
            b.target = None;
        }
        match run.phase {
            EndPhase::Portal => self.end_portal(host, b, run, pos),
            EndPhase::Arrive => self.end_arrive(host, b, run, pos),
            EndPhase::Scout => self.end_scout(host, b, run, pos),
            EndPhase::Crystals => self.end_crystals(host, b, run, pos),
            EndPhase::Dragon => self.end_dragon(host, b, run, pos),
            EndPhase::Exit => self.end_exit(host, b, run, pos),
            EndPhase::Done | EndPhase::Failed => {}
        }
        true
    }

    // ---- what it sees ---------------------------------------------------------------

    /// Whether sight passes block `p` (air, bars, glass, plants, fire).
    fn end_clear(&self, p: [i32; 3]) -> bool {
        if !self.world.loaded(p[0], p[2]) {
            return false;
        }
        let f = self.res.facts().get(self.world.block(p[0], p[1], p[2]));
        f.fluid == Fluid::None && (!f.full_collision && !f.can_occlude || f.name.contains("glass"))
    }

    /// Whether `to` is in sight from `from` within `range`, and whether the
    /// line passes iron bars on the way.
    fn end_sight(&self, from: [f64; 3], to: [f64; 3], range: f64) -> (bool, bool) {
        let d = c::distance(from, to);
        if d > range || d < 1e-6 {
            return (d < 1e-6, false);
        }
        let dir = [
            (to[0] - from[0]) / d,
            (to[1] - from[1]) / d,
            (to[2] - from[2]) / d,
        ];
        let target = to.map(|v| v.floor() as i32);
        let (mut seen, mut bars) = (true, false);
        walk_ray(from, dir, d, |p| {
            if p == target {
                return false;
            }
            if !self.end_clear(p) {
                seen = false;
                return false;
            }
            if self.block_name(p).ends_with("iron_bars") {
                bars = true;
            }
            true
        });
        (seen, bars)
    }

    /// Whether `to` is in front of a body looking along `yaw`.
    fn in_view(from: [f64; 3], yaw: f32, to: [f64; 3]) -> bool {
        let want = c::yaw_toward(from, to);
        c::wrap_degrees(want - f64::from(yaw)).abs() <= VIEW
    }

    /// What it knows of the crystals now: All-knowing every one (and which
    /// are caged); Fair those it sees in front of it, and those it sees or
    /// hears gone.
    fn end_see_crystals(&mut self, run: &mut EndRun, pos: [f64; 3], yaw: f32) {
        let from = eye(pos);
        // The fight's crystals come with its arena: none there before then
        // says nothing.
        run.arena |= !self.crystals.is_empty();
        // (All-knowing knows where they stand all the same.)
        if !run.arena && !(run.level == PerceptionLevel::AllKnowing && run.known.is_empty()) {
            return;
        }
        if run.level == PerceptionLevel::AllKnowing {
            if run.known.is_empty() {
                for s in horizon_world::worldgen::end_spike::spikes_for_seed(self.level.seed) {
                    let at = [
                        f64::from(s.center_x) + 0.5,
                        f64::from(s.height + 1),
                        f64::from(s.center_z) + 0.5,
                    ];
                    run.known.push(Known {
                        at,
                        spike: Spike::under(at, s.guarded),
                        gone: false,
                        skipped: false,
                    });
                }
            }
            for i in 0..run.known.len() {
                if run.arena && !run.known[i].gone && !self.end_crystal_there(run.known[i].at) {
                    run.known[i].gone = true;
                }
            }
            return;
        }
        // Fair (and the levels between, for crystals): by sight.
        let crystals: Vec<[f64; 3]> = self
            .crystals
            .iter()
            .map(|x| [x.position.x, x.position.y, x.position.z])
            .collect();
        for at in crystals {
            if run
                .known
                .iter()
                .any(|k| (k.at[0] - at[0]).abs() < 0.5 && (k.at[2] - at[2]).abs() < 0.5)
            {
                continue;
            }
            let middle = [at[0], at[1] + 1.0, at[2]];
            if !Self::in_view(from, yaw, middle) {
                continue;
            }
            let (seen, bars) = self.end_sight(from, middle, ENTITY_SIGHT);
            if seen {
                run.known.push(Known {
                    at,
                    spike: Spike::under(at, bars),
                    gone: false,
                    skipped: false,
                });
            }
        }
        // The healing beam to the dragon shows the crystal at its end (a
        // bright line across the sky, seen from anywhere near).
        if let Some(at) = self.dragon_healer_view()
            && c::distance(from, at) < ENTITY_SIGHT
            && !run
                .known
                .iter()
                .any(|k| (k.at[0] - at[0]).abs() < 0.6 && (k.at[2] - at[2]).abs() < 0.6)
        {
            // (Caged or not: the bars round it show from close by.)
            let bars = horizon_world::worldgen::end_spike::spikes_for_seed(self.level.seed)
                .iter()
                .any(|s| {
                    s.guarded
                        && (f64::from(s.center_x) + 0.5 - at[0]).abs() < 1.0
                        && (f64::from(s.center_z) + 0.5 - at[2]).abs() < 1.0
                });
            run.known.push(Known {
                at,
                spike: Spike::under(at, bars),
                gone: false,
                skipped: false,
            });
        }
        // A spike top in sight with no crystal on it (one blown up before it
        // saw it): that one is accounted for too, so the count can reach ten.
        for s in horizon_world::worldgen::end_spike::spikes_for_seed(self.level.seed) {
            let at = [
                f64::from(s.center_x) + 0.5,
                f64::from(s.height + 1),
                f64::from(s.center_z) + 0.5,
            ];
            if run
                .known
                .iter()
                .any(|k| (k.at[0] - at[0]).abs() < 0.6 && (k.at[2] - at[2]).abs() < 0.6)
                || self.end_crystal_there(at)
            {
                continue;
            }
            let middle = [at[0], at[1] + 1.0, at[2]];
            if Self::in_view(from, yaw, middle)
                && c::distance(from, middle) < 64.0
                && self.end_sight(from, middle, ENTITY_SIGHT).0
            {
                run.known.push(Known {
                    at,
                    spike: Spike::under(at, s.guarded),
                    gone: true,
                    skipped: false,
                });
            }
        }
        // Gone: where it would see one, none (the blast is loud and bright).
        for i in 0..run.known.len() {
            let k = run.known[i].at;
            if run.known[i].gone || self.end_crystal_there(k) {
                continue;
            }
            let middle = [k[0], k[1] + 1.0, k[2]];
            if self.end_sight(from, middle, ENTITY_SIGHT).0 || c::distance(from, k) < 64.0 {
                run.known[i].gone = true;
            }
        }
    }

    /// Endermen: never let its look rest on one's eyes (EnderMan
    /// .isBeingStaredBy's cone, widened); it looks down or up instead.
    fn end_mind_the_endermen(&self, b: &mut Body, pos: [f64; 3]) {
        let from = eye(pos);
        let calm: Vec<[f64; 3]> = self
            .mobs
            .iter()
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:enderman" && m.ai.target.is_none())
            .map(|m| [m.position.x, m.position.y + 2.55, m.position.z])
            .filter(|to| c::distance(from, *to) < 64.0)
            .collect();
        if calm.is_empty() {
            return;
        }
        let staring = |yaw: f32, pitch: f32| {
            let v = view(yaw, pitch);
            calm.iter().any(|to| {
                let d = c::distance(from, *to).max(0.5);
                let dir = [
                    (to[0] - from[0]) / d,
                    (to[1] - from[1]) / d,
                    (to[2] - from[2]) / d,
                ];
                let dot = v[0] * dir[0] + v[1] * dir[1] + v[2] * dir[2];
                dot > 1.0 - 0.1 / d && self.end_sight(from, *to, 64.0).0
            })
        };
        if staring(b.yaw, b.pitch) {
            for dp in [25.0, -25.0, 45.0, 60.0] {
                let p = (b.pitch + dp).clamp(-90.0, 90.0);
                if !staring(b.yaw, p) {
                    b.pitch = p;
                    return;
                }
            }
        }
    }

    // ---- moving -------------------------------------------------------------------------

    /// Walk to `goal` along the island (the vanilla walker, which keeps off
    /// the edges).
    fn end_walk(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        goal: [f64; 3],
        near: f64,
        pos: [f64; 3],
    ) -> Steer {
        let sprint = c::horizontal(pos, goal) > 6.0;
        self.companion_steer(host, b, goal, near, sprint)
    }

    /// The top of the ground in column `x`, `z` below `from` (feet height:
    /// the first room for a player over a full block that isn't a spike).
    fn end_ground(&self, x: i32, z: i32, from: i32) -> Option<i32> {
        if !self.world.loaded(x, z) {
            return None;
        }
        (1..from).rev().find(|&y| {
            self.floor([x, y - 1, z])
                && self.passable([x, y, z])
                && self.passable([x, y + 1, z])
                && !self.block_name([x, y - 1, z]).ends_with("obsidian")
        })
    }

    fn end_guest(&self, host: &LanHost, b: &Body) -> Option<(bool, [f64; 3])> {
        let g = host.hub.guest(b.conn)?;
        let v = g
            .motion
            .as_ref()
            .map_or([0.0; 3], |m| [m.velocity.x, m.velocity.y, m.velocity.z]);
        Some((g.on_ground, v))
    }

    fn end_inventory(&self, host: &LanHost, b: &Body) -> Vec<Option<ItemStack>> {
        host.hub
            .guest(b.conn)
            .map(|g| g.inventory.clone())
            .unwrap_or_default()
    }

    /// A right click on `against`'s `face` holding the first of `items`,
    /// facing `yaw` (a bed lies the way its placer faces).
    #[allow(clippy::too_many_arguments)]
    fn end_use_on(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        against: [i32; 3],
        face: u8,
        items: &[&str],
        yaw: Option<f32>,
        pos: [f64; 3],
    ) -> bool {
        let inv = self.end_inventory(host, b);
        // The first kind in `items` it has (blocks: end stone last).
        let Some(slot) = items.iter().find_map(|i| slot_of(&inv, &[i])) else {
            return false;
        };
        self.companion_hold_slot(host, b, slot);
        let to = centre(against);
        b.yaw = yaw.unwrap_or_else(|| yaw_to(pos, to));
        b.pitch = look_pitch(eye(pos), to);
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = (b.yaw, b.pitch);
        }
        self.companion_use_block(host, b, against, face, pos);
        (b.yaw, b.pitch) = (yaw.unwrap_or(b.yaw), look_pitch(eye(pos), to));
        true
    }

    /// Dig block `p` (it looks at it): Done once it is gone.
    fn end_dig(&mut self, host: &mut LanHost, b: &mut Body, p: [i32; 3], pos: [f64; 3]) -> Prog {
        if self.passable(p) && !self.block_name(p).ends_with("iron_bars") {
            if b.work.dig.is_some_and(|(d, _)| d == p) {
                self.companion_stop_dig(host, b);
            }
            return Prog::Done;
        }
        if to_block(eye(pos), p) > 4.4 {
            return Prog::Failed;
        }
        let face = face_toward(p, eye(pos));
        match self.companion_dig_step(host, b, p, face) {
            Ok(()) => Prog::Busy,
            Err(()) => Prog::Failed,
        }
    }

    /// Pillar up where it stands until its feet are at `to_y`: a jump, and a
    /// block under its feet at the top of it. (End primitive: "pillar to
    /// height H here".)
    fn end_pillar(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        to_y: i32,
        pos: [f64; 3],
    ) -> Prog {
        let f = feet(pos);
        let Some((on_ground, _)) = self.end_guest(host, b) else {
            return Prog::Failed;
        };
        if f[1] >= to_y && on_ground {
            return Prog::Done;
        }
        run.hands = true;
        if count_any(&self.end_inventory(host, b), &BLOCKS) == 0 {
            return Prog::Failed;
        }
        // Something over its head: dug first.
        let above = [f[0], f[1] + 2, f[2]];
        if !self.passable(above) {
            return match self.end_dig(host, b, above, pos) {
                Prog::Failed => Prog::Failed,
                _ => Prog::Busy,
            };
        }
        b.pitch = 90.0;
        // Over the middle of its column.
        let mid = [f64::from(f[0]) + 0.5, pos[1], f64::from(f[2]) + 0.5];
        if c::horizontal(pos, mid) > 0.2 && on_ground {
            b.yaw = yaw_to(pos, mid);
            b.keys.forward = true;
            b.keys.shift = true;
            return Prog::Busy;
        }
        if on_ground {
            b.keys.jump = true;
            return Prog::Busy;
        }
        // Over the cell it left: a block in it (a torch there, on the exit
        // portal's pillar, taken off first: nothing goes into its cell).
        let base = [f[0], f[1] - 1, f[2]];
        if self.block_name(base).ends_with("torch") {
            let _ = self.end_dig(host, b, base, pos);
            return Prog::Busy;
        }
        if pos[1] - pos[1].floor() > 0.15
            && self.passable(base)
            && self.floor([base[0], base[1] - 1, base[2]])
        {
            let yaw = b.yaw;
            self.end_use_on(
                host,
                b,
                [base[0], base[1] - 1, base[2]],
                1,
                &BLOCKS,
                Some(yaw),
                pos,
            );
            b.pitch = 90.0;
        }
        Prog::Busy
    }

    /// A two-block wall round it where it stands on a pillar (the dragon's
    /// wings lift it over one): each side's cell at its feet filled, on a
    /// block put against its pillar's side first, then the cell above that,
    /// except on the `open` side at head height (where it shoots through).
    /// Done once every side is blocked (or a side can't be). (End primitive.)
    fn end_fence(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        open: [i32; 3],
        pos: [f64; 3],
    ) -> Prog {
        let f = feet(pos);
        let pillar = [f[0], f[1] - 1, f[2]];
        if !self.floor(pillar) {
            return Prog::Done;
        }
        run.hands = true;
        // End stone where it has some: the dragon flies through anything
        // else (`dragon_immune`), wall and roof with it.
        let sturdy: &[&str] = if count_any(&self.end_inventory(host, b), &STURDY) > 0 {
            &STURDY
        } else {
            &BLOCKS
        };
        let sides = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];
        let cells = sides
            .iter()
            .map(|&d| (d, 0))
            .chain(sides.iter().filter(|&&d| d != open).map(|&d| (d, 1)));
        for (d, dy) in cells {
            let side = add(add(f, d), [0, dy, 0]);
            if !self.passable(side) || self.block_name(side).ends_with("iron_bars") {
                continue;
            }
            let below = [side[0], side[1] - 1, side[2]];
            let yaw = b.yaw;
            if self.floor(below) {
                if !self.end_use_on(host, b, below, 1, sturdy, Some(yaw), pos) {
                    return Prog::Done;
                }
            } else if dy > 0 {
                // Its feet row on that side couldn't be filled: leave it.
                continue;
            } else if !self.end_use_on(host, b, pillar, face_of(d), sturdy, Some(yaw), pos) {
                return Prog::Done;
            }
            run.fence_tries += 1;
            if run.fence_tries > 40 {
                return Prog::Done;
            }
            return Prog::Busy;
        }
        // A roof (on a block set on one wall's top): the wings' lift can't
        // throw it over the wall either.
        let roof = [f[0], f[1] + 2, f[2]];
        let wall = sides
            .iter()
            .copied()
            .filter(|&d| d != open)
            .find(|&d| self.floor(add(add(f, d), [0, 1, 0])));
        if self.passable(roof)
            && let Some(d) = wall
            && run.fence_tries <= 40
        {
            let head_wall = add(add(f, d), [0, 1, 0]);
            let support = add(head_wall, [0, 1, 0]);
            let yaw = b.yaw;
            let placed = if self.passable(support) {
                self.end_use_on(host, b, head_wall, 1, sturdy, Some(yaw), pos)
            } else {
                self.end_use_on(
                    host,
                    b,
                    support,
                    face_of([-d[0], 0, -d[2]]),
                    sturdy,
                    Some(yaw),
                    pos,
                )
            };
            if placed {
                run.fence_tries += 1;
                return Prog::Busy;
            }
        }
        run.fence_tries = 0;
        Prog::Done
    }

    /// End stone gathered where it stands: the end stone floor round its
    /// feet (not under them), dug one at a time and picked up. Done with
    /// `want` in hand, or when there is none left in reach. (End primitive.)
    fn end_quarry(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        want: u32,
        pos: [f64; 3],
    ) -> Prog {
        if count_any(&self.end_inventory(host, b), &STURDY) >= want {
            return Prog::Done;
        }
        let f = feet(pos);
        let cell = (-1..=1)
            .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
            .filter(|&(dx, dz)| (dx, dz) != (0, 0))
            .map(|(dx, dz)| [f[0] + dx, f[1] - 1, f[2] + dz])
            .find(|&p| self.block_name(p).ends_with("end_stone"));
        let Some(cell) = cell else {
            return Prog::Done;
        };
        run.hands = true;
        run.note = format!(
            "Destroying crystals {}/10: digging end stone for a pillar's walls",
            run.destroyed()
        );
        match self.end_dig(host, b, cell, pos) {
            Prog::Failed => Prog::Done,
            _ => Prog::Busy,
        }
    }

    /// Down its own pillar to feet height `to_y`: the block under its feet,
    /// dug, one at a time. (End primitive.)
    fn end_descend(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        to_y: i32,
        pos: [f64; 3],
    ) -> Prog {
        let f = feet(pos);
        let Some((on_ground, _)) = self.end_guest(host, b) else {
            return Prog::Failed;
        };
        if f[1] <= to_y {
            return Prog::Done;
        }
        if !on_ground {
            return Prog::Busy;
        }
        run.hands = true;
        match self.end_dig(host, b, [f[0], f[1] - 1, f[2]], pos) {
            Prog::Failed => Prog::Failed,
            _ => Prog::Busy,
        }
    }

    /// Toward `to` in a straight line at its own height or up: a staircase
    /// dug up through what is in the way, a bridge over a gap. The way off
    /// the arrival platform, whatever the island does there. (End primitive.)
    fn end_traverse(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        to: [f64; 3],
        pos: [f64; 3],
    ) -> Prog {
        let f = feet(pos);
        let Some((on_ground, _)) = self.end_guest(host, b) else {
            return Prog::Failed;
        };
        let (dx, dz) = (to[0] - pos[0], to[2] - pos[2]);
        if dx.hypot(dz) < 1.0 {
            return Prog::Done;
        }
        let dir = if dx.abs() >= dz.abs() {
            [dx.signum() as i32, 0, 0]
        } else {
            [0, 0, dz.signum() as i32]
        };
        let a = add(f, dir);
        let up = |p: [i32; 3], n: i32| [p[0], p[1] + n, p[2]];
        if !on_ground {
            // Up a step: on into it.
            b.keys.forward = v_up(self.end_guest(host, b));
            return Prog::Busy;
        }
        run.hands = true;
        // A wall ahead: a step up through it.
        if !self.passable(a) {
            // Room over its head for a full jump, and for itself on the step.
            for p in [up(f, 2), up(f, 3), up(a, 1), up(a, 2)] {
                if !self.passable(p) {
                    return match self.end_dig(host, b, p, pos) {
                        Prog::Failed => Prog::Failed,
                        _ => Prog::Busy,
                    };
                }
            }
            self.companion_stop_dig(host, b);
            b.yaw = yaw_to(pos, centre(a));
            b.pitch = 0.0;
            b.keys.forward = true;
            b.keys.jump = true;
            return Prog::Busy;
        }
        if !self.passable(up(a, 1)) {
            return match self.end_dig(host, b, up(a, 1), pos) {
                Prog::Failed => Prog::Failed,
                _ => Prog::Busy,
            };
        }
        // A gap ahead: a block to walk on, against its own floor's side.
        // (A drop of up to four blocks it takes; deeper, a bridge.)
        let floor_ahead = (1..=4).any(|d| self.floor([a[0], a[1] - d, a[2]]));
        if !floor_ahead {
            b.keys.shift = true;
            let yaw = yaw_to(pos, centre(a));
            if !self.end_use_on(host, b, up(f, -1), face_of(dir), &BLOCKS, Some(yaw), pos) {
                return Prog::Failed;
            }
            return Prog::Busy;
        }
        b.yaw = yaw_to(pos, centre(a));
        b.pitch = 0.0;
        b.keys.forward = true;
        Prog::Busy
    }

    /// Whether it is on the island's surface: on end stone, the sky over it.
    fn end_on_surface(&self, pos: [f64; 3]) -> bool {
        let f = feet(pos);
        let under = [f[0], f[1] - 1, f[2]];
        self.floor(under)
            && !self.block_name(under).ends_with("obsidian")
            && (2..24).all(|dy| self.passable([f[0], f[1] + dy, f[2]]))
    }

    // ---- the portal room ----------------------------------------------------------------

    /// A portal frame's eye and facing, if `p` is one.
    fn end_frame(&self, p: [i32; 3]) -> Option<(bool, [i32; 2])> {
        let s = self
            .pack
            .assets
            .registry
            .state(self.world.block(p[0], p[1], p[2]))?;
        if s.name != "minecraft:end_portal_frame" {
            return None;
        }
        let eye = s.properties.get("eye").map(String::as_str) == Some("true");
        let facing = match s.properties.get("facing").map(String::as_str) {
            Some("north") => [0, -1],
            Some("south") => [0, 1],
            Some("east") => [1, 0],
            _ => [-1, 0],
        };
        Some((eye, facing))
    }

    fn end_portal(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        let f = feet(pos);
        // The frames it knows of: those it sees (All-knowing and Sharp
        // senses: those near, through walls).
        if run.frames.len() < 12 {
            let from = eye(pos);
            let xray = run.level.xray_radius(12).is_some();
            for x in -12..=12 {
                for y in -5..=5 {
                    for z in -12..=12 {
                        let p = [f[0] + x, f[1] + y, f[2] + z];
                        if run.frames.contains(&p) || self.end_frame(p).is_none() {
                            continue;
                        }
                        let top = [
                            f64::from(p[0]) + 0.5,
                            f64::from(p[1]) + 0.9,
                            f64::from(p[2]) + 0.5,
                        ];
                        if xray
                            || Self::in_view(from, b.yaw, top) && self.end_sight(from, top, 24.0).0
                        {
                            run.frames.push(p);
                        }
                    }
                }
            }
            if run.frames.len() < 12 && run.looked < 60 {
                run.looked += 1;
                run.note = "Looking for the portal frames".into();
                b.yaw += 6.0;
                b.pitch = 20.0;
                return;
            }
            // (Just teleported there, its world may still be loading: a
            // few more looks round first.)
            if run.frames.is_empty() && run.walk_fails < 5 {
                run.walk_fails += 1;
                run.looked = 0;
                return;
            }
            if run.frames.is_empty() {
                self.end_fail(b, run, "no End portal frames in sight");
                return;
            }
        }
        // The opening's middle: two blocks in from each frame, on average.
        let inward: Vec<[f64; 2]> = run
            .frames
            .iter()
            .filter_map(|p| {
                let (_, d) = self.end_frame(*p)?;
                Some([
                    f64::from(p[0] + 2 * d[0]) + 0.5,
                    f64::from(p[2] + 2 * d[1]) + 0.5,
                ])
            })
            .collect();
        let n = inward.len().max(1) as f64;
        let middle = [
            inward.iter().map(|p| p[0]).sum::<f64>() / n,
            f64::from(run.frames[0][1]),
            inward.iter().map(|p| p[1]).sum::<f64>() / n,
        ];
        let mid_cell = middle.map(|v| v.floor() as i32);
        let open = (-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                self.block_name([mid_cell[0] + dx, mid_cell[1], mid_cell[2] + dz])
                    .ends_with("end_portal")
            })
        });
        if open {
            if run.splits.iter().all(|(n, _)| n != "portal open") {
                self.end_say(b, "The portal's open. In we go.");
            }
            run.split("portal open", now);
            run.note = "Into the End portal".into();
            // Straight in, over the frame (it hops the step).
            b.yaw = yaw_to(pos, middle);
            b.pitch = 0.0;
            b.keys.forward = c::horizontal(pos, middle) > 0.2;
            if !self.watched(b) && now - run.since > 600 {
                self.end_fail(
                    b,
                    run,
                    "the End portal takes a watched companion (Watch mode)",
                );
            }
            return;
        }
        let missing: Vec<[i32; 3]> = run
            .frames
            .iter()
            .copied()
            .filter(|p| self.end_frame(*p).is_some_and(|(eye, _)| !eye))
            .collect();
        let filled = run.frames.len() - missing.len();
        run.note = format!("Filling the portal frames {filled}/12");
        let inv = self.end_inventory(host, b);
        let eyes = count_any(&inv, &["minecraft:ender_eye"]);
        if eyes < missing.len() as u32 {
            let why = format!("{} eyes of ender short", missing.len() as u32 - eyes);
            self.end_fail(b, run, &why);
            return;
        }
        let Some(&next) = missing
            .iter()
            .min_by(|a, z| c::distance(pos, centre(**a)).total_cmp(&c::distance(pos, centre(**z))))
        else {
            // Every frame it saw is full and no portal: one it hasn't seen.
            if run.looked < 160 {
                run.looked += 1;
                b.yaw += 6.0;
                b.pitch = 20.0;
                run.frames.clear();
            } else {
                self.end_fail(b, run, "the frames are full but no portal opened");
            }
            return;
        };
        run.hands = true;
        let top = [
            f64::from(next[0]) + 0.5,
            f64::from(next[1]) + 0.8125,
            f64::from(next[2]) + 0.5,
        ];
        if c::distance(eye(pos), top) <= 4.0 && self.end_sight(eye(pos), top, 5.0).0 {
            self.end_use_on(host, b, next, 1, &["minecraft:ender_eye"], None, pos);
            return;
        }
        // Walk behind it, outside the ring, the frame in reach.
        let d = self.end_frame(next).map_or([0, 0], |(_, d)| d);
        let side = [d[1], -d[0]];
        let spots = [
            [next[0] - d[0], next[2] - d[1]],
            [next[0] - 2 * d[0], next[2] - 2 * d[1]],
            [next[0] - d[0] + side[0], next[2] - d[1] + side[1]],
            [next[0] - d[0] - side[0], next[2] - d[1] - side[1]],
        ];
        let spot = spots
            .iter()
            .filter_map(|c| {
                (-2..=1).rev().find_map(|dy| {
                    let f = [c[0], next[1] + dy, c[1]];
                    (self.floor([f[0], f[1] - 1, f[2]])
                        && self.passable(f)
                        && self.passable([f[0], f[1] + 1, f[2]]))
                    .then(|| {
                        [
                            f64::from(f[0]) + 0.5,
                            f64::from(f[1]),
                            f64::from(f[2]) + 0.5,
                        ]
                    })
                })
            })
            .filter(|s| !run.tried.iter().any(|t| c::horizontal(*t, *s) < 0.5))
            .min_by(|a, z| c::distance(pos, *a).total_cmp(&c::distance(pos, *z)))
            .or_else(|| self.companion_spot_for(next, pos, &run.tried));
        let Some(spot) = spot else {
            self.end_fail(b, run, "no place to stand by a portal frame");
            return;
        };
        if self.end_walk(host, b, spot, 0.3, pos) == Steer::Stuck || self.end_stalled(run, pos, now)
        {
            run.tried.push(spot);
            b.nav.reset();
        }
    }

    /// Whether it has stood still for 100 ticks while it should be walking.
    fn end_stalled(&self, run: &mut EndRun, pos: [f64; 3], now: u64) -> bool {
        match run.stall {
            Some((at, since)) if c::distance(at, pos) < 0.5 => {
                if now - since > 100 {
                    run.stall = None;
                    return true;
                }
                false
            }
            _ => {
                run.stall = Some((pos, now));
                false
            }
        }
    }

    // ---- arriving -----------------------------------------------------------------------

    fn end_arrive(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        let f = feet(pos);
        // The ground comes in first.
        if !self.world.loaded(f[0], f[2]) {
            run.note = "Arriving in the End".into();
            return;
        }
        let arrived = *run.arrived.get_or_insert(now);
        if now - arrived < 20 {
            return;
        }
        run.note = "Over to the island".into();
        if b.ticks.is_multiple_of(100) {
            log::info!(
                "[end] {} arriving at {:?}, surface {}",
                b.file.name,
                f,
                self.end_on_surface(pos)
            );
        }
        let fountain = [e::FOUNTAIN[0], pos[1], e::FOUNTAIN[1]];
        let d = c::horizontal(pos, fountain).max(0.1);
        let surface = self.end_on_surface(pos);
        if surface && d < 70.0 {
            run.split("on the island", now);
            let next = if run.level == PerceptionLevel::AllKnowing {
                EndPhase::Crystals
            } else {
                EndPhase::Scout
            };
            run.set_phase(next, now);
            return;
        }
        // On the surface already: walk in; else (or when walking fails)
        // dig up and bridge.
        if surface && run.walk_fails < 3 {
            let x = (pos[0] + (fountain[0] - pos[0]) / d * 30.0).floor() as i32;
            let z = (pos[2] + (fountain[2] - pos[2]) / d * 30.0).floor() as i32;
            if let Some(y) = self.end_ground(x, z, f[1] + 20) {
                let goal = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                if self.end_walk(host, b, goal, 2.0, pos) == Steer::Stuck {
                    run.walk_fails += 1;
                }
                return;
            }
        }
        if self.end_traverse(host, b, run, fountain, pos) == Prog::Failed {
            self.end_fail(b, run, "couldn't get off the arrival platform");
        }
    }

    // ---- scouting and the crystals ----------------------------------------------------

    /// Where it looks round from: on the island, 10 to 22 blocks from the
    /// fountain on its side, turned a little each scout.
    fn end_lookout(&self, pos: [f64; 3], turn: u32) -> Option<[f64; 3]> {
        let a = (pos[2] - e::FOUNTAIN[1]).atan2(pos[0] - e::FOUNTAIN[0]) + f64::from(turn) * 1.9;
        for r in [18.0, 14.0, 22.0, 10.0] {
            let x = (e::FOUNTAIN[0] + a.cos() * r).floor() as i32;
            let z = (e::FOUNTAIN[1] + a.sin() * r).floor() as i32;
            if let Some(y) = self.end_ground(x, z, 110) {
                return Some([f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5]);
            }
        }
        None
    }

    fn end_scout(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        run.note = format!("Looking for the crystals ({} seen)", run.known.len());
        self.end_see_crystals(run, pos, b.yaw);
        // (A look round takes 72 ticks; driven off its lookout again and
        // again, it gives up on the rest after a few minutes.)
        if run.known.len() >= 10 || run.looked >= 72 || now - run.since > 6000 {
            run.looked = 0;
            run.lookout = None;
            run.scouts += 1;
            run.split("crystals scouted", now);
            run.set_phase(EndPhase::Crystals, now);
            return;
        }
        if run.looked == 0 && run.lookout.is_none() {
            run.lookout = self.end_lookout(pos, run.scouts);
        }
        if run.looked == 0
            && let Some(spot) = run.lookout
        {
            // The walker; where it can't get there, straight on, digging
            // steps and bridging; where that fails too, it looks from here.
            if run.walk_fails >= 2 {
                match self.end_traverse(host, b, run, spot, pos) {
                    Prog::Busy if c::horizontal(pos, spot) > 2.5 => return,
                    _ => {}
                }
            } else {
                match self.end_walk(host, b, spot, 2.5, pos) {
                    Steer::Arrived => {}
                    Steer::Moving if !self.end_stalled(run, pos, now) => return,
                    _ => {
                        run.walk_fails += 1;
                        b.nav.reset();
                        return;
                    }
                }
            }
        }
        // A slow turn, looking up at the spike tops.
        run.looked += 1;
        b.yaw += 5.0;
        b.pitch = -25.0;
    }

    fn end_crystals(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        self.end_see_crystals(run, pos, b.yaw);
        run.note = format!("Destroying crystals {}/10", run.destroyed());
        // Done with the one in hand?
        if let Some(i) = run.shot.as_ref().map(|s| s.crystal)
            && run.known[i].gone
        {
            run.shot = None;
            self.end_put_bow_away(host, b);
            self.end_report_crystal(b, run);
        }
        if let Some(i) = run.cage.as_ref().map(|k| k.crystal)
            && run.known[i].gone
            && run
                .cage
                .as_ref()
                .is_some_and(|k| k.step < CageStep::DownFar)
        {
            let up = run
                .cage
                .as_ref()
                .is_some_and(|k| k.step >= CageStep::PillarFar);
            if let Some(k) = run.cage.as_mut() {
                k.step = if up {
                    CageStep::DownFar
                } else {
                    CageStep::DownNear
                };
            }
            self.end_put_bow_away(host, b);
            self.end_report_crystal(b, run);
        }
        if run.cage.is_some() {
            self.end_cage(host, b, run, pos);
            return;
        }
        if let Some(i) = run.shot.as_ref().map(|s| s.crystal) {
            let was = run.known[i].skipped;
            self.end_shoot_open(host, b, run, pos);
            if !was && run.known[i].skipped {
                log::info!(
                    "[end] {} gives up on crystal {i} from {:?} ({} spots tried)",
                    b.file.name,
                    feet(pos),
                    run.shot.as_ref().map_or(0, |s| s.tried.len())
                );
            }
            return;
        }
        // The next: open ones first, the nearest; then the caged ones.
        // (Caged first, while the dragon rarely perches and so rarely
        // charges at a pillar, did worse: 2/10 against 3/10, the first
        // pillar going up under the strafes that follow the first crystal.)
        let inv = self.end_inventory(host, b);
        let arrows = count_any(&inv, &ARROWS) > 0 && slot_of(&inv, &["minecraft:bow"]).is_some();
        let pickaxe = inv.iter().flatten().any(|s| s.item.ends_with("_pickaxe"));
        let blocks = count_any(&inv, &BLOCKS);
        let next = run
            .known
            .iter()
            .enumerate()
            .filter(|(_, k)| !k.gone && !k.skipped)
            .min_by(|(_, a), (_, z)| {
                (a.spike.caged, c::horizontal(pos, a.at))
                    .partial_cmp(&(z.spike.caged, c::horizontal(pos, z.at)))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, k)| (i, k.spike));
        let Some((i, spike)) = next else {
            // Fair: maybe some it hasn't seen yet.
            if run.known.len() < 10 && run.scouts < 4 {
                run.set_phase(EndPhase::Scout, now);
                return;
            }
            // One left heals the dragon for good: back over the ones it gave
            // up on (the dragon has moved, the clouds have gone).
            if run.passes < 2 && run.known.iter().any(|k| k.skipped && !k.gone) {
                run.passes += 1;
                log::info!(
                    "[end] {} goes back over the crystals it gave up on",
                    b.file.name
                );
                for k in &mut run.known {
                    k.skipped = false;
                }
                run.shot = None;
                run.cage = None;
                return;
            }
            run.split("crystals done", now);
            let left = run.crystals_left();
            if left > 0 {
                self.end_say(b, &format!("{left} crystals left; going for the dragon."));
            } else {
                self.end_say(b, "All the crystals are down. Now the dragon.");
            }
            run.set_phase(EndPhase::Dragon, now);
            return;
        };
        if !arrows {
            run.known[i].skipped = true;
            return;
        }
        if spike.caged {
            // Just off the obsidian (its radius seen along the way in).
            let [dx, dz] = spike.inward();
            let near = (1..8)
                .find(|n| {
                    !self
                        .block_name([spike.x + dx * n, spike.height - 1, spike.z + dz * n])
                        .ends_with("obsidian")
                })
                .unwrap_or(3);
            let plan = e::cage_plan(&spike, near, CAGE_FAR);
            let ground = self
                .end_ground(plan.near[0], plan.near[1], plan.stand_y)
                .unwrap_or(plan.stand_y - 30);
            let need = (plan.stand_y - ground + 2).max(0) as u32;
            if !pickaxe || blocks < need {
                // Plan B: the dragon heals from it, the fight is longer.
                run.known[i].skipped = true;
                return;
            }
            log::info!(
                "[end] {} cage plan for crystal {i}: spike ({}, {}) height {} near {:?} far {:?} ground {ground} (near from {near})",
                b.file.name,
                spike.x,
                spike.z,
                spike.height,
                plan.near,
                plan.far,
            );
            run.walk_fails = 0;
            run.cage = Some(Cage {
                crystal: i,
                plan,
                step: CageStep::GoNear,
                ground,
                since: now,
                fired: None,
                misses: 0,
                retreats: 0,
            });
        } else {
            run.shot = Some(Shot {
                crystal: i,
                since: now,
                ..Default::default()
            });
        }
    }

    fn end_report_crystal(&mut self, b: &mut Body, run: &mut EndRun) {
        let line = match run.destroyed() {
            1 => "First crystal down.".to_owned(),
            10 => "That's the last crystal!".to_owned(),
            n => format!("Crystal down, {n} of 10."),
        };
        self.end_say(b, &line);
    }

    /// Where to shoot `spike`'s crystal from: on the island 16 to 32 blocks
    /// off, on its inward side, the whole arc clear; nearest `pos` first.
    fn end_shooting_spots(
        &self,
        spike: &Spike,
        pos: [f64; 3],
        tried: &[[f64; 3]],
    ) -> Vec<[f64; 3]> {
        let target = spike.crystal_centre();
        let inward = (e::FOUNTAIN[1] - target[2]).atan2(e::FOUNTAIN[0] - target[0]);
        let clouds = self.breath_view();
        let mut out = Vec::new();
        let mut lobs = Vec::new();
        for r in [16.0, 22.0, 28.0, 34.0, 40.0, 46.0, 52.0, 58.0] {
            for k in -4i32..=4 {
                let a = inward + f64::from(k) * 0.3;
                let x = (target[0] + a.cos() * r).floor() as i32;
                let z = (target[2] + a.sin() * r).floor() as i32;
                let Some(y) = self.end_ground(x, z, 100) else {
                    continue;
                };
                let spot = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                if tried.iter().any(|t| c::horizontal(*t, spot) < 2.0)
                    || clouds
                        .iter()
                        .any(|(at, r, _)| e::in_breath(spot, *at, *r, 2.5))
                    || !self.end_safe_ground(spot)
                {
                    continue;
                }
                // A flat arc first: a lob flies long and lands wide.
                match self.end_arc(eye(spot), spike) {
                    Some(a) if !a.lob => out.push(spot),
                    Some(_) => lobs.push(spot),
                    None => {}
                }
            }
        }
        let near =
            |a: &[f64; 3], z: &[f64; 3]| c::horizontal(pos, *a).total_cmp(&c::horizontal(pos, *z));
        out.sort_by(near);
        lobs.sort_by(near);
        out.extend(lobs);
        out
    }

    /// Ground with ground round it (not a spike's foot, not an edge).
    fn end_safe_ground(&self, spot: [f64; 3]) -> bool {
        let f = feet(spot);
        [[2, 0], [-2, 0], [0, 2], [0, -2]]
            .iter()
            .all(|[dx, dz]| (1..=4).any(|dy| self.floor([f[0] + dx, f[1] - dy, f[2] + dz])))
            && !self
                .block_name([f[0], f[1] - 1, f[2]])
                .ends_with("obsidian")
    }

    /// The bow's aim from `from` (the eye) onto `spike`'s crystal: yaw and
    /// pitch, when an arc gets there clear of blocks.
    fn end_arc(&self, from: [f64; 3], spike: &Spike) -> Option<Aim> {
        let target = spike.crystal_centre();
        let d = c::horizontal(from, target);
        let start = [from[0], from[1] - e::ARROW_DROP, from[2]];
        let yaw = yaw_to(from, target);
        let (lo, hi) = spike.crystal_box();
        let (lo, hi) = (
            [lo[0] - 0.3, lo[1] - 0.3, lo[2] - 0.3],
            [hi[0] + 0.3, hi[1] + 0.3, hi[2] + 0.3],
        );
        for (i, pitch) in e::aim_pitches(d, target[1] - start[1], e::ARROW_SPEED)
            .into_iter()
            .enumerate()
        {
            let flight = e::arrow_flight(start, yaw, pitch, e::ARROW_SPEED, 120, d + 4.0);
            for w in flight.windows(2) {
                if e::segment_box(w[0].at, w[1].at, lo, hi).is_some() {
                    return Some(Aim {
                        yaw,
                        pitch,
                        ticks: w[1].tick,
                        lob: i > 0 || pitch < -60.0,
                    });
                }
                if self.end_segment_blocked(w[0].at, w[1].at) {
                    break;
                }
            }
        }
        None
    }

    /// Whether a block an arrow would hit lies between `a` and `b`.
    fn end_segment_blocked(&self, a: [f64; 3], b: [f64; 3]) -> bool {
        let d = c::distance(a, b);
        if d < 1e-6 {
            return false;
        }
        let dir = [(b[0] - a[0]) / d, (b[1] - a[1]) / d, (b[2] - a[2]) / d];
        let mut blocked = !self.end_passes(a.map(|v| v.floor() as i32));
        walk_ray(a, dir, d, |p| {
            if !self.end_passes(p) {
                blocked = true;
                return false;
            }
            true
        });
        blocked
    }

    /// Whether an arrow flies through block `p` (no collision shape).
    fn end_passes(&self, p: [i32; 3]) -> bool {
        if !self.world.loaded(p[0], p[2]) {
            return false;
        }
        let state = self.world.block(p[0], p[1], p[2]);
        let f = self.res.facts().get(state);
        if self.res.facts().is_air(state) || f.fluid != Fluid::None {
            return true;
        }
        let n = &*f.name;
        n.ends_with(":fire")
            || n.ends_with("soul_fire")
            || n.ends_with("end_portal")
            || n.ends_with("torch")
            || !f.full_collision
                && !f.legacy_solid
                && !n.contains("bars")
                && !n.contains("pane")
                && !n.contains("fence")
                && !n.ends_with("_wall")
    }

    /// The bow: drawn at the aim, released after a full draw. Some(true)
    /// the tick the arrow flies; None without a bow and arrows.
    fn end_bow(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        aim: (f32, f32),
    ) -> Option<bool> {
        let inv = self.end_inventory(host, b);
        let bow = slot_of(&inv, &["minecraft:bow"])?;
        if count_any(&inv, &ARROWS) == 0 {
            return None;
        }
        run.hands = true;
        (b.yaw, b.pitch) = aim;
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = aim;
        }
        let drawing = host
            .play
            .get(&b.conn)
            .and_then(super::lan_actions::GuestPlay::using)
            .is_some();
        match b.work.draw {
            Some(since) if drawing && b.ticks - since > u64::from(FULL_DRAW) => {
                // BowItem.releaseUsing: the arrow flies.
                self.lan_guest_packet(
                    host,
                    b.conn,
                    S::PlayerAction(play::PlayerAction {
                        action: 6,
                        pos: [0; 3],
                        direction: 0,
                        sequence: 0,
                    }),
                );
                b.work.draw = None;
                run.arrows += 1;
                Some(true)
            }
            Some(_) if drawing => Some(false),
            _ => {
                b.work.draw = None;
                let held_bow = host.hub.guest(b.conn).is_some_and(|g| {
                    g.inventory
                        .get(g.selected)
                        .and_then(|s| s.as_ref())
                        .is_some_and(|s| &*s.item == "minecraft:bow")
                });
                if !held_bow {
                    self.companion_hold_slot(host, b, bow);
                }
                self.lan_guest_packet(
                    host,
                    b.conn,
                    S::UseItem(play::UseItem {
                        hand: 0,
                        sequence: 0,
                        yaw: aim.0,
                        pitch: aim.1,
                    }),
                );
                if host
                    .play
                    .get(&b.conn)
                    .and_then(super::lan_actions::GuestPlay::using)
                    .is_some()
                {
                    b.work.draw = Some(b.ticks);
                }
                Some(false)
            }
        }
    }

    /// Eating stops (a player lets go of the item to run).
    fn end_stop_eating(&mut self, host: &mut LanHost, b: &mut Body) {
        if b.eating.take().is_some() {
            self.lan_guest_packet(
                host,
                b.conn,
                S::PlayerAction(play::PlayerAction {
                    action: 6,
                    pos: [0; 3],
                    direction: 0,
                    sequence: 0,
                }),
            );
        }
    }

    fn end_put_bow_away(&mut self, host: &mut LanHost, b: &mut Body) {
        if b.work.draw.take().is_some() {
            let slot = host.hub.guest(b.conn).map_or(0, |g| g.selected as i16);
            self.lan_guest_packet(
                host,
                b.conn,
                S::SetCarriedItem(play::SetCarriedItem { slot }),
            );
            // Player.stopUsingItem: the drawn bow no longer slows it.
            self.as_guest(host, b.conn, |g| g.using = None);
        }
    }

    fn end_shoot_open(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        pos: [f64; 3],
    ) {
        let now = now_of(self);
        let Some(mut s) = run.shot.take() else {
            return;
        };
        let spike = run.known[s.crystal].spike;
        if now - s.since > 2400 {
            run.known[s.crystal].skipped = true;
            self.end_put_bow_away(host, b);
            return;
        }
        if let Some(at) = s.fired {
            // Watching it fly (about 15 ticks at 40 blocks).
            if now - at < s.flight.max(30) {
                run.hands = true;
                run.shot = Some(s);
                return;
            }
            s.fired = None;
            s.misses += 1;
            log::info!("[end] {} missed crystal {}", b.file.name, s.crystal);
            if s.misses >= 3 {
                if let Some(spot) = s.spot.take() {
                    s.tried.push(spot);
                }
                s.misses = 0;
            }
        }
        if s.spot.is_none() {
            s.spot = self
                .end_shooting_spots(&spike, pos, &s.tried)
                .first()
                .copied();
            // Its ground not in yet (the game runs ahead of the chunks under
            // /tick sprint): toward the middle while it comes in.
            let around = (-3..=3)
                .all(|i| (-3..=3).all(|j| self.world.loaded(spike.x + i * 8, spike.z + j * 8)));
            if s.spot.is_none() && !around {
                run.note = format!(
                    "Destroying crystals {}/10: waiting for the island to load",
                    run.destroyed()
                );
                let mid = [e::FOUNTAIN[0] + 12.0, pos[1], e::FOUNTAIN[1]];
                let _ = self.end_walk(host, b, mid, 4.0, pos);
                s.since = now;
                run.shot = Some(s);
                return;
            }
            if s.spot.is_none() {
                // Nowhere to shoot it from.
                let target = spike.crystal_centre();
                let inward = (e::FOUNTAIN[1] - target[2]).atan2(e::FOUNTAIN[0] - target[0]);
                let (mut ground, mut safe, mut arcs) = (0, 0, 0);
                for r in [16.0, 28.0, 40.0] {
                    for k in -4i32..=4 {
                        let a = inward + f64::from(k) * 0.3;
                        let x = (target[0] + a.cos() * r).floor() as i32;
                        let z = (target[2] + a.sin() * r).floor() as i32;
                        let Some(y) = self.end_ground(x, z, 100) else {
                            continue;
                        };
                        ground += 1;
                        let spot = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                        if self.end_safe_ground(spot) {
                            safe += 1;
                            arcs += i32::from(self.end_arc(eye(spot), &spike).is_some());
                        }
                    }
                }
                log::info!(
                    "[end] no spot for crystal {} at {:?}: ground {ground} safe {safe} arcs {arcs}, tried {}, loaded {}",
                    s.crystal,
                    target,
                    s.tried.len(),
                    self.world.loaded(spike.x, spike.z)
                );
                run.known[s.crystal].skipped = true;
                return;
            }
        }
        let spot = s.spot.unwrap_or(pos);
        // A cloud where it would stand: somewhere else.
        if self
            .breath_view()
            .iter()
            .any(|(at, r, _)| e::in_breath(spot, *at, *r, 2.0))
        {
            s.tried.push(spot);
            s.spot = None;
            run.shot = Some(s);
            return;
        }
        if c::horizontal(pos, spot) > 1.2 {
            self.end_put_bow_away(host, b);
            // The walker; where it can't get there (a valley wall, a pit),
            // straight on, digging steps up and bridging.
            if s.stuck >= 2 {
                if self.end_traverse(host, b, run, spot, pos) == Prog::Failed {
                    s.tried.push(spot);
                    s.spot = None;
                    s.stuck = 0;
                }
            } else if self.end_walk(host, b, spot, 0.8, pos) == Steer::Stuck
                || self.end_stalled(run, pos, now)
            {
                s.stuck += 1;
                b.nav.reset();
            }
            run.shot = Some(s);
            return;
        }
        // There: aimed from where it really stands, on its feet.
        if !self.end_guest(host, b).is_some_and(|(g, _)| g) {
            run.shot = Some(s);
            return;
        }
        match self.end_arc(eye(pos), &spike) {
            Some(arc) => match self.end_bow(host, b, run, (arc.yaw, arc.pitch)) {
                Some(true) => {
                    s.fired = Some(now);
                    s.flight = u64::from(arc.ticks) + 10;
                    log::info!(
                        "[end] {} shoots crystal {} from {:?}: {:.1} blocks, yaw {:.1} pitch {:.1}",
                        b.file.name,
                        s.crystal,
                        pos.map(|v| (v * 10.0).round() / 10.0),
                        c::distance(eye(pos), spike.crystal_centre()),
                        arc.yaw,
                        arc.pitch
                    );
                }
                Some(false) => {}
                None => run.known[s.crystal].skipped = true,
            },
            None => {
                s.tried.push(spot);
                s.spot = None;
            }
        }
        run.shot = Some(s);
    }

    fn end_cage(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let Some(i) = run.cage.as_ref().map(|k| k.crystal) else {
            return;
        };
        let (was, step) = (run.known[i].skipped, run.cage.as_ref().map(|k| k.step));
        self.end_cage_step(host, b, run, pos);
        if !was && run.known[i].skipped {
            log::info!(
                "[end] {} gives up on caged crystal {i} at step {step:?}, feet {:?}",
                b.file.name,
                feet(pos)
            );
        }
    }

    fn end_cage_step(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        let Some(mut k) = run.cage.take() else {
            return;
        };
        let spike = run.known[k.crystal].spike;
        if now - k.since > 6000 && !matches!(k.step, CageStep::DownFar | CageStep::DownNear) {
            // Too long: down, and leave it.
            run.known[k.crystal].skipped = true;
            k.step = if k.step >= CageStep::GoFar {
                CageStep::DownFar
            } else {
                CageStep::DownNear
            };
        }
        let col =
            |p: [i32; 2], y: i32| [f64::from(p[0]) + 0.5, f64::from(y), f64::from(p[1]) + 0.5];
        let f = feet(pos);
        run.note = format!(
            "Destroying crystals {}/10: caged one, {}",
            run.destroyed(),
            match k.step {
                CageStep::GoNear | CageStep::PillarNear => "pillaring up beside the cage",
                CageStep::Window => "breaking the bars",
                CageStep::DownNear | CageStep::DownFar => "coming down",
                CageStep::GoFar | CageStep::PillarFar => "pillaring up out of its blast",
                CageStep::Shoot => "shooting through the bars",
            }
        );
        let next = match k.step {
            CageStep::GoNear | CageStep::GoFar => {
                let near = k.step == CageStep::GoNear;
                let at = if near { k.plan.near } else { k.plan.far };
                let Some(y) = self.end_ground(at[0], at[1], k.plan.stand_y) else {
                    log::info!("[end] {} no ground in column {at:?}", b.file.name);
                    run.known[k.crystal].skipped = true;
                    return;
                };
                let goal = col(at, y);
                if self
                    .breath_view()
                    .iter()
                    .any(|(c, r, _)| e::in_breath(goal, *c, *r, 3.0))
                {
                    run.note = format!(
                        "Destroying crystals {}/10: waiting for the breath to clear",
                        run.destroyed()
                    );
                    run.cage = Some(k);
                    return;
                }
                // In its column, on the ground or on its own bridge.
                let there = [f[0], f[2]] == at && f[1] >= y - 1;
                if there {
                    k.ground = f[1];
                }
                if there
                    && self.end_quarry(host, b, run, STURDY_WANT, pos) == Prog::Busy
                    && now - k.since < 2400
                {
                    None
                } else if there {
                    Some(if near {
                        CageStep::PillarNear
                    } else {
                        CageStep::PillarFar
                    })
                } else if c::horizontal(pos, goal) < 3.0 {
                    // Close: straight there, sneaking (no step off an edge).
                    b.yaw = yaw_to(pos, goal);
                    b.pitch = 0.0;
                    b.keys.forward = true;
                    b.keys.shift = c::horizontal(pos, goal) < 1.5;
                    b.keys.jump = !self.passable(add(feet(pos), {
                        let d = [goal[0] - pos[0], goal[2] - pos[2]];
                        if d[0].abs() >= d[1].abs() {
                            [d[0].signum() as i32, 0, 0]
                        } else {
                            [0, 0, d[1].signum() as i32]
                        }
                    }));
                    if self.end_stalled(run, pos, now) {
                        run.walk_fails += 1;
                    }
                    // Stuck a step from it (its own old walls, a hole):
                    // straight in, digging and bridging.
                    if run.walk_fails > 3 {
                        b.keys = Keys::default();
                        if self.end_traverse(host, b, run, goal, pos) == Prog::Failed {
                            run.known[k.crystal].skipped = true;
                            return;
                        }
                    }
                    None
                } else {
                    // The walker first; where it can't get there, straight
                    // on, digging up and bridging (no step down).
                    if run.walk_fails >= 2 {
                        if self.end_traverse(host, b, run, goal, pos) == Prog::Failed
                            || now - k.since > 3000
                        {
                            run.known[k.crystal].skipped = true;
                            return;
                        }
                    } else if self.end_walk(host, b, goal, 2.0, pos) == Steer::Stuck
                        || self.end_stalled(run, pos, now)
                    {
                        run.walk_fails += 1;
                        b.nav.reset();
                    }
                    None
                }
            }
            // Pushed off its column (a dodge, a blow): back to it, or
            // down and up again.
            CageStep::PillarNear | CageStep::PillarFar
                if [f[0], f[2]]
                    != if k.step == CageStep::PillarNear {
                        k.plan.near
                    } else {
                        k.plan.far
                    } =>
            {
                let near = k.step == CageStep::PillarNear;
                if f[1] < k.ground + 2 {
                    Some(if near {
                        CageStep::GoNear
                    } else {
                        CageStep::GoFar
                    })
                } else {
                    k.retreats += 1;
                    Some(if near {
                        CageStep::DownNear
                    } else {
                        CageStep::DownFar
                    })
                }
            }
            // Partway up when the dragon makes a pass at it (a strafe
            // aimed at it, a charge, a fireball coming): walled in and
            // roofed where it is, crouched, till the pass is over; the
            // climb goes on through the roof.
            CageStep::PillarNear | CageStep::PillarFar
                if f[1] >= k.ground + 3 && self.end_pass_at(b, pos) =>
            {
                run.note = format!(
                    "Destroying crystals {}/10: walled in up its pillar while the dragon passes",
                    run.destroyed()
                );
                let on_ground = host.hub.guest(b.conn).is_some_and(|g| g.on_ground);
                b.keys = Keys::default();
                if !on_ground || self.end_fence(host, b, run, [0; 3], pos) != Prog::Busy {
                    b.keys = Keys::default();
                    b.keys.shift = true;
                }
                None
            }
            CageStep::PillarNear | CageStep::PillarFar
                if f[1] < k.ground + 2 && !self.end_calm(host, b) =>
            {
                // Not up a pillar while the dragon makes passes at it.
                run.note = format!(
                    "Destroying crystals {}/10: waiting for the dragon to pass",
                    run.destroyed()
                );
                None
            }
            CageStep::PillarNear | CageStep::PillarFar => {
                let up = self.end_pillar(host, b, run, k.plan.stand_y, pos);
                // A fence round the top: the dragon's charge and wings
                // knock it against blocks, not off a 20-block pillar.
                let up = if up == Prog::Done {
                    // Open at head height toward the spike from the
                    // far pillar: it shoots that way.
                    let [ix, iz] = spike.inward();
                    let open = if k.step == CageStep::PillarFar {
                        [-ix, 0, -iz]
                    } else {
                        [0; 3]
                    };
                    self.end_fence(host, b, run, open, pos)
                } else {
                    up
                };
                match up {
                    Prog::Done => Some(if k.step == CageStep::PillarNear {
                        CageStep::Window
                    } else {
                        CageStep::Shoot
                    }),
                    Prog::Failed => {
                        run.known[k.crystal].skipped = true;
                        Some(if k.step == CageStep::PillarNear {
                            CageStep::DownNear
                        } else {
                            CageStep::DownFar
                        })
                    }
                    Prog::Busy => None,
                }
            }
            CageStep::Window => {
                run.hands = true;
                let bar = k
                    .plan
                    .window
                    .iter()
                    .copied()
                    .find(|p| !self.passable(*p) || self.block_name(*p).ends_with("iron_bars"));
                match bar {
                    None => Some(CageStep::DownNear),
                    Some(bar) => match self.end_dig(host, b, bar, pos) {
                        Prog::Failed => {
                            // Down, and up again where it can reach (up
                            // to three times).
                            k.retreats += 1;
                            Some(CageStep::DownNear)
                        }
                        _ => None,
                    },
                }
            }
            CageStep::DownNear | CageStep::DownFar => {
                match self.end_descend(host, b, run, k.ground, pos) {
                    Prog::Busy => None,
                    _ => {
                        let crystal = &run.known[k.crystal];
                        // Down out of a cloud: back up once it's clear.
                        let bars = k.plan.window.iter().any(|p| {
                            !self.passable(*p) || self.block_name(*p).ends_with("iron_bars")
                        });
                        if k.retreats > 0 && !crystal.skipped && !crystal.gone {
                            if k.retreats > 3 {
                                run.known[k.crystal].skipped = true;
                            } else if k.step == CageStep::DownNear && bars {
                                k.step = CageStep::PillarNear;
                                run.cage = Some(k);
                                return;
                            } else if k.step == CageStep::DownFar {
                                k.step = CageStep::PillarFar;
                                run.cage = Some(k);
                                return;
                            }
                        }
                        let crystal = &run.known[k.crystal];
                        if k.step == CageStep::DownFar || crystal.skipped || crystal.gone {
                            self.companion_stop_dig(host, b);
                            self.end_put_bow_away(host, b);
                            return;
                        }
                        // Through the window from the ground, when an arc
                        // gets there; else from the second pillar.
                        if !self.end_shooting_spots(&spike, pos, &[]).is_empty() {
                            self.companion_stop_dig(host, b);
                            run.shot = Some(Shot {
                                crystal: k.crystal,
                                since: now,
                                ..Default::default()
                            });
                            return;
                        }
                        Some(CageStep::GoFar)
                    }
                }
            }
            CageStep::Shoot => {
                if let Some(at) = k.fired {
                    run.hands = true;
                    if now - at < 25 {
                        None
                    } else {
                        k.fired = None;
                        k.misses += 1;
                        (k.misses >= 4).then(|| {
                            run.known[k.crystal].skipped = true;
                            CageStep::DownFar
                        })
                    }
                } else {
                    let from = eye(pos);
                    let target = spike.crystal_centre();
                    let d = c::horizontal(from, target);
                    let rise = target[1] - (from[1] - e::ARROW_DROP);
                    match e::aim_pitches(d, rise, e::ARROW_SPEED).first() {
                        Some(&pitch) => {
                            match self.end_bow(host, b, run, (yaw_to(from, target), pitch)) {
                                Some(true) => {
                                    k.fired = Some(now);
                                    None
                                }
                                Some(false) => None,
                                None => {
                                    run.known[k.crystal].skipped = true;
                                    Some(CageStep::DownFar)
                                }
                            }
                        }
                        None => Some(CageStep::DownFar),
                    }
                }
            }
        };
        if let Some(step) = next {
            k.step = step;
            self.companion_stop_dig(host, b);
        }
        run.cage = Some(k);
    }

    // ---- the dragon ---------------------------------------------------------------------

    /// Breath and fireballs, eating and backing off: before whatever else.
    /// True when it moved for its safety this tick.
    fn end_keep_safe(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        pos: [f64; 3],
        critical: bool,
    ) -> bool {
        let (health, food) = host
            .hub
            .guest(b.conn)
            .map_or((20.0, 20), |g| (g.health, g.food));
        // Up its pillar it can't step aside.
        let up_high = run.cage.as_ref().is_some_and(|k| {
            let f = feet(pos);
            [f[0], f[2]] == k.plan.near && f[1] >= k.ground + 2
                || [f[0], f[2]] == k.plan.far && f[1] >= k.ground + 2
        });
        // The dragon landing or taking off: its head and neck sweep where
        // it stood beside them (a bite through iron armour is 6); away from
        // them first, a raise or a step given up.
        if run.bed.is_none()
            && let Some(d) = self.dragon_view()
            && d.alive
            && matches!(d.phase, 3 | 4)
        {
            let parts = Self::end_parts(&d);
            if !parts.clear_of(pos, 2.0) {
                let hm = [
                    (parts.head.0[0] + parts.head.1[0]) / 2.0,
                    pos[1],
                    (parts.head.0[2] + parts.head.1[2]) / 2.0,
                ];
                let (dx, dz) = (pos[0] - hm[0], pos[2] - hm[2]);
                let len = dx.hypot(dz).max(0.1);
                let yaw = yaw_to(pos, [pos[0] + dx / len, pos[1], pos[2] + dz / len]);
                run.raised = None;
                run.perch = None;
                run.danger = "the dragon";
                run.note = "Out of the way of its head as it takes off".into();
                self.end_put_bow_away(host, b);
                self.companion_stop_dig(host, b);
                if self.companion_safe_at(pos, yaw, 1.5) {
                    b.yaw = yaw;
                    b.pitch = 0.0;
                    b.keys.forward = true;
                    b.keys.sprint = true;
                }
                return true;
            }
        }
        // In a cloud: out of it, away from its middle.
        let clouds = self.breath_view();
        // Up its pillar in one: up two, out of the cloud's reach (sooner
        // than twenty down); without the blocks, down.
        if up_high
            && run.raised.is_none()
            && clouds
                .iter()
                .any(|(at, r, _)| e::in_breath(pos, *at, *r, 0.5))
            && let Some(k) = run.cage.as_mut()
        {
            run.danger = "dragon breath";
            self.end_put_bow_away(host, b);
            if count_any(&self.end_inventory(host, b), &BLOCKS) > 1 {
                self.companion_stop_dig(host, b);
                run.raised = Some(feet(pos)[1] + 2);
            } else if !matches!(k.step, CageStep::DownNear | CageStep::DownFar) {
                k.retreats += 1;
                k.step = if k.step >= CageStep::GoFar {
                    CageStep::DownFar
                } else {
                    CageStep::DownNear
                };
            }
        }
        // A cloud come down on it, deep in: a block under its feet lifts it
        // out of the breath's reach (a block up) sooner than running would;
        // it waits there for the cloud to go (or the dragon to perch, from
        // its step).
        let on_ground = host.hub.guest(b.conn).is_some_and(|g| g.on_ground);
        if !up_high
            && run.raised.is_none()
            && run.bed.is_none()
            && now_of(self) >= run.no_raise_until
            && on_ground
            && count_any(&self.end_inventory(host, b), &BLOCKS) > 0
            && clouds.iter().any(|(at, r, _)| {
                e::in_breath(pos, *at, *r, 0.0) && f64::from(*r) - c::horizontal(pos, *at) > 2.0
            })
        {
            run.raised = Some(feet(pos)[1] + 1);
            self.end_stop_eating(host, b);
            self.end_put_bow_away(host, b);
            self.companion_stop_dig(host, b);
        }
        // Its feet a full block over every cloud round it (a fireball's
        // cloud hangs where it burst, not on the ground): a new one at its
        // height (a fireball at it, sitting still) sends it higher.
        if let Some(top) = run.raised.as_mut() {
            let need = clouds
                .iter()
                .filter(|(at, r, _)| {
                    c::horizontal(pos, *at) <= f64::from(*r) + 1.0 && at[1] < pos[1] + 1.0
                })
                .map(|(at, _, _)| at[1].floor() as i32 + 2)
                .max();
            if let Some(need) = need
                && need > *top
                && count_any(&self.end_inventory(host, b), &BLOCKS) > 0
            {
                *top = need;
            }
        }
        if let Some(top) = run.raised {
            let under = clouds.iter().any(|(at, r, _)| {
                c::horizontal(pos, *at) <= f64::from(*r) + 0.5 && (pos[1] - at[1]).abs() < 3.0
            });
            if std::env::var_os("HORIZON_END_TRACE").is_some() && b.ticks.is_multiple_of(4) {
                log::info!(
                    "[end] raised to {top} pos {:?} under {under} ground {on_ground} perch {:?} clouds {:?}",
                    pos.map(|v| (v * 10.0).round() / 10.0),
                    run.perch.map(|p| p.step),
                    clouds
                        .iter()
                        .map(|(a, r, _)| (a.map(|v| (v * 10.0).round() / 10.0), *r))
                        .collect::<Vec<_>>(),
                );
            }
            if !under {
                run.raised = None;
            } else if feet(pos)[1] < top || !on_ground {
                // (Up in its jump it is still building: the block goes in
                // under it on the way.) Stuck on the ground a second: it
                // runs instead.
                run.danger = "dragon breath";
                run.raise_stuck = if on_ground { run.raise_stuck + 1 } else { 0 };
                if run.raise_stuck > 20 {
                    log::info!("[end] {} can't raise at {:?}: runs", b.file.name, feet(pos));
                    run.raised = None;
                    run.raise_stuck = 0;
                    run.no_raise_until = now_of(self) + 200;
                } else {
                    match self.end_pillar(host, b, run, top, pos) {
                        Prog::Failed => run.raised = None,
                        _ => return true,
                    }
                }
            } else if run.perch.is_none_or(|p| {
                let f = feet(pos);
                [f[0], f[2]] != [p.step[0], p.step[2]]
            }) {
                run.danger = "dragon breath";
                run.note = "Out of the breath, a block up".into();
                b.keys = Keys::default();
                b.keys.shift = true;
                return true;
            }
        }
        let margin = if run.fleeing { 5.0 } else { 2.5 };
        run.fleeing = false;
        if !up_high
            && let Some((at, r, _)) = clouds
                .iter()
                .find(|(at, r, _)| e::in_breath(pos, *at, *r, margin))
        {
            run.fleeing = true;
            run.danger = "dragon breath";
            // The way out: of 16 headings, the one whose point 5 blocks off is
            // furthest out of every cloud, on ground it can walk (straight
            // away from this cloud's middle breaks ties).
            let (dx, dz) = (pos[0] - at[0], pos[2] - at[2]);
            let away = dz.atan2(dx);
            let best = (0..16)
                .map(|i| f64::from(i) / 16.0 * std::f64::consts::TAU)
                .filter_map(|a| {
                    let far = [pos[0] + a.cos() * 6.0, pos[1], pos[2] + a.sin() * 6.0];
                    let yaw = yaw_to(pos, far);
                    if !self.companion_safe_at(pos, yaw, 1.0) {
                        return None;
                    }
                    // As far as it can run that way (walls, steps of more
                    // than a block stop it).
                    let (to, open) = self.end_open_way(pos, a, 6.0);
                    let clear = clouds
                        .iter()
                        .map(|(c, rr, _)| c::horizontal(to, *c) - f64::from(*rr))
                        .fold(f64::INFINITY, f64::min);
                    let turn = c::wrap_degrees((a - away).to_degrees()).abs() / 180.0;
                    Some((clear - turn + open, far, open))
                })
                .max_by(|x, y| x.0.total_cmp(&y.0));
            // Walled in all round (its own tunnel, a pit): dig its way out.
            if best.is_none_or(|(_, _, open)| open < 1.0) {
                let a = dz.atan2(dx);
                let to = [pos[0] + a.cos() * 6.0, pos[1], pos[2] + a.sin() * 6.0];
                if self.end_traverse(host, b, run, to, pos) != Prog::Failed {
                    return true;
                }
            }
            let best = best.map(|(_, to, _)| to);
            let out = best.unwrap_or([pos[0] + dx, pos[1], pos[2] + dz]);
            if std::env::var_os("HORIZON_END_TRACE").is_some() && b.ticks.is_multiple_of(4) {
                let g = host.hub.guest(b.conn);
                log::info!(
                    "[end] cloud escape pos {:?} cloud {:?} r {r:.1} out {:?} best {} eating {} draw {} play-using {} v {:?} food {:?} ground {:?}",
                    pos.map(|v| (v * 10.0).round() / 10.0),
                    at.map(|v| (v * 10.0).round() / 10.0),
                    out.map(|v| (v * 10.0).round() / 10.0),
                    best.is_some(),
                    b.eating.is_some(),
                    b.work.draw.is_some(),
                    host.play
                        .get(&b.conn)
                        .and_then(super::lan_actions::GuestPlay::using)
                        .is_some(),
                    self.end_guest(host, b)
                        .map(|(_, v)| v.map(|x| (x * 100.0).round() / 100.0)),
                    g.map(|g| g.food),
                    g.map(|g| g.on_ground),
                );
            }
            self.end_put_bow_away(host, b);
            self.end_stop_eating(host, b);
            self.companion_stop_dig(host, b);
            run.perch = None;
            b.yaw = yaw_to(pos, out);
            b.pitch = 0.0;
            b.keys.forward = true;
            b.keys.sprint = true;
            b.keys.jump = true;
            return true;
        }
        // A strafe at it: the fireball goes where it stands when fired (the
        // dragon within 64 blocks and facing it), so it never stands still
        // under one: it runs across the dragon's line, turning back now and
        // then, on ground it can run on, out of the clouds.
        if !up_high
            && run.bed.is_none()
            && run.raised.is_none()
            && run.perch.is_none()
            && let Some(d) = self.dragon_view()
            && d.alive
            && d.phase == 1
            && d.strafing == Some(b.conn)
            && c::distance(pos, d.position) < 90.0
        {
            run.danger = "a strafe";
            self.end_put_bow_away(host, b);
            self.end_stop_eating(host, b);
            self.companion_stop_dig(host, b);
            let toward = (d.position[2] - pos[2]).atan2(d.position[0] - pos[0]);
            let side = if (b.ticks / 50).is_multiple_of(2) {
                1.0
            } else {
                -1.0
            };
            let best = [90.0f64, 60.0, 120.0, -90.0, -60.0, -120.0]
                .iter()
                .map(|deg| toward + (deg * side).to_radians())
                .filter(|&a| {
                    let far = [pos[0] + a.cos() * 4.0, pos[1], pos[2] + a.sin() * 4.0];
                    self.companion_safe_at(pos, yaw_to(pos, far), 1.0)
                })
                .map(|a| {
                    let (to, open) = self.end_open_way(pos, a, 5.0);
                    let clear = clouds
                        .iter()
                        .map(|(c, rr, _)| c::horizontal(to, *c) - f64::from(*rr))
                        .fold(f64::INFINITY, f64::min)
                        .min(6.0);
                    (open + clear * 0.5, a)
                })
                .max_by(|x, y| x.0.total_cmp(&y.0));
            if let Some((score, a)) = best
                && score > 1.0
            {
                run.note = "Running across the dragon's strafe".into();
                b.yaw = yaw_to(pos, [pos[0] + a.cos(), pos[1], pos[2] + a.sin()]);
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
                b.keys.jump = !self.passable(add(feet(pos), {
                    let (x, z) = (a.cos(), a.sin());
                    if x.abs() >= z.abs() {
                        [x.signum() as i32, 0, 0]
                    } else {
                        [0, 0, z.signum() as i32]
                    }
                }));
                return true;
            }
        }
        // A fireball on its way: a few steps aside.
        if !up_high && run.bed.is_none() {
            for (at, v) in self.dragon_fireball_view() {
                if let Some(side) = e::dodge(pos, at, v, 7.5) {
                    run.danger = "a dragon fireball";
                    let to = [pos[0] + side[0] * 4.0, pos[1], pos[2] + side[1] * 4.0];
                    let yaw = yaw_to(pos, to);
                    if self.companion_safe_at(pos, yaw, 1.0) {
                        self.end_put_bow_away(host, b);
                        self.end_stop_eating(host, b);
                        b.yaw = yaw;
                        b.pitch = 0.0;
                        b.keys.forward = true;
                        b.keys.sprint = true;
                        return true;
                    }
                }
            }
        }
        // The dragon charging at it: aside, out of its line.
        if !up_high
            && run.bed.is_none()
            && let Some(d) = self.dragon_view()
            && d.alive
            && d.phase == 8
            && c::distance(pos, d.position) < 40.0
            && let Some(side) = e::dodge(pos, d.position, d.velocity, 9.0)
        {
            run.danger = "the dragon";
            let to = [pos[0] + side[0] * 4.0, pos[1], pos[2] + side[1] * 4.0];
            let yaw = yaw_to(pos, to);
            if self.companion_safe_at(pos, yaw, 1.0) {
                self.end_put_bow_away(host, b);
                self.end_stop_eating(host, b);
                run.perch = None;
                b.yaw = yaw;
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
                return true;
            }
        }
        // Flying low past it, whatever it is doing: out of its wings' way
        // (their knockback throws a body tens of blocks, off the island).
        if !up_high
            && run.bed.is_none()
            && run.perch.is_none()
            && run.raised.is_none()
            && run.tower.is_none()
            && let Some(d) = self.dragon_view()
            && d.alive
            && !d.sitting
            && c::horizontal(pos, d.position) < 10.0
            && (-4.0..8.0).contains(&(d.position[1] - pos[1]))
        {
            let (dx, dz) = (pos[0] - d.position[0], pos[2] - d.position[2]);
            let len = dx.hypot(dz).max(0.1);
            let yaw = yaw_to(pos, [pos[0] + dx / len, pos[1], pos[2] + dz / len]);
            if self.companion_safe_at(pos, yaw, 1.5) {
                run.danger = "the dragon";
                self.end_put_bow_away(host, b);
                self.end_stop_eating(host, b);
                self.companion_stop_dig(host, b);
                b.yaw = yaw;
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
                return true;
            }
        }
        if critical || up_high {
            return false;
        }
        // Low (`run.retreating`): away from the dragon to eat, back once healed.
        let dragon_close = self.dragon_view().is_some_and(|d| {
            d.alive && c::horizontal(pos, d.position) < 16.0 && d.position[1] - pos[1] < 10.0
        });
        if run.retreating && dragon_close {
            self.end_stop_eating(host, b);
        } else if b.eating.is_some() {
            return true;
        }
        let perched = self
            .dragon_view()
            .is_some_and(|d| d.sitting || d.phase == 3);
        // (Below 7 food it can't sprint out of a cloud; below 18 no healing.)
        let hungry = food < 14 || (run.retreating || health < 18.0 && !perched) && food < 20;
        // (Not with a mob on it: up out of its reach first.)
        let hunted = run.tower.is_none() && self.end_hunter(host, b, pos, 8.0).is_some();
        if hungry && !dragon_close && !hunted {
            self.end_put_bow_away(host, b);
            if self.companion_start_eating(host, b) {
                return true;
            }
        }
        // The dragon down on its podium while it isn't fighting it (busy
        // with the crystals, or too hurt): well away from its head.
        let landed = self
            .dragon_view()
            .filter(|d| d.alive && matches!(d.phase, 2 | 3 | 5 | 6 | 7 | 10))
            .filter(|d| c::horizontal(d.position, [e::FOUNTAIN[0], 0.0, e::FOUNTAIN[1]]) < 12.0);
        let keep_off = landed.is_some() && (run.phase != EndPhase::Dragon || run.retreating);
        if run.retreating || keep_off {
            run.note = if run.retreating {
                "Backing off to heal".into()
            } else {
                format!(
                    "Destroying crystals {}/10: keeping away from the perched dragon",
                    run.destroyed()
                )
            };
            if let Some(d) = self.dragon_view()
                && c::horizontal(pos, d.position) < 24.0
            {
                let (dx, dz) = (pos[0] - d.position[0], pos[2] - d.position[2]);
                let len = dx.hypot(dz).max(0.1);
                let x = (d.position[0] + dx / len * 28.0).floor() as i32;
                let z = (d.position[2] + dz / len * 28.0).floor() as i32;
                if let Some(y) = self.end_ground(x, z, 100) {
                    let goal = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                    let _ = self.end_walk(host, b, goal, 2.0, pos);
                }
                return true;
            }
            return run.retreating;
        }
        false
    }

    /// An enderman (or anything else) on it, on the ground: the sword.
    fn end_fight(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        id: u32,
        pos: [f64; 3],
    ) -> bool {
        let Some((at, height, enderman)) =
            self.mobs.iter().find(|m| m.id == id && m.alive()).map(|m| {
                (
                    [m.position.x, m.position.y, m.position.z],
                    m.kind.height,
                    &*m.kind.id == "minecraft:enderman",
                )
            })
        else {
            return false;
        };
        if c::distance(pos, at) > 24.0 {
            return false;
        }
        run.danger = if enderman { "an enderman" } else { "a mob" };
        self.end_put_bow_away(host, b);
        self.companion_stop_dig(host, b);
        let middle = [at[0], at[1] + height * 0.5, at[2]];
        if c::horizontal(pos, at) > 2.2 {
            let _ = self.end_walk(host, b, at, 1.5, pos);
        }
        b.yaw = yaw_to(pos, at);
        b.pitch = look_pitch(eye(pos), middle);
        if c::distance(eye(pos), middle) < 3.2 {
            if let Some(sword) = slot_of(&self.end_inventory(host, b), &SWORDS) {
                self.companion_hold_slot(host, b, sword);
            }
            // Critical hits: jump, strike falling, once the sword has charged.
            let (on_ground, v) = self.end_guest(host, b).unwrap_or((true, [0.0; 3]));
            let since = b.ticks - run.last_strike;
            if on_ground && since >= 6 {
                b.keys.jump = true;
            } else if !on_ground && v[1] < -0.05 && since >= 13 {
                self.companion_strike(host, b, id);
                run.last_strike = b.ticks;
            }
        }
        true
    }

    /// Hurt with an enderman (or anything) after it: three blocks up, out of
    /// its reach and with no room for it beside, hitting back at whatever
    /// comes close; down again once nothing is after it (or after 45 s).
    fn end_tower(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        health: f32,
        pos: [f64; 3],
    ) -> bool {
        let now = now_of(self);
        let f = feet(pos);
        let on_ground = host.hub.guest(b.conn).is_some_and(|g| g.on_ground);
        if run.tower.is_none()
            && health < 14.0
            && on_ground
            && count_any(&self.end_inventory(host, b), &BLOCKS) >= 4
            && self.end_hunter(host, b, pos, 12.0).is_some()
            && self.end_hunter(host, b, pos, 4.0).is_none()
        {
            log::info!("[end] {} towers up away from a mob at {f:?}", b.file.name);
            run.tower = Some((f[1] + 3, f[1], now));
            b.target = None;
        }
        let Some((top, ground, since)) = run.tower else {
            return false;
        };
        run.danger = "an enderman";
        let hunter = self.end_hunter(host, b, pos, 20.0);
        if hunter.is_none() || now - since > 900 {
            // Down again.
            run.note = "Climbing down".into();
            return match self.end_descend(host, b, run, ground, pos) {
                Prog::Busy => true,
                _ => {
                    run.tower = None;
                    false
                }
            };
        }
        if f[1] < top || !on_ground {
            run.note = "Up out of an enderman's reach".into();
            if self.end_pillar(host, b, run, top, pos) == Prog::Failed {
                run.tower = None;
                return false;
            }
            return true;
        }
        run.note = "Up out of an enderman's reach, hitting back".into();
        b.keys = Keys::default();
        b.keys.shift = true;
        if let Some(id) = hunter
            && let Some((at, height)) = self
                .mobs
                .iter()
                .find(|m| m.id == id)
                .map(|m| ([m.position.x, m.position.y, m.position.z], m.kind.height))
        {
            let top = [at[0], at[1] + height - 0.3, at[2]];
            b.yaw = yaw_to(pos, at);
            b.pitch = look_pitch(eye(pos), top);
            if c::distance(eye(pos), top) < 3.0 {
                if let Some(sword) = slot_of(&self.end_inventory(host, b), &SWORDS) {
                    self.companion_hold_slot(host, b, sword);
                }
                if b.ticks - run.last_strike >= 13 {
                    self.companion_strike(host, b, id);
                    run.last_strike = b.ticks;
                }
            }
        }
        true
    }

    /// A sword hit on `id` without a step (up a pillar).
    fn end_strike_in_place(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        id: u32,
        pos: [f64; 3],
    ) -> bool {
        let Some((at, height)) = self
            .mobs
            .iter()
            .find(|m| m.id == id && m.alive())
            .map(|m| ([m.position.x, m.position.y, m.position.z], m.kind.height))
        else {
            return false;
        };
        let middle = [at[0], at[1] + height * 0.5, at[2]];
        if c::distance(eye(pos), middle) > 3.2 {
            return false;
        }
        run.danger = "an enderman";
        self.end_put_bow_away(host, b);
        self.companion_stop_dig(host, b);
        b.keys = Keys::default();
        b.keys.shift = true;
        b.yaw = yaw_to(pos, at);
        b.pitch = look_pitch(eye(pos), middle);
        if let Some(sword) = slot_of(&self.end_inventory(host, b), &SWORDS) {
            self.companion_hold_slot(host, b, sword);
        }
        self.companion_strike(host, b, id);
        true
    }

    /// Where it waits for the dragon: `WAIT_RING` from the fountain, on
    /// firm ground, out of the clouds, on its own side.
    fn end_wait_spot(&self, pos: [f64; 3], ring: f64) -> Option<[f64; 3]> {
        let base = (pos[2] - e::FOUNTAIN[1]).atan2(pos[0] - e::FOUNTAIN[0]);
        let clouds = self.breath_view();
        for k in 0..16 {
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            let a = base + f64::from(k / 2) * 0.4 * sign;
            let x = (e::FOUNTAIN[0] + a.cos() * ring).floor() as i32;
            let z = (e::FOUNTAIN[1] + a.sin() * ring).floor() as i32;
            let Some(y) = self.end_ground(x, z, 80) else {
                continue;
            };
            let spot = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
            if self.end_safe_ground(spot)
                && !clouds
                    .iter()
                    .any(|(at, r, _)| e::in_breath(spot, *at, *r, 2.0))
            {
                return Some(spot);
            }
        }
        None
    }

    /// The dragon on a pass that may reach it: a strafe aimed at it, a
    /// charge, a fireball in the air near, the dragon flying close.
    fn end_pass_at(&self, b: &Body, pos: [f64; 3]) -> bool {
        let pass = self.dragon_view().is_some_and(|d| {
            d.alive
                && (d.phase == 1
                    && d.strafing == Some(b.conn)
                    && c::distance(pos, d.position) < 80.0
                    || d.phase == 8 && c::distance(pos, d.position) < 48.0
                    // (Flying round near the spikes: a wing's knockback
                    // throws it off its pillar.)
                    || !d.sitting && c::distance(pos, d.position) < 24.0)
        });
        pass || self
            .dragon_fireball_view()
            .iter()
            .any(|(at, _)| c::distance(pos, *at) < 32.0)
    }

    /// Whether it may be stuck up a pillar for a while: the dragon not on a
    /// pass (strafing, charging), no fireball in the air, its health up.
    fn end_calm(&self, host: &LanHost, b: &Body) -> bool {
        let health = host.hub.guest(b.conn).map_or(20.0, |g| g.health);
        let pos = host.hub.guest(b.conn).map_or([0.0; 3], |g| g.position);
        // (Not while it strafes or charges, nor while it is down on the
        // fountain: from there it charges at a player far off, up a pillar.)
        let pass = self.dragon_view().is_some_and(|d| {
            d.alive
                && (matches!(d.phase, 1 | 2 | 3 | 5 | 6 | 7 | 8)
                    || c::distance(pos, d.position) < 40.0)
        });
        health >= HEALED
            && !pass
            && self.dragon_fireball_view().is_empty()
            && self.end_hunter(host, b, pos, 32.0).is_none()
    }

    /// Where it gets running from `pos` along heading `a` (radians) for up
    /// to `max` blocks: a block up at a time, down at most three, stopped by
    /// a wall. The point, and how far that is.
    fn end_open_way(&self, pos: [f64; 3], a: f64, max: f64) -> ([f64; 3], f64) {
        let mut y = feet(pos)[1];
        let mut reached = 0.0;
        let mut d = 0.5;
        while d <= max {
            let x = (pos[0] + a.cos() * d).floor() as i32;
            let z = (pos[2] + a.sin() * d).floor() as i32;
            let room = |y: i32| self.passable([x, y, z]) && self.passable([x, y + 1, z]);
            if room(y) {
                // Level, or down a few.
                match (0..=3).find(|k| self.floor([x, y - 1 - k, z]) && room(y - k)) {
                    Some(k) => y -= k,
                    None => break,
                }
            } else if self.floor([x, y, z]) && room(y + 1) && self.passable([x, y + 2, z]) {
                y += 1;
            } else {
                break;
            }
            reached = d;
            d += 0.5;
        }
        (
            [
                pos[0] + a.cos() * reached,
                f64::from(y),
                pos[2] + a.sin() * reached,
            ],
            reached,
        )
    }

    /// A mob newly after it: logged with how it looked at it (an enderman
    /// is set off by a gaze).
    fn end_note_new_hunters(&self, host: &LanHost, b: &Body, run: &mut EndRun, pos: [f64; 3]) {
        let now: Vec<u32> = self
            .mobs
            .iter()
            .filter(|m| m.alive() && m.ai.target == Some(Who::Player))
            .filter(|m| host.mob_players.get(&m.id) == Some(&b.conn))
            .map(|m| m.id)
            .collect();
        for m in self
            .mobs
            .iter()
            .filter(|m| now.contains(&m.id) && !run.hunted_by.contains(&m.id))
        {
            let from = eye(pos);
            let to = [m.position.x, m.position.y + 2.55, m.position.z];
            let d = c::distance(from, to).max(0.01);
            let v = view(b.yaw, b.pitch);
            let dot =
                (v[0] * (to[0] - from[0]) + v[1] * (to[1] - from[1]) + v[2] * (to[2] - from[2]))
                    / d;
            let g = host.hub.guest(b.conn);
            log::info!(
                "[end] {} is hunted by {} {} at {d:.1}: gaze dot {dot:.4} (body yaw {:.0} pitch {:.0}, guest {:?}) doing '{}' last hurt by {:?} ({:?} at age {} now {})",
                b.file.name,
                m.kind.id,
                m.id,
                b.yaw,
                b.pitch,
                g.map(|g| (g.yaw.round(), g.pitch.round())),
                run.note,
                m.ai.last_hurt_by,
                m.last_damage,
                m.ai.last_hurt_by_time,
                m.age,
            );
        }
        run.hunted_by = now;
    }

    /// The nearest mob after it within `r` (an angry enderman, mostly).
    fn end_hunter(&self, host: &LanHost, b: &Body, pos: [f64; 3], r: f64) -> Option<u32> {
        self.mobs
            .iter()
            .filter(|m| m.alive() && m.ai.target == Some(Who::Player))
            .filter(|m| host.mob_players.get(&m.id) == Some(&b.conn))
            .map(|m| {
                (
                    c::distance(pos, [m.position.x, m.position.y, m.position.z]),
                    m.id,
                )
            })
            .filter(|(d, _)| *d < r)
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .map(|(_, id)| id)
    }

    fn end_parts(d: &DragonView) -> DragonParts {
        DragonParts {
            head: d.parts[0],
            neck: d.parts[1],
            wings: [d.parts[6], d.parts[7]],
        }
    }

    fn end_dragon(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        self.end_see_crystals(run, pos, b.yaw);
        let dead =
            self.fight.killed || self.dragon_view().is_some_and(|d| !d.alive || d.phase == 9);
        if dead {
            run.perch = None;
            run.bed = None;
            self.companion_stop_dig(host, b);
            run.split("dragon killed", now);
            self.end_say(b, "The dragon is down!");
            run.set_phase(EndPhase::Exit, now);
            return;
        }
        let Some(d) = self.dragon_view() else {
            run.note = "Waiting for the dragon".into();
            return;
        };
        if d.health < run.dragon_health - 0.01 {
            run.hits += 1;
        }
        run.dragon_health = d.health;
        let left = run.crystals_left();
        let holding = matches!(d.phase, 5 | 7);
        if run.bed.is_some() || run.beds && d.sitting && holding && self.end_has_beds(host, b) {
            run.note = format!("Dragon perched: beds ({:.0} health left)", d.health);
            self.end_beds(host, b, run, &d, pos);
            return;
        }
        if d.sitting && d.phase == 6 {
            // Scanning, its head swinging round to it: close, but out of reach.
            run.note = format!(
                "Dragon perched: getting ready ({:.0} health left)",
                d.health
            );
            let head = d.parts[0];
            let hm = [
                (head.0[0] + head.1[0]) / 2.0,
                pos[1],
                (head.0[2] + head.1[2]) / 2.0,
            ];
            let dist = c::horizontal(pos, hm);
            if dist < 9.0 {
                let (dx, dz) = (pos[0] - hm[0], pos[2] - hm[2]);
                let len = dx.hypot(dz).max(0.1);
                let yaw = yaw_to(pos, [pos[0] + dx / len, pos[1], pos[2] + dz / len]);
                if self.companion_safe_at(pos, yaw, 1.5) {
                    b.yaw = yaw;
                    b.keys.forward = true;
                    b.keys.sprint = true;
                }
            } else if dist > 12.0 {
                let to = [
                    hm[0] + (pos[0] - hm[0]) / dist * 10.0,
                    pos[1],
                    hm[2] + (pos[2] - hm[2]) / dist * 10.0,
                ];
                let _ = self.end_walk(host, b, to, 1.0, pos);
            } else {
                b.yaw = yaw_to(pos, hm);
            }
            return;
        }
        let health = host.hub.guest(b.conn).map_or(20.0, |g| g.health);
        let on_step = run
            .perch
            .is_some_and(|p| feet(pos)[1] > p.step[1] && c::horizontal(pos, p.spot) < 0.6);
        if d.sitting && matches!(d.phase, 5 | 7 | 10) && (health >= 14.0 || on_step) {
            run.note = format!("Dragon perched: attacking ({:.0} health left)", d.health);
            self.end_perch(host, b, run, &d, pos);
            return;
        }
        // Off its step once it takes off.
        if run.perch.take().is_some() {
            self.companion_stop_dig(host, b);
        }
        // A crystal left heals it for good: every five minutes in the air,
        // back for it (a few times).
        if left > 0 && !d.sitting && run.passes < 5 && now - run.since > 6000 {
            run.passes += 1;
            log::info!(
                "[end] {} goes back for the {left} crystals left",
                b.file.name
            );
            for k in &mut run.known {
                k.skipped = false;
            }
            run.shot = None;
            run.cage = None;
            run.set_phase(EndPhase::Crystals, now);
            return;
        }
        run.note = if left > 0 {
            format!(
                "Waiting for the dragon to perch ({left} crystals left, {:.0} health)",
                d.health
            )
        } else {
            format!("Waiting for the dragon to perch ({:.0} health)", d.health)
        };
        if d.phase == 1 {
            run.danger = "the dragon's fireballs";
        }
        // Up on its step over a breath cloud: it waits there till it's gone.
        let over_cloud = self.breath_view().iter().any(|(at, r, _)| {
            c::horizontal(pos, *at) <= f64::from(*r) + 1.0
                && (0.99..3.5).contains(&(pos[1] - at[1]))
        });
        if over_cloud
            && !(!d.sitting
                && c::horizontal(pos, d.position) < 10.0
                && d.position[1] - pos[1] < 8.0)
        {
            run.note = "Waiting on its step for the breath to clear".into();
            b.yaw = yaw_to(pos, d.position);
            b.keys.shift = true;
            return;
        }
        // Flying low past it: out of its wings' way.
        if !d.sitting && c::horizontal(pos, d.position) < 10.0 && d.position[1] - pos[1] < 8.0 {
            run.danger = "the dragon";
            let (dx, dz) = (pos[0] - d.position[0], pos[2] - d.position[2]);
            let len = dx.hypot(dz).max(0.1);
            let yaw = yaw_to(pos, [pos[0] + dx / len, pos[1], pos[2] + dz / len]);
            if self.companion_safe_at(pos, yaw, 1.5) {
                b.yaw = yaw;
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
                return;
            }
        }
        // Perched (and it too hurt to go in): well out of its head's way.
        let ring = if d.sitting {
            WAIT_RING * 2.0
        } else {
            WAIT_RING
        };
        match self.end_wait_spot(pos, ring) {
            Some(spot) if c::horizontal(pos, spot) > 2.0 => {
                let _ = self.end_walk(host, b, spot, 1.5, pos);
            }
            _ => {
                // Watching it.
                b.yaw = yaw_to(pos, d.position);
                b.pitch = look_pitch(eye(pos), d.position).clamp(-60.0, 30.0);
            }
        }
    }

    /// At the perched head: onto a step beside it, then critical hits.
    fn end_perch(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        d: &DragonView,
        pos: [f64; 3],
    ) {
        let now = now_of(self);
        let parts = Self::end_parts(d);
        let head = parts.head;
        let head_mid = [
            (head.0[0] + head.1[0]) / 2.0,
            head.0[1],
            (head.0[2] + head.1[2]) / 2.0,
        ];
        run.danger = "the dragon";
        self.end_put_bow_away(host, b);
        // In a wing's knockback (it throws a body tens of blocks, off the
        // island) or by the head's bite: out first, the shortest way.
        if !parts.clear_of(pos, 0.1) {
            let out = (0..16)
                .map(|i| f64::from(i) / 16.0 * std::f64::consts::TAU)
                .filter_map(|a| {
                    let (cos, sin) = (a.cos(), a.sin());
                    let steps = (1..=12).find(|n| {
                        let d = f64::from(*n) * 0.5;
                        parts.clear_of([pos[0] + cos * d, pos[1], pos[2] + sin * d], 0.3)
                    })?;
                    Some((steps, [pos[0] + cos * 3.0, pos[1], pos[2] + sin * 3.0]))
                })
                .min_by_key(|(n, _)| *n)
                .map(|(_, to)| to);
            if let Some(to) = out {
                b.yaw = yaw_to(pos, to);
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
            }
            run.perch = None;
            return;
        }
        // A spot beside the head, a block up (out of the breath on the ground).
        if run.perch.is_none_or(|p| {
            !parts.clear_of(p.spot, 0.05)
                || e::reach_to(eye(p.spot), head) > 3.0
                || now - p.since > 300 && feet(pos)[1] <= p.step[1]
        }) {
            // The breath lies on the ground ahead of the head (radius 5,
            // 2.5 forward, at the ground there): inside its reach it stands
            // on a step a block or two over its own column's ground, and a
            // full block over the breath; outside it, on the ground too.
            let ahead = {
                let (dx, dz) = (head_mid[0] - d.position[0], head_mid[2] - d.position[2]);
                let len = dx.hypot(dz).max(0.1);
                [
                    head_mid[0] + dx / len * 2.5,
                    0.0,
                    head_mid[2] + dz / len * 2.5,
                ]
            };
            let breath_y = self
                .end_ground(
                    ahead[0].floor() as i32,
                    ahead[2].floor() as i32,
                    head_mid[1] as i32 + 2,
                )
                .unwrap_or(64);
            let blocks = count_any(&self.end_inventory(host, b), &BLOCKS);
            let base = head.0[1].floor() as i32;
            let floors: Vec<f64> = (base - 4..=base + 3).map(f64::from).collect();
            let clouds = self.breath_view();
            let clouded = |s: &[f64; 3]| {
                clouds
                    .iter()
                    .any(|(at, r, _)| e::in_breath(*s, *at, *r, 0.5))
            };
            // (spot, blocks to stack under it)
            let mut spots: Vec<([f64; 3], i32)> = e::perch_spots(&parts, &floors, pos, 2.9, 0.8)
                .into_iter()
                .filter_map(|s| {
                    let f = feet(s);
                    if !self.passable(f) || !self.passable([f[0], f[1] + 1, f[2]]) {
                        return None;
                    }
                    let g = self.end_ground(f[0], f[2], f[1] + 1)?;
                    let stack = f[1] - g;
                    let in_reach = c::horizontal(s, ahead) <= 5.8;
                    let ok = (0..=3).contains(&stack)
                        && stack <= i32::try_from(blocks).unwrap_or(i32::MAX)
                        && (!in_reach || stack >= 1 && f[1] > breath_y)
                        && !self.block_name([f[0], g - 1, f[2]]).ends_with("end_portal")
                        && !self.block_name([f[0], g - 1, f[2]]).ends_with("bedrock")
                        && !clouded(&s);
                    ok.then_some((s, stack))
                })
                .collect();
            // A step it built before first (nothing to stack), then fewer
            // blocks, then nearer (perch_spots' order).
            spots.sort_by_key(|(_, stack)| *stack);
            let pick = spots.first().map(|(s, _)| *s);
            if pick.is_none()
                && std::env::var_os("HORIZON_END_TRACE").is_some()
                && b.ticks.is_multiple_of(20)
            {
                let all = e::perch_spots(&parts, &floors, pos, 6.0, 0.0).len();
                let reach = e::perch_spots(&parts, &floors, pos, 2.9, 0.0).len();
                let clear = e::perch_spots(&parts, &floors, pos, 2.9, 0.8).len();
                log::info!(
                    "[end] no perch: floors {floors:?} breath at {breath_y} head {:?}-{:?} spots any {all} in reach {reach} clear {clear}",
                    head.0.map(|v| (v * 10.0).round() / 10.0),
                    head.1.map(|v| (v * 10.0).round() / 10.0),
                );
            }
            run.perch = pick.map(|spot| Perch {
                spot,
                step: [
                    spot[0].floor() as i32,
                    spot[1].floor() as i32 - 1,
                    spot[2].floor() as i32,
                ],
                since: now,
            });
        }
        let Some(p) = run.perch else {
            run.note = "Dragon perched: no place by its head".into();
            return;
        };
        if std::env::var_os("HORIZON_END_TRACE").is_some() && b.ticks.is_multiple_of(5) {
            let r = |v: [f64; 3]| v.map(|x| (x * 10.0).round() / 10.0);
            log::info!(
                "[end] perch phase {} pos {:?} spot {:?} step {:?} head {:?}-{:?} neck {:?}-{:?} clouds {:?} ground {:?}",
                d.phase,
                r(pos),
                r(p.spot),
                p.step,
                r(head.0),
                r(head.1),
                r(parts.neck.0),
                r(parts.neck.1),
                self.breath_view()
                    .iter()
                    .map(|(a, rr, _)| (r(*a), *rr))
                    .collect::<Vec<_>>(),
                self.end_guest(host, b).map(|g| g.0),
            );
        }
        let f = feet(pos);
        let step_col = [p.step[0], p.step[2]];
        if [f[0], f[2]] == step_col && f[1] <= p.step[1] {
            // In the step's column on the ground: the step, by pillaring one.
            let _ = self.end_pillar(host, b, run, p.step[1] + 1, pos);
            return;
        }
        if [f[0], f[2]] != step_col {
            // To the step's column on the ground (or up onto the step).
            let goal = [
                f64::from(p.step[0]) + 0.5,
                f64::from(p.step[1]),
                f64::from(p.step[2]) + 0.5,
            ];
            // The straight way there through a wing's reach: round by the
            // outside, along the head's radius through the spot.
            let crosses = (1..=12).any(|n| {
                let t = f64::from(n) / 12.0;
                let at = [
                    pos[0] + (goal[0] - pos[0]) * t,
                    pos[1].max(goal[1]),
                    pos[2] + (goal[2] - pos[2]) * t,
                ];
                !parts.clear_of(at, 0.2)
            });
            if crosses {
                let (dx, dz) = (goal[0] - head_mid[0], goal[2] - head_mid[2]);
                let len = dx.hypot(dz).max(0.1);
                let outside = [
                    head_mid[0] + dx / len * 7.0,
                    pos[1],
                    head_mid[2] + dz / len * 7.0,
                ];
                if c::horizontal(pos, outside) > 1.0 {
                    let _ = self.end_walk(host, b, outside, 0.8, pos);
                    return;
                }
            }
            if c::horizontal(pos, goal) < 3.0 {
                // Close: straight there (the walker would go round the head).
                b.yaw = yaw_to(pos, goal);
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.jump =
                    self.floor(p.step) && f[1] <= p.step[1] && c::horizontal(pos, goal) < 1.2;
            } else if self.end_walk(host, b, goal, 0.3, pos) == Steer::Stuck {
                run.perch = None;
            }
            return;
        }
        let Some((on_ground, v)) = self.end_guest(host, b) else {
            return;
        };
        // On the step: to the middle of it, sneaking, then the head. In the
        // air (its jump) it doesn't steer: it lands back on the step.
        if on_ground && c::horizontal(pos, p.spot) > 0.25 {
            b.yaw = yaw_to(pos, p.spot);
            b.pitch = 0.0;
            b.keys.forward = true;
            b.keys.shift = true;
            return;
        }
        let head_centre = [head_mid[0], (head.0[1] + head.1[1]) / 2.0, head_mid[2]];
        b.yaw = yaw_to(pos, head_centre);
        b.pitch = look_pitch(eye(pos), head_centre);
        if let Some(sword) = slot_of(&self.end_inventory(host, b), &SWORDS) {
            self.companion_hold_slot(host, b, sword);
        }
        // A sword charges in 12.5 ticks: jump, strike falling (a critical hit).
        let since = b.ticks - run.last_strike;
        if on_ground && since >= 6 {
            b.keys.jump = true;
        } else if !on_ground && v[1] < -0.05 && since >= 13 {
            self.companion_strike(host, b, d.id + 1);
            run.last_strike = b.ticks;
        }
    }

    /// A bed plan dropped: another side next time, the sword after three.
    fn end_bed_miss(&self, b: &Body, run: &mut EndRun, why: &str) {
        run.bed = None;
        run.bed_misses += 1;
        let give_up = run.bed_misses >= 3;
        log::info!(
            "[companion] {}: bed plan dropped ({why}){}",
            b.file.name,
            if give_up {
                ", the sword from now on"
            } else {
                ""
            }
        );
        if give_up {
            run.beds = false;
        }
    }

    fn end_has_beds(&self, host: &LanHost, b: &Body) -> bool {
        host.hub.guest(b.conn).is_some_and(|g| {
            g.inventory
                .iter()
                .flatten()
                .any(|s| s.item.ends_with("_bed"))
        })
    }

    /// The bed one-cycle at a perch: a hole two deep beside where the head
    /// hangs, the bed laid toward the head from its edge, then clicked: the
    /// blast (at the bed's head) hits the dragon's head; the ground between
    /// takes it for the companion.
    fn end_beds(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut EndRun,
        d: &DragonView,
        pos: [f64; 3],
    ) {
        let now = now_of(self);
        run.danger = "a bed's blast";
        run.hands = true;
        self.end_put_bow_away(host, b);
        let f = feet(pos);
        let head = d.parts[0];
        let head_mid = [
            (head.0[0] + head.1[0]) / 2.0,
            head.0[1],
            (head.0[2] + head.1[2]) / 2.0,
        ];
        if std::env::var_os("HORIZON_END_TRACE").is_some() && b.ticks.is_multiple_of(10) {
            log::info!(
                "[end] beds: phase {} sitting {} has {} plan {:?} pos {:?}",
                d.phase,
                d.sitting,
                self.end_has_beds(host, b),
                run.bed
                    .map(|x| (x.hole, x.foot, x.head, x.ground, x.tries, x.placed)),
                pos.map(|v| (v * 10.0).round() / 10.0),
            );
        }
        // The dragon flew, or the beds ran out: out of the hole.
        if !d.sitting || !self.end_has_beds(host, b) {
            if let Some(bed) = run.bed
                && f[1] < bed.ground
            {
                // Up its own hole; out of blocks, a staircase dug out of it.
                if self.end_pillar(host, b, run, bed.ground, pos) == Prog::Failed {
                    let out = [
                        f64::from(bed.foot[0]) + 0.5,
                        f64::from(bed.ground),
                        f64::from(bed.foot[1]) + 0.5,
                    ];
                    if self.end_traverse(host, b, run, out, pos) == Prog::Failed {
                        run.bed = None;
                    }
                }
                return;
            }
            run.bed = None;
            return;
        }
        if run.bed.is_none() {
            // The bed's head under the dragon's head, its foot and the hole
            // further along: toward the companion first, else another side
            // where the hole can be dug (not into the portal's bedrock).
            let hx = head_mid[0].floor() as i32;
            let hz = head_mid[2].floor() as i32;
            let (dx, dz) = (pos[0] - head_mid[0], pos[2] - head_mid[2]);
            let mut dirs = [[1, 0], [-1, 0], [0, 1], [0, -1]];
            dirs.sort_by(|a, z| {
                let score = |v: &[i32; 2]| -(f64::from(v[0]) * dx + f64::from(v[1]) * dz);
                score(a).total_cmp(&score(z))
            });
            let plan = dirs
                .iter()
                .skip(run.bed_misses as usize % 4)
                .find_map(|dir| {
                    let head = [hx, hz];
                    let foot = [hx + dir[0], hz + dir[1]];
                    let hole = [hx + 2 * dir[0], hz + 2 * dir[1]];
                    let ground = self.end_ground(foot[0], foot[1], 80)?;
                    let diggable = |x: i32, z: i32| {
                        (1..=2).all(|k| {
                            let name = self.block_name([x, ground - k, z]);
                            !name.ends_with("bedrock") && !name.ends_with("portal")
                        })
                    };
                    let level =
                        |c: [i32; 2]| self.end_ground(c[0], c[1], ground + 3) == Some(ground);
                    (diggable(hole[0], hole[1]) && level(hole) && level(head)).then_some(Bed {
                        hole,
                        foot,
                        head,
                        ground,
                        since: now,
                        placed: None,
                        tries: 0,
                    })
                });
            let Some(plan) = plan else {
                self.end_bed_miss(b, run, "no side of the head to dig a hole");
                return;
            };
            run.bed = Some(plan);
        }
        let Some(mut bed) = run.bed else {
            return;
        };
        if now - bed.since > 600 || bed.tries > 12 {
            self.end_bed_miss(b, run, "the bed wouldn't go down or go off");
            return;
        }
        let g = bed.ground;
        let in_hole = [f[0], f[2]] == bed.hole && f[1] <= g - 2;
        if !in_hole {
            if [f[0], f[2]] != bed.hole {
                let goal = [
                    f64::from(bed.hole[0]) + 0.5,
                    f64::from(g),
                    f64::from(bed.hole[1]) + 0.5,
                ];
                if c::horizontal(pos, goal) < 1.2 {
                    b.yaw = yaw_to(pos, goal);
                    b.keys.forward = true;
                } else if self.end_walk(host, b, goal, 0.3, pos) == Steer::Stuck {
                    self.end_bed_miss(b, run, "couldn't reach the hole");
                    return;
                }
            } else if self.end_descend(host, b, run, g - 2, pos) == Prog::Failed {
                // The hole: two blocks dug down where it stands.
                self.end_bed_miss(b, run, "couldn't dig the hole");
                return;
            }
            run.bed = Some(bed);
            return;
        }
        let foot = [bed.foot[0], g, bed.foot[1]];
        let head_cell = [bed.head[0], g, bed.head[1]];
        if !self.block_name(foot).ends_with("_bed") {
            if bed.placed.is_some_and(|t| now - t < 4) {
                run.bed = Some(bed);
                return;
            }
            // The ground under the bed's two cells, filled if a blast took it.
            for cell in [
                [foot[0], g - 1, foot[2]],
                [head_cell[0], g - 1, head_cell[2]],
            ] {
                if !self.floor(cell) {
                    let below = [cell[0], cell[1] - 1, cell[2]];
                    if self.floor(below) {
                        self.end_use_on(host, b, below, 1, &BLOCKS, None, pos);
                    }
                    bed.tries += 1;
                    run.bed = Some(bed);
                    return;
                }
            }
            if !self.passable(head_cell) || !self.passable(foot) {
                bed.tries += 1;
                run.bed = Some(bed);
                return;
            }
            // Laid facing the head: its head half goes there.
            let beds: Vec<String> = self
                .end_inventory(host, b)
                .iter()
                .flatten()
                .filter(|s| s.item.ends_with("_bed"))
                .map(|s| s.item.to_string())
                .collect();
            let names: Vec<&str> = beds.iter().map(String::as_str).collect();
            let toward = yaw_to(centre(foot), centre(head_cell));
            self.end_use_on(
                host,
                b,
                [foot[0], g - 1, foot[2]],
                1,
                &names,
                Some(toward),
                pos,
            );
            bed.placed = Some(now);
            bed.tries += 1;
            run.bed = Some(bed);
            return;
        }
        // Clicked from the hole with an empty hand (a bed in hand would be
        // laid): the blast is at the bed's head.
        let before = d.health;
        let inv = self.end_inventory(host, b);
        if let Some(empty) = (0..9).find(|i| inv.get(*i).is_some_and(Option::is_none)) {
            self.companion_hold_slot(host, b, empty);
        } else if let Some(sword) = slot_of(&inv, &SWORDS) {
            self.companion_hold_slot(host, b, sword);
        }
        let face = face_toward(foot, eye(pos));
        b.yaw = yaw_to(pos, centre(foot));
        b.pitch = look_pitch(eye(pos), centre(foot));
        if let Some(gg) = host.hub.guest_mut(b.conn) {
            (gg.yaw, gg.pitch) = (b.yaw, b.pitch);
        }
        self.companion_use_block(host, b, foot, face, pos);
        if !self.block_name(foot).ends_with("_bed") {
            run.beds_used += 1;
            let after = self.dragon_view().map_or(before, |d| d.health);
            run.bed_damage += (before - after).max(0.0);
            bed.placed = None;
            bed.tries = 0;
        }
        run.bed = Some(bed);
    }

    fn end_exit(&mut self, host: &mut LanHost, b: &mut Body, run: &mut EndRun, pos: [f64; 3]) {
        let now = now_of(self);
        run.note = "The dragon is dead: into the exit portal".into();
        if now - run.since > 3600 {
            self.end_fail(b, run, "couldn't reach the exit portal");
            return;
        }
        let Some(exit) = self.exit_portal() else {
            return;
        };
        // The portal's blocks round the bedrock column, once they are there.
        let mut cells = Vec::new();
        for dx in -3..=3 {
            for dz in -3..=3 {
                let p = [exit[0] + dx, exit[1], exit[2] + dz];
                if self.block_name(p).ends_with("end_portal") {
                    cells.push(p);
                }
            }
        }
        let Some(&cell) = cells
            .iter()
            .min_by(|a, z| c::distance(pos, centre(**a)).total_cmp(&c::distance(pos, centre(**z))))
        else {
            run.note = "Waiting for the exit portal".into();
            return;
        };
        let goal = [
            f64::from(cell[0]) + 0.5,
            f64::from(cell[1]),
            f64::from(cell[2]) + 0.5,
        ];
        if c::horizontal(pos, goal) < 3.5 {
            b.yaw = yaw_to(pos, goal);
            b.pitch = 0.0;
            b.keys.forward = true;
        } else if self.end_walk(host, b, [goal[0], goal[1] + 1.0, goal[2]], 2.0, pos)
            == Steer::Stuck
        {
            b.yaw = yaw_to(pos, goal);
            b.keys.forward = true;
        }
    }
}

/// Whether the End skill has its hands on something (its gear stays put).
const PUMPKIN: &str = "minecraft:carved_pumpkin";

/// Whether its head is to keep the carved pumpkin on (an End run wearing
/// one): the usual gear swap would put the helmet back.
pub(super) fn keeps_pumpkin(b: &Body, worn: Option<&ItemStack>) -> bool {
    b.end.as_ref().is_some_and(|r| r.active()) && worn.is_some_and(|s| &*s.item == PUMPKIN)
}

pub(super) fn holds(b: &Body) -> bool {
    b.end.as_ref().is_some_and(|r| r.hands)
}
