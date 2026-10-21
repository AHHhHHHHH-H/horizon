//! The Speedrunner's middle part (docs/companion-speedrun.md, N1–S5), the
//! part without the game: from a lit Nether portal to a filled End portal
//! frame. The legs and what the Watch HUD says of them, the targets (rods,
//! pearls, eyes), and the reasoning each leg needs that can be tested on
//! made-up numbers:
//!
//! - **triangulation** from two eye-of-ender throws ([`Throw`],
//!   [`intersect`]), as speedrunners do it: an eye's horizontal flight lies on
//!   the line from where it was thrown toward the stronghold;
//! - **the Fair search** for a fortress ([`Search`]): region by region
//!   (`nether_complexes`: one fortress or bastion per 27 × 27 chunks, in its
//!   first 23 × 23), the middle of each region's start area first, nearest
//!   next, skipping the region of a bastion it saw;
//! - what the blocks it sees mean: fortress, bastion, stronghold, portal
//!   room ([`cue`]);
//! - the **stance** against blazes ([`blaze_stance`]): shield up against a
//!   volley, a blow when one is within reach, the bow at range, back off to
//!   heal;
//! - the **pit** against endermen ([`Pit`]): two blocks deep, a block under
//!   its feet to look out and stare, mined away to drop in: an enderman on
//!   the rim can't reach down to it (a mob's reach is horizontal), it hits
//!   the enderman's legs;
//! - where to look next in a stronghold ([`frontier`]) and where to stand to
//!   fill the frames ([`frame_stand`]).
//!
//! The game side is game/companion_nether.rs.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// What the middle part gathers before it heads for the stronghold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Targets {
    /// Blaze rods (two powder each; 12 eyes need 6, one spare).
    pub rods: u32,
    /// Ender pearls (12 eyes and a spare).
    pub pearls: u32,
    /// Eyes to craft (a frame takes up to 12; throws lose about 1 in 5).
    pub eyes: u32,
    /// Trade gold with piglins for pearls too (poor in 26.3: ~0.064 pearls
    /// an ingot), when it carries gold and wears a gold piece.
    pub barter: bool,
}

impl Default for Targets {
    fn default() -> Self {
        Self {
            rods: 7,
            pearls: 13,
            eyes: 12,
            barter: false,
        }
    }
}

/// A leg of the middle part, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Leg {
    /// Through the lit portal into the Nether, the portal remembered.
    Enter,
    /// Gold from a bastion's gold blocks traded with piglins for fire
    /// resistance (and pearls) before the blazes, when `barter` is on and
    /// it knows of a bastion near the way.
    Barter,
    /// A fortress in the memory (seen, or located at Scout and All-knowing).
    Fortress,
    /// Blaze rods at a spawner (or roaming blazes).
    Blazes,
    /// Ender pearls from endermen (warped forest; an overworld night).
    Pearls,
    /// Back to the remembered portal and through it.
    Home,
    /// Blaze powder, then eyes of ender.
    Eyes,
    /// Two throws, the stronghold where their lines cross.
    Triangulate,
    /// To the stronghold's start chunk.
    Travel,
    /// A staircase down into it, then its corridors to the portal room.
    Room,
    /// Eyes in the frames.
    Fill,
    /// The frame is full: the End part takes over.
    Done,
}

impl Leg {
    pub const ALL: [Self; 12] = [
        Self::Enter,
        Self::Barter,
        Self::Fortress,
        Self::Blazes,
        Self::Pearls,
        Self::Home,
        Self::Eyes,
        Self::Triangulate,
        Self::Travel,
        Self::Room,
        Self::Fill,
        Self::Done,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Barter => "barter",
            Self::Fortress => "fortress",
            Self::Blazes => "blazes",
            Self::Pearls => "pearls",
            Self::Home => "home",
            Self::Eyes => "eyes",
            Self::Triangulate => "triangulate",
            Self::Travel => "travel",
            Self::Room => "room",
            Self::Fill => "fill",
            Self::Done => "done",
        }
    }
    pub fn parse(word: &str) -> Option<Self> {
        let w = word.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|l| l.id() == w)
            .or(match w.as_str() {
                "nether" => Some(Self::Enter),
                "bastion" | "bartering" => Some(Self::Barter),
                "rods" | "blaze" => Some(Self::Blazes),
                "endermen" | "enderman" => Some(Self::Pearls),
                "portal" | "back" => Some(Self::Home),
                "stronghold" => Some(Self::Triangulate),
                "dig" | "portal_room" => Some(Self::Room),
                "frame" => Some(Self::Fill),
                _ => None,
            })
    }
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|l| *l == self).unwrap_or(0);
        Self::ALL[(i + 1).min(Self::ALL.len() - 1)]
    }
    /// Whether the leg is played in the Nether (else the Overworld).
    pub fn in_nether(self) -> bool {
        matches!(
            self,
            Self::Barter | Self::Fortress | Self::Blazes | Self::Home
        )
    }
}

