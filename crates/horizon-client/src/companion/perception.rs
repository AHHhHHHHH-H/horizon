//! What a companion knows of the world (docs/companion-speedrun.md,
//! "Perception: one interface, four levels"). Every skill that looks for a
//! block asks through here, so the perception level is one switch:
//!
//! - **Fair** sees what a player could: a block is known once a ray from its
//!   eye reaches one of its faces through air, glass, plants or a little
//!   water, within its sight. What it saw stays in its memory until it sees
//!   the place changed.
//! - **Sharp senses** is Fair plus every block within [`SHARP_RADIUS`],
//!   through walls.
//! - **Scout** is Fair plus where structures are, as `/locate` says.
//! - **All-knowing** knows every block of the loaded world and the
//!   structures.
//!
//! The memory ([`Memory`]) is also the companion's knowledge of the world:
//! chests and what was in them, ores, lava, water, portals, structures, its
//! bed and dangers, with "nearest known X" queries. It is saved with the
//! companion ([`KnowledgeFile`]).
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// How much of the world a companion knows (a player setting; default Fair).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerceptionLevel {
    #[default]
    Fair,
    Sharp,
    Scout,
    AllKnowing,
}

/// Sharp senses see blocks through walls this far (blocks).
pub const SHARP_RADIUS: i32 = 16;

impl PerceptionLevel {
    pub const ALL: [Self; 4] = [Self::Fair, Self::Sharp, Self::Scout, Self::AllKnowing];

    /// `fair`, `sharp` (`sharp_senses`), `scout`, `all` (`all_knowing`).
    pub fn parse(word: &str) -> Option<Self> {
        Some(
            match word.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
                "fair" | "1" => Self::Fair,
                "sharp" | "sharp_senses" | "2" => Self::Sharp,
                "scout" | "3" => Self::Scout,
                "all" | "all_knowing" | "allknowing" | "omniscient" | "4" => Self::AllKnowing,
                _ => return None,
            },
        )
    }
    /// The id saved and typed (`all_knowing`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Fair => "fair",
            Self::Sharp => "sharp",
            Self::Scout => "scout",
            Self::AllKnowing => "all_knowing",
        }
    }
    /// The name the Watch HUD and run summaries show.
    pub fn label(self) -> &'static str {
        match self {
            Self::Fair => "Fair",
            Self::Sharp => "Sharp senses",
            Self::Scout => "Scout",
            Self::AllKnowing => "All-knowing",
        }
    }
    /// How far a search of radius `r` may see through walls: None when only
    /// what it saw counts.
    pub fn xray_radius(self, r: i32) -> Option<i32> {
        match self {
            Self::AllKnowing => Some(r),
            Self::Sharp => Some(r.min(SHARP_RADIUS)),
            Self::Fair | Self::Scout => None,
        }
    }
    /// Whether it knows where structures are without seeing them (`/locate`).
    pub fn knows_structures(self) -> bool {
        matches!(self, Self::Scout | Self::AllKnowing)
    }
}

// ---- seeing ---------------------------------------------------------------------

/// How sight passes a block state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Air, glass, plants, torches: sight goes on.
    Clear,
    /// Water: sight goes on for a few blocks.
    Water,
    /// Anything else stops sight; its face is seen.
    Opaque,
}

/// Whether a seen block is worth remembering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    No,
    /// Plain terrain (stone, dirt, leaves): kept, the nearest few thousand.
    Common,
    /// Ores, logs, lava, chests, stations, portals, structure blocks: kept.
    Notable,
}

/// The world as sight reads it.
pub trait SightWorld {
    /// The block state at `p`; None where nothing is loaded.
    fn state(&self, p: [i32; 3]) -> Option<u32>;
    fn look(&self, state: u32) -> Look;
    fn keep(&self, state: u32) -> Keep;
}

