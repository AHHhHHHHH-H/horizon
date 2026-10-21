//! The Speedrunner's middle part in the game (docs/companion-speedrun.md,
//! N1–S5; the pure reasoning is companion/nether.rs): from a lit Nether
//! portal to a filled End portal frame.
//!
//! It runs inside the overworld route (game/companion_run.rs) once that one
//! is done, or on its own from any leg (`nether [leg] [until leg] [level]
//! [rods N] [pearls N]`, the harness's segments). Each leg has a time
//! budget; one that runs out, or a death, ends the run with its reason (the
//! harness's failure classes).
//!
//! - **Enter / Home**: into the portal it remembers; Watch mode moves the
//!   owner's view and the body comes through (game/companion_watch.rs).
//! - **Fortress**: Scout and All-knowing know where it is; Fair walks a
//!   square spiral around the portal, 128 blocks a leg, and goes for the
//!   nether bricks or blazes it sees; a bastion's region is skipped.
//! - **Blazes**: to the spawner (seen, or through walls at All-knowing),
//!   then each tick the stance: shield up against a charged blaze, a blow
//!   at one in reach, the bow at range, back off to heal.
//! - **Pearls**: a warped forest (seen nylium, the biome it stands in, or
//!   `/locate biome` at All-knowing), or an overworld night; a two-deep pit:
//!   it looks out from a block in it, stares an enderman angry, mines the
//!   block away and hits the enderman's legs from below, out of its reach.
//! - **Eyes**: powder, then eyes (the craft skill).
//! - **Triangulate**: two throws, the bearing of each eye's flight, where
//!   the lines cross (Scout and All-knowing: located).
//! - **Travel, Room**: there, a staircase down until stronghold bricks show,
//!   then the corridors' unexplored ends until it sees the frames;
//!   All-knowing goes straight to the frames.
//! - **Fill**: a spot in reach of the empty frames, an eye in each.
use super::{
    Dim, Game,
    companion::Body,
    companion_body::{Steer, look_pitch, mob_at, threat},
    companion_walk::THROWAWAY,
    companion_work::{BlockSearch, eye, feet},
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c, CompanionReport,
        moves::Goal,
        nether::{self as n, Carried, Cue, Foe, Leg, Pit, Search, Stance, Targets, Throw},
        perception::PerceptionLevel,
    },
    items::ItemStack,
    mobs::Who,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use serde_json::{Value, json};
use std::sync::Arc;

/// Ticks each leg may take before the run ends on it.
fn budget(leg: Leg) -> u64 {
    match leg {
        Leg::Enter => 1200,
        Leg::Fortress => 14_400,
        Leg::Blazes => 14_400,
        Leg::Pearls => 24_000,
        Leg::Home => 9600,
        Leg::Eyes => 2400,
        Leg::Triangulate => 6000,
        Leg::Travel => 30_000,
        Leg::Room => 14_400,
        Leg::Fill => 2400,
        Leg::Done => u64::MAX,
    }
}

/// Ticks into the pearls leg after which, with under a third of its pearls,
/// it hunts them on an Overworld night instead.
const OVERWORLD_PEARLS_AFTER: u64 = 6000;

/// Blocks from the portal the Fair fortress search goes at most.
const SEARCH_LIMIT: i32 = 1200;
/// The spiral's leg (sight is 64 blocks each side).
const SEARCH_STEP: i32 = 128;
/// Ticks between two looks at its memory for cues.
const CUE_EVERY: u64 = 20;
/// Blazes this near (to the box) are in the fight.
const BLAZE_RANGE: f64 = 24.0;
/// A sword's attack charges in 1/1.6 s.
const SWING: u64 = 13;

const ROD: &str = "minecraft:blaze_rod";
const POWDER: &str = "minecraft:blaze_powder";
const PEARL: &str = "minecraft:ender_pearl";
const EYE: &str = "minecraft:ender_eye";
const SHIELD: &str = "minecraft:shield";

/// Where the pit fight is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PitStep {
    /// Digging the two blocks, top first.
    Dig,
    /// A block under its feet in the bottom cell: up to look out.
    Climb,
    /// Looking out for endermen, staring one angry.
    Look,
    /// The block under it mined away: down.
    Drop,
    /// At the bottom, hitting the legs of those on the rim.
    Fight,
}

/// A box by a blaze spawner: walls on three sides, a roof, open toward the
/// spawner.
#[derive(Clone, Copy, Debug)]
struct Bunker {
    spawner: [i32; 3],
    feet: [i32; 3],
    /// Toward the spawner (x, z): the opening.
    front: [i32; 2],
    built: bool,
    /// Walled in where it stood to heal (no spawner by): forgotten once it
    /// has healed and opened it.
    heal_only: bool,
}

impl Bunker {
    /// The three walled sides (x, z).
    fn sides(front: [i32; 2]) -> [[i32; 2]; 3] {
        [
            [front[1], front[0]],
            [-front[1], -front[0]],
            [-front[0], -front[1]],
        ]
    }
}

struct PitFight {
    pit: Pit,
    step: PitStep,
    since: u64,
    /// Ticks since it last saw an enderman.
    quiet: u64,
    jumped: bool,
}

/// An eye it threw, watched in flight.
struct Flight {
    from: [f64; 2],
    /// Eyes in flight before the throw.
    before: usize,
    at: u64,
    dir: Option<[f64; 2]>,
    /// Where it last was (to pick it up where it dropped).
    last: Option<[f64; 3]>,
}

/// The middle part under way (`Body::nether`).
pub(super) struct NetherRun {
    pub leg: Leg,
    /// The run stops after this leg (a harness segment).
    pub until: Leg,
    pub targets: Targets,
    pub start: u64,
    leg_start: u64,
    pub splits: Vec<(String, u64)>,
    pub note: String,
    /// Why it died, each time.
    pub deaths: Vec<String>,
    /// How the run ended: "done", or the failure that ended it.
    pub outcome: Option<String>,
    /// Block state → what it says (built once).
    cues: Option<Arc<Vec<Cue>>>,
    search: Option<Search>,
    waypoint: Option<[i32; 3]>,
    pub fortress: Option<[i32; 3]>,
    pub spawner: Option<[i32; 3]>,
    scan: Option<BlockSearch>,
    scanned: bool,
    visited: Vec<[i32; 3]>,
    last_cue: u64,
    shield: bool,
    shield_until: u64,
    last_swing: u64,
    pit: Option<PitFight>,
    pub throws: Vec<Throw>,
    flight: Option<Flight>,
    /// An eye on the ground to pick up, and until when.
    fetch: Option<([f64; 3], u64)>,
    pub stronghold: Option<[i32; 2]>,
    /// The depth the staircase heads for at the stronghold.
    depth: i32,
    pub kills: u32,
    pub min_health: f32,
    /// Blocks it gave up on (unreachable waypoints and targets).
    bad: Vec<[i32; 3]>,
    forest: Option<[i32; 3]>,
    stuck: u32,
    /// The goal it walks to, the nearest it got, and walks since.
    progress: Option<([i32; 3], f64, u32)>,
    /// A hoglin that stayed out of reach (across a gap): left alone, and
    /// since when.
    hoglin_wait: Option<(u32, u64)>,
    /// Ticks a hoglin in reach went without attacking (from below a
    /// ledge, it can't): then it gets hit anyway.
    hoglin_quiet: u32,
    /// The body's tick count when it left the hub (a new body counts
    /// from 0; the run's times go on from here).
    pub body_ticks: u64,
    /// The cheap long walk (game/companion_cruise.rs), and until when it is
    /// off after it found no way.
    cruise: super::companion_cruise::Cruise,
    cruise_off_until: u64,
    /// The column its staircase goes down by (dry ground near the
    /// stronghold's start).
    shaft: Option<[i32; 2]>,
    /// The dry ground it makes for when it stands in water there.
    ashore: Option<[i32; 3]>,
    /// The portal room's frames, once known.
    frames: Option<[i32; 3]>,
    /// Where it hides from the blazes while it heals, and when that was
    /// worked out.
    cover: Option<[i32; 3]>,
    cover_at: u64,
    /// How near it came to the cover, and since when.
    cover_best: Option<(f64, u64)>,
    /// Firmer ground it makes for to dig its pit.
    pit_spot: Option<[i32; 3]>,
    firm_tries: u32,
    /// The hoglin or wither skeleton it is fighting: since when (the last
    /// check) and its health then; and those it gave up on, until when.
    engaged: Option<(u32, u64, f32)>,
    ignored: Vec<(u32, u64)>,
    /// An angry enderman it goes after out of the pit.
    chase: Option<u32>,
    /// Its box by the blaze spawner.
    bunker: Option<Bunker>,
    /// Its box closed on the open side while it heals.
    sealed: bool,
    /// The blaze it is after (id, since, its health then): one it can't
    /// hurt for a while (out of reach, no way there) is left alone.
    blaze_engaged: Option<(u32, u64, f32)>,
}

impl NetherRun {
    pub fn new(from: Leg, until: Leg, targets: Targets, now: u64) -> Self {
        Self {
            leg: from,
            until,
            targets,
            start: now,
            leg_start: now,
            splits: Vec::new(),
            note: "starting".into(),
            deaths: Vec::new(),
            outcome: None,
            cues: None,
            search: None,
            waypoint: None,
            fortress: None,
            spawner: None,
            scan: None,
            scanned: false,
            visited: Vec::new(),
            last_cue: 0,
            shield: false,
            shield_until: 0,
            last_swing: 0,
            pit: None,
            throws: Vec::new(),
            flight: None,
            fetch: None,
            stronghold: None,
            depth: 30,
            kills: 0,
            min_health: 20.0,
            bad: Vec::new(),
            forest: None,
            stuck: 0,
            progress: None,
            hoglin_wait: None,
            hoglin_quiet: 0,
            body_ticks: 0,
            cruise: Default::default(),
            cruise_off_until: 0,
            shaft: None,
            ashore: None,
            frames: None,
            cover: None,
            cover_at: 0,
            cover_best: None,
            pit_spot: None,
            firm_tries: 0,
            engaged: None,
            ignored: Vec::new(),
            chase: None,
            bunker: None,
            sealed: false,
            blaze_engaged: None,
        }
    }
    /// A report the body made: deaths are kept with their cause, kills
    /// counted.
    pub fn heard(&mut self, report: &CompanionReport) {
        if let CompanionReport::Died { cause } = report {
            self.deaths.push(format!("{} ({})", cause, self.leg.id()));
            if self.outcome.is_none() {
                self.outcome = Some(format!("died in {}: {cause}", self.leg.id()));
            }
        }
    }
    fn split(&mut self, name: &str, ticks: u64) -> bool {
        if self.splits.iter().any(|(n, _)| n == name) {
            return false;
        }
        self.splits.push((name.to_owned(), ticks));
        true
    }
    pub fn finished(&self) -> bool {
        self.outcome.is_some()
    }
}

fn count(inventory: &[Option<ItemStack>], item: &str) -> u32 {
    super::companion_body::carried(inventory, item)
}

fn d2(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum()
}

fn at_block(p: [i32; 3]) -> [f64; 3] {
    [
        f64::from(p[0]) + 0.5,
        f64::from(p[1]),
        f64::from(p[2]) + 0.5,
    ]
}