/// What it carries toward the targets, for the HUD line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Carried {
    pub rods: u32,
    pub powder: u32,
    pub pearls: u32,
    pub eyes: u32,
    /// Frames already holding an eye (in the portal room).
    pub filled: u32,
}

impl Carried {
    /// Rods it has or has turned into powder and eyes.
    pub fn rods_worth(&self) -> u32 {
        self.rods + (self.powder + self.eyes + self.filled).div_ceil(2)
    }
    pub fn pearls_worth(&self) -> u32 {
        self.pearls + self.eyes + self.filled
    }
}

/// The Watch HUD's step line for a leg: "Blaze rods 4/7".
pub fn step_text(leg: Leg, have: &Carried, t: &Targets) -> String {
    match leg {
        Leg::Enter => "Into the Nether".into(),
        Leg::Barter => "Bartering with piglins".into(),
        Leg::Fortress => "Searching for a fortress".into(),
        Leg::Blazes => format!("Blaze rods {}/{}", have.rods_worth().min(t.rods), t.rods),
        Leg::Pearls => format!("Pearls {}/{}", have.pearls_worth().min(t.pearls), t.pearls),
        Leg::Home => "Back to the portal".into(),
        Leg::Eyes => format!("Eyes of ender {}/{}", have.eyes.min(t.eyes), t.eyes),
        Leg::Triangulate => "Triangulating the stronghold".into(),
        Leg::Travel => "Travelling to the stronghold".into(),
        Leg::Room => "Searching for the portal room".into(),
        Leg::Fill => format!("Filling the frame {}/12", have.filled.min(12)),
        Leg::Done => "Portal open".into(),
    }
}

/// Whether the targets of `leg` are met, so the route can skip it.
pub fn leg_met(leg: Leg, have: &Carried, t: &Targets) -> bool {
    match leg {
        Leg::Barter => !t.barter,
        Leg::Blazes => have.rods_worth() >= t.rods,
        Leg::Pearls => have.pearls_worth() >= t.pearls,
        Leg::Eyes => have.eyes + have.filled >= t.eyes.min(have.pearls_worth()),
        _ => false,
    }
}

// ---- triangulation ------------------------------------------------------------

pub type P2 = [f64; 2];

fn sub(a: P2, b: P2) -> P2 {
    [a[0] - b[0], a[1] - b[1]]
}
fn len(a: P2) -> f64 {
    a[0].hypot(a[1])
}
/// The 2-D cross product `aₓb_z − a_zbₓ`.
pub fn cross(a: P2, b: P2) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// One eye's throw: where it left, and the way it flew (unit, horizontal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Throw {
    pub from: P2,
    pub dir: P2,
}

/// The bearing of an eye seen at `eye` that was thrown from `from`; None
/// while it is too close to tell (under half a block).
pub fn bearing(from: P2, eye: P2) -> Option<P2> {
    let d = sub(eye, from);
    let l = len(d);
    (l >= 0.5).then(|| [d[0] / l, d[1] / l])
}

/// Two lines closer to parallel than this (the sine of the angle) give no
/// trustworthy crossing.
pub const MIN_ANGLE_SIN: f64 = 0.017_452; // 1°

/// Where the two throws' lines cross, ahead of both; None when they are too
/// close to parallel or cross behind.
pub fn intersect(a: &Throw, b: &Throw) -> Option<P2> {
    let d = cross(a.dir, b.dir);
    if d.abs() < MIN_ANGLE_SIN {
        return None;
    }
    let ab = sub(b.from, a.from);
    let t = cross(ab, b.dir) / d;
    let s = cross(ab, a.dir) / d;
    (t > 0.0 && s > 0.0).then(|| [a.from[0] + t * a.dir[0], a.from[1] + t * a.dir[1]])
}

/// The stronghold rings start this far from the origin (the first ring:
/// 1,280–2,816 blocks).
pub const FIRST_RING: f64 = 1280.0;

/// Where to throw the second eye from: `baseline` blocks to the side of the
/// first line, and, inside the first ring, ahead along it (the stronghold
/// is at least that far out).
pub fn second_throw(a: &Throw, baseline: f64) -> P2 {
    // Not far ahead: a walk of a few hundred blocks costs minutes, and the
    // reading is sharp enough for a short baseline.
    let ahead = (FIRST_RING - len(a.from) - 200.0).clamp(0.0, 160.0);
    // To the side away from the origin's side, so the walk doesn't cross
    // the line back.
    let n = [-a.dir[1], a.dir[0]];
    [
        a.from[0] + a.dir[0] * ahead + n[0] * baseline,
        a.from[1] + a.dir[1] * ahead + n[1] * baseline,
    ]
}