/// Sight reaches this far by default (a render distance of 4 chunks).
pub const SIGHT: f64 = 64.0;
/// Blocks of water sight goes through.
const WATER_SIGHT: u32 = 12;
/// Directions of the full sphere the rays take turns over.
const DIRECTIONS: u32 = 4096;
/// Plain terrain remembered at most; the farthest go first.
const COMMON_CAP: usize = 8192;
/// Notable blocks remembered at most.
const NOTABLE_CAP: usize = 40_000;

/// The `i`-th of `n` directions spread evenly over the sphere (a Fibonacci
/// spiral, so that consecutive turns land far apart).
pub fn sphere_direction(i: u32, n: u32) -> [f64; 3] {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let y = 1.0 - 2.0 * (f64::from(i) + 0.5) / f64::from(n);
    let r = (1.0 - y * y).max(0.0).sqrt();
    let a = golden * f64::from(i);
    [a.cos() * r, y, a.sin() * r]
}

/// Walk the blocks a ray from `from` along unit `dir` crosses, up to
/// `range` (Amanatides and Woo); `visit` returns false to stop.
pub fn walk_ray(
    from: [f64; 3],
    dir: [f64; 3],
    range: f64,
    mut visit: impl FnMut([i32; 3]) -> bool,
) {
    let mut cell = from.map(|v| v.floor() as i32);
    let mut step = [0i32; 3];
    let mut next = [f64::INFINITY; 3];
    let mut delta = [f64::INFINITY; 3];
    for k in 0..3 {
        if dir[k] > 1e-12 {
            step[k] = 1;
            delta[k] = 1.0 / dir[k];
            next[k] = (f64::from(cell[k]) + 1.0 - from[k]) / dir[k];
        } else if dir[k] < -1e-12 {
            step[k] = -1;
            delta[k] = -1.0 / dir[k];
            next[k] = (from[k] - f64::from(cell[k])) / -dir[k];
        }
    }
    loop {
        let k = if next[0] < next[1] {
            if next[0] < next[2] { 0 } else { 2 }
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        if next[k] > range {
            return;
        }
        cell[k] += step[k];
        next[k] += delta[k];
        if !visit(cell) {
            return;
        }
    }
}

/// A structure it knows of, and how.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnownStructure {
    /// `fortress`, `village`, `stronghold`, `bastion_remnant`, `ruined_portal`...
    pub kind: String,
    pub pos: [i32; 3],
    /// Located by the level (`/locate`), not seen.
    pub located: bool,
}

/// What it remembers of one dimension.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    notable: HashMap<[i32; 3], u32>,
    common: HashMap<[i32; 3], u32>,
    pub structures: Vec<KnownStructure>,
    /// Named places: `bed`, `portal`, `spawn`, `death`, `home`...
    pub places: BTreeMap<String, [i32; 3]>,
    /// What was in the chests it opened, by chest.
    pub chests: HashMap<[i32; 3], Vec<(String, u32)>>,
    /// Where it last looked from (eviction keeps what is near it).
    centre: [i32; 3],
}