impl Game<'_> {
    /// `nether [leg] [until leg] [level] [rods N] [pearls N] [eyes N]
    /// [barter]`: the middle part from a leg (Enter by default), alone.
    pub(super) fn companion_start_nether(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        from: Leg,
        until: Leg,
        level: Option<PerceptionLevel>,
        targets: Targets,
    ) {
        // The overworld route's frame (its hooks: the walker that digs and
        // builds, creepers, the HUD), done already.
        self.companion_start_run(host, b, level);
        if let Some(run) = b.run.as_mut() {
            run.stage = super::companion_run::Stage::Done;
        }
        let now = b.ticks;
        b.nether = Some(Box::new(NetherRun::new(from, until, targets, now)));
        // Started on a later leg: the portal it stands by is the way back.
        if let Some(g) = host.hub.guest(b.conn)
            && let Some(p) = self.portal_cell_within(feet(g.position), 32)
        {
            b.senses.memory_mut().places.insert("portal".into(), p);
        }
        self.companion_nether_gear(host, b);
        self.companion_report(
            b,
            CompanionReport::Milestone {
                name: format!("nether from {}", from.id()),
                detail: format!(
                    "{} rods, {} pearls, {} eyes",
                    targets.rods, targets.pearls, targets.eyes
                ),
            },
        );
    }

    /// The overworld route is done (its portal lit): on to the Nether.
    pub(super) fn companion_nether_after_overworld(&mut self, host: &mut LanHost, b: &mut Body) {
        if b.nether.is_some() {
            return;
        }
        let now = b.ticks;
        b.nether = Some(Box::new(NetherRun::new(
            Leg::Enter,
            Leg::Fill,
            Targets::default(),
            now,
        )));
        self.companion_nether_gear(host, b);
    }

    /// The run's status for `companion list` and the harness.
    pub(super) fn companion_nether_status(&self, b: &Body) -> Value {
        let Some(run) = b.nether.as_ref() else {
            return Value::Null;
        };
        let now = b.ticks;
        let have = host_carried(self, b);
        json!({
            "leg": run.leg.id(),
            "until": run.until.id(),
            "step": n::step_text(run.leg, &have, &run.targets),
            "note": run.note,
            "ticks": now.saturating_sub(run.start),
            "leg_ticks": now.saturating_sub(run.leg_start),
            "level": b.senses.level.id(),
            "splits": run.splits.iter().map(|(n, t)| json!({ "name": n, "ticks": t })).collect::<Vec<_>>(),
            "deaths": run.deaths,
            "outcome": run.outcome,
            "kills": run.kills,
            "min_health": run.min_health,
            "rods": have.rods_worth(),
            "pearls": have.pearls_worth(),
            "eyes": have.eyes,
            "filled": have.filled,
            "fortress": run.fortress,
            "spawner": run.spawner,
            "stronghold": run.stronghold,
            "throws": run.throws.iter().map(|t| json!({ "from": t.from, "dir": t.dir })).collect::<Vec<_>>(),
            "pit": run.pit.as_ref().map(|p| format!("{:?} at {:?}", p.step, p.pit.top)),
            "waypoint": run.waypoint,
            "cruise": format!("{:?} off until {}", run.cruise, run.cruise_off_until),
        })
    }

    /// The step line the Watch HUD shows.
    pub(super) fn companion_nether_step(&self, host: &LanHost, b: &Body) -> Option<String> {
        let run = b.nether.as_ref()?;
        let have = carried_now(self, host, b, run);
        let step = n::step_text(run.leg, &have, &run.targets);
        Some(match &run.outcome {
            Some(o) if o != "done" => format!("{step} - stopped: {o}"),
            _ if run.note.is_empty() => step,
            _ => format!("{step} - {}", run.note),
        })
    }

    /// Shield to the off hand, if it carries one and the off hand is free.
    fn companion_nether_gear(&mut self, host: &mut LanHost, b: &mut Body) {
        let Some(g) = host.hub.guest_mut(b.conn) else {
            return;
        };
        if g.offhand.is_some() {
            return;
        }
        if let Some(i) = g
            .inventory
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| &*s.item == SHIELD))
        {
            let shield = g.inventory[i].take();
            g.offhand = shield.clone();
            self.as_guest(host, b.conn, |game| {
                game.player_on_equip("offhand", None, shield.as_ref());
            });
        }
    }

    /// Every tick of the run before the body's own fighting
    /// (`companion_move`): the fights this part fights its own way (blazes
    /// with the shield, endermen from the pit, ghasts blocked, wither
    /// skeletons avoided when weak). True when it took the tick.
    pub(super) fn companion_nether_take(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        pos: [f64; 3],
    ) -> bool {
        let Some(mut run) = b.nether.take() else {
            return false;
        };
        let took = if run.finished() {
            self.companion_shield(host, b, &mut run, None, pos);
            false
        } else {
            self.companion_nether_fights(host, b, &mut run, pos)
        };
        b.nether = Some(run);
        took
    }

    /// The leg's own tick (the route's when nothing else has the body).
    pub(super) fn companion_nether_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        pos: [f64; 3],
    ) {
        let Some(mut run) = b.nether.take() else {
            return;
        };
        if !run.finished() {
            // Short of air under water: straight up before anything else.
            let air = host
                .hub
                .guest(b.conn)
                .and_then(|g| g.motion.as_ref())
                .map_or(300, |m| m.air);
            let e = feet(eye(pos));
            let under = self
                .res
                .facts()
                .get(self.world.block(e[0], e[1], e[2]))
                .fluid
                == Fluid::Water;
            if under && air < 120 {
                self.companion_stop_dig(host, b);
                b.keys = Default::default();
                b.keys.jump = true;
                b.pitch = -90.0;
                run.note = "up for air".into();
                b.nether = Some(run);
                return;
            }
            self.companion_nether_leg(host, b, &mut run, pos);
            // Never sinks: in water up to its eyes, it swims up.
            let e = feet(eye(pos));
            if self
                .res
                .facts()
                .get(self.world.block(e[0], e[1], e[2]))
                .fluid
                == Fluid::Water
            {
                b.keys.jump = true;
            }
        }
        b.nether = Some(run);
    }

    fn companion_nether_fights(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) -> bool {
        let Some(g) = host.hub.guest(b.conn) else {
            return false;
        };
        let health = g.health;
        let shielded = g.offhand.as_ref().is_some_and(|s| &*s.item == SHIELD);
        run.min_health = run.min_health.min(health);
        let from = eye(pos);
        let now = b.ticks;
        // Ghasts: never chased (they float over lava); the shield against
        // one that is charging and sees it.
        if let Some(id) = b.target
            && self
                .mobs
                .iter()
                .any(|m| m.id == id && &*m.kind.id == "minecraft:ghast")
        {
            b.target = None;
        }
        // In lava (knocked in, a flow over the path): out by the nearest
        // dry spot, jumping, before anything else.
        // In fire (a fireball lit the floor): out of it, the nearest way.
        let fire_at = |p: [f64; 3]| {
            let name = &self
                .res
                .facts()
                .get(self.world.block(
                    p[0].floor() as i32,
                    p[1].floor() as i32,
                    p[2].floor() as i32,
                ))
                .name;
            name.ends_with("minecraft:fire") || name.ends_with("soul_fire")
        };
        if fire_at(pos) || fire_at([pos[0], pos[1] + 0.9, pos[2]]) {
            let here = feet(pos);
            // Put out first (a blow breaks a fire block at once): in its box
            // it stays, and a step away still burns.
            let cell = if fire_at(pos) {
                here
            } else {
                [here[0], here[1] + 1, here[2]]
            };
            if self.companion_dig_step(host, b, cell, 1).is_ok() {
                b.target = None;
                self.companion_shield(host, b, run, None, pos);
                run.note = "putting out a fire".into();
                return true;
            }
            let out = (1..=3).find_map(|r| {
                (-r..=r)
                    .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
                    .filter(|&(dx, dz): &(i32, i32)| dx.abs().max(dz.abs()) == r)
                    .find_map(|(dx, dz)| {
                        let (x, z) = (here[0] + dx, here[2] + dz);
                        self.cruise_stand(x, here[1], z)
                            .filter(|y| (y - here[1]).abs() <= 1)
                            .map(|y| [x, y, z])
                    })
            });
            if let Some(o) = out {
                b.target = None;
                self.companion_shield(host, b, run, None, pos);
                b.yaw = c::yaw_toward(pos, at_block(o)) as f32;
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = true;
                b.keys.jump = o[1] > here[1];
                run.note = "out of the fire".into();
                return true;
            }
        }
        let lava_at = |p: [f64; 3]| {
            self.res
                .facts()
                .get(self.world.block(
                    p[0].floor() as i32,
                    p[1].floor() as i32,
                    p[2].floor() as i32,
                ))
                .fluid
                == Fluid::Lava
        };
        if lava_at(pos) || lava_at([pos[0], pos[1] + 0.9, pos[2]]) {
            let here = feet(pos);
            let out = (1..=5).find_map(|r| {
                (-r..=r)
                    .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
                    .filter(|&(dx, dz): &(i32, i32)| dx.abs().max(dz.abs()) == r)
                    .find_map(|(dx, dz)| {
                        let (x, z) = (here[0] + dx, here[2] + dz);
                        self.cruise_stand(x, here[1] + 1, z).map(|y| [x, y, z])
                    })
            });
            b.target = None;
            self.companion_shield(host, b, run, None, pos);
            b.keys.jump = true;
            run.note = "out of the lava".into();
            // A dry spot it can step up to: there. Else a pillar out: a
            // block into the lava under it at the top of each jump.
            let reachable = out.filter(|o| o[1] <= here[1] + 1);
            if let Some(o) = reachable {
                b.yaw = c::yaw_toward(pos, at_block(o)) as f32;
                b.pitch = 0.0;
                b.keys.forward = true;
                b.keys.sprint = false;
                return true;
            }
            let floor = [here[0], here[1] - 1, here[2]];
            let solid_below = self
                .res
                .facts()
                .get(self.world.block(floor[0], floor[1], floor[2]))
                .full_collision;
            let blocks = host.hub.guest(b.conn).and_then(|g| {
                g.inventory
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
            });
            if let Some(slot) = blocks
                && solid_below
                && pos[1] >= f64::from(here[1]) + 0.9
            {
                self.companion_hold_slot(host, b, slot);
                self.companion_use_block(host, b, floor, 1, pos);
            } else if let Some(slot) = blocks
                && pos[1] - f64::from(here[1]) > 0.4
                && !solid_below
            {
                // Over lava deeper than a block: a floor under it first.
                let under = [here[0], here[1] - 2, here[2]];
                if self
                    .res
                    .facts()
                    .get(self.world.block(under[0], under[1], under[2]))
                    .full_collision
                {
                    self.companion_hold_slot(host, b, slot);
                    self.companion_use_block(host, b, under, 1, pos);
                }
            }
            b.pitch = 90.0;
            return true;
        }
        // Magma cubes (hard hits, they split, fire doesn't hurt them): low,
        // it gets out of their way (they hop slowly); else they are fought
        // like hoglins, below (one after it can't be outrun for long).
        let cube = self
            .mobs
            .iter()
            .filter(|_| health < 8.0)
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:magma_cube")
            .map(mob_at)
            .filter(|at| c::distance(*at, pos) < 6.0 && (at[1] - pos[1]).abs() < 3.0)
            .min_by(|x, y| c::distance(*x, pos).total_cmp(&c::distance(*y, pos)));
        if let Some(at) = cube {
            if b.target.is_some_and(|id| {
                self.mobs
                    .iter()
                    .any(|m| m.id == id && &*m.kind.id == "minecraft:magma_cube")
            }) {
                b.target = None;
            }
            let away = [
                pos[0] + (pos[0] - at[0]) * 3.0,
                pos[1],
                pos[2] + (pos[2] - at[2]) * 3.0,
            ];
            let safe = self.cruise_safe(pos, c::yaw_toward(pos, away) as f32);
            if safe {
                self.companion_shield(host, b, run, None, pos);
                run.note = "out of a magma cube's way".into();
                if self.companion_steer(host, b, away, 0.5, true) != Steer::Stuck {
                    return true;
                }
            }
        }
        // Something on it already (a magma cube, a piglin): that fight first.
        let pressed = self.mobs.iter().any(|m| {
            threat(m) && &*m.kind.id != "minecraft:ghast" && c::distance(mob_at(m), pos) < 6.0
        });
        let ghast = self
            .mobs
            .iter()
            .filter(|_| !pressed)
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:ghast" && m.extra.flag)
            .map(|m| [m.position.x, m.position.y + 2.0, m.position.z])
            .find(|at| c::distance(*at, pos) < 64.0 && self.in_sight(from, *at));
        // A ghast after it while it stands at an edge (lava or a drop
        // beside it): onto solid ground first; a blast there throws it in
        // even through the shield.
        let ghast_near = self.mobs.iter().any(|m| {
            m.alive() && &*m.kind.id == "minecraft:ghast" && c::distance(mob_at(m), pos) < 64.0
        });
        if ghast_near
            && !pressed
            && self.on_edge(feet(pos))
            && let Some(g) = self.solid_ground_near(feet(pos))
        {
            self.companion_shield(host, b, run, None, pos);
            run.note = "off the edge, a ghast about".into();
            if self.companion_steer(host, b, at_block(g), 0.3, true) != Steer::Stuck {
                return true;
            }
        }
        // A fireball on its way to it (a ghast's, a blaze's): the shield
        // toward it until it lands.
        let incoming = self
            .creatures
            .projectiles
            .iter()
            .filter(|p| !p.stuck && p.owner.is_some_and(|o| o != Who::Player))
            .filter(|p| {
                use horizon_client::projectiles::ProjectileKind as K;
                matches!(p.kind, K::LargeFireball { .. } | K::SmallFireball)
            })
            .map(|p| {
                (
                    [p.position.x, p.position.y, p.position.z],
                    [p.velocity.x, p.velocity.y, p.velocity.z],
                )
            })
            .filter_map(|(at, v)| {
                let d = c::distance(at, from);
                let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let toward = d < 24.0
                    && speed > 1e-4
                    && ((from[0] - at[0]) * v[0]
                        + (from[1] - at[1]) * v[1]
                        + (from[2] - at[2]) * v[2])
                        / (d.max(1e-6) * speed)
                        > 0.9;
                // Ticks until it is on it (they speed up: a little sooner).
                toward.then(|| (at, d / speed.max(0.5)))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1));
        // A blaze within reach and the attack charged: the blow first,
        // unless a fireball lands before the shield could be up again (it
        // blocks 5 ticks after it is raised). Blazes keep their distance
        // and shoot: one in reach is the chance to kill.
        let charged = now.saturating_sub(run.last_swing) >= SWING;
        let in_reach = self
            .mobs
            .iter()
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:blaze")
            .map(|m| {
                (
                    m.id,
                    [m.position.x, m.position.y + 0.9, m.position.z],
                    box_distance(from, m.bounds()),
                )
            })
            .filter(|(_, at, reach)| *reach <= n::REACH && self.in_sight(from, *at))
            .min_by(|x, y| x.2.total_cmp(&y.2));
        if !pressed
            && charged
            && let Some((id, at, _)) = in_reach
            && incoming.is_none_or(|(_, eta)| eta > 7.0)
        {
            b.target = None;
            self.companion_shield(host, b, run, None, pos);
            b.yaw = c::yaw_toward(pos, at) as f32;
            b.pitch = look_pitch(from, at);
            self.companion_strike(host, b, id);
            run.last_swing = now;
            run.shield_until = 0;
            run.note = "a blow at a blaze in reach".into();
            return true;
        }
        if !pressed && let Some((at, _)) = incoming {
            run.shield_until = now + 4;
            self.companion_shield(host, b, run, Some(at), pos);
            run.note = "shield up: a fireball".into();
            return true;
        }
        if let Some(at) = ghast {
            run.shield_until = now + 30;
            self.companion_shield(host, b, run, Some(at), pos);
            run.note = "shield up: a ghast".into();
            return true;
        }
        if now < run.shield_until && !pressed {
            // The fireball is on its way: the shield stays up.
            return true;
        }
        // Wither skeletons: the shield against them (with the hoglins,
        // below); without one, not when it is weak.
        if health < 12.0
            && !shielded
            && let Some(at) = self
                .mobs
                .iter()
                .filter(|m| m.alive() && &*m.kind.id == "minecraft:wither_skeleton")
                .map(mob_at)
                .find(|at| c::distance(*at, pos) < 10.0)
        {
            b.target = None;
            self.companion_shield(host, b, run, None, pos);
            let away = [
                pos[0] + (pos[0] - at[0]) * 3.0,
                pos[1],
                pos[2] + (pos[2] - at[2]) * 3.0,
            ];
            run.note = "backing off from a wither skeleton".into();
            let _ = self.companion_steer(host, b, away, 0.5, true);
            return true;
        }
        // Health comes back only above 18 food: it eats sooner than the
        // body would on its own (walking on as it eats).
        if health < 16.0
            && b.eating.is_none()
            && host.hub.guest(b.conn).is_some_and(|g| g.food < 18)
            && !self
                .mobs
                .iter()
                .any(|m| threat(m) && c::distance(mob_at(m), pos) < 6.0)
        {
            self.companion_shield(host, b, run, None, pos);
            self.companion_start_eating(host, b);
        }
        // A fight coming (something hostile near) at an edge (a drop or
        // lava beside it, a bridge): onto solid ground first, where a
        // knock back doesn't throw it off.
        // (A ghast's blast throws it too, from much further.)
        let danger = self.mobs.iter().any(|m| {
            m.alive()
                && threat(m)
                && (c::distance(mob_at(m), pos) < 16.0
                    || &*m.kind.id == "minecraft:ghast" && c::distance(mob_at(m), pos) < 64.0)
        });
        if danger
            && self.on_edge(feet(pos))
            && let Some(g) = self.solid_ground_near(feet(pos))
        {
            self.companion_shield(host, b, run, None, pos);
            run.note = "onto solid ground before the fight".into();
            if self.companion_steer(host, b, at_block(g), 0.3, true) != Steer::Stuck {
                return true;
            }
        }
        // Hoglins (40 health, a toss in the air each hit) and wither
        // skeletons (the wither on each hit): the shield toward the one after
        // it, a blow in the moment after each of its attacks lands on the
        // shield.
        let hoglin = self
            .mobs
            .iter()
            .filter(|m| {
                !run.ignored
                    .iter()
                    .any(|&(id, until)| id == m.id && now < until)
            })
            .filter(|m| {
                m.alive()
                    && ((&*m.kind.id == "minecraft:hoglin" && !m.is_baby())
                        || &*m.kind.id == "minecraft:magma_cube"
                        || (shielded && &*m.kind.id == "minecraft:wither_skeleton"))
            })
            .filter(|m| {
                m.ai.target == Some(Who::Player)
                    && host.mob_players.get(&m.id) == Some(&b.conn)
                    && c::distance(mob_at(m), pos) < 8.0
            })
            .map(|m| {
                (
                    m.id,
                    mob_at(m),
                    box_distance(from, m.bounds()),
                    // A magma cube shows no swing: any moment is one.
                    if m.ai.swinging || &*m.kind.id == "minecraft:magma_cube" {
                        1
                    } else {
                        m.extra.anim_ticks
                    },
                )
            })
            .min_by(|x, y| x.2.total_cmp(&y.2));
        // One that stays just out of reach (it can't get round) is left
        // after a few seconds.
        // One it has fought a long while without hurting it (out of its
        // reach behind something): left alone for a time.
        let hoglin = hoglin.filter(|(id, _, reach, _)| {
            let hp = self
                .mobs
                .iter()
                .find(|m| m.id == *id)
                .map_or(0.0, |m| m.health);
            match run.engaged {
                Some((e, since, start)) if e == *id => {
                    if now.saturating_sub(since) > 400 {
                        // (One at it is fought on.)
                        if hp >= start && *reach > n::REACH {
                            run.ignored.push((*id, now + 600));
                            run.engaged = None;
                            return false;
                        }
                        run.engaged = Some((*id, now, hp));
                    }
                }
                _ => run.engaged = Some((*id, now, hp)),
            }
            true
        });
        let hoglin = hoglin.filter(|(id, _, reach, _)| {
            if *reach <= n::REACH {
                run.hoglin_wait = None;
                return true;
            }
            match run.hoglin_wait {
                Some((w, since)) if w == *id => now.saturating_sub(since) < 80,
                _ => {
                    run.hoglin_wait = Some((*id, now));
                    true
                }
            }
        });
        if let Some((_, _, reach, _)) = hoglin
            && reach > 4.5
        {
            // Not on it yet: on its way, no chase.
            b.target = None;
        } else if let Some((id, at, reach, swinging)) = hoglin {
            b.target = None;
            b.walk.reset();
            let centre = [at[0], at[1] + 0.7, at[2]];
            run.note = "shield up: hoglin or wither skeleton".into();
            let charged = now.saturating_sub(run.last_swing) >= SWING;
            // Its attack just came (its swing animation runs 10 ticks).
            run.hoglin_quiet = if swinging > 0 || reach > n::REACH {
                0
            } else {
                run.hoglin_quiet + 1
            };
            if reach <= n::REACH && charged && (swinging > 0 || run.hoglin_quiet > 40) {
                self.companion_shield(host, b, run, None, pos);
                b.yaw = c::yaw_toward(pos, at) as f32;
                b.pitch = look_pitch(from, centre);
                self.companion_strike(host, b, id);
                run.last_swing = now;
                // Up again straight after.
                run.shield_until = 0;
            } else {
                self.companion_shield(host, b, run, Some(centre), pos);
            }
            return true;
        }
        // The pit against endermen.
        if run.leg == Leg::Pearls && run.pit.is_some() {
            b.target = None;
            self.companion_pit(host, b, run, pos);
            return true;
        }
        // Blazes.
        let foes: Vec<(u32, [f64; 3], Foe)> = self
            .mobs
            .iter()
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:blaze")
            .filter_map(|m| {
                let centre = [m.position.x, m.position.y + 0.9, m.position.z];
                let reach = box_distance(from, m.bounds());
                (reach < BLAZE_RANGE).then(|| {
                    let sees = self.in_sight(from, centre);
                    (
                        m.id,
                        centre,
                        Foe {
                            reach,
                            sees,
                            charging: m.extra.flag,
                        },
                    )
                })
            })
            .filter(|(_, _, f)| f.sees || f.reach < 6.0)
            .filter(|(id, _, _)| {
                !run.ignored
                    .iter()
                    .any(|&(i, until)| i == *id && now < until)
            })
            .collect();
        if foes.is_empty() {
            self.companion_shield(host, b, run, None, pos);
            // Hidden from blazes still about: it stays until it has healed
            // (while its food lets it), so it doesn't walk back into their
            // fire hurt.
            let blazes_about = self.mobs.iter().any(|m| {
                m.alive()
                    && &*m.kind.id == "minecraft:blaze"
                    && box_distance(from, m.bounds()) < BLAZE_RANGE
            });
            let food = host.hub.guest(b.conn).map_or(0, |g| g.food);
            if health < 16.0 && blazes_about && run.cover.is_some() {
                if food < 20 {
                    self.companion_start_eating(host, b);
                }
                if b.eating.is_some() || food >= 18 {
                    b.keys = Default::default();
                    run.note = "healing in cover".into();
                    return true;
                }
            }
            if health >= 16.0 {
                run.cover = None;
            }
            return false;
        }
        b.target = None;
        // Its box by the spawner is where it heals: hurt, it goes in, closes
        // it, eats and heals out of sight, then opens it and fights on in the
        // open (from inside the blazes keep their distance and none die).
        // Hurt with no box near: walled in where it stands (the side toward
        // the blaze first), to close it and heal in.
        let box_near = run
            .bunker
            .is_some_and(|bx| bx.built && c::horizontal(pos, at_block(bx.feet)) < 10.0);
        let here = feet(pos);
        if health < 12.0
            && !box_near
            && foes.iter().any(|f| f.2.sees)
            && foes.iter().all(|f| f.2.reach > 4.0)
            && b.eating.is_none()
            && self.floor([here[0], here[1] - 1, here[2]])
            && !self.on_edge(here)
        {
            let nearest = foes
                .iter()
                .min_by(|x, y| x.2.reach.total_cmp(&y.2.reach))
                .map_or(pos, |f| f.1);
            let (dx, dz) = (nearest[0] - pos[0], nearest[2] - pos[2]);
            // The opening away from it.
            let front = if dx.abs() >= dz.abs() {
                [-(dx.signum() as i32), 0]
            } else {
                [0, -(dz.signum() as i32)]
            };
            let bx = run
                .bunker
                .filter(|bx| bx.feet == here && bx.heal_only)
                .unwrap_or(Bunker {
                    spawner: run.spawner.unwrap_or(here),
                    feet: here,
                    front,
                    built: false,
                    heal_only: true,
                });
            run.bunker = Some(bx);
            b.walk.reset();
            b.keys = Default::default();
            self.companion_shield(host, b, run, None, pos);
            if self.bunker_build(host, b, bx, pos) == Some(true) {
                run.note = "walling itself in to heal".into();
                return true;
            }
            run.bunker = Some(Bunker { built: true, ..bx });
        }
        if let Some(bx) = run.bunker
            && bx.built
        {
            if feet(pos) == bx.feet {
                if self.companion_box_heal(host, b, run, bx, health, pos) {
                    return true;
                }
            } else if health < 12.0 {
                let shooting = foes.iter().find(|f| f.2.sees && f.2.charging).map(|f| f.1);
                self.companion_shield(host, b, run, shooting, pos);
                run.note = "to its box to heal".into();
                if self.nether_go(host, b, run, Goal::exact(bx.feet), pos) != Steer::Stuck {
                    return true;
                }
            }
        }
        let arrows = host.hub.guest(b.conn).is_some_and(|g| {
            count(&g.inventory, "minecraft:bow") > 0 && count(&g.inventory, "minecraft:arrow") > 0
        });
        let charged = now.saturating_sub(run.last_swing) >= SWING;
        let list: Vec<Foe> = foes.iter().map(|f| f.2).collect();
        let stance = n::blaze_stance(health, charged, &list, arrows);
        if let Stance::Strike(i) | Stance::Block(i) | Stance::Close(i) | Stance::Shoot(i) = stance {
            let id = foes[i].0;
            let hp = self
                .mobs
                .iter()
                .find(|m| m.id == id)
                .map_or(0.0, |m| m.health);
            match run.blaze_engaged {
                Some((e, since, start)) if e == id => {
                    if hp < start {
                        run.blaze_engaged = Some((id, now, hp));
                    } else if now.saturating_sub(since) > 400 {
                        run.ignored.push((id, now + 1200));
                        run.blaze_engaged = None;
                    }
                }
                _ => run.blaze_engaged = Some((id, now, hp)),
            }
        }
        match stance {
            Stance::Strike(i) => {
                let (id, at, _) = foes[i];
                self.companion_shield(host, b, run, None, pos);
                b.yaw = c::yaw_toward(pos, at) as f32;
                b.pitch = look_pitch(from, at);
                self.companion_strike(host, b, id);
                run.last_swing = now;
                run.note = "fighting a blaze".into();
            }
            Stance::Block(i) => {
                let (_, at, f) = foes[i];
                self.companion_shield(host, b, run, Some(at), pos);
                // Close in on one near enough and not too high to reach.
                if f.reach < 7.0 && at[1] - pos[1] < 3.0 {
                    b.keys.forward = f.reach > 2.2;
                }
                run.note = "shield up: blazes".into();
            }
            Stance::Close(i) => {
                let (_, at, _) = foes[i];
                self.companion_shield(host, b, run, None, pos);
                let goal = Goal {
                    at: feet([at[0], at[1] - 0.9, at[2]]),
                    near: 2.0,
                    vertical: 2,
                };
                // The walker looks where it goes (it digs and builds).
                if self.nether_go(host, b, run, goal, pos) == Steer::Stuck {
                    b.walk.reset();
                }
                run.note = "closing in on a blaze".into();
            }
            Stance::Shoot(i) => {
                let (id, at, _) = foes[i];
                self.companion_shield(host, b, run, None, pos);
                if !self.companion_bow(host, b, id, pos) {
                    self.companion_shield(host, b, run, Some(at), pos);
                }
                run.note = "shooting a blaze".into();
            }
            Stance::Retreat => {
                // Out of their sight: behind a wall or a pillar it heals
                // (eating if it can), and the fight goes on one blaze at a
                // time when it comes out. On the way, the shield only while
                // one is about to shoot.
                let hungry = host.hub.guest(b.conn).is_some_and(|g| g.food < 20);
                if foes.iter().all(|f| !f.2.sees) {
                    self.companion_shield(host, b, run, None, pos);
                    b.keys = Default::default();
                    if hungry {
                        self.companion_start_eating(host, b);
                    }
                    run.note = "healing in cover".into();
                    return true;
                }
                let shooting = foes.iter().find(|f| f.2.sees && f.2.charging);
                let eyes: Vec<[f64; 3]> = foes.iter().map(|f| f.1).collect();
                if now >= run.cover_at + 10 || run.cover.is_some_and(|c| !self.hidden_at(c, &eyes))
                {
                    let found = self.cover_spot(pos, &eyes, &run.bad);
                    if found != run.cover
                        && let (Some(f), Some((_, since))) = (found, run.cover_best)
                    {
                        // Another spot: measured afresh, but the clock
                        // runs on (two spots in turn mustn't hold it).
                        run.cover_best = Some((c::horizontal(pos, at_block(f)), since));
                    }
                    run.cover = found;
                    run.cover_at = now;
                }
                if let Some((_, at, _)) = shooting
                    && b.eating.is_none()
                {
                    self.companion_shield(host, b, run, Some(*at), pos);
                } else {
                    self.companion_shield(host, b, run, None, pos);
                }
                run.note = "backing off to heal".into();
                if let Some(cv) = run.cover {
                    // A straight run there (cover_spot checked the line);
                    // no nearer after a second: that one is no good.
                    run.note = "into cover from the blazes".into();
                    let target = at_block(cv);
                    let d = c::horizontal(pos, target);
                    match run.cover_best {
                        Some((best, _)) if d < best - 0.3 => run.cover_best = Some((d, now)),
                        Some((_, since)) if now.saturating_sub(since) > 20 => {
                            run.bad.push(cv);
                            run.cover = None;
                            run.cover_best = None;
                        }
                        None => run.cover_best = Some((d, now)),
                        _ => {}
                    }
                    if d > 0.3 {
                        b.yaw = c::yaw_toward(pos, target) as f32;
                        b.pitch = 0.0;
                        b.keys.forward = true;
                        b.keys.sprint = !run.shield;
                        b.keys.jump = cv[1] > feet(pos)[1];
                    } else {
                        b.keys = Default::default();
                    }
                } else {
                    let (_, at, _) = shooting.copied().unwrap_or(foes[0]);
                    let away = [
                        pos[0] + (pos[0] - at[0]) * 2.0,
                        pos[1],
                        pos[2] + (pos[2] - at[2]) * 2.0,
                    ];
                    let _ = self.companion_steer(host, b, away, 0.5, false);
                }
            }
            Stance::Wait => return false,
        }
        true
    }

    /// The shield up toward `toward` (Some) or down (None).
    fn companion_shield(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        toward: Option<[f64; 3]>,
        pos: [f64; 3],
    ) {
        let has = host
            .hub
            .guest(b.conn)
            .is_some_and(|g| g.offhand.as_ref().is_some_and(|s| &*s.item == SHIELD));
        let using = host
            .play
            .get(&b.conn)
            .and_then(super::lan_actions::GuestPlay::using);
        match toward {
            Some(at) if has && b.eating.is_none() => {
                b.yaw = c::yaw_toward(pos, at) as f32;
                b.pitch = look_pitch(eye(pos), at);
                if using.is_none() {
                    if let Some(g) = host.hub.guest_mut(b.conn) {
                        (g.yaw, g.pitch) = (b.yaw, b.pitch);
                    }
                    let (yaw, pitch) = (b.yaw, b.pitch);
                    self.lan_guest_packet(
                        host,
                        b.conn,
                        S::UseItem(play::UseItem {
                            hand: 1,
                            sequence: 0,
                            yaw,
                            pitch,
                        }),
                    );
                }
                run.shield = true;
            }
            _ => {
                if run.shield {
                    run.shield = false;
                    if using.is_some_and(|(slot, _)| slot == super::shield::OFFHAND_SLOT) {
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
            }
        }
    }

    /// Whether a ray from `from` reaches `to` through no block.
    /// A block into the nearest water cell round its body (feet to a block
    /// over its head, one out, and below its feet) it can reach, against a
    /// solid neighbour: true while it walls one off (one every 4 ticks).
    fn companion_dry_shaft(&mut self, host: &mut LanHost, b: &mut Body, pos: [f64; 3]) -> bool {
        let here = feet(pos);
        let facts = self.res.facts();
        let wet = |p: [i32; 3]| facts.get(self.world.block(p[0], p[1], p[2])).fluid == Fluid::Water;
        let solid = |p: [i32; 3]| facts.get(self.world.block(p[0], p[1], p[2])).full_collision;
        let mut cells: Vec<[i32; 3]> = Vec::new();
        for dy in -2..=2 {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let q = [here[0] + dx, here[1] + dy, here[2] + dz];
                    // Not where its own body is.
                    let body = dx == 0 && dz == 0 && (0..=1).contains(&dy);
                    if !body && wet(q) {
                        cells.push(q);
                    }
                }
            }
        }
        if cells.is_empty() {
            return false;
        }
        let Some(slot) = host.hub.guest(b.conn).and_then(|g| {
            g.inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
        }) else {
            return false;
        };
        const SIDES: [([i32; 3], u8); 6] = [
            ([0, -1, 0], 1),
            ([0, 1, 0], 0),
            ([0, 0, 1], 2),
            ([0, 0, -1], 3),
            ([1, 0, 0], 4),
            ([-1, 0, 0], 5),
        ];
        let from = eye(pos);
        let pick = cells.iter().find_map(|&q| {
            SIDES.iter().find_map(|&(d, face)| {
                let n = [q[0] + d[0], q[1] + d[1], q[2] + d[2]];
                (solid(n) && super::companion_work::to_block(from, n) < 4.4).then_some((n, face))
            })
        });
        let Some((n, face)) = pick else {
            return false;
        };
        if b.ticks.is_multiple_of(4) {
            self.companion_stop_dig(host, b);
            b.keys = Default::default();
            self.companion_hold_slot(host, b, slot);
            self.companion_use_block(host, b, n, face, pos);
        }
        true
    }

    /// A spot 2 to 5 blocks from the spawner, near its height, off any edge,
    /// its opening (toward the spawner) clear and walls placeable round it:
    /// near where it stands, then needing few blocks.
    fn bunker_site(&self, spawner: [i32; 3], here: [i32; 3], avoid: &[[i32; 3]]) -> Option<Bunker> {
        let mut best: Option<(i32, i32, Bunker)> = None;
        for dx in -5..=5i32 {
            for dz in -5..=5i32 {
                let r = dx.abs().max(dz.abs());
                if !(2..=5).contains(&r) {
                    continue;
                }
                let (x, z) = (spawner[0] + dx, spawner[2] + dz);
                let Some(y) = self.cruise_stand(x, spawner[1], z) else {
                    continue;
                };
                let c = [x, y, z];
                if (y - spawner[1]).abs() > 2 || avoid.contains(&c) || self.on_edge(c) {
                    continue;
                }
                let front = if dx.abs() >= dz.abs() {
                    [-dx.signum(), 0]
                } else {
                    [0, -dz.signum()]
                };
                let open = |p: [i32; 3]| self.passable(p);
                if !open([x + front[0], y, z + front[1]])
                    || !open([x + front[0], y + 1, z + front[1]])
                {
                    continue;
                }
                let mut need = 0;
                let mut ok = true;
                for s in Bunker::sides(front) {
                    let low = [x + s[0], y, z + s[1]];
                    if open(low) {
                        need += 2;
                        ok &= self.floor([low[0], y - 1, low[2]]);
                    } else if open([low[0], y + 1, low[2]]) {
                        need += 1;
                    }
                }
                if open([x, y + 2, z]) {
                    need += 2;
                }
                if !ok {
                    continue;
                }
                let bx = Bunker {
                    spawner,
                    feet: c,
                    front,
                    built: false,
                    heal_only: false,
                };
                // Near where it is (each step there is under fire), then few
                // blocks.
                let walk = (x - here[0]).abs() + (z - here[2]).abs() + (y - here[1]).abs() * 2;
                let score = walk * 2 + need;
                if best
                    .as_ref()
                    .is_none_or(|(s0, r0, _)| (score, r) < (*s0, *r0))
                {
                    best = Some((score, r, bx));
                }
            }
        }
        best.map(|(_, _, bx)| bx)
    }

    /// The next block of the box: Some(true) while it builds (a block every
    /// 4 ticks), Some(false) when it stands, None without blocks.
    fn bunker_build(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        bx: Bunker,
        pos: [f64; 3],
    ) -> Option<bool> {
        let f = bx.feet;
        // (cell, block clicked, face)
        let mut todo: Vec<([i32; 3], [i32; 3], u8)> = Vec::new();
        for s in Bunker::sides(bx.front) {
            let low = [f[0] + s[0], f[1], f[2] + s[1]];
            let high = [low[0], f[1] + 1, low[2]];
            todo.push((low, [low[0], low[1] - 1, low[2]], 1));
            todo.push((high, low, 1));
        }
        // The roof, held by a third block on the first side wall.
        let s0 = Bunker::sides(bx.front)[0];
        let prop = [f[0] + s0[0], f[1] + 2, f[2] + s0[1]];
        todo.push((prop, [prop[0], prop[1] - 1, prop[2]], 1));
        let face = match s0 {
            [1, 0] => 4,
            [-1, 0] => 5,
            [0, 1] => 2,
            _ => 3,
        };
        todo.push(([f[0], f[1] + 2, f[2]], prop, face));
        let Some(&(_, against, face)) = todo
            .iter()
            .find(|(cell, against, _)| self.passable(*cell) && !self.passable(*against))
        else {
            return Some(false);
        };
        let slot = host.hub.guest(b.conn).and_then(|g| {
            g.inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
        })?;
        if b.ticks.is_multiple_of(4) {
            self.companion_hold_slot(host, b, slot);
            self.companion_use_block(host, b, against, face, pos);
        }
        Some(true)
    }

    /// Hurt in its box: it closes the open side (two blocks), eats and
    /// heals out of the blazes' sight (health comes back only above 17
    /// food, and fast on saturation), then opens it again. True while it
    /// takes the tick.
    fn companion_box_heal(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        bx: Bunker,
        health: f32,
        pos: [f64; 3],
    ) -> bool {
        let Some(g) = host.hub.guest(b.conn) else {
            return false;
        };
        let food = g.food;
        let beef = g.inventory.iter().flatten().any(|s| {
            self.res
                .items()
                .get(&s.item)
                .is_some_and(|i| i.food.is_some())
        });
        let blocks = g
            .inventory
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)));
        let f = bx.feet;
        let low = [f[0] + bx.front[0], f[1], f[2] + bx.front[1]];
        let high = [low[0], f[1] + 1, low[2]];
        if !run.sealed && health < 12.0 {
            run.sealed = true;
        }
        if run.sealed && health >= 19.0 && (food >= 18 || !beef) {
            run.sealed = false;
        }
        if run.sealed {
            self.companion_shield(host, b, run, None, pos);
            // Closed: one block at a time, the lower first (it holds the
            // upper).
            if let Some((cell, against)) = [(low, [low[0], low[1] - 1, low[2]]), (high, low)]
                .into_iter()
                .find(|(cell, _)| self.passable(*cell))
            {
                if self.passable(against) {
                    return false;
                }
                let Some(slot) = blocks else {
                    run.sealed = false;
                    return false;
                };
                if b.eating.is_none() && b.ticks.is_multiple_of(3) {
                    self.companion_hold_slot(host, b, slot);
                    b.yaw = c::yaw_toward(pos, at_block(cell)) as f32;
                    self.companion_use_block(host, b, against, 1, pos);
                }
                run.note = "closing its box to heal".into();
                return true;
            }
            if food < 20 && beef && b.eating.is_none() {
                self.companion_start_eating(host, b);
            }
            run.note = "healing in its closed box".into();
            return true;
        }
        // Healed: the open side open again, the upper block first.
        let face = match bx.front {
            [1, 0] => 4,
            [-1, 0] => 5,
            [0, 1] => 2,
            _ => 3,
        };
        if let Some(cell) = [high, low].into_iter().find(|c| !self.passable(*c))
            && self.companion_dig_step(host, b, cell, face).is_ok()
        {
            run.note = "opening its box".into();
            return true;
        }
        if bx.heal_only {
            run.bunker = None;
        }
        false
    }

    /// Whether a column next to `p` drops more than 3 blocks (or onto
    /// lava): a knock back there throws it off.
    fn on_edge(&self, p: [i32; 3]) -> bool {
        let facts = self.res.facts();
        (-1..=1).any(|dx| {
            (-1..=1).any(|dz| {
                let floor = (1..=4).find_map(|d| {
                    let f = facts.get(self.world.block(p[0] + dx, p[1] - d, p[2] + dz));
                    (f.full_collision || f.fluid != Fluid::None).then_some(f.fluid)
                });
                !matches!(floor, Some(Fluid::None | Fluid::Water))
            })
        })
    }

    /// The nearest spot within 6 blocks, near its height, with no edge
    /// beside it, that it can run straight to.
    fn solid_ground_near(&self, p: [i32; 3]) -> Option<[i32; 3]> {
        let pos = at_block(p);
        (1..=6).find_map(|r| {
            (-r..=r)
                .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
                .filter(|&(dx, dz): &(i32, i32)| dx.abs().max(dz.abs()) == r)
                .filter_map(|(dx, dz)| {
                    let (x, z) = (p[0] + dx, p[2] + dz);
                    let y = self.cruise_stand(x, p[1], z)?;
                    ((y - p[1]).abs() <= 1).then_some([x, y, z])
                })
                .find(|&c| !self.on_edge(c) && self.straight_run(pos, c))
        })
    }

    /// The pit's roof: a pillar of two beside its top on firm ground and a
    /// block from it over the pit, a block above its head when it looks
    /// out. True while it builds (a block every 4 ticks).
    fn pit_roof(&mut self, host: &mut LanHost, b: &mut Body, pit: Pit, pos: [f64; 3]) -> bool {
        let t = pit.look_feet();
        let roof = [t[0], t[1] + 2, t[2]];
        if !self.passable(roof) {
            return false;
        }
        let Some(slot) = host.hub.guest(b.conn).and_then(|g| {
            g.inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
        }) else {
            return false;
        };
        // (direction, face from the pillar's top toward the roof)
        let sides: [([i32; 2], u8); 4] = [([1, 0], 4), ([-1, 0], 5), ([0, 1], 2), ([0, -1], 3)];
        let Some((d, face)) = sides.into_iter().find(|(d, _)| {
            let ground = [t[0] + d[0], t[1], t[2] + d[1]];
            let low = [t[0] + d[0], t[1] + 1, t[2] + d[1]];
            self.floor(ground) || !self.passable(low)
        }) else {
            return false;
        };
        let low = [t[0] + d[0], t[1] + 1, t[2] + d[1]];
        let high = [t[0] + d[0], t[1] + 2, t[2] + d[1]];
        if !b.ticks.is_multiple_of(4) {
            return true;
        }
        self.companion_hold_slot(host, b, slot);
        if self.passable(low) {
            self.companion_use_block(host, b, [low[0], low[1] - 1, low[2]], 1, pos);
        } else if self.passable(high) {
            self.companion_use_block(host, b, low, 1, pos);
        } else {
            self.companion_use_block(host, b, high, face, pos);
        }
        true
    }

    /// No blaze at `eyes` sees a body standing at `c` (its head nor its
    /// middle).
    fn hidden_at(&self, c: [i32; 3], eyes: &[[f64; 3]]) -> bool {
        let x = f64::from(c[0]) + 0.5;
        let z = f64::from(c[2]) + 0.5;
        eyes.iter().all(|&e| {
            !self.in_sight(e, [x, f64::from(c[1]) + 1.5, z])
                && !self.in_sight(e, [x, f64::from(c[1]) + 0.5, z])
        })
    }

    /// Whether it can run straight from `pos` to the spot `c`: every column
    /// on the way has a safe floor at most a step up or down from the last.
    fn straight_run(&self, pos: [f64; 3], c: [i32; 3]) -> bool {
        let to = at_block(c);
        let d = c::horizontal(pos, to);
        let steps = (d / 0.4).ceil().max(1.0) as i32;
        let mut y = feet(pos)[1];
        for i in 1..=steps {
            let t = f64::from(i) / f64::from(steps);
            let x = (pos[0] + (to[0] - pos[0]) * t).floor() as i32;
            let z = (pos[2] + (to[2] - pos[2]) * t).floor() as i32;
            match self.cruise_stand(x, y, z) {
                Some(fy) if (fy - y).abs() <= 1 => y = fy,
                _ => return false,
            }
        }
        y == c[1]
    }

    /// The nearest safe spot within 8 blocks, near its height, that none of
    /// the blazes sees.
    fn cover_spot(&self, pos: [f64; 3], eyes: &[[f64; 3]], avoid: &[[i32; 3]]) -> Option<[i32; 3]> {
        let here = feet(pos);
        (1..=8).find_map(|r| {
            (-r..=r)
                .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
                .filter(|&(dx, dz): &(i32, i32)| dx.abs().max(dz.abs()) == r)
                .filter_map(|(dx, dz)| {
                    let (x, z) = (here[0] + dx, here[2] + dz);
                    let y = self.cruise_stand(x, here[1], z)?;
                    ((y - here[1]).abs() <= 2).then_some([x, y, z])
                })
                .filter(|c| !avoid.contains(c))
                .filter(|&c| self.straight_run(pos, c))
                .find(|&c| self.hidden_at(c, eyes))
        })
    }

    fn in_sight(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        let d = c::distance(from, to).max(1e-6);
        let dir = [
            (to[0] - from[0]) / d,
            (to[1] - from[1]) / d,
            (to[2] - from[2]) / d,
        ];
        self.clip_ray(from, dir, d).is_none()
    }

    // ---- the legs ----------------------------------------------------------------

    fn companion_nether_leg(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) {
        let now = b.ticks;
        let have = carried_now(self, host, b, run);
        // Out of time: the run ends on this leg, classified. (Its clock is
        // its own ticks: a dimension's loading under /tick sprint doesn't
        // count.)
        if now.saturating_sub(run.leg_start) > budget(run.leg)
            && !(run.leg == Leg::Enter && b.dim == Dim::Nether)
        {
            let why = format!("{} out of time", run.leg.id());
            self.companion_nether_end(b, run, why);
            return;
        }
        // A leg whose targets are met already is skipped.
        if n::leg_met(run.leg, &have, &run.targets) {
            self.companion_nether_next(b, run, now);
            return;
        }
        match run.leg {
            Leg::Enter => self.leg_enter(host, b, run, pos, now),
            Leg::Fortress => self.leg_fortress(host, b, run, pos, now),
            Leg::Blazes => self.leg_blazes(host, b, run, pos, now),
            Leg::Pearls => self.leg_pearls(host, b, run, pos, now),
            Leg::Home => self.leg_home(host, b, run, pos, now),
            Leg::Eyes => self.leg_eyes(host, b, run, &have),
            Leg::Triangulate => self.leg_triangulate(host, b, run, pos, now),
            Leg::Travel => self.leg_travel(host, b, run, pos, now),
            Leg::Room => self.leg_room(host, b, run, pos, now),
            Leg::Fill => self.leg_fill(host, b, run, pos, now),
            Leg::Done => {
                run.note = "waiting at the portal".into();
            }
        }
    }

    fn companion_nether_next(&mut self, b: &mut Body, run: &mut NetherRun, now: u64) {
        let done = run.leg;
        let ticks = now.saturating_sub(run.start);
        log::info!(
            "[companion] {} nether: {} done at {ticks}",
            b.file.name,
            done.id()
        );
        let split = match done {
            Leg::Enter => "nether",
            Leg::Fortress => "fortress",
            Leg::Blazes => "rods",
            Leg::Pearls => "pearls",
            Leg::Home => "overworld",
            Leg::Eyes => "eyes",
            Leg::Triangulate => "stronghold_located",
            Leg::Travel => "at_stronghold",
            Leg::Room => "portal_room",
            Leg::Fill => "portal_open",
            Leg::Done => "done",
        };
        if run.split(split, ticks) {
            self.companion_report(
                b,
                CompanionReport::Milestone {
                    name: split.into(),
                    detail: format!("{} game seconds", ticks / 20),
                },
            );
        }
        run.leg = done.next();
        run.leg_start = now;
        run.waypoint = None;
        run.scan = None;
        run.scanned = false;
        run.search = None;
        run.visited.clear();
        run.bad.clear();
        run.stuck = 0;
        b.walk.reset();
        if done == run.until || done == Leg::Fill {
            run.outcome = Some("done".into());
            run.note = "done".into();
        }
    }

    fn companion_nether_end(&mut self, b: &mut Body, run: &mut NetherRun, why: String) {
        log::info!("[companion] {} nether: stopped: {why}", b.file.name);
        self.companion_report(
            b,
            CompanionReport::SkillFailed {
                skill: c::Skill::Run,
                reason: why.clone(),
            },
        );
        run.outcome = Some(why);
        b.walk.reset();
    }

    /// Block state → its cue, once.
    fn cue_table(&mut self, run: &mut NetherRun) -> Arc<Vec<Cue>> {
        if let Some(t) = &run.cues {
            return t.clone();
        }
        let t: Arc<Vec<Cue>> = Arc::new(
            self.res
                .facts()
                .states
                .iter()
                .map(|f| n::cue(&f.name))
                .collect(),
        );
        run.cues = Some(t.clone());
        t
    }

    /// The remembered blocks of a cue within `radius`, nearest first.
    fn recall_cue(
        &mut self,
        b: &Body,
        run: &mut NetherRun,
        cue: Cue,
        around: [i32; 3],
        radius: i32,
    ) -> Vec<[i32; 3]> {
        let t = self.cue_table(run);
        b.senses
            .memory()
            .recall(|s| t.get(s as usize) == Some(&cue), around, radius)
    }

    fn leg_enter(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if b.dim == Dim::Nether {
            // It stands in the exit portal: remembered, both ways.
            let here = feet(pos);
            let portal = self.portal_cell_near(here).unwrap_or(here);
            b.senses.memory_mut().places.insert("portal".into(), portal);
            self.companion_nether_next(b, run, now);
            return;
        }
        let here = feet(pos);
        let known = b
            .senses
            .memory()
            .places
            .get("portal")
            .copied()
            .filter(|p| self.is_portal(*p));
        let Some(portal) = known.or_else(|| self.portal_cell_near(here)) else {
            run.note = "no lit portal near".into();
            if now.saturating_sub(run.leg_start) > 200 {
                self.companion_nether_end(b, run, "no lit portal".into());
            }
            return;
        };
        b.senses.memory_mut().places.insert("portal".into(), portal);
        run.note = if self.watched(b) {
            "into the portal".into()
        } else {
            "in the portal: watch me to come along".into()
        };
        if feet(pos) != portal {
            let _ = self.nether_go(host, b, run, Goal::exact(portal), pos);
        }
    }

    /// Walk toward `goal` (the walker that digs, bridges and pillars):
    /// Stuck also when 600 walking ticks got it no nearer.
    fn nether_go(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        goal: Goal,
        pos: [f64; 3],
    ) -> Steer {
        let f = feet(pos);
        let left = (f64::from(f[0] - goal.at[0]).hypot(f64::from(f[2] - goal.at[2])) - goal.near)
            .max(0.0)
            + f64::from(((f[1] - goal.at[1]).abs() - goal.vertical).max(0));
        match run.progress.as_mut() {
            Some((at, best, walks)) if *at == goal.at => {
                if left < *best - 1.5 {
                    *best = left;
                    *walks = 0;
                } else {
                    *walks += 1;
                    if *walks > 600 {
                        log::info!(
                            "[companion] {} nether: no nearer to {:?} in 600 ticks",
                            b.file.name,
                            goal.at
                        );
                        run.progress = None;
                        b.walk.reset();
                        return Steer::Stuck;
                    }
                }
            }
            _ => run.progress = Some((goal.at, left, 0)),
        }
        // Far: the cheap walk over the ground (game/companion_cruise.rs);
        // near, or where it found no way, the walker that digs and builds.
        let far = (left - 24.0).max(0.0);
        // Only to somewhere about its own height (not down to a room).
        let level = goal.vertical >= 64 || (goal.at[1] - f[1]).abs() <= 8;
        let steer = if far > 0.0 && level && b.ticks >= run.cruise_off_until {
            match self.companion_cruise(
                host,
                b,
                &mut run.cruise,
                [goal.at[0], goal.at[2]],
                goal.near + 24.0,
                pos,
            ) {
                Steer::Stuck => {
                    // Its own way for a stretch, then the cheap walk again.
                    run.cruise_off_until = b.ticks + 200;
                    b.walk.reset();
                    Steer::Moving
                }
                Steer::Arrived => self.companion_travel(host, b, goal, pos),
                Steer::Moving => {
                    b.walk.reset();
                    Steer::Moving
                }
            }
        } else {
            run.cruise.reset();
            self.companion_travel(host, b, goal, pos)
        };
        if steer != Steer::Moving {
            run.progress = None;
        }
        steer
    }

    /// A way round where the walker got no nearer: a waypoint off to one
    /// side first (True while it walks there).
    fn detour(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) -> bool {
        let Some(w) = run.waypoint else {
            return false;
        };
        run.note = format!("{} (a way round)", run.note);
        if self.nether_go(host, b, run, Goal::column(w, 4.0), pos) != Steer::Moving {
            run.waypoint = None;
        }
        true
    }

    fn is_portal(&self, p: [i32; 3]) -> bool {
        &*self
            .res
            .facts()
            .get(self.world.block(p[0], p[1], p[2]))
            .name
            == "minecraft:nether_portal"
    }

    /// The nearest nether portal block on a portal's bottom row within 6
    /// blocks (the one it stands by).
    fn portal_cell_near(&self, at: [i32; 3]) -> Option<[i32; 3]> {
        self.portal_cell_within(at, 6)
    }

    /// The nearest lit portal's bottom cell within `r` blocks across.
    fn portal_cell_within(&self, at: [i32; 3], r: i32) -> Option<[i32; 3]> {
        let mut best: Option<[i32; 3]> = None;
        for dx in -r..=r {
            for dy in -4..=4 {
                for dz in -r..=r {
                    let p = [at[0] + dx, at[1] + dy, at[2] + dz];
                    if self.is_portal(p)
                        && !self.is_portal([p[0], p[1] - 1, p[2]])
                        && best.is_none_or(|q| d2(p, at) < d2(q, at))
                    {
                        best = Some(p);
                    }
                }
            }
        }
        best
    }

    fn leg_fortress(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if b.dim != Dim::Nether {
            return self.back_to_nether(host, b, run, pos);
        }
        let here = feet(pos);
        if b.senses.level.knows_structures() && run.fortress.is_none() {
            run.fortress = self.companion_structure(b, "fortress", pos);
        }
        if run.fortress.is_none() && now.saturating_sub(run.last_cue) >= CUE_EVERY {
            run.last_cue = now;
            // Nether bricks it saw, or a blaze in view.
            let bricks = self.recall_cue(b, run, Cue::Fortress, here, 200);
            if let Some(p) = bricks.first() {
                run.fortress = Some(*p);
                run.waypoint = None;
                b.senses.memory_mut().learn_structure("fortress", *p, false);
            } else if let Some(at) = self
                .mobs
                .iter()
                .filter(|m| m.alive() && &*m.kind.id == "minecraft:blaze")
                .map(mob_at)
                .find(|at| c::distance(*at, pos) < 64.0 && self.in_sight(eye(pos), *at))
            {
                run.fortress = Some(feet(at));
                run.waypoint = None;
            }
            // A bastion's region holds no fortress.
            if let Some(p) = self
                .recall_cue(b, run, Cue::Bastion, here, 200)
                .first()
                .copied()
                && let Some(s) = run.search.as_mut()
            {
                s.rule_out(p[0], p[2]);
            }
        }
        if let Some(f) = run.fortress {
            // There: within sight of it (its bricks in view), or close.
            let close = c::horizontal(pos, at_block(f)) < 40.0;
            let seen = !self.recall_cue(b, run, Cue::Fortress, here, 48).is_empty();
            if close || seen && !b.senses.level.knows_structures() {
                self.companion_nether_next(b, run, now);
                run.fortress = Some(f);
                return;
            }
            run.note = format!(
                "to the fortress, {:.0} blocks",
                c::horizontal(pos, at_block(f))
            );
            if self.detour(host, b, run, pos) {
                return;
            }
            if self.nether_go(host, b, run, Goal::column([f[0], 70, f[2]], 24.0), pos)
                == Steer::Stuck
            {
                run.stuck += 1;
                b.walk.reset();
                if run.stuck >= 2 {
                    run.waypoint = Some(n::detour(here, [f[0], f[2]], run.stuck, 24));
                }
            }
            return;
        }
        run.note = "exploring".into();
        self.explore(host, b, run, pos, 70);
    }

    /// The Fair search's next leg of the spiral around the portal.
    fn explore(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        y: i32,
    ) {
        let here = feet(pos);
        let origin = b
            .senses
            .memory()
            .places
            .get("portal")
            .copied()
            .unwrap_or(here);
        let search = run
            .search
            .get_or_insert_with(|| Search::new([origin[0], origin[2]], SEARCH_STEP));
        if run
            .waypoint
            .is_none_or(|w| search.ruled_out.contains(&n::region_of(w[0], w[2])))
        {
            match search.next(SEARCH_LIMIT) {
                Some(p) => run.waypoint = Some([p[0], y, p[1]]),
                None => {
                    let why = format!("{}: searched out", run.leg.id());
                    self.companion_nether_end(b, run, why);
                    return;
                }
            }
            b.walk.reset();
        }
        let Some(w) = run.waypoint else { return };
        match self.nether_go(host, b, run, Goal::column(w, 8.0), pos) {
            Steer::Arrived => run.waypoint = None,
            Steer::Stuck => {
                log::info!(
                    "[companion] {} nether: waypoint {w:?} out of reach",
                    b.file.name
                );
                if let Some(s) = run.search.as_mut() {
                    s.skipped += 1;
                }
                run.waypoint = None;
            }
            Steer::Moving => {}
        }
    }

    /// Died there and came back in the Overworld: the portal again.
    fn back_to_nether(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) {
        let portal = b
            .senses
            .memory_of(Dim::Overworld.id())
            .and_then(|m| m.places.get("portal").copied());
        match portal {
            Some(p) => {
                run.note = "back to the portal".into();
                let _ = self.nether_go(host, b, run, Goal::exact(p), pos);
            }
            None => {
                let why = format!("{}: in the overworld, no portal known", run.leg.id());
                self.companion_nether_end(b, run, why);
            }
        }
    }

    fn leg_blazes(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        _now: u64,
    ) {
        if b.dim != Dim::Nether {
            return self.back_to_nether(host, b, run, pos);
        }
        let here = feet(pos);
        let fortress = run.fortress.unwrap_or(here);
        if b.ticks.is_multiple_of(40) {
            run.visited.push(here);
            if run.visited.len() > 256 {
                run.visited.remove(0);
            }
        }
        // The spawner: seen, or through walls (All-knowing, Sharp senses).
        if run.spawner.is_none() {
            let wanted = self.states_where(|n| n == "minecraft:spawner");
            let centre = if run.scanned { here } else { fortress };
            if let Some(found) =
                self.companion_known(b, &wanted, centre, 112, &mut run.scan, 6 * 4096)
            {
                run.scan = None;
                run.scanned = true;
                log::info!(
                    "[companion] {} nether: spawners round {centre:?}: {:?}",
                    b.file.name,
                    &found[..found.len().min(4)]
                );
                // Only this fortress's (what it remembers reaches further).
                run.spawner = found.into_iter().find(|p| {
                    !run.bad.contains(p)
                        && (p[0] - centre[0]).abs().max((p[2] - centre[2]).abs()) <= 112
                });
            }
        }
        // Rods on the floor: picked up when no blaze is close.
        let blaze_close = self.mobs.iter().any(|m| {
            m.alive() && &*m.kind.id == "minecraft:blaze" && c::distance(mob_at(m), pos) < 10.0
        });
        if !blaze_close && let Some(at) = self.dropped_near(ROD, pos, 14.0) {
            run.note = "picking up rods".into();
            let _ = self.nether_go(host, b, run, Goal::near(feet(at), 0.5), pos);
            return;
        }
        let Some(spawner) = run.spawner else {
            // Through the fortress: its corridors' far ends.
            if run.waypoint.is_none() {
                let seen = self.recall_cue(b, run, Cue::Fortress, fortress, 160);
                run.waypoint = n::frontier(&seen, &run.visited, here, 20);
                if run.waypoint.is_none() {
                    run.note = "looking round the fortress".into();
                    run.scanned = false;
                    return self.explore(host, b, run, pos, here[1]);
                }
            }
            let Some(w) = run.waypoint else { return };
            run.note = "searching the fortress for a spawner".into();
            if self.nether_go(host, b, run, Goal::near(w, 3.0), pos) != Steer::Moving {
                run.visited.push(w);
                run.waypoint = None;
                // Look again from here.
                run.scanned = false;
            }
            return;
        };
        // A box by the spawner (walls on three sides and a roof, open toward
        // it): there, built, it waits; the fights come to its opening.
        let near_spawner = c::horizontal(pos, at_block(spawner)) < 10.0;
        if near_spawner && run.bunker.is_none_or(|bx| bx.spawner != spawner) {
            run.bunker = self.bunker_site(spawner, here, &run.bad);
            log::info!(
                "[companion] {} nether: box by the spawner {:?}",
                b.file.name,
                run.bunker
            );
        }
        if let Some(mut bx) = run.bunker {
            if here != bx.feet {
                run.note = "to a spot for a box by the spawner".into();
                bx.built = false;
                run.bunker = Some(bx);
                if self.nether_go(host, b, run, Goal::exact(bx.feet), pos) == Steer::Stuck {
                    run.bad.push(bx.feet);
                    run.bunker = None;
                }
                return;
            }
            b.walk.reset();
            if !bx.built {
                run.note = "walling itself in by the spawner".into();
                match self.bunker_build(host, b, bx, pos) {
                    Some(true) => return,
                    Some(false) => bx.built = true,
                    None => {
                        // Out of blocks: as it is.
                        bx.built = true;
                    }
                }
                run.bunker = Some(bx);
            }
            run.note = "waiting in the box".into();
            let front = at_block([
                bx.feet[0] + bx.front[0],
                bx.feet[1],
                bx.feet[2] + bx.front[1],
            ]);
            b.yaw = c::yaw_toward(pos, front) as f32;
            b.pitch = -15.0;
            return;
        }
        // At the spawner: it waits there, the fights come to it.
        let goal = Goal {
            at: spawner,
            near: 2.5,
            vertical: 2,
        };
        if goal.reached(here) {
            run.note = "camping the spawner".into();
            b.walk.reset();
            // Look round slowly (blazes come from every side).
            b.yaw += 3.0;
            b.pitch = -10.0;
            return;
        }
        run.note = "to the spawner".into();
        if self.nether_go(host, b, run, goal, pos) == Steer::Stuck {
            run.stuck += 1;
            if run.stuck > 3 {
                run.bad.push(spawner);
                run.spawner = None;
                run.stuck = 0;
            }
        }
    }

    /// The nearest dropped `item` within `radius` it could pick up.
    fn dropped_near(&self, item: &str, pos: [f64; 3], radius: f64) -> Option<[f64; 3]> {
        self.dropped
            .iter()
            .filter(|d| &*d.stack.item == item && d.pickup_delay < 20)
            .map(|d| [d.position.x, d.position.y, d.position.z])
            .filter(|at| c::distance(*at, pos) <= radius && (at[1] - pos[1]).abs() < 4.0)
            .min_by(|x, y| c::distance(*x, pos).total_cmp(&c::distance(*y, pos)))
    }

    // ---- pearls ------------------------------------------------------------------

    fn leg_pearls(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        let here = feet(pos);
        let night = (13000..23000).contains(&self.world.day_time.rem_euclid(24000));
        let in_forest = self.companion_biome(pos) == "minecraft:warped_forest";
        // Near the middle it aimed for counts too (the forest's edge
        // wanders; it should not walk in and out of it).
        let near_aim = run
            .forest
            .is_some_and(|f| c::horizontal(pos, at_block(f)) < 12.0);
        let hunting_ground = match b.dim {
            Dim::Nether => in_forest || near_aim,
            Dim::Overworld => night,
            Dim::End => true,
        };
        // Pearls lying near (a kill outside the pit).
        if run.pit.is_none()
            && let Some(at) = self.dropped_near(PEARL, pos, 12.0)
        {
            run.note = "picking up pearls".into();
            let _ = self.nether_go(host, b, run, Goal::near(feet(at), 0.5), pos);
            return;
        }
        // Endermen are few in the Nether (a warped forest's are shared with
        // a crowd of other mobs under the cap): after a while with few
        // pearls, an Overworld night instead, by the portal it came in.
        if b.dim == Dim::Nether
            && run.pit.is_none()
            && now.saturating_sub(run.leg_start) > OVERWORLD_PEARLS_AFTER
            && carried_now(self, host, b, run).pearls_worth() * 3 < run.targets.pearls
        {
            self.leg_home(host, b, run, pos, now);
            run.note = format!("{} (for endermen at night)", run.note);
            return;
        }
        // An angry enderman that wouldn't come to the pit: after it, the
        // shield toward it between blows.
        if run.pit.is_none()
            && let Some(id) = run.chase
        {
            let foe = self
                .mobs
                .iter()
                .find(|m| m.id == id && m.alive())
                .map(|m| (mob_at(m), box_distance(eye(pos), m.bounds())));
            match foe {
                Some((at, reach)) if c::distance(at, pos) < 40.0 => {
                    run.note = "after an angry enderman".into();
                    let charged = now.saturating_sub(run.last_swing) >= SWING;
                    if reach <= n::REACH && charged {
                        self.companion_shield(host, b, run, None, pos);
                        let legs = [at[0], at[1] + 0.3, at[2]];
                        b.yaw = c::yaw_toward(pos, legs) as f32;
                        b.pitch = look_pitch(eye(pos), legs);
                        self.companion_strike(host, b, id);
                        run.last_swing = now;
                    } else if reach <= n::REACH {
                        self.companion_shield(host, b, run, Some([at[0], at[1] + 1.5, at[2]]), pos);
                    } else {
                        self.companion_shield(host, b, run, None, pos);
                        if self.nether_go(host, b, run, Goal::near(feet(at), 2.0), pos)
                            == Steer::Stuck
                        {
                            run.chase = None;
                        }
                    }
                    return;
                }
                _ => run.chase = None,
            }
        }
        // Off to firmer ground or a quieter spot first (kept to even where
        // the forest's edge wanders: a goal that changes every few steps
        // holds the walker on the spot).
        if run.pit.is_none()
            && let Some(w) = run.pit_spot
        {
            run.note = "to a spot for a pit".into();
            if self.nether_go(host, b, run, Goal::near(w, 1.5), pos) != Steer::Moving {
                run.pit_spot = None;
            }
            return;
        }
        if hunting_ground {
            match self.pit_site(here) {
                Some(pit) => {
                    run.firm_tries = 0;
                    run.pit = Some(PitFight {
                        pit,
                        step: PitStep::Dig,
                        since: b.ticks,
                        quiet: 0,
                        jumped: false,
                    });
                    b.walk.reset();
                }
                None => {
                    // Firm ground a few blocks off; after a few tries there,
                    // further (a whole patch may be no good).
                    run.note = "looking for firm ground".into();
                    run.firm_tries += 1;
                    let far = if run.firm_tries > 4 { 16 } else { 5 };
                    if run.firm_tries > 4 {
                        run.firm_tries = 0;
                    }
                    let k = b.next_int(4);
                    let d = [[far, 0], [-far, 0], [0, far], [0, -far]][k as usize];
                    run.pit_spot = Some([here[0] + d[0], here[1], here[2] + d[1]]);
                }
            }
            return;
        }
        if b.dim == Dim::Overworld {
            run.note = "waiting for the night".into();
            return;
        }
        // To a warped forest.
        if run.forest.is_none() && now.saturating_sub(run.last_cue) >= CUE_EVERY {
            run.last_cue = now;
            run.forest = if b.senses.level == PerceptionLevel::AllKnowing {
                // The nearest it hasn't given up on (searched from a little
                // way off when the nearest is one).
                let given_up = |p: [i32; 3]| {
                    run.bad
                        .iter()
                        .any(|q| (q[0] - p[0]).abs() + (q[2] - p[2]).abs() < 48)
                };
                [[0, 0], [160, 0], [-160, 0], [0, 160], [0, -160]]
                    .iter()
                    .filter_map(|d| {
                        self.locate_biome(
                            "minecraft:warped_forest",
                            [here[0] + d[0], here[1], here[2] + d[1]],
                        )
                    })
                    .find(|&p| !given_up(p))
            } else {
                self.recall_cue(b, run, Cue::WarpedForest, here, 220)
                    .into_iter()
                    .find(|p| !run.bad.contains(p))
            }
            // Its nearest edge: on, 20 blocks into it.
            .map(|f| {
                let (dx, dz) = (f64::from(f[0] - here[0]), f64::from(f[2] - here[2]));
                let len = dx.hypot(dz).max(1.0);
                [
                    f[0] + (dx / len * 20.0) as i32,
                    f[1],
                    f[2] + (dz / len * 20.0) as i32,
                ]
            });
            if run.forest.is_some() {
                run.waypoint = None;
                b.walk.reset();
            }
        }
        match run.forest {
            Some(f) => {
                run.note = "to a warped forest".into();
                match self.nether_go(host, b, run, Goal::column(f, 4.0), pos) {
                    Steer::Moving => {}
                    Steer::Arrived => {
                        // There but not in it (an edge): a little further.
                        run.forest = Some([f[0] + 8, f[1], f[2] + 8]);
                    }
                    Steer::Stuck => {
                        run.bad.push(f);
                        run.forest = None;
                    }
                }
            }
            None => {
                run.note = "looking for a warped forest".into();
                self.explore(host, b, run, pos, 70);
            }
        }
    }

    /// `/locate biome`: the nearest block column of a biome (All-knowing).
    fn locate_biome(&self, biome: &str, from: [i32; 3]) -> Option<[i32; 3]> {
        let allowed = |name: &str| name == biome;
        let search = horizon_world::worldgen::generator::BiomeSearch {
            allowed: &allowed,
            origin: from,
            radius: 1600,
            horizontal: 32,
            vertical: 64,
            min_y: self.dim.min_y(),
            max_y: self.dim.min_y() + self.dim.logical_height() - 1,
        };
        self.end
            .locator()
            .and_then(|t| t.find_closest_biome(&search))
            .map(|(_, p)| p)
    }

    /// A pit where it stands: two diggable blocks under it, a solid floor
    /// below them, no liquid beside any.
    fn pit_site(&self, here: [i32; 3]) -> Option<Pit> {
        let facts = self.res.facts();
        let pit = Pit::at(here);
        let ok_dig = |p: [i32; 3]| {
            let f = facts.get(self.world.block(p[0], p[1], p[2]));
            f.full_collision && f.hardness >= 0.0 && f.hardness < 3.0 && !f.block_entity
        };
        let floor = pit.floor();
        let ff = facts.get(self.world.block(floor[0], floor[1], floor[2]));
        if !pit.dig().iter().all(|p| ok_dig(*p)) || !ff.full_collision {
            return None;
        }
        let wet = |p: [i32; 3]| facts.get(self.world.block(p[0], p[1], p[2])).fluid != Fluid::None;
        for c in pit.dig() {
            for d in [[1, 0], [-1, 0], [0, 1], [0, -1]] {
                if wet([c[0] + d[0], c[1], c[2] + d[1]]) {
                    return None;
                }
            }
        }
        Some(pit)
    }

    /// The pit fight's tick.
    fn companion_pit(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
    ) {
        let Some(mut fight) = run.pit.take() else {
            return;
        };
        if self.pit_tick(host, b, run, &mut fight, pos) {
            run.pit = Some(fight);
        } else {
            self.companion_stop_dig(host, b);
            b.walk.reset();
        }
    }

    /// One tick in the pit; false when it leaves it (a new one elsewhere).
    fn pit_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        fight: &mut PitFight,
        pos: [f64; 3],
    ) -> bool {
        let f = feet(pos);
        let pit = fight.pit;
        let from = eye(pos);
        let on_ground = host.hub.guest(b.conn).is_some_and(|g| g.on_ground);
        // Endermen about: angry ones (after it), and the rest in view.
        let mut angry: Vec<(u32, [f64; 3], f64)> = Vec::new();
        let mut calm: Vec<(u32, [f64; 3], f64)> = Vec::new();
        for m in self
            .mobs
            .iter()
            .filter(|m| m.alive() && &*m.kind.id == "minecraft:enderman")
        {
            let at = mob_at(m);
            let d = c::distance(at, pos);
            if d > 64.0 {
                continue;
            }
            if m.ai.target == Some(Who::Player) {
                angry.push((m.id, at, d));
            } else {
                calm.push((m.id, at, d));
            }
        }
        angry.sort_by(|a, b| a.2.total_cmp(&b.2));
        calm.sort_by(|a, b| a.2.total_cmp(&b.2));
        let elapsed = b.ticks.saturating_sub(fight.since);
        // Anything else at it in the pit (a zombie, a spider on the rim, a
        // piglin): struck back when the blow is charged, shield up between.
        let other = self
            .mobs
            .iter()
            .filter(|m| m.alive() && threat(m) && &*m.kind.id != "minecraft:enderman")
            .map(|m| (m.id, mob_at(m), box_distance(from, m.bounds())))
            .filter(|o| o.2 <= n::REACH)
            .min_by(|x, y| x.2.total_cmp(&y.2));
        if let Some((id, at, _)) = other {
            let centre = [at[0], at[1] + 0.8, at[2]];
            run.note = "fighting off a mob from the pit".into();
            if b.ticks.saturating_sub(run.last_swing) >= SWING {
                self.companion_shield(host, b, run, None, pos);
                b.yaw = c::yaw_toward(pos, centre) as f32;
                b.pitch = look_pitch(from, centre);
                self.companion_strike(host, b, id);
                run.last_swing = b.ticks;
            } else {
                self.companion_shield(host, b, run, Some(centre), pos);
            }
            return true;
        }
        self.companion_shield(host, b, run, None, pos);
        let set = |fight: &mut PitFight, step: PitStep, ticks: u64| {
            fight.step = step;
            fight.since = ticks;
            fight.jumped = false;
        };
        match fight.step {
            PitStep::Dig => {
                run.note = "digging a pit".into();
                // The block under its feet, until it stands at the bottom.
                let below = [f[0], f[1] - 1, f[2]];
                if f[1] <= pit.fight_feet()[1] && on_ground {
                    self.companion_stop_dig(host, b);
                    set(fight, PitStep::Climb, b.ticks);
                    return true;
                }
                if elapsed > 400 || f[0] != pit.top[0] || f[2] != pit.top[2] {
                    // Pushed off, or it won't dig: elsewhere.
                    run.pit_spot = Some([f[0] + 5, f[1], f[2] - 4]);
                    return false;
                }
                if !self.passable(below) && self.companion_dig_step(host, b, below, 1).is_err() {
                    run.pit_spot = Some([f[0] + 5, f[1], f[2] - 4]);
                    return false;
                }
                true
            }
            PitStep::Climb => {
                run.note = "up to look out".into();
                if f[1] >= pit.look_feet()[1] && on_ground {
                    set(fight, PitStep::Look, b.ticks);
                    return true;
                }
                if elapsed > 200 {
                    // It can't climb (no blocks?): fight from the bottom.
                    set(fight, PitStep::Fight, b.ticks);
                    return true;
                }
                // Jump; at the top of the jump, a block where its feet were.
                let slot = host.hub.guest(b.conn).and_then(|g| {
                    g.inventory
                        .iter()
                        .position(|s| s.as_ref().is_some_and(|s| THROWAWAY.contains(&&*s.item)))
                });
                let Some(slot) = slot else {
                    // No block to climb on: out by the walker (it digs a
                    // stair) to a new pit a little way off, whose digging
                    // gives blocks again.
                    let k = b.next_int(4);
                    let d = [[6, 0], [-6, 0], [0, 6], [0, -6]][k as usize];
                    run.pit_spot = Some([f[0] + d[0], pit.look_feet()[1], f[2] + d[1]]);
                    return false;
                };
                b.pitch = 90.0;
                if on_ground && !fight.jumped {
                    b.keys.jump = true;
                    fight.jumped = true;
                    return true;
                }
                let bottom = pit.bottom();
                if pos[1] >= f64::from(bottom[1]) + 1.0 && !self.floor(bottom) {
                    self.companion_hold_slot(host, b, slot);
                    self.companion_use_block(
                        host,
                        b,
                        [bottom[0], bottom[1] - 1, bottom[2]],
                        1,
                        pos,
                    );
                } else if on_ground && fight.jumped {
                    fight.jumped = false;
                }
                true
            }
            PitStep::Look => {
                // Pearls lying close: out to them (one block up), then back.
                if angry.iter().all(|a| a.2 > 16.0)
                    && let Some(at) = self.dropped_near(PEARL, pos, 10.0)
                    && c::horizontal(at, pos) > 1.2
                {
                    run.note = "picking up a pearl".into();
                    let _ = self.nether_go(host, b, run, Goal::near(feet(at), 0.5), pos);
                    return true;
                }
                if f != pit.look_feet() {
                    // Out of the pit (a pickup): back into it.
                    run.note = "back to the pit".into();
                    return self.nether_go(host, b, run, Goal::exact(pit.look_feet()), pos)
                        != Steer::Stuck;
                }
                b.walk.reset();
                // A roof over its head a block up (a pillar beside it to
                // hold it): no enderman fits under (2.9 high), and arrows
                // from above stop.
                if self.pit_roof(host, b, pit, pos) {
                    run.note = "roofing the pit".into();
                    return true;
                }
                // One coming at it: down.
                if angry.first().is_some_and(|a| a.2 < 24.0)
                    || calm.first().is_some_and(|a| a.2 < 3.0)
                {
                    set(fight, PitStep::Drop, b.ticks);
                    return true;
                }
                // Stare at the nearest in view until it turns.
                let target = calm
                    .iter()
                    .filter(|e| e.2 < 40.0)
                    .find(|e| self.in_sight(from, [e.1[0], e.1[1] + 2.55, e.1[2]]))
                    .copied();
                match target {
                    Some((_, at, _)) => {
                        fight.quiet = 0;
                        let eye_at = [at[0], at[1] + 2.55, at[2]];
                        b.yaw = c::yaw_toward(pos, eye_at) as f32;
                        b.pitch = look_pitch(from, eye_at);
                        run.note = "staring at an enderman".into();
                    }
                    None => {
                        fight.quiet += 1;
                        b.yaw += 4.0;
                        b.pitch = 0.0;
                        run.note = "looking for endermen".into();
                        // None about for a minute: a pit elsewhere.
                        if fight.quiet > 1200 && angry.is_empty() {
                            let h = f64::from(b.next_int(360)).to_radians();
                            run.pit_spot = Some([
                                f[0] + (h.cos() * 24.0) as i32,
                                f[1] + 1,
                                f[2] + (h.sin() * 24.0) as i32,
                            ]);
                            return false;
                        }
                    }
                }
                true
            }
            PitStep::Drop => {
                run.note = "down into the pit".into();
                let bottom = pit.bottom();
                if f[1] <= pit.fight_feet()[1] || elapsed > 200 {
                    self.companion_stop_dig(host, b);
                    set(fight, PitStep::Fight, b.ticks);
                    return true;
                }
                if !self.passable(bottom) {
                    let _ = self.companion_dig_step(host, b, bottom, 1);
                }
                true
            }
            PitStep::Fight => {
                if f[1] > pit.fight_feet()[1] + 1 {
                    // Not in the pit any more.
                    return false;
                }
                // The nearest angry one in reach: a blow at its legs (never
                // its eyes: that close a stare sends it away).
                let target = angry
                    .iter()
                    .copied()
                    .chain(calm.iter().copied().filter(|e| e.2 < 3.5))
                    .find(|e| e.2 < 4.5);
                match target {
                    Some((id, at, _)) => {
                        let legs = [at[0], at[1] + 0.3, at[2]];
                        b.yaw = c::yaw_toward(pos, legs) as f32;
                        b.pitch = look_pitch(from, legs);
                        run.note = "fighting an enderman".into();
                        if b.ticks.saturating_sub(run.last_swing) >= SWING {
                            self.companion_strike(host, b, id);
                            run.last_swing = b.ticks;
                        }
                        fight.since = b.ticks;
                    }
                    None => {
                        run.note = "waiting in the pit".into();
                        b.yaw += 2.0;
                        b.pitch = -20.0;
                        // Quiet a while, or the angry ones won't come: up
                        // to look out again.
                        // One angry that won't come down to it (stuck on
                        // the rim's far side): out after it.
                        if elapsed > 400
                            && let Some(&(id, _, _)) = angry.first()
                        {
                            run.chase = Some(id);
                            return false;
                        }
                        if elapsed > 60 && angry.iter().all(|a| a.2 > 16.0) || elapsed > 400 {
                            set(fight, PitStep::Climb, b.ticks);
                        }
                    }
                }
                true
            }
        }
    }

    // ---- home ---------------------------------------------------------------------

    fn leg_home(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if b.dim == Dim::Overworld {
            self.companion_nether_next(b, run, now);
            return;
        }
        let portal = b
            .senses
            .memory_of(Dim::Nether.id())
            .and_then(|m| m.places.get("portal").copied());
        let Some(portal) = portal else {
            self.companion_nether_end(b, run, "home: no portal remembered".into());
            return;
        };
        if !self.is_portal(portal) && c::horizontal(pos, at_block(portal)) < 8.0 {
            // Put out (a ghast): lit again.
            self.relight_portal(host, b, run, portal, pos);
            return;
        }
        run.note = if self.watched(b) {
            "to the portal".into()
        } else {
            "to the portal (watch me to come along)".into()
        };
        if feet(pos) != portal {
            let _ = self.nether_go(host, b, run, Goal::exact(portal), pos);
        }
    }

    /// Flint and steel on the frame's bottom, inside.
    fn relight_portal(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        portal: [i32; 3],
        pos: [f64; 3],
    ) {
        if !self.companion_hold(host, b, "minecraft:flint_and_steel") {
            self.companion_nether_end(b, run, "home: portal out, no flint and steel".into());
            return;
        }
        run.note = "lighting the portal again".into();
        let floor = [portal[0], portal[1] - 1, portal[2]];
        if super::companion_work::to_block(eye(pos), floor) > 4.0 {
            let _ = self.nether_go(host, b, run, Goal::near(portal, 2.0), pos);
            return;
        }
        self.companion_use_block(host, b, floor, 1, pos);
    }

    // ---- eyes ---------------------------------------------------------------------

    fn leg_eyes(&mut self, host: &mut LanHost, b: &mut Body, run: &mut NetherRun, have: &Carried) {
        if !b.work.tasks.is_empty() {
            run.note = "crafting".into();
            return;
        }
        let want = run.targets.eyes.saturating_sub(have.filled);
        let can = have.pearls.min(have.powder + have.rods * 2);
        let make = want.saturating_sub(have.eyes).min(can);
        if make == 0 {
            if have.eyes + have.filled == 0 {
                self.companion_nether_end(b, run, "eyes: nothing to make them from".into());
                return;
            }
            // As many as it could: on with those.
            let now = b.ticks;
            run.targets.eyes = have.eyes + have.filled;
            self.companion_nether_next(b, run, now);
            return;
        }
        let command = if have.powder < make {
            run.note = "crafting blaze powder".into();
            c::CompanionCommand::Craft {
                item: POWDER.into(),
                count: (make - have.powder).div_ceil(2) * 2,
                gather: false,
            }
        } else {
            run.note = "crafting eyes of ender".into();
            c::CompanionCommand::Craft {
                item: EYE.into(),
                count: make,
                gather: false,
            }
        };
        self.companion_start_work(host, b, command);
    }

    // ---- the stronghold -------------------------------------------------------------

    fn leg_triangulate(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if b.dim != Dim::Overworld {
            self.companion_nether_end(b, run, "triangulate: not in the overworld".into());
            return;
        }
        if b.senses.level.knows_structures() {
            if let Some(s) = self.companion_structure(b, "stronghold", pos) {
                self.companion_nether_next(b, run, now);
                run.stronghold = Some([s[0], s[2]]);
            }
            return;
        }
        // An eye in flight: its bearing, then where it lands.
        if let Some(flight) = run.flight.as_mut() {
            let eyes = self.ender_eyes_in_flight();
            match eyes.get(flight.before).copied() {
                Some(e) => {
                    flight.last = Some(e);
                    if flight.dir.is_none()
                        && b.ticks.saturating_sub(flight.at) >= 3
                        && let Some(d) = n::bearing(flight.from, [e[0], e[2]])
                    {
                        flight.dir = Some(d);
                    }
                    run.note = "watching the eye".into();
                    return;
                }
                None if b.ticks.saturating_sub(flight.at) < 5 => return,
                None => {
                    // It came down (or broke): the throw is read.
                    if let Some(flight) = run.flight.take() {
                        if let Some(dir) = flight.dir {
                            run.throws.push(Throw {
                                from: flight.from,
                                dir,
                            });
                            self.companion_report(
                                b,
                                CompanionReport::Milestone {
                                    name: format!("throw {}", run.throws.len()),
                                    detail: format!(
                                        "bearing {:.2} degrees",
                                        (-dir[0]).atan2(dir[1]).to_degrees()
                                    ),
                                },
                            );
                        }
                        if let Some(at) = flight.last {
                            run.fetch = Some((at, b.ticks + 200));
                        }
                    }
                }
            }
        }
        // The eye where it dropped.
        if let Some((at, until)) = run.fetch {
            match self.dropped_near(EYE, at, 6.0) {
                _ if b.ticks > until => run.fetch = None,
                // Still falling: a moment.
                None if b.ticks + 180 < until => return,
                None => run.fetch = None,
                Some(item) => {
                    run.note = "picking up the eye".into();
                    if self.nether_go(host, b, run, Goal::near(feet(item), 0.5), pos)
                        == Steer::Stuck
                    {
                        run.fetch = None;
                    }
                    return;
                }
            }
        }
        // Where the lines cross.
        let k = run.throws.len();
        if k >= 2
            && let Some(at) = n::intersect(&run.throws[k - 2], &run.throws[k - 1])
        {
            let target = [at[0].round() as i32, at[1].round() as i32];
            log::info!(
                "[companion] {} stronghold at {target:?} (±{:.0})",
                b.file.name,
                n::crossing_error(&run.throws[k - 2], &run.throws[k - 1], at)
            );
            self.companion_nether_next(b, run, now);
            run.stronghold = Some(target);
            return;
        }
        // Where to throw from: here first, then to the side of the line.
        if let Some(a) = run.throws.last() {
            let t = n::second_throw(a, 64.0 * k as f64);
            let goal = Goal::column([t[0].round() as i32, 70, t[1].round() as i32], 3.0);
            run.note = "to the next throw".into();
            match self.nether_go(host, b, run, goal, pos) {
                Steer::Arrived => {}
                Steer::Moving => return,
                // As far as it got: throw from here (when it got away).
                Steer::Stuck if c::horizontal(pos, [t[0], pos[1], t[1]]) > 300.0 => return,
                Steer::Stuck => {}
            }
        }
        if !self.companion_hold(host, b, EYE) {
            self.companion_nether_end(b, run, "triangulate: out of eyes".into());
            return;
        }
        let before = self.ender_eyes_in_flight().len();
        b.pitch = -30.0;
        let (yaw, pitch) = (b.yaw, b.pitch);
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = (yaw, pitch);
        }
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
        run.flight = Some(Flight {
            from: [pos[0], pos[2]],
            before,
            at: b.ticks,
            dir: None,
            last: None,
        });
        run.note = "throwing an eye".into();
    }

    fn leg_travel(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        if run.stronghold.is_none() && b.senses.level.knows_structures() {
            run.stronghold = self
                .companion_structure(b, "stronghold", pos)
                .map(|s| [s[0], s[2]]);
        }
        let Some(s) = run.stronghold else {
            self.companion_nether_end(b, run, "travel: no stronghold".into());
            return;
        };
        // The start piece sits a few blocks into its chunk.
        let at = [s[0] + 4, 70, s[1] + 4];
        let left = c::horizontal(pos, at_block(at));
        run.note = format!("{left:.0} blocks to go");
        run.waypoint = None;
        match self.nether_go(host, b, run, Goal::column(at, 6.0), pos) {
            Steer::Arrived => self.companion_nether_next(b, run, now),
            Steer::Stuck => {
                run.stuck += 1;
                b.walk.reset();
            }
            Steer::Moving => {}
        }
    }

    fn leg_room(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        let here = feet(pos);
        let s = run.stronghold.unwrap_or([here[0], here[2]]);
        let centre = [s[0] + 4, here[1], s[1] + 4];
        if b.ticks.is_multiple_of(20) {
            run.visited.push(here);
            if run.visited.len() > 512 {
                run.visited.remove(0);
            }
        }
        // In water (a lake or the sea over it): the staircase starts on the
        // nearest dry ground instead (no digging down under water).
        let wet = |y: i32| {
            self.res
                .facts()
                .get(self.world.block(here[0], y, here[2]))
                .fluid
                != Fluid::None
        };
        // Once it heads ashore it keeps on until there (bobbing at the
        // surface its feet come out of the water now and then).
        // (Only before the staircase starts, at the surface: water met on
        // the way down, an aquifer or a flooded corridor, is gone through,
        // with the air rule over it.)
        let surface = run.shaft.is_none() && here[1] >= 56;
        let near_water = surface && !self.inland(here);
        if surface
            && (wet(here[1]) || wet(here[1] + 1) || wet(here[1] - 1) || near_water)
            && run.ashore.is_none()
        {
            run.ashore = self.dry_spot_near(here, 96, &run.bad);
            log::info!(
                "[companion] {} room: in water at {here:?}, ashore at {:?}",
                b.file.name,
                run.ashore
            );
            if run.ashore.is_none() && (wet(here[1]) || wet(here[1] + 1)) {
                // Open water as far as it sees: no shaft to dig (it would
                // flood); stopped, classified, before it drowns.
                self.companion_nether_end(b, run, "room: under open water, no land in view".into());
                return;
            }
        }
        if let Some(dry) = run.ashore {
            if c::horizontal(pos, at_block(dry)) < 1.0 && !wet(here[1]) {
                run.ashore = None;
                run.shaft = Some([here[0], here[2]]);
            } else {
                run.note = "out of the water to dig down".into();
                // The vanilla-style path swims and climbs out of water.
                let steer = if c::horizontal(pos, at_block(dry)) < 40.0 {
                    self.companion_steer(host, b, at_block(dry), 0.6, true)
                } else {
                    self.nether_go(host, b, run, Goal::near(dry, 0.5), pos)
                };
                if steer == Steer::Stuck {
                    log::info!("[companion] {} room: no way ashore to {dry:?}", b.file.name);
                    run.bad.push(dry);
                    run.ashore = None;
                    b.walk.reset();
                }
                return;
            }
        }
        // Digging down: the water it opens into (an aquifer, a flooded
        // corridor) walled off with blocks as it goes, a dry shaft instead
        // of a dig under water.
        if run.shaft.is_some() && self.companion_dry_shaft(host, b, pos) {
            run.note = "walling off water".into();
            return;
        }
        // The frames: seen, or through walls.
        let wanted = self.states_where(|n| n == "minecraft:end_portal_frame");
        let radius = if b.senses.level == PerceptionLevel::AllKnowing {
            160
        } else {
            96
        };
        // Looked for until found, then kept (a goal that changed every few
        // ticks restarted the dig each time and it never got a block out).
        if run.frames.is_none() {
            let around = if run.scanned { here } else { centre };
            if let Some(found) =
                self.companion_known(b, &wanted, around, radius, &mut run.scan, 8 * 4096)
            {
                run.scan = None;
                run.scanned = b.senses.level.xray_radius(radius).is_none() || !run.scanned;
                run.frames = found.first().copied();
                if let Some(f) = run.frames {
                    log::info!("[companion] {} room: frames at {f:?}", b.file.name);
                }
            }
        }
        if let Some(f) = run.frames {
            let goal = Goal {
                at: f,
                near: 3.0,
                vertical: 2,
            };
            if goal.reached(here) {
                self.companion_nether_next(b, run, now);
                return;
            }
            // Straight down to their level first (a staircase), then across.
            if here[1] > f[1] + 3 && run.stuck < 3 {
                let shaft = *run.shaft.get_or_insert([here[0], here[2]]);
                run.note = format!("digging down to the portal room, y {}", f[1]);
                let down = Goal {
                    at: [shaft[0], f[1], shaft[1]],
                    near: 2.0,
                    vertical: 1,
                };
                if self.nether_go(host, b, run, down, pos) == Steer::Stuck {
                    run.stuck += 1;
                    b.walk.reset();
                }
                return;
            }
            run.note = "to the portal room".into();
            if self.nether_go(host, b, run, goal, pos) == Steer::Stuck {
                run.stuck += 1;
                b.walk.reset();
            }
            return;
        }
        if run.waypoint.is_none() {
            let bricks = self.recall_cue(b, run, Cue::Stronghold, here, 64);
            if bricks.is_empty() {
                // A staircase down where it is (the stronghold is below).
                if here[1] <= run.depth + 1 {
                    run.depth -= 12;
                    if run.depth < -40 {
                        // Round about the start chunk at the bottom.
                        let k = run.stuck as i32;
                        run.stuck += 1;
                        run.depth = 20;
                        run.stronghold =
                            Some([s[0] + (k % 3 - 1) * 24, s[1] + (k / 3 % 3 - 1) * 24]);
                        run.shaft = None;
                    }
                }
                run.note = format!("digging down to y {}", run.depth);
                // Down from where it stands (dry ground near the start).
                let shaft = *run.shaft.get_or_insert([here[0], here[2]]);
                let goal = Goal {
                    at: [shaft[0], run.depth, shaft[1]],
                    near: 2.0,
                    vertical: 1,
                };
                if self.nether_go(host, b, run, goal, pos) == Steer::Stuck {
                    run.depth -= 4;
                    b.walk.reset();
                }
                return;
            }
            run.waypoint = n::frontier(&bricks, &run.visited, here, 14);
            if run.waypoint.is_none() {
                // Every end it saw is explored: further down.
                run.depth = here[1] - 12;
                run.visited.clear();
                return;
            }
        }
        let Some(w) = run.waypoint else { return };
        run.note = "searching the stronghold".into();
        let goal = Goal {
            at: w,
            near: 2.0,
            vertical: 2,
        };
        if self.nether_go(host, b, run, goal, pos) != Steer::Moving {
            run.visited.push(w);
            run.waypoint = None;
        }
    }

    fn leg_fill(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        run: &mut NetherRun,
        pos: [f64; 3],
        now: u64,
    ) {
        let here = feet(pos);
        let wanted = self.states_where(|n| n == "minecraft:end_portal_frame");
        let Some(frames) = self.companion_known(b, &wanted, here, 24, &mut run.scan, 8 * 4096)
        else {
            return;
        };
        run.scan = None;
        let registry = self.pack.assets.registry;
        let filled = |p: &[i32; 3]| {
            registry
                .state(self.world.block(p[0], p[1], p[2]))
                .is_some_and(|s| s.properties.get("eye").map(String::as_str) == Some("true"))
        };
        let empty: Vec<[i32; 3]> = frames.iter().copied().filter(|p| !filled(p)).collect();
        let lit = frames.iter().any(|f| {
            (-3..=3).any(|dx| {
                (-3..=3).any(|dz| {
                    let p = [f[0] + dx, f[1], f[2] + dz];
                    &*self
                        .res
                        .facts()
                        .get(self.world.block(p[0], p[1], p[2]))
                        .name
                        == "minecraft:end_portal"
                })
            })
        });
        if lit || frames.len() >= 12 && empty.is_empty() {
            self.companion_nether_next(b, run, now);
            run.note = "portal open".into();
            return;
        }
        if frames.len() < 12 && now.saturating_sub(run.leg_start) < 200 {
            // Not all in sight yet: look round.
            b.yaw += 6.0;
            b.pitch = 20.0;
        }
        let eyes = host
            .hub
            .guest(b.conn)
            .map_or(0, |g| count(&g.inventory, EYE));
        if eyes == 0 {
            let why = format!("fill: out of eyes with {} empty frames", empty.len());
            self.companion_nether_end(b, run, why);
            return;
        }
        // An empty frame in reach: an eye in it.
        if let Some(f) = empty
            .iter()
            .copied()
            .filter(|f| super::companion_work::to_block(eye(pos), *f) < 4.3)
            .min_by_key(|f| d2(*f, here))
        {
            run.note = "eye in a frame".into();
            b.walk.reset();
            if b.ticks.is_multiple_of(4) && self.companion_hold(host, b, EYE) {
                self.companion_use_block(host, b, f, 1, pos);
            }
            return;
        }
        let stand = n::frame_stand(&frames, &empty, |p| {
            self.floor([p[0], p[1] - 1, p[2]])
                && self.passable(p)
                && self.passable([p[0], p[1] + 1, p[2]])
                && !self.lava_below(p)
        });
        match stand {
            Some((stand, _)) => {
                run.note = "to the frames".into();
                let _ = self.nether_go(host, b, run, Goal::exact(stand), pos);
            }
            None => {
                run.note = "no spot by the frames".into();
                let _ = self.nether_go(host, b, run, Goal::near(empty[0], 2.0), pos);
            }
        }
    }

    fn lava_below(&self, p: [i32; 3]) -> bool {
        (1..=2).any(|d| {
            self.res
                .facts()
                .get(self.world.block(p[0], p[1] - d, p[2]))
                .fluid
                == Fluid::Lava
        })
    }
}