/// How far off a crossing may be, given the angle between the lines and
/// how far the target is: the bearing is read to a hundredth of a degree.
pub fn crossing_error(a: &Throw, b: &Throw, at: P2) -> f64 {
    let s = cross(a.dir, b.dir).abs().max(1e-9);
    let d = len(sub(at, a.from)).max(len(sub(at, b.from)));
    d * 1.75e-4 / s
}

// ---- what blocks mean -----------------------------------------------------------

/// What a seen block says about the place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cue {
    None,
    Fortress,
    Bastion,
    Stronghold,
    /// End portal frames (the portal room).
    Frame,
    Spawner,
    WarpedForest,
}

/// The cue of a block by name.
pub fn cue(name: &str) -> Cue {
    let n = name.trim_start_matches("minecraft:");
    match n {
        "nether_bricks"
        | "nether_brick_fence"
        | "nether_brick_stairs"
        | "nether_brick_slab"
        | "nether_brick_wall"
        | "cracked_nether_bricks"
        | "chiseled_nether_bricks" => Cue::Fortress,
        "polished_blackstone_bricks"
        | "cracked_polished_blackstone_bricks"
        | "gilded_blackstone"
        | "chiseled_polished_blackstone"
        | "polished_blackstone_brick_stairs"
        | "polished_blackstone_brick_slab"
        | "polished_blackstone_brick_wall" => Cue::Bastion,
        "stone_bricks"
        | "mossy_stone_bricks"
        | "cracked_stone_bricks"
        | "chiseled_stone_bricks"
        | "stone_brick_stairs"
        | "stone_brick_slab"
        | "infested_stone_bricks"
        | "infested_mossy_stone_bricks"
        | "infested_cracked_stone_bricks"
        | "infested_chiseled_stone_bricks"
        | "iron_door" => Cue::Stronghold,
        "end_portal_frame" => Cue::Frame,
        "spawner" => Cue::Spawner,
        "warped_nylium" | "warped_wart_block" | "warped_stem" => Cue::WarpedForest,
        _ => Cue::None,
    }
}

// ---- the fortress search ----------------------------------------------------------

/// Nether complexes (a fortress or a bastion, one per region) sit in
/// regions of this many chunks (`nether_complexes` spacing).
pub const REGION_CHUNKS: i32 = 27;

/// The Nether-complex region of a block column.
pub fn region_of(x: i32, z: i32) -> [i32; 2] {
    let r = REGION_CHUNKS * 16;
    [x.div_euclid(r), z.div_euclid(r)]
}

/// `nether_complexes` separation: a complex starts in the first
/// `REGION_CHUNKS - SEPARATION_CHUNKS` chunks of its region on each axis
/// (RandomSpreadStructurePlacement: offset `nextInt(spacing - separation)`).
pub const SEPARATION_CHUNKS: i32 = 4;

/// The Fair search for a fortress, by Nether-complex region: each region
/// (27 x 27 chunks) holds one complex, a fortress (weight 2) or a bastion
/// (weight 3), starting somewhere in its first 23 x 23 chunks. So the search
/// goes region by region: first the middle of each region's start area
/// (from there sight, 64 blocks, and the complex's own size reach most of
/// it), the nearest region next, then the four quarters of those it has not
/// ruled out. A region where it saw a bastion is done (no fortress there).
#[derive(Clone, Debug, Default)]
pub struct Search {
    pub origin: [i32; 2],
    /// Where it is going, the last waypoint given.
    pub at: Option<[i32; 2]>,
    /// The waypoints given so far.
    pub visited: HashSet<[i32; 2]>,
    /// Regions with a bastion: no fortress there.
    pub ruled_out: HashSet<[i32; 2]>,
    /// Waypoints given up on.
    pub skipped: u32,
}