impl Memory {
    pub fn len(&self) -> usize {
        self.notable.len() + self.common.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Every block it remembers, and its state.
    pub fn iter(&self) -> impl Iterator<Item = ([i32; 3], u32)> + '_ {
        self.notable
            .iter()
            .chain(self.common.iter())
            .map(|(p, s)| (*p, *s))
    }
    /// The state remembered at `p`.
    pub fn get(&self, p: [i32; 3]) -> Option<u32> {
        self.notable
            .get(&p)
            .or_else(|| self.common.get(&p))
            .copied()
    }
    pub fn remember(&mut self, p: [i32; 3], state: u32, keep: Keep) {
        match keep {
            Keep::No => self.forget(p),
            Keep::Common => {
                self.notable.remove(&p);
                self.common.insert(p, state);
                if self.common.len() > COMMON_CAP {
                    evict(&mut self.common, self.centre, COMMON_CAP * 3 / 4);
                }
            }
            Keep::Notable => {
                self.common.remove(&p);
                self.notable.insert(p, state);
                if self.notable.len() > NOTABLE_CAP {
                    evict(&mut self.notable, self.centre, NOTABLE_CAP * 3 / 4);
                }
            }
        }
    }
    pub fn forget(&mut self, p: [i32; 3]) {
        if self.notable.remove(&p).is_none() {
            self.common.remove(&p);
        }
    }
    /// The remembered blocks `wanted` holds of, within `radius` of
    /// `around`, nearest first.
    pub fn recall(
        &self,
        wanted: impl Fn(u32) -> bool,
        around: [i32; 3],
        radius: i32,
    ) -> Vec<[i32; 3]> {
        let r2 = i64::from(radius) * i64::from(radius);
        let mut out: Vec<(i64, [i32; 3])> = self
            .notable
            .iter()
            .chain(self.common.iter())
            .filter(|(_, s)| wanted(**s))
            .map(|(p, _)| (dist2(*p, around), *p))
            .filter(|(d, _)| *d <= r2)
            .collect();
        out.sort_unstable();
        out.into_iter().map(|(_, p)| p).collect()
    }
    /// The nearest remembered block `wanted` holds of.
    pub fn nearest(&self, wanted: impl Fn(u32) -> bool, around: [i32; 3]) -> Option<[i32; 3]> {
        self.notable
            .iter()
            .chain(self.common.iter())
            .filter(|(_, s)| wanted(**s))
            .min_by_key(|(p, _)| dist2(**p, around))
            .map(|(p, _)| *p)
    }
    /// The nearest structure of `kind` it knows of.
    pub fn structure(&self, kind: &str, around: [i32; 3]) -> Option<&KnownStructure> {
        self.structures
            .iter()
            .filter(|s| s.kind == kind)
            .min_by_key(|s| dist2(s.pos, around))
    }
    /// Learn of a structure (seen or located); the same kind within 48
    /// blocks is the same one.
    pub fn learn_structure(&mut self, kind: &str, pos: [i32; 3], located: bool) {
        match self
            .structures
            .iter_mut()
            .find(|s| s.kind == kind && dist2(s.pos, pos) <= 48 * 48)
        {
            Some(s) => s.located |= located,
            None => self.structures.push(KnownStructure {
                kind: kind.to_owned(),
                pos,
                located,
            }),
        }
    }
    /// Pools of the blocks `wanted` holds of (lava sources, water): groups of
    /// remembered blocks touching each other (sides or edges), as (the block
    /// nearest `around` of the pool, how many), nearest first.
    pub fn pools(
        &self,
        wanted: impl Fn(u32) -> bool,
        around: [i32; 3],
        radius: i32,
    ) -> Vec<([i32; 3], usize)> {
        let cells: Vec<[i32; 3]> = self.recall(&wanted, around, radius);
        let set: std::collections::HashSet<[i32; 3]> = cells.iter().copied().collect();
        let mut seen: std::collections::HashSet<[i32; 3]> = std::collections::HashSet::new();
        let mut out = Vec::new();
        for start in &cells {
            if !seen.insert(*start) {
                continue;
            }
            let mut stack = vec![*start];
            let mut count = 0;
            while let Some(p) = stack.pop() {
                count += 1;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                            if set.contains(&q) && seen.insert(q) {
                                stack.push(q);
                            }
                        }
                    }
                }
            }
            // `cells` is nearest first: the start is the pool's nearest block.
            out.push((*start, count));
        }
        out
    }
    fn look_from(&mut self, p: [i32; 3]) {
        self.centre = p;
    }
}

fn dist2(a: [i32; 3], b: [i32; 3]) -> i64 {
    (0..3).map(|k| i64::from(a[k] - b[k]).pow(2)).sum()
}

/// Keep the `keep` entries nearest `centre`.
fn evict(map: &mut HashMap<[i32; 3], u32>, centre: [i32; 3], keep: usize) {
    let mut d: Vec<i64> = map.keys().map(|p| dist2(*p, centre)).collect();
    if d.len() <= keep {
        return;
    }
    let (_, limit, _) = d.select_nth_unstable(keep);
    let limit = *limit;
    map.retain(|p, _| dist2(*p, centre) < limit);
}