/// What a body carries toward the targets (and the frames it filled).
/// What a body carries toward the targets (and the frames it filled), from
/// outside its tick (the hub in place).
fn host_carried(game: &Game<'_>, b: &Body) -> Carried {
    match (game.lan.as_ref(), b.nether.as_deref()) {
        (Some(host), Some(run)) => carried_now(game, host, b, run),
        _ => Carried::default(),
    }
}

/// The same during its tick (the hub taken out, the run too).
fn carried_now(game: &Game<'_>, host: &LanHost, b: &Body, run: &NetherRun) -> Carried {
    let Some(g) = host.hub.guest(b.conn) else {
        return Carried::default();
    };
    carried_of(&g.inventory, filled_frames(game, run, g.position))
}

fn carried_of(inv: &[Option<ItemStack>], filled: u32) -> Carried {
    Carried {
        rods: count(inv, ROD),
        powder: count(inv, POWDER),
        pearls: count(inv, PEARL),
        eyes: count(inv, EYE),
        filled,
    }
}

/// Frames with an eye near the body (only in the Fill leg).
fn filled_frames(game: &Game<'_>, run: &NetherRun, at: [f64; 3]) -> u32 {
    if run.leg != Leg::Fill && run.leg != Leg::Done {
        return 0;
    }
    let f = feet(at);
    let registry = game.pack.assets.registry;
    let mut n = 0;
    for dx in -8..=8 {
        for dy in -3..=3 {
            for dz in -8..=8 {
                let p = [f[0] + dx, f[1] + dy, f[2] + dz];
                if let Some(s) = registry.state(game.world.block(p[0], p[1], p[2]))
                    && s.name == "minecraft:end_portal_frame"
                    && s.properties.get("eye").map(String::as_str) == Some("true")
                {
                    n += 1;
                }
            }
        }
    }
    n
}

/// The distance from `eye` to the nearest point of a box.
fn box_distance(eye: [f64; 3], b: horizon_core::geometry::Aabb) -> f64 {
    let (lo, hi) = (b.min_position(), b.max_position());
    let p = [
        eye[0].clamp(lo.x, hi.x),
        eye[1].clamp(lo.y, hi.y),
        eye[2].clamp(lo.z, hi.z),
    ];
    c::distance(eye, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carried_counts_what_goes_into_eyes() {
        let inv = vec![
            Some(ItemStack::new(ROD, 3)),
            Some(ItemStack::new(POWDER, 2)),
            Some(ItemStack::new(PEARL, 5)),
            Some(ItemStack::new(EYE, 4)),
            None,
        ];
        let c = carried_of(&inv, 2);
        assert_eq!(c.rods_worth(), 3 + 4);
        assert_eq!(c.pearls_worth(), 5 + 4 + 2);
    }

    #[test]
    fn every_leg_has_a_budget() {
        for l in Leg::ALL {
            assert!(budget(l) > 0);
        }
    }
}