impl Search {
    pub fn new(origin: [i32; 2], _step: i32) -> Self {
        Self {
            origin,
            ..Default::default()
        }
    }
    /// The candidate waypoints of the regions within `limit` blocks of the
    /// origin: pass 0 the middles of the start areas, pass 1 the quarters.
    fn candidates(&self, limit: i32, pass: u8) -> Vec<[i32; 2]> {
        let r = REGION_CHUNKS * 16;
        let area = (REGION_CHUNKS - SEPARATION_CHUNKS) * 16;
        let offsets: &[[i32; 2]] = if pass == 0 {
            &[[area / 2, area / 2]]
        } else {
            &[
                [area / 4, area / 4],
                [area * 3 / 4, area / 4],
                [area / 4, area * 3 / 4],
                [area * 3 / 4, area * 3 / 4],
            ]
        };
        let n = limit / r + 1;
        let home = region_of(self.origin[0], self.origin[1]);
        let mut out = Vec::new();
        for rx in home[0] - n..=home[0] + n {
            for rz in home[1] - n..=home[1] + n {
                for o in offsets {
                    let p = [rx * r + o[0], rz * r + o[1]];
                    let d =
                        f64::from(p[0] - self.origin[0]).hypot(f64::from(p[1] - self.origin[1]));
                    if d <= f64::from(limit) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }
    /// The next waypoint (a column): the nearest to the last one not yet
    /// given, in a region not ruled out; within `limit` blocks of the
    /// origin, else None (searched out).
    pub fn next(&mut self, limit: i32) -> Option<[i32; 2]> {
        let from = self.at.unwrap_or(self.origin);
        for pass in 0..2 {
            let best = self
                .candidates(limit, pass)
                .into_iter()
                .filter(|p| !self.visited.contains(p))
                .filter(|p| !self.ruled_out.contains(&region_of(p[0], p[1])))
                .min_by_key(|p| {
                    let d = |a: [i32; 2], b: [i32; 2]| {
                        i64::from(a[0] - b[0]).pow(2) + i64::from(a[1] - b[1]).pow(2)
                    };
                    (d(*p, from), d(*p, self.origin))
                });
            if let Some(p) = best {
                self.visited.insert(p);
                self.at = Some(p);
                return Some(p);
            }
        }
        None
    }
    /// The current waypoint again.
    pub fn current(&self) -> [i32; 2] {
        self.at.unwrap_or(self.origin)
    }
    pub fn rule_out(&mut self, x: i32, z: i32) {
        if self.ruled_out.insert(region_of(x, z)) {
            self.skipped += 1;
        }
    }
}

/// The middle of what it saw of a structure: the mean of the blocks.
pub fn centre_of(points: &[[i32; 3]]) -> Option<[i32; 3]> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as i64;
    let s = points.iter().fold([0i64; 3], |a, p| {
        [
            a[0] + i64::from(p[0]),
            a[1] + i64::from(p[1]),
            a[2] + i64::from(p[2]),
        ]
    });
    Some([(s[0] / n) as i32, (s[1] / n) as i32, (s[2] / n) as i32])
}

// ---- blazes ---------------------------------------------------------------------

/// A blaze as the fight sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Foe {
    /// From its eye to the blaze's box.
    pub reach: f64,
    pub sees: bool,
    /// It is charging a volley (lit) or shooting.
    pub charging: bool,
}

/// What it does this tick in a blaze fight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    /// Strike the blaze at this index.
    Strike(usize),
    /// Draw the bow at it.
    Shoot(usize),
    /// Shield up toward it.
    Block(usize),
    /// Walk up to it between its volleys (shield down to keep the pace).
    Close(usize),
    /// Back off into cover, eat.
    Retreat,
    /// Nothing near: wait by the spawner, pick up drops.
    Wait,
}

/// Melee reach (from the eye to the box).
pub const REACH: f64 = 3.0;

/// The stance against the blazes around: low on health it backs off; a
/// blaze within reach gets a blow (when the attack is charged); one that
/// sees it from further gets the shield while it shoots, the bow when it
/// has arrows and none is shooting, else it walks up to the nearest
/// between volleys (a blaze keeps its distance while it sees you).
/// `low`: the health under which it backs off to heal (higher away from
/// the spawner, where blazes come from every side and no box waits).
pub fn blaze_stance(health: f32, low: f32, charged: bool, foes: &[Foe], arrows: bool) -> Stance {
    if foes.is_empty() {
        return Stance::Wait;
    }
    // Low: out of their sight to heal (fire resistance is not to be had);
    // they come round the cover one at a time. Not before: with the
    // shield up a volley does nothing, and blazes never come closer while
    // they see it (BlazeAttackGoal), so backing off from a crowd at a
    // spawner only leaves it hurt and none of them dead.
    if health < low {
        return Stance::Retreat;
    }
    let nearest = (0..foes.len())
        .min_by(|a, b| foes[*a].reach.total_cmp(&foes[*b].reach))
        .unwrap_or(0);
    if foes[nearest].reach <= REACH && charged {
        return Stance::Strike(nearest);
    }
    let shooting = foes.iter().position(|f| f.sees && f.charging);
    if let Some(i) = shooting {
        return Stance::Block(i);
    }
    if arrows
        && let Some(i) = (0..foes.len())
            .filter(|i| foes[*i].sees && foes[*i].reach > 5.0 && foes[*i].reach < 24.0)
            .min_by(|a, b| foes[*a].reach.total_cmp(&foes[*b].reach))
    {
        return Stance::Shoot(i);
    }
    if foes[nearest].reach <= REACH {
        // Waiting for the attack to charge: shield up meanwhile.
        return Stance::Block(nearest);
    }
    Stance::Close(nearest)
}