/// A companion's senses: its perception level, what it remembers of each
/// dimension and where its rays are in their turn.
#[derive(Clone, Debug, Default)]
pub struct Senses {
    pub level: PerceptionLevel,
    memories: HashMap<String, Memory>,
    dim: String,
    turn: u32,
    /// Blocks it saw, in total (for tests and the cost line).
    pub seen: u64,
}

impl Senses {
    pub fn new(level: PerceptionLevel, dim: &str) -> Self {
        Self {
            level,
            dim: dim.to_owned(),
            ..Default::default()
        }
    }
    /// The dimension it is in now (its memory of it is the one used).
    pub fn enter(&mut self, dim: &str) {
        if self.dim != dim {
            self.dim = dim.to_owned();
        }
    }
    pub fn dimension(&self) -> &str {
        &self.dim
    }
    pub fn memory(&self) -> &Memory {
        static EMPTY: std::sync::OnceLock<Memory> = std::sync::OnceLock::new();
        self.memories
            .get(&self.dim)
            .unwrap_or_else(|| EMPTY.get_or_init(Memory::default))
    }
    pub fn memory_mut(&mut self) -> &mut Memory {
        self.memories.entry(self.dim.clone()).or_default()
    }
    pub fn memory_of(&self, dim: &str) -> Option<&Memory> {
        self.memories.get(dim)
    }

    /// Look around: `rays` rays from `eye`, the next directions of the
    /// sphere in turn, each to the first block that stops sight within
    /// `range`. What they reach is remembered; what they pass through and
    /// remembered as something else is forgotten (it changed).
    pub fn look<W: SightWorld>(&mut self, w: &W, eye: [f64; 3], rays: u32, range: f64) {
        let centre = eye.map(|v| v.floor() as i32);
        self.memory_mut().look_from(centre);
        for _ in 0..rays {
            // A stride coprime with the count visits every direction once
            // per turn, consecutive rays far apart.
            let i = self.turn.wrapping_mul(1597) % DIRECTIONS;
            self.turn = (self.turn + 1) % DIRECTIONS;
            let dir = sphere_direction(i, DIRECTIONS);
            self.cast(w, eye, dir, range);
        }
    }

    /// One ray at block `p`; true when it reaches it (the block is seen).
    pub fn look_at<W: SightWorld>(
        &mut self,
        w: &W,
        eye: [f64; 3],
        p: [i32; 3],
        range: f64,
    ) -> bool {
        let to = [
            f64::from(p[0]) + 0.5,
            f64::from(p[1]) + 0.5,
            f64::from(p[2]) + 0.5,
        ];
        let d = (0..3).map(|k| (to[k] - eye[k]).powi(2)).sum::<f64>().sqrt();
        if d < 1e-6 || d > range {
            return false;
        }
        let dir = [
            (to[0] - eye[0]) / d,
            (to[1] - eye[1]) / d,
            (to[2] - eye[2]) / d,
        ];
        self.cast(w, eye, dir, d + 1.0) == Some(p)
    }

    /// Cast one ray; the block that stopped it.
    fn cast<W: SightWorld>(
        &mut self,
        w: &W,
        eye: [f64; 3],
        dir: [f64; 3],
        range: f64,
    ) -> Option<[i32; 3]> {
        let mut water = 0;
        let mut stopped = None;
        let memory = self.memories.entry(self.dim.clone()).or_default();
        let mut seen = 0;
        walk_ray(eye, dir, range, |p| {
            let Some(state) = w.state(p) else {
                return false;
            };
            match w.look(state) {
                Look::Clear => {
                    match w.keep(state) {
                        Keep::No => {
                            if memory.get(p).is_some() {
                                memory.forget(p);
                            }
                        }
                        keep => {
                            memory.remember(p, state, keep);
                            seen += 1;
                        }
                    }
                    true
                }
                Look::Water => {
                    let keep = w.keep(state);
                    if keep != Keep::No {
                        memory.remember(p, state, keep);
                        seen += 1;
                    }
                    water += 1;
                    water < WATER_SIGHT
                }
                Look::Opaque => {
                    memory.remember(p, state, w.keep(state));
                    seen += 1;
                    stopped = Some(p);
                    false
                }
            }
        });
        self.seen += seen;
        stopped
    }

    /// The blocks `wanted` holds of that it knows within `radius` of
    /// `around`, nearest first: what it saw, plus, at a level that sees
    /// through walls, what `scan` finds within its x-ray radius. `scan`
    /// lists the blocks of the world within a radius (the game's palette
    /// search).
    pub fn known_blocks(
        &self,
        wanted: impl Fn(u32) -> bool,
        around: [i32; 3],
        radius: i32,
        scan: impl FnOnce(i32) -> Vec<[i32; 3]>,
    ) -> Vec<[i32; 3]> {
        let mut out = self.memory().recall(&wanted, around, radius);
        if let Some(r) = self.level.xray_radius(radius) {
            let mut seen: std::collections::HashSet<[i32; 3]> = out.iter().copied().collect();
            out.extend(scan(r).into_iter().filter(|p| seen.insert(*p)));
            out.sort_by_key(|p| dist2(*p, around));
        }
        out
    }

    // ---- saving --------------------------------------------------------------------

    /// What it knows, to save: the notable blocks by name (`name` of a
    /// state, its block name), structures, places and chests, per dimension.
    pub fn to_file(&self, name: impl Fn(u32) -> String) -> KnowledgeFile {
        let mut dims = BTreeMap::new();
        for (dim, m) in &self.memories {
            let mut blocks: BTreeMap<String, Vec<[i32; 3]>> = BTreeMap::new();
            for (p, s) in &m.notable {
                blocks.entry(name(*s)).or_default().push(*p);
            }
            for v in blocks.values_mut() {
                v.sort_unstable();
            }
            dims.insert(
                dim.clone(),
                DimFile {
                    blocks,
                    structures: m.structures.clone(),
                    places: m.places.clone(),
                    chests: m
                        .chests
                        .iter()
                        .map(|(p, items)| (format!("{} {} {}", p[0], p[1], p[2]), items.clone()))
                        .collect(),
                },
            );
        }
        KnowledgeFile {
            level: self.level,
            dims,
        }
    }

    /// Back from a file: `state` gives a block name's state (any of them:
    /// searches take every state of a block).
    pub fn from_file(file: &KnowledgeFile, dim: &str, state: impl Fn(&str) -> Option<u32>) -> Self {
        let mut s = Self::new(file.level, dim);
        for (d, f) in &file.dims {
            let m = s.memories.entry(d.clone()).or_default();
            for (name, ps) in &f.blocks {
                let Some(id) = state(name) else { continue };
                for p in ps {
                    m.notable.insert(*p, id);
                }
            }
            m.structures = f.structures.clone();
            m.places = f.places.clone();
            for (k, items) in &f.chests {
                let v: Vec<i32> = k
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if let [x, y, z] = v[..] {
                    m.chests.insert([x, y, z], items.clone());
                }
            }
        }
        s
    }
}