/// Where to hold the shield so it covers every one of `threats` (the
/// shield blocks what comes within 90 degrees of the view, flattened): the
/// mean of their horizontal directions from `eye`, a point 4 blocks that way
/// at their mean height. None when they come from opposite sides (no one
/// direction covers them all) or there are none.
pub fn cover_point(eye: [f64; 3], threats: &[[f64; 3]]) -> Option<[f64; 3]> {
    if threats.is_empty() {
        return None;
    }
    let (mut sx, mut sz, mut sy) = (0.0, 0.0, 0.0);
    for t in threats {
        let (dx, dz) = (t[0] - eye[0], t[2] - eye[2]);
        let len = dx.hypot(dz).max(1e-6);
        sx += dx / len;
        sz += dz / len;
        sy += t[1];
    }
    let n = threats.len() as f64;
    let (mx, mz) = (sx / n, sz / n);
    let len = mx.hypot(mz);
    // Every threat within 90 degrees of the mean, with a margin.
    let covers = threats.iter().all(|t| {
        let (dx, dz) = (t[0] - eye[0], t[2] - eye[2]);
        (dx * mx + dz * mz) / (dx.hypot(dz).max(1e-6) * len.max(1e-6)) > 0.17
    });
    (len > 1e-3 && covers).then(|| [eye[0] + mx / len * 4.0, sy / n, eye[2] + mz / len * 4.0])
}

/// A waypoint `far` blocks off the line from `here` toward `goal`
/// (horizontal), turning 60 degrees one way, then the other, then wider,
/// with each `tries`: a way round what the walker could not get past.
pub fn detour(here: [i32; 3], goal: [i32; 2], tries: u32, far: i32) -> [i32; 3] {
    let (dx, dz) = (f64::from(goal[0] - here[0]), f64::from(goal[1] - here[2]));
    let toward = dz.atan2(dx);
    let side = if tries.is_multiple_of(2) { 1.0 } else { -1.0 };
    let turn = (60.0 + 30.0 * f64::from((tries / 2).saturating_sub(1).min(2))).to_radians();
    let a = toward + side * turn;
    [
        here[0] + (a.cos() * f64::from(far)).round() as i32,
        here[1],
        here[2] + (a.sin() * f64::from(far)).round() as i32,
    ]
}

// ---- endermen --------------------------------------------------------------------

/// The pit it fights endermen from: two blocks dug at `at` (its feet when
/// in it stand on `at.y - 1`... the floor below the lower one), one block
/// placed in the lower cell to look out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pit {
    /// The upper dug cell (level with the ground it stood on).
    pub top: [i32; 3],
}

impl Pit {
    /// A pit where it stands: its feet at `feet` on solid ground.
    pub fn at(feet: [i32; 3]) -> Self {
        Self {
            top: [feet[0], feet[1] - 1, feet[2]],
        }
    }
    /// The lower cell (where its feet are at the bottom).
    pub fn bottom(&self) -> [i32; 3] {
        [self.top[0], self.top[1] - 1, self.top[2]]
    }
    /// Its feet when looking out (on the step block in the bottom cell).
    pub fn look_feet(&self) -> [i32; 3] {
        self.top
    }
    /// Its feet when fighting (at the bottom).
    pub fn fight_feet(&self) -> [i32; 3] {
        self.bottom()
    }
    /// The blocks to dig, top first.
    pub fn dig(&self) -> [[i32; 3]; 2] {
        [self.top, self.bottom()]
    }
    /// The floor that must stay under the bottom (not lava, not air).
    pub fn floor(&self) -> [i32; 3] {
        [self.top[0], self.top[1] - 2, self.top[2]]
    }
    /// The rim cells around it (where an enderman stands to come at it).
    pub fn rim(&self) -> [[i32; 3]; 4] {
        let [x, y, z] = self.top;
        [
            [x + 1, y + 1, z],
            [x - 1, y + 1, z],
            [x, y + 1, z + 1],
            [x, y + 1, z - 1],
        ]
    }
}

/// Whether a mob standing with its feet at `mob_y`, `mob_h` tall, can hit
/// a player whose feet are at `y` (`Mob.getAttackBoundingBox` grows only
/// sideways, so its box and the player's must share a height).
pub fn mob_reaches_down(mob_y: f64, y: f64) -> bool {
    mob_y < y + 1.8
}

// ---- the stronghold ----------------------------------------------------------------