/// [`Senses`] as saved beside the companion's file (`<uuid>.knowledge`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeFile {
    pub level: PerceptionLevel,
    pub dims: BTreeMap<String, DimFile>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DimFile {
    pub blocks: BTreeMap<String, Vec<[i32; 3]>>,
    pub structures: Vec<KnownStructure>,
    pub places: BTreeMap<String, [i32; 3]>,
    pub chests: BTreeMap<String, Vec<(String, u32)>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small world: 1 = stone, 2 = iron ore, 3 = water, 4 = lava, 5 =
    /// glass, 6 = leaves; everything else air. Out of `size` is unloaded.
    struct Fake {
        blocks: HashMap<[i32; 3], u32>,
        size: i32,
    }

    impl Fake {
        fn new(size: i32) -> Self {
            Self {
                blocks: HashMap::new(),
                size,
            }
        }
        fn fill(&mut self, a: [i32; 3], b: [i32; 3], s: u32) {
            for x in a[0]..=b[0] {
                for y in a[1]..=b[1] {
                    for z in a[2]..=b[2] {
                        self.blocks.insert([x, y, z], s);
                    }
                }
            }
        }
        /// Every block of `s` within `r` of `c` (the x-ray scan).
        fn scan(&self, s: u32, c: [i32; 3], r: i32) -> Vec<[i32; 3]> {
            let mut v: Vec<[i32; 3]> = self
                .blocks
                .iter()
                .filter(|(p, b)| **b == s && dist2(**p, c) <= i64::from(r * r))
                .map(|(p, _)| *p)
                .collect();
            v.sort_by_key(|p| dist2(*p, c));
            v
        }
    }

    impl SightWorld for Fake {
        fn state(&self, p: [i32; 3]) -> Option<u32> {
            (p.iter().all(|v| v.abs() <= self.size))
                .then(|| self.blocks.get(&p).copied().unwrap_or(0))
        }
        fn look(&self, s: u32) -> Look {
            match s {
                0 | 5 => Look::Clear,
                3 => Look::Water,
                _ => Look::Opaque,
            }
        }
        fn keep(&self, s: u32) -> Keep {
            match s {
                0 | 5 => Keep::No,
                1 | 6 => Keep::Common,
                _ => Keep::Notable,
            }
        }
    }

    /// A stone room 21 wide around the origin (air inside from -9 to 9),
    /// walls 3 thick; ore on the inner wall (exposed), ore buried inside the
    /// wall (2 in), ore buried 30 blocks away.
    fn room() -> Fake {
        let mut w = Fake::new(64);
        w.fill([-12, -12, -12], [12, 12, 12], 1);
        w.fill([-9, -9, -9], [9, 9, 9], 0);
        w.blocks.insert([-9, 0, 0], 0);
        w.blocks.insert([-10, 0, 0], 2); // exposed: its face looks into the room
        w.blocks.insert([0, 0, 11], 2); // buried 2 blocks into the wall
        w.fill([28, -2, -2], [32, 2, 2], 1);
        w.blocks.insert([30, 0, 0], 2); // buried far away
        w
    }

    fn look_all(s: &mut Senses, w: &Fake) {
        s.look(w, [0.5, 0.5, 0.5], DIRECTIONS, SIGHT);
    }

    #[test]
    fn levels_parse_and_label() {
        for l in PerceptionLevel::ALL {
            assert_eq!(PerceptionLevel::parse(l.id()), Some(l));
        }
        assert_eq!(
            PerceptionLevel::parse("All-knowing"),
            Some(PerceptionLevel::AllKnowing)
        );
        assert_eq!(
            PerceptionLevel::parse("sharp senses"),
            Some(PerceptionLevel::Sharp)
        );
        assert_eq!(PerceptionLevel::default(), PerceptionLevel::Fair);
        assert_eq!(PerceptionLevel::AllKnowing.label(), "All-knowing");
        assert_eq!(PerceptionLevel::parse("x-ray"), None);
    }

    #[test]
    fn fair_sees_only_exposed_faces_in_sight() {
        let w = room();
        let mut s = Senses::new(PerceptionLevel::Fair, "overworld");
        look_all(&mut s, &w);
        let ore = |b: u32| b == 2;
        let known = s.known_blocks(ore, [0, 0, 0], 64, |r| w.scan(2, [0, 0, 0], r));
        assert_eq!(
            known,
            vec![[-10, 0, 0]],
            "only the ore whose face looks into the room"
        );
        // Fairness: every block it remembers had a clear ray from the eye.
        for p in s.memory().recall(|_| true, [0, 0, 0], 64) {
            let mut again = Senses::new(PerceptionLevel::Fair, "overworld");
            assert!(
                again.look_at(&w, [0.5, 0.5, 0.5], p, SIGHT) || {
                    // Grazing rays may reach a face the centre ray misses: a
                    // neighbour of the remembered block is in the room then.
                    [
                        [1, 0, 0],
                        [-1, 0, 0],
                        [0, 1, 0],
                        [0, -1, 0],
                        [0, 0, 1],
                        [0, 0, -1],
                    ]
                    .iter()
                    .any(|d| w.state([p[0] + d[0], p[1] + d[1], p[2] + d[2]]) == Some(0))
                },
                "{p:?} was never in sight"
            );
        }
        // Walls are remembered as common terrain, never the buried ones.
        assert!(s.memory().get([0, 0, 10]).is_some_and(|b| b == 1));
        assert!(s.memory().get([0, 0, 11]).is_none());
    }

    #[test]
    fn sharp_sees_through_walls_within_16() {
        let w = room();
        let mut s = Senses::new(PerceptionLevel::Sharp, "overworld");
        look_all(&mut s, &w);
        let known = s.known_blocks(
            |b| b == 2,
            [0, 0, 0],
            48,
            |r| {
                assert_eq!(r, SHARP_RADIUS);
                w.scan(2, [0, 0, 0], r)
            },
        );
        assert_eq!(known, vec![[-10, 0, 0], [0, 0, 11]]);
    }

    #[test]
    fn scout_sees_like_fair_but_knows_structures() {
        let w = room();
        let mut s = Senses::new(PerceptionLevel::Scout, "overworld");
        look_all(&mut s, &w);
        let known = s.known_blocks(|b| b == 2, [0, 0, 0], 48, |_| panic!("no x-ray"));
        assert_eq!(known, vec![[-10, 0, 0]]);
        assert!(PerceptionLevel::Scout.knows_structures());
        assert!(!PerceptionLevel::Fair.knows_structures());
        assert!(!PerceptionLevel::Sharp.knows_structures());
    }

    #[test]
    fn all_knowing_knows_every_block() {
        let w = room();
        let s = Senses::new(PerceptionLevel::AllKnowing, "overworld");
        let known = s.known_blocks(|b| b == 2, [0, 0, 0], 48, |r| w.scan(2, [0, 0, 0], r));
        assert_eq!(known, vec![[-10, 0, 0], [0, 0, 11], [30, 0, 0]]);
    }

    #[test]
    fn sight_passes_glass_and_a_little_water_and_stops_unloaded() {
        let mut w = Fake::new(40);
        w.fill([5, -1, -1], [5, 1, 1], 5); // glass pane wall
        w.blocks.insert([8, 0, 0], 2);
        let mut s = Senses::new(PerceptionLevel::Fair, "overworld");
        assert!(s.look_at(&w, [0.5, 0.5, 0.5], [8, 0, 0], SIGHT));
        // Under 20 blocks of water it sees nothing past 12.
        let mut w = Fake::new(40);
        w.fill([1, -1, -1], [20, 1, 1], 3);
        w.blocks.insert([21, 0, 0], 2);
        let mut s = Senses::new(PerceptionLevel::Fair, "overworld");
        assert!(!s.look_at(&w, [0.5, 0.5, 0.5], [21, 0, 0], SIGHT));
        assert!(
            s.memory().get([3, 0, 0]) == Some(3),
            "the water it saw is remembered"
        );
        // Nothing loaded past the world's edge: nothing seen there.
        let mut w = Fake::new(10);
        w.blocks.insert([12, 0, 0], 2);
        let mut s = Senses::new(PerceptionLevel::Fair, "overworld");
        assert!(!s.look_at(&w, [0.5, 0.5, 0.5], [12, 0, 0], SIGHT));
    }

    #[test]
    fn a_changed_block_is_forgotten_when_seen_again() {
        let mut w = room();
        let mut s = Senses::new(PerceptionLevel::Fair, "overworld");
        look_all(&mut s, &w);
        assert!(s.memory().get([-10, 0, 0]).is_some());
        // Mined: air now, and the stone behind it shows.
        w.blocks.insert([-10, 0, 0], 0);
        look_all(&mut s, &w);
        assert!(s.memory().get([-10, 0, 0]).is_none());
        assert_eq!(s.memory().get([-11, 0, 0]), Some(1));
    }

    #[test]
    fn memories_are_per_dimension_and_saved() {
        let w = room();
        let mut s = Senses::new(PerceptionLevel::Fair, "minecraft:overworld");
        look_all(&mut s, &w);
        s.memory_mut().places.insert("portal".into(), [1, 2, 3]);
        s.memory_mut()
            .learn_structure("village", [100, 64, 100], false);
        s.memory_mut()
            .learn_structure("village", [110, 64, 90], true);
        s.memory_mut()
            .chests
            .insert([4, 5, 6], vec![("minecraft:bread".into(), 3)]);
        assert_eq!(s.memory().structures.len(), 1, "the same village twice");
        assert!(s.memory().structures[0].located);
        s.enter("minecraft:the_nether");
        assert!(s.memory().is_empty());
        s.enter("minecraft:overworld");
        let file = s.to_file(|b| format!("b{b}"));
        let text = serde_json::to_string(&file).unwrap();
        let back: KnowledgeFile = serde_json::from_str(&text).unwrap();
        let t = Senses::from_file(&back, "minecraft:overworld", |n| n[1..].parse().ok());
        assert_eq!(t.memory().get([-10, 0, 0]), Some(2), "the ore");
        assert_eq!(
            t.memory().get([0, 0, 10]),
            None,
            "plain terrain is not saved"
        );
        assert_eq!(t.memory().places["portal"], [1, 2, 3]);
        assert_eq!(
            t.memory().chests[&[4, 5, 6]],
            vec![("minecraft:bread".to_string(), 3)]
        );
        assert_eq!(
            t.memory().structure("village", [0, 0, 0]).map(|s| s.pos),
            Some([100, 64, 100])
        );
    }

    #[test]
    fn nearest_known_and_pools() {
        let mut m = Memory::default();
        for x in 0..4 {
            for z in 0..3 {
                m.remember([x, 10, z], 4, Keep::Notable);
            }
        }
        m.remember([40, 10, 0], 4, Keep::Notable);
        m.remember([41, 10, 0], 4, Keep::Notable);
        m.remember([5, 5, 5], 2, Keep::Notable);
        assert_eq!(m.nearest(|b| b == 2, [0, 0, 0]), Some([5, 5, 5]));
        let pools = m.pools(|b| b == 4, [50, 10, 0], 100);
        assert_eq!(pools, vec![([41, 10, 0], 2), ([3, 10, 0], 12)]);
    }

    #[test]
    fn common_terrain_is_capped_nearest_kept() {
        let mut m = Memory::default();
        m.look_from([0, 0, 0]);
        for i in 0..(COMMON_CAP as i32 + 10) {
            m.remember([i, 0, 0], 1, Keep::Common);
        }
        assert!(m.len() <= COMMON_CAP);
        assert!(m.get([0, 0, 0]).is_some());
        assert!(
            m.get([COMMON_CAP as i32, 0, 0]).is_none(),
            "the farthest went"
        );
    }

    #[test]
    fn ray_walk_visits_cells_in_order() {
        let mut cells = Vec::new();
        walk_ray([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 3.0, |p| {
            cells.push(p);
            true
        });
        assert_eq!(cells, vec![[1, 0, 0], [2, 0, 0], [3, 0, 0]]);
        let mut n = 0;
        walk_ray([0.5, 0.5, 0.5], sphere_direction(7, 64), 10.0, |_| {
            n += 1;
            true
        });
        assert!(n >= 10);
    }
}