/// The next place to look in a stronghold: of the stronghold blocks it saw,
/// the nearest that is at least `explored` blocks from everywhere it has
/// been (its corridors' far ends), ties to the nearest to `here`.
pub fn frontier(
    seen: &[[i32; 3]],
    visited: &[[i32; 3]],
    here: [i32; 3],
    explored: i32,
) -> Option<[i32; 3]> {
    let d2 =
        |a: [i32; 3], b: [i32; 3]| -> i64 { (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum() };
    let r2 = i64::from(explored).pow(2);
    seen.iter()
        .copied()
        .filter(|p| visited.iter().all(|v| d2(*p, *v) >= r2))
        .min_by_key(|p| d2(*p, here))
}

/// Where to stand to put eyes in the frames: a floor cell on the frames'
/// level just outside the ring, within reach of the most empty frames.
/// `frames` are the frames' blocks, `empty` those without an eye, `floor`
/// says whether a cell is something to stand on with room above it.
pub fn frame_stand(
    frames: &[[i32; 3]],
    empty: &[[i32; 3]],
    standable: impl Fn([i32; 3]) -> bool,
) -> Option<([i32; 3], usize)> {
    if empty.is_empty() {
        return None;
    }
    let frame_set: HashSet<[i32; 3]> = frames.iter().copied().collect();
    let y = frames.iter().map(|f| f[1]).min()?;
    let (x0, x1) = (
        frames.iter().map(|f| f[0]).min()? - 3,
        frames.iter().map(|f| f[0]).max()? + 3,
    );
    let (z0, z1) = (
        frames.iter().map(|f| f[2]).min()? - 3,
        frames.iter().map(|f| f[2]).max()? + 3,
    );
    let mut best: Option<([i32; 3], usize, i64)> = None;
    for x in x0..=x1 {
        for z in z0..=z1 {
            // Its feet beside the frames (they are 13/16 tall: on their
            // level), or one up.
            for dy in [0, 1] {
                let feet = [x, y + dy, z];
                if frame_set.contains(&feet) || !standable(feet) {
                    continue;
                }
                let eye = [
                    f64::from(x) + 0.5,
                    f64::from(feet[1]) + 1.62,
                    f64::from(z) + 0.5,
                ];
                let n = empty
                    .iter()
                    .filter(|f| {
                        let top = [
                            f64::from(f[0]) + 0.5,
                            f64::from(f[1]) + 0.8125,
                            f64::from(f[2]) + 0.5,
                        ];
                        (0..3)
                            .map(|k| (top[k] - eye[k]).powi(2))
                            .sum::<f64>()
                            .sqrt()
                            < 4.3
                    })
                    .count();
                // Closest to the ring's middle on ties (fewer walks).
                let cx = (x0 + x1) / 2;
                let cz = (z0 + z1) / 2;
                let off = i64::from((x - cx).abs() + (z - cz).abs());
                if n > 0 && best.is_none_or(|(_, bn, bo)| n > bn || n == bn && off < bo) {
                    best = Some((feet, n, off));
                }
            }
        }
    }
    best.map(|(p, n, _)| (p, n))
}

/// The twelve frames of a portal: the 5 × 5 ring's frames, with the
/// three-wide sides (vanilla's portal room), around `centre` at `y`.
pub fn frame_ring(centre: [i32; 2], y: i32) -> Vec<[i32; 3]> {
    let mut v = Vec::with_capacity(12);
    for i in -1..=1 {
        v.push([centre[0] + i, y, centre[1] - 2]);
        v.push([centre[0] + i, y, centre[1] + 2]);
        v.push([centre[0] - 2, y, centre[1] + i]);
        v.push([centre[0] + 2, y, centre[1] + i]);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(a: f64) -> P2 {
        [a.cos(), a.sin()]
    }

    #[test]
    fn two_throws_find_the_stronghold() {
        let target = [1664.0, -512.0];
        let a_from = [10.5, 20.5];
        // The eye a few ticks out, on its line toward the target.
        let u_true = bearing(a_from, target).unwrap();
        let eye = [a_from[0] + 3.0 * u_true[0], a_from[1] + 3.0 * u_true[1]];
        let a = Throw {
            from: a_from,
            dir: bearing(a_from, eye).unwrap(),
        };
        let b_from = second_throw(&a, 64.0);
        let b = Throw {
            from: b_from,
            dir: bearing(b_from, target).unwrap(),
        };
        let at = intersect(&a, &b).expect("they cross");
        assert!(
            (at[0] - target[0]).abs() < 0.01 && (at[1] - target[1]).abs() < 0.01,
            "{at:?}"
        );
        assert!(crossing_error(&a, &b, at) < 16.0);
    }

    #[test]
    fn readings_off_by_the_precision_still_land_near() {
        let target = [-2100.0, 900.0];
        let a_from = [100.0, -40.0];
        let a = Throw {
            from: a_from,
            dir: bearing(a_from, target).unwrap(),
        };
        let b_from = second_throw(&a, 64.0);
        let mut v = bearing(b_from, target).unwrap();
        // A hundredth of a degree off.
        let ang = v[1].atan2(v[0]) + 1.75e-4;
        v = unit(ang);
        let b = Throw {
            from: b_from,
            dir: v,
        };
        let at = intersect(&a, &b).unwrap();
        let err = ((at[0] - target[0]).powi(2) + (at[1] - target[1]).powi(2)).sqrt();
        assert!(err < 16.0, "{err} blocks off at {at:?}");
    }

    #[test]
    fn parallel_or_behind_is_rejected() {
        let a = Throw {
            from: [0.0, 0.0],
            dir: [1.0, 0.0],
        };
        let b = Throw {
            from: [0.0, 10.0],
            dir: unit(0.001),
        };
        assert_eq!(intersect(&a, &b), None, "almost parallel");
        let c = Throw {
            from: [0.0, 10.0],
            dir: unit(-0.5 - std::f64::consts::PI),
        };
        assert_eq!(intersect(&a, &c), None, "crosses behind");
        assert_eq!(bearing([0.0, 0.0], [0.2, 0.1]), None);
    }

    #[test]
    fn inside_the_first_ring_the_second_throw_is_ahead() {
        let a = Throw {
            from: [0.0, 0.0],
            dir: [0.0, 1.0],
        };
        let b = second_throw(&a, 64.0);
        assert!(b[1] > 100.0 && (b[0].abs() - 64.0).abs() < 1e-9, "{b:?}");
        let far = Throw {
            from: [0.0, 2000.0],
            dir: [0.0, 1.0],
        };
        let b = second_throw(&far, 64.0);
        assert!((b[1] - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn one_shield_direction_covers_two_blazes_unless_opposite() {
        let eye = [0.0, 64.0, 0.0];
        let c = cover_point(eye, &[[10.0, 66.0, 0.0], [0.0, 66.0, 10.0]]).unwrap();
        assert!((c[0] - c[2]).abs() < 1e-9 && c[0] > 0.0);
        assert!(cover_point(eye, &[[10.0, 66.0, 0.0], [-10.0, 66.0, 0.0]]).is_none());
        assert!(cover_point(eye, &[]).is_none());
    }

    #[test]
    fn the_search_goes_region_by_region_and_skips_bastion_regions() {
        // From the middle of region (0, 0)'s start area: there first.
        let mut s = Search::new([184, 184], 128);
        assert_eq!(s.next(1200), Some([184, 184]));
        // The nearest region's middle next (432 blocks over).
        let second = s.next(1200).unwrap();
        assert_eq!(
            (second[0] - 184).abs() + (second[1] - 184).abs(),
            432,
            "{second:?}"
        );
        // A bastion seen in region (1, 1): its waypoints are skipped.
        s.rule_out(432 + 10, 432 + 10);
        let mut n = 0;
        while let Some(p) = s.next(1200) {
            assert_ne!(region_of(p[0], p[1]), [1, 1], "{p:?}");
            n += 1;
        }
        // The middles, then the quarters, all within the limit.
        assert!((20..200).contains(&n), "{n}");
        assert_eq!(region_of(-1, 431), [-1, 0]);
        assert_eq!(region_of(432, -432), [1, -1]);
    }

    #[test]
    fn blocks_tell_where_it_is() {
        assert_eq!(cue("minecraft:nether_bricks"), Cue::Fortress);
        assert_eq!(cue("minecraft:nether_brick_fence"), Cue::Fortress);
        assert_eq!(cue("minecraft:red_nether_bricks"), Cue::None);
        assert_eq!(cue("minecraft:gilded_blackstone"), Cue::Bastion);
        assert_eq!(cue("minecraft:mossy_stone_bricks"), Cue::Stronghold);
        assert_eq!(cue("minecraft:infested_stone_bricks"), Cue::Stronghold);
        assert_eq!(cue("minecraft:end_portal_frame"), Cue::Frame);
        assert_eq!(cue("minecraft:warped_nylium"), Cue::WarpedForest);
        assert_eq!(centre_of(&[[0, 0, 0], [10, 2, -4]]), Some([5, 1, -2]));
    }

    #[test]
    fn blaze_stances() {
        let near = Foe {
            reach: 2.5,
            sees: true,
            charging: false,
        };
        let far_shooting = Foe {
            reach: 10.0,
            sees: true,
            charging: true,
        };
        let far = Foe {
            reach: 10.0,
            sees: true,
            charging: false,
        };
        assert_eq!(blaze_stance(20.0, 8.0, true, &[], false), Stance::Wait);
        assert_eq!(
            blaze_stance(6.0, 8.0, true, &[near], false),
            Stance::Retreat
        );
        // Away from the spawner it backs off sooner.
        assert_eq!(
            blaze_stance(11.0, 12.0, true, &[near], false),
            Stance::Retreat
        );
        // Two at it, half its health gone: it stays, the shield up to the
        // one shooting.
        assert_eq!(
            blaze_stance(12.0, 8.0, true, &[far_shooting, far], false),
            Stance::Block(0)
        );
        assert_eq!(
            blaze_stance(12.0, 8.0, true, &[near], false),
            Stance::Strike(0)
        );
        assert_eq!(
            blaze_stance(20.0, 8.0, true, &[far_shooting, near], false),
            Stance::Strike(1)
        );
        assert_eq!(
            blaze_stance(20.0, 8.0, false, &[far_shooting, near], false),
            Stance::Block(0)
        );
        assert_eq!(
            blaze_stance(20.0, 8.0, true, &[far], true),
            Stance::Shoot(0)
        );
        assert_eq!(
            blaze_stance(20.0, 8.0, true, &[far], false),
            Stance::Close(0)
        );
    }

    #[test]
    fn a_detour_turns_off_the_line_and_alternates() {
        let here = [0, 70, 0];
        let a = detour(here, [100, 0], 2, 24);
        let b = detour(here, [100, 0], 3, 24);
        // Off to either side, ahead rather than back.
        assert!(a[0] > 0 && b[0] > 0);
        assert!(a[2] > 0 && b[2] < 0);
        assert_eq!(a[1], 70);
        let r = f64::from(a[0]).hypot(f64::from(a[2]));
        assert!((r - 24.0).abs() < 1.0);
        // Wider later.
        let c = detour(here, [100, 0], 4, 24);
        assert!(c[0] < a[0]);
    }

    #[test]
    fn the_pit_keeps_endermen_out_of_reach() {
        let p = Pit::at([5, 70, 5]);
        assert_eq!(p.dig(), [[5, 69, 5], [5, 68, 5]]);
        assert_eq!(p.floor(), [5, 67, 5]);
        assert_eq!(p.fight_feet(), [5, 68, 5]);
        // An enderman on the rim stands on the ground (feet at 70): it can't
        // reach feet at 68, it reaches feet at 69 (looking out).
        let rim_y = f64::from(p.rim()[0][1]);
        assert!(!mob_reaches_down(rim_y, f64::from(p.fight_feet()[1])));
        assert!(mob_reaches_down(rim_y, f64::from(p.look_feet()[1])));
        // From the bottom its eye (68 + 1.62) is level with the enderman's
        // legs, a block and a bit away: within reach.
        let eye_y = f64::from(p.fight_feet()[1]) + 1.62;
        assert!(eye_y < rim_y && rim_y - eye_y < 0.5);
    }

    #[test]
    fn frontier_goes_to_the_unexplored_end() {
        let seen = vec![[0, 10, 0], [5, 10, 0], [40, 10, 0], [0, 10, 30]];
        let visited = vec![[0, 10, 0]];
        assert_eq!(frontier(&seen, &visited, [0, 10, 0], 12), Some([0, 10, 30]));
        let visited = vec![[0, 10, 0], [0, 10, 30]];
        assert_eq!(
            frontier(&seen, &visited, [0, 10, 30], 12),
            Some([40, 10, 0])
        );
        assert_eq!(frontier(&seen, &seen, [0, 10, 0], 12), None);
    }

    #[test]
    fn a_stand_reaches_the_empty_frames() {
        let ring = frame_ring([100, -50], 20);
        assert_eq!(ring.len(), 12);
        let set: HashSet<[i32; 3]> = ring.iter().copied().collect();
        assert_eq!(set.len(), 12);
        // Floor all round on the frames' level, nothing inside (lava below).
        let standable =
            |p: [i32; 3]| p[1] == 20 && ((p[0] - 100).abs() > 2 || (p[2] + 50).abs() > 2);
        let (at, n) = frame_stand(&ring, &ring, standable).unwrap();
        assert!(n >= 3, "{at:?} reaches {n}");
        assert!(!set.contains(&at));
        // Only the far side empty: it stands there.
        let empty: Vec<[i32; 3]> = ring.iter().copied().filter(|f| f[0] == 102).collect();
        let (at, n) = frame_stand(&ring, &empty, standable).unwrap();
        assert_eq!(n, 3);
        assert!(at[0] >= 102, "{at:?}");
    }

    #[test]
    fn legs_and_hud_lines() {
        assert_eq!(Leg::parse("blazes"), Some(Leg::Blazes));
        assert_eq!(Leg::parse("rods"), Some(Leg::Blazes));
        assert_eq!(Leg::Blazes.next(), Leg::Pearls);
        assert_eq!(Leg::Done.next(), Leg::Done);
        let t = Targets::default();
        let have = Carried {
            rods: 2,
            powder: 4,
            ..Default::default()
        };
        assert_eq!(step_text(Leg::Blazes, &have, &t), "Blaze rods 4/7");
        assert_eq!(
            step_text(
                Leg::Pearls,
                &Carried {
                    pearls: 9,
                    ..Default::default()
                },
                &t
            ),
            "Pearls 9/13"
        );
        assert_eq!(
            step_text(Leg::Triangulate, &have, &t),
            "Triangulating the stronghold"
        );
        assert!(!leg_met(Leg::Blazes, &have, &t));
        let have = Carried {
            rods: 3,
            eyes: 8,
            ..Default::default()
        };
        assert!(leg_met(Leg::Blazes, &have, &t));
    }
}
