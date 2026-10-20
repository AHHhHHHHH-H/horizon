//! Movement that changes the world (docs/companion-speedrun.md, "Movement
//! 2.0"): a path search for a player-sized body that may dig through
//! blocks, place blocks to bridge a gap or pillar up, step up and down,
//! fall a little, swim and staircase down, never into lava, fire or a big
//! fall. The world is read as [`Cell`]s through [`Terrain`] so that the
//! search is tested on made-up terrain (ravines, caves, mountains, rivers,
//! ocean edges); the game reads its blocks, the companion's tools and its
//! perception into it (an unknown block is planned as stone).
//!
//! A path is a list of [`Step`]s, each a move to a block (its feet) and the
//! blocks to dig and place on the way; the game's walker carries them out
//! (game/companion_walk.rs).
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// What a block is to a body going through it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Nothing in the way: air, plants, torches, snow layers.
    Open,
    /// Water: it swims; a fall into it is soft.
    Water,
    Lava,
    /// Never stepped into or stood on: fire, cactus, berry bushes, powder
    /// snow, magma.
    Hazard,
    /// A block in the way or to stand on: the ticks it takes to break with
    /// the tools it carries (0: it can't or mustn't), and whether it falls
    /// when what holds it goes (sand, gravel).
    Solid {
        dig: u16,
        falls: bool,
    },
    /// Not seen (Fair): planned as stone.
    Unknown,
}

impl Cell {
    pub fn passable(self) -> bool {
        matches!(self, Cell::Open | Cell::Water)
    }
    /// A floor to stand on.
    pub fn floor(self) -> bool {
        matches!(self, Cell::Solid { .. } | Cell::Unknown)
    }
    /// Ticks to clear it out of the way: Some(0) when it is clear already.
    fn clear_cost(self, stone: u16) -> Option<u32> {
        match self {
            Cell::Open | Cell::Water => Some(0),
            Cell::Solid { dig, .. } if dig > 0 => Some(u32::from(dig)),
            Cell::Unknown if stone > 0 => Some(u32::from(stone)),
            _ => None,
        }
    }
    /// A block may be placed in it (to stand on or to pillar).
    pub fn placeable(self) -> bool {
        matches!(self, Cell::Open | Cell::Water | Cell::Lava)
    }
}

/// The world as a path search reads it.
pub trait Terrain {
    fn cell(&self, p: [i32; 3]) -> Cell;
}

/// How a step is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    /// Walk to a side neighbour on the same level (on a block it may place
    /// first: a bridge).
    Walk,
    /// Jump up one onto a side neighbour.
    Up,
    /// Walk off an edge and fall onto the floor below (3 blocks at most; 16
    /// into water).
    Fall,
    /// Down one onto a side neighbour (a staircase step, dug if needed).
    Down,
    /// Jump and place a block under itself.
    Pillar,
    /// Dig the block under it and drop in (only onto a known floor).
    DigDown,
    /// Swim to a neighbour (sides, up, down) in water.
    Swim,
}

/// One step of a path: where its feet go, how, what to dig first (top
/// first) and where to place a block (a bridge's floor, a pillar's block).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub to: [i32; 3],
    pub how: Move,
    pub dig: Vec<[i32; 3]>,
    pub place: Option<[i32; 3]>,
}

/// A path found, and whether it reaches the goal (else it ends at the
/// nearest it got).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    pub steps: Vec<Step>,
    pub reached: bool,
    /// Blocks it places on the way.
    pub places: u32,
    pub digs: u32,
    /// The estimated ticks.
    pub cost: u32,
    /// Nodes the search opened.
    pub nodes: u32,
}

/// What the search may do and how it weighs it.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Blocks it carries to place (0: no bridging or pillaring).
    pub blocks: u32,
    /// Ticks to dig an unknown block (as stone with its tools; 0 when it
    /// can't).
    pub stone: u16,
    /// Whether it may dig at all.
    pub dig: bool,
    /// Nodes the search opens at most.
    pub budget: u32,
    /// Ticks a walked block costs (sprinting is ~3.6).
    pub walk: u32,
    /// Ticks a placed block costs on top of its move.
    pub place: u32,
    /// Extra ticks for each block dug (moving the view, the tool).
    pub dig_extra: u32,
    /// The lowest block it may stand on (above the void).
    pub min_y: i32,
    /// How much the estimate of what is left weighs (1: a shortest path;
    /// more: a quicker search for a longer path, since digging and
    /// placing cost much more per block than walking).
    pub greed: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            blocks: 0,
            stone: 0,
            dig: true,
            budget: 6000,
            walk: 5,
            place: 8,
            dig_extra: 4,
            min_y: -63,
            greed: 2.5,
        }
    }
}

/// Where a search may stop: within `near` blocks horizontally of `at` and
/// `vertical` up or down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Goal {
    pub at: [i32; 3],
    pub near: f64,
    pub vertical: i32,
}

impl Goal {
    pub fn exact(at: [i32; 3]) -> Self {
        Self {
            at,
            near: 0.0,
            vertical: 0,
        }
    }
    pub fn near(at: [i32; 3], near: f64) -> Self {
        Self {
            at,
            near,
            vertical: 1,
        }
    }
    /// Anywhere in the column (a long walk's waypoint).
    pub fn column(at: [i32; 3], near: f64) -> Self {
        Self {
            at,
            near,
            vertical: 512,
        }
    }
    pub fn reached(&self, p: [i32; 3]) -> bool {
        let (dx, dz) = (f64::from(p[0] - self.at[0]), f64::from(p[2] - self.at[2]));
        (dx * dx + dz * dz).sqrt() <= self.near + 1e-9 && (p[1] - self.at[1]).abs() <= self.vertical
    }
    /// What is left, in blocks, as the search's estimate.
    fn distance(&self, p: [i32; 3]) -> f64 {
        let (dx, dz) = (f64::from(p[0] - self.at[0]), f64::from(p[2] - self.at[2]));
        let h = ((dx * dx + dz * dz).sqrt() - self.near).max(0.0);
        let v = f64::from(((p[1] - self.at[1]).abs() - self.vertical).max(0));
        h + v
    }
}

const SIDES: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn up(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] + n, p[2]]
}

struct Node {
    p: [i32; 3],
    g: u32,
    placed: u32,
    parent: Option<usize>,
    step: Option<Step>,
}

/// Search a path from `from` (its feet) to `goal`.
pub fn find<T: Terrain>(t: &T, from: [i32; 3], goal: Goal, rules: &Rules) -> Route {
    let mut nodes: Vec<Node> = Vec::new();
    let mut index: HashMap<[i32; 3], usize> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(u32, u32, usize)>> = BinaryHeap::new();
    let h = |p: [i32; 3]| {
        (goal.distance(p) * f64::from(rules.walk.max(1)) * rules.greed.max(1.0)) as u32
    };
    nodes.push(Node {
        p: from,
        g: 0,
        placed: 0,
        parent: None,
        step: None,
    });
    index.insert(from, 0);
    heap.push(Reverse((h(from), h(from), 0)));
    let mut best = (h(from), 0usize);
    let mut opened = 0u32;
    let mut closed: HashSet<usize> = HashSet::new();
    let mut found = None;
    let mut moves: Vec<(Step, u32)> = Vec::with_capacity(16);
    while let Some(Reverse((_, _, i))) = heap.pop() {
        if !closed.insert(i) {
            continue;
        }
        let (p, g, placed) = (nodes[i].p, nodes[i].g, nodes[i].placed);
        if goal.reached(p) {
            found = Some(i);
            break;
        }
        let hi = h(p);
        if hi < best.0 || hi == best.0 && g < nodes[best.1].g {
            best = (hi, i);
        }
        opened += 1;
        if opened > rules.budget {
            break;
        }
        moves.clear();
        // What its own path dug and placed lately reads as it will be.
        let mut view = Overlay {
            t,
            dug: Vec::new(),
            placed: Vec::new(),
            stone: rules.stone.max(1),
        };
        let mut at = i;
        for _ in 0..24 {
            let Some(s) = nodes[at].step.as_ref() else {
                break;
            };
            view.dug.extend(s.dig.iter().copied());
            view.placed.extend(s.place);
            match nodes[at].parent {
                Some(parent) => at = parent,
                None => break,
            }
        }
        neighbours(
            &view,
            p,
            rules,
            rules.blocks.saturating_sub(placed),
            &mut moves,
        );
        for (step, cost) in moves.drain(..) {
            let ng = g + cost;
            let np = placed + u32::from(step.place.is_some());
            let to = step.to;
            let j = match index.get(&to) {
                Some(&j) if nodes[j].g <= ng || closed.contains(&j) => continue,
                Some(&j) => {
                    nodes[j].g = ng;
                    nodes[j].placed = np;
                    nodes[j].parent = Some(i);
                    nodes[j].step = Some(step);
                    j
                }
                None => {
                    nodes.push(Node {
                        p: to,
                        g: ng,
                        placed: np,
                        parent: Some(i),
                        step: Some(step),
                    });
                    index.insert(to, nodes.len() - 1);
                    nodes.len() - 1
                }
            };
            heap.push(Reverse((ng + h(to), h(to), j)));
        }
    }
    let (end, reached) = match found {
        Some(i) => (i, true),
        None => (best.1, false),
    };
    let mut steps = Vec::new();
    let mut at = end;
    while let Some(parent) = nodes[at].parent {
        if let Some(s) = nodes[at].step.clone() {
            steps.push(s);
        }
        at = parent;
    }
    steps.reverse();
    Route {
        places: steps.iter().filter(|s| s.place.is_some()).count() as u32,
        digs: steps.iter().map(|s| s.dig.len() as u32).sum(),
        cost: nodes[end].g,
        steps,
        reached,
        nodes: opened,
    }
}

/// Whether a block may be dug without letting lava in: no lava beside it or
/// above it (it would flow into the hole).
pub fn lava_free<T: Terrain>(t: &T, p: [i32; 3]) -> bool {
    [[0, 1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
        .iter()
        .all(|d| t.cell(add(p, *d)) != Cell::Lava)
}

/// The cost of clearing `cells` (top first), pushing those to dig; None when
/// one can't be cleared. A falling block above a dug one comes down into
/// it: that column costs again.
fn clear<T: Terrain>(
    t: &T,
    cells: &[[i32; 3]],
    rules: &Rules,
    dig: &mut Vec<[i32; 3]>,
) -> Option<u32> {
    let mut cost = 0;
    for &c in cells {
        let cell = t.cell(c);
        let ticks = cell.clear_cost(rules.stone)?;
        if ticks == 0 {
            continue;
        }
        if !rules.dig || !lava_free(t, c) {
            return None;
        }
        cost += ticks + rules.dig_extra;
        dig.push(c);
        // Gravel or sand stacked above falls in: dug again, a few times.
        let mut above = up(c, 1);
        let mut falls = 0;
        while let Cell::Solid {
            dig: d,
            falls: true,
        } = t.cell(above)
        {
            if !cells.contains(&above) {
                cost += u32::from(d.max(1)) + rules.dig_extra + 10;
            }
            falls += 1;
            if falls > 6 {
                return None;
            }
            above = up(above, 1);
        }
    }
    Some(cost)
}

/// Whether a body may stand with its feet in `p` once its cells are clear:
/// no lava or hazard at its feet or head.
fn body_ok<T: Terrain>(t: &T, p: [i32; 3]) -> bool {
    ![p, up(p, 1)]
        .iter()
        .any(|q| matches!(t.cell(*q), Cell::Lava | Cell::Hazard))
}

/// Extra ticks for standing beside lava (a slip, a flow).
fn lava_near<T: Terrain>(t: &T, p: [i32; 3]) -> u32 {
    let mut n = 0;
    for d in SIDES {
        for dy in -1..=1 {
            if t.cell(add(up(p, dy), d)) == Cell::Lava {
                n += 1;
            }
        }
    }
    if t.cell(up(p, -1)) == Cell::Lava {
        n += 2;
    }
    n * 12
}

/// The terrain as a path will have left it: what it dug is open, what it
/// placed is solid.
struct Overlay<'a, T: Terrain> {
    t: &'a T,
    dug: Vec<[i32; 3]>,
    placed: Vec<[i32; 3]>,
    stone: u16,
}

impl<T: Terrain> Terrain for Overlay<'_, T> {
    fn cell(&self, p: [i32; 3]) -> Cell {
        if self.placed.is_empty() && self.dug.is_empty() {
            return self.t.cell(p);
        }
        if self.placed.contains(&p) {
            Cell::Solid {
                dig: self.stone,
                falls: false,
            }
        } else if self.dug.contains(&p) {
            Cell::Open
        } else {
            self.t.cell(p)
        }
    }
}

fn neighbours<T: Terrain>(
    t: &T,
    p: [i32; 3],
    rules: &Rules,
    blocks: u32,
    out: &mut Vec<(Step, u32)>,
) {
    // Where it stands is clear (dug on the way if need be).
    let here = if t.cell(p) == Cell::Water {
        Cell::Water
    } else {
        Cell::Open
    };
    let swimming = here == Cell::Water;
    let floor = t.cell(up(p, -1));
    let standing = floor.floor() || swimming;
    let walk = rules.walk;
    for d in SIDES {
        let q = add(p, d);
        // Flat: walk, bridge or swim.
        let mut dig = Vec::new();
        if let Some(c) = clear(t, &[up(q, 1), q], rules, &mut dig)
            && body_ok(t, q)
        {
            let below = t.cell(up(q, -1));
            let wet = t.cell(q) == Cell::Water;
            let near = lava_near(t, q);
            if below.floor() || wet {
                let how = if wet || swimming {
                    Move::Swim
                } else {
                    Move::Walk
                };
                let cost = if wet { walk * 2 } else { walk } + c + near;
                out.push((
                    Step {
                        to: q,
                        how,
                        dig: dig.clone(),
                        place: None,
                    },
                    cost,
                ));
            } else if below.placeable() && blocks > 0 && floor.floor() {
                // A bridge: a block against the side of its own floor.
                out.push((
                    Step {
                        to: q,
                        how: Move::Walk,
                        dig: dig.clone(),
                        place: Some(up(q, -1)),
                    },
                    walk + rules.place + c + near + if below == Cell::Lava { 20 } else { 0 },
                ));
            }
            // A fall off the edge onto a floor below, or into water.
            if matches!(below, Cell::Open | Cell::Water) && !wet {
                let mut y = q[1] - 1;
                let mut depth = 0u32;
                loop {
                    let c2 = t.cell([q[0], y, q[2]]);
                    if c2 == Cell::Water {
                        // Into water: soft, whatever the height (16 at most).
                        if y > rules.min_y {
                            out.push((
                                Step {
                                    to: [q[0], y, q[2]],
                                    how: Move::Fall,
                                    dig: dig.clone(),
                                    place: None,
                                },
                                walk + c + 2 + depth,
                            ));
                        }
                        break;
                    }
                    if c2.floor() {
                        let land = [q[0], y + 1, q[2]];
                        if depth <= 3 && body_ok(t, land) && land[1] > rules.min_y {
                            out.push((
                                Step {
                                    to: land,
                                    how: Move::Fall,
                                    dig: dig.clone(),
                                    place: None,
                                },
                                walk + c + 2 + depth + lava_near(t, land),
                            ));
                        }
                        break;
                    }
                    if c2 != Cell::Open || depth >= 16 || y <= rules.min_y {
                        break;
                    }
                    depth += 1;
                    y -= 1;
                }
            }
        }
        // Up a step: head room above it, the step under the new feet.
        let q = add(up(p, 1), d);
        let mut dig = Vec::new();
        if standing
            && t.cell(up(q, -1)).floor()
            && let Some(c) = clear(t, &[up(p, 2), up(q, 1), q], rules, &mut dig)
            && body_ok(t, q)
        {
            out.push((
                Step {
                    to: q,
                    how: Move::Up,
                    dig,
                    place: None,
                },
                walk + 3 + c + lava_near(t, q),
            ));
        }
        // Down a step: through the side neighbour's column (head, feet), into
        // the block below it, onto a floor. Only when something is dug (else
        // it is a fall or a walk).
        let side = add(p, d);
        let q = up(side, -1);
        let mut dig = Vec::new();
        if standing
            && !swimming
            && t.cell(up(q, -1)).floor()
            && !t.cell(q).passable()
            && let Some(c) = clear(t, &[up(side, 1), side, q], rules, &mut dig)
            && !dig.is_empty()
            && body_ok(t, q)
            && body_ok(t, side)
            && q[1] > rules.min_y
        {
            out.push((
                Step {
                    to: q,
                    how: Move::Down,
                    dig,
                    place: None,
                },
                walk + 2 + c + lava_near(t, q),
            ));
        }
    }
    // Pillar: up one, a block placed where its feet were.
    let mut dig = Vec::new();
    if blocks > 0
        && floor.floor()
        && !swimming
        && here.placeable()
        && here != Cell::Lava
        && let Some(c) = clear(t, &[up(p, 2)], rules, &mut dig)
        && body_ok(t, up(p, 1))
    {
        out.push((
            Step {
                to: up(p, 1),
                how: Move::Pillar,
                dig,
                place: Some(p),
            },
            walk * 2 + rules.place + c,
        ));
    }
    // Dig straight down: only onto a floor it knows, never toward lava.
    let below = up(p, -1);
    let mut dig = Vec::new();
    if !swimming
        && matches!(t.cell(up(p, -2)), Cell::Solid { falls: false, .. })
        && let Some(c) = clear(t, &[below], rules, &mut dig)
        && !dig.is_empty()
        && body_ok(t, below)
        && lava_near(t, below) == 0
        && below[1] > rules.min_y
    {
        out.push((
            Step {
                to: below,
                how: Move::DigDown,
                dig,
                place: None,
            },
            walk + c + 2,
        ));
    }
    // Swimming up and down.
    if swimming {
        for dy in [1, -1] {
            let q = up(p, dy);
            let c = t.cell(q);
            if c == Cell::Water || dy == 1 && c == Cell::Open && body_ok(t, q) {
                out.push((
                    Step {
                        to: q,
                        how: Move::Swim,
                        dig: Vec::new(),
                        place: None,
                    },
                    walk * 2,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STONE: Cell = Cell::Solid {
        dig: 8,
        falls: false,
    };
    const GRAVEL: Cell = Cell::Solid {
        dig: 4,
        falls: true,
    };
    const BEDROCK: Cell = Cell::Solid {
        dig: 0,
        falls: false,
    };

    /// Terrain from a function of the position.
    struct Made<F: Fn([i32; 3]) -> Cell>(F);
    impl<F: Fn([i32; 3]) -> Cell> Terrain for Made<F> {
        fn cell(&self, p: [i32; 3]) -> Cell {
            (self.0)(p)
        }
    }

    /// Flat ground: stone at y < 0, air above.
    fn flat(p: [i32; 3]) -> Cell {
        if p[1] < 0 { STONE } else { Cell::Open }
    }

    fn rules(blocks: u32) -> Rules {
        Rules {
            blocks,
            stone: 8,
            ..Default::default()
        }
    }

    /// Replay a route on the terrain: every step's cells are clear once dug,
    /// its feet stand on a floor (or in water), no lava or hazard is entered
    /// or dug, places stay within what it carries. Where it ends.
    fn replay<T: Terrain>(t: &T, from: [i32; 3], r: &Route, carried: u32) -> [i32; 3] {
        let mut dug: Vec<[i32; 3]> = Vec::new();
        let mut placed: Vec<[i32; 3]> = Vec::new();
        let cell = |q: [i32; 3], dug: &Vec<[i32; 3]>, placed: &Vec<[i32; 3]>| {
            if placed.contains(&q) {
                STONE
            } else if dug.contains(&q) {
                Cell::Open
            } else {
                t.cell(q)
            }
        };
        let mut at = from;
        for s in &r.steps {
            for d in &s.dig {
                let c = cell(*d, &dug, &placed);
                // (The search doesn't know what its own path dug: a block
                // dug twice is dug once.)
                assert!(!matches!(c, Cell::Lava | Cell::Hazard), "digs {d:?}: {c:?}");
                assert!(c != Cell::Open || dug.contains(d), "digs air at {d:?}");
                dug.push(*d);
            }
            if let Some(p) = s.place {
                assert!(cell(p, &dug, &placed).placeable(), "places into {p:?}");
                placed.push(p);
            }
            let to = s.to;
            let (dx, dy, dz) = (to[0] - at[0], to[1] - at[1], to[2] - at[2]);
            assert!(
                dx.abs() + dz.abs() <= 1,
                "one block at a time: {at:?} -> {to:?}"
            );
            match s.how {
                Move::Fall => assert!(dy < 0),
                Move::Up | Move::Pillar => assert_eq!(dy, 1),
                Move::Down | Move::DigDown => assert_eq!(dy, -1),
                _ => {}
            }
            for q in [to, up(to, 1)] {
                let c = cell(q, &dug, &placed);
                assert!(c.passable(), "{:?} into {q:?}: {c:?}", s.how);
            }
            let floor = cell(up(to, -1), &dug, &placed);
            let wet = cell(to, &dug, &placed) == Cell::Water;
            assert!(
                floor.floor() || wet || s.how == Move::Swim,
                "{to:?} stands on {floor:?}"
            );
            at = to;
        }
        assert!(
            placed.len() as u32 <= carried,
            "placed {} of {carried}",
            placed.len()
        );
        at
    }

    #[test]
    fn walks_flat_ground_without_digging_or_placing() {
        let t = Made(flat);
        let r = find(&t, [0, 0, 0], Goal::exact([10, 0, 5]), &rules(64));
        assert!(r.reached);
        assert_eq!(r.steps.len(), 15);
        assert_eq!((r.places, r.digs), (0, 0));
        assert_eq!(replay(&t, [0, 0, 0], &r, 64), [10, 0, 5]);
    }

    #[test]
    fn bridges_a_ravine_with_blocks_and_climbs_out() {
        // A ravine 5 wide (x 3..=7) and 20 deep across the way, bedrock
        // walls beside so it can't go round.
        let ravine = |p: [i32; 3]| {
            if p[2].abs() > 2 && p[1] >= -20 {
                BEDROCK
            } else if (3..=7).contains(&p[0]) {
                if p[1] < -20 { STONE } else { Cell::Open }
            } else {
                flat(p)
            }
        };
        let t = Made(ravine);
        let r = find(&t, [0, 0, 0], Goal::exact([10, 0, 0]), &rules(16));
        assert!(r.reached);
        assert_eq!(r.places, 5, "a 5-block bridge: {:?}", r.steps);
        assert_eq!(replay(&t, [0, 0, 0], &r, 16), [10, 0, 0]);
        // Without blocks or digging it can't cross (the fall is 20): it gets
        // as near as it can.
        let mut no_blocks = rules(0);
        no_blocks.dig = false;
        let r = find(&t, [0, 0, 0], Goal::exact([10, 0, 0]), &no_blocks);
        assert!(!r.reached);
        assert_eq!(r.steps.last().map(|s| s.to), Some([2, 0, 0]));
        // From the bottom, with blocks: pillars or digs a staircase up the side.
        let r = find(&t, [5, -20, 0], Goal::exact([10, 0, 0]), &rules(32));
        assert!(r.reached, "climbs out of the ravine");
        replay(&t, [5, -20, 0], &r, 32);
    }

    #[test]
    fn climbs_out_of_a_hole_by_pillar_or_staircase() {
        // A 1x1 hole 6 deep.
        let hole = |p: [i32; 3]| {
            if p[0] == 0 && p[2] == 0 && (-6..0).contains(&p[1]) {
                Cell::Open
            } else {
                flat(p)
            }
        };
        let t = Made(hole);
        let r = find(&t, [0, -6, 0], Goal::near([3, 0, 0], 1.0), &rules(16));
        assert!(r.reached);
        replay(&t, [0, -6, 0], &r, 16);
        // No blocks: a staircase dug up through the stone.
        let r = find(&t, [0, -6, 0], Goal::near([3, 0, 0], 1.0), &rules(0));
        assert!(r.reached, "digs its way up");
        assert_eq!(r.places, 0);
        replay(&t, [0, -6, 0], &r, 0);
    }

    #[test]
    fn tunnels_through_a_mountain_or_goes_over() {
        // A stone ridge 12 high from x 4 to 8, falling away in steps of 3
        // past it, walls of bedrock beside.
        let mountain = |p: [i32; 3]| {
            let top = match p[0] {
                4..=8 => 12,
                9 => 9,
                10 => 6,
                11 => 3,
                _ => 0,
            };
            if p[2].abs() > 1 && p[1] >= 0 && p[1] < 40 {
                BEDROCK
            } else if p[1] < top {
                STONE
            } else {
                flat(p)
            }
        };
        let t = Made(mountain);
        let r = find(&t, [0, 0, 0], Goal::exact([16, 0, 0]), &rules(0));
        assert!(r.reached);
        assert!(r.digs > 0);
        replay(&t, [0, 0, 0], &r, 0);
        // Can't dig, has blocks: over the top by pillars.
        let mut over = rules(64);
        over.dig = false;
        over.budget = 20_000;
        let r = find(&t, [0, 0, 0], Goal::exact([16, 0, 0]), &over);
        assert!(r.reached, "pillars up and over");
        assert!(r.places >= 11, "{}", r.places);
        replay(&t, [0, 0, 0], &r, 64);
    }

    #[test]
    fn crosses_a_river_by_swimming() {
        let river = |p: [i32; 3]| {
            if p[2].abs() > 2 && p[1] >= -4 {
                BEDROCK
            } else if (3..=9).contains(&p[0]) {
                if p[1] < -4 {
                    STONE
                } else if p[1] < 0 {
                    Cell::Water
                } else {
                    Cell::Open
                }
            } else {
                flat(p)
            }
        };
        let t = Made(river);
        let mut r0 = rules(0);
        r0.dig = false;
        let r = find(&t, [0, 0, 0], Goal::exact([12, 0, 0]), &r0);
        assert!(r.reached, "{:?}", r.steps);
        assert!(r.steps.iter().any(|s| s.how == Move::Swim));
        replay(&t, [0, 0, 0], &r, 0);
    }

    #[test]
    fn never_into_lava_bridges_over_it() {
        // A lava pool 4 wide at floor level, in a corridor.
        let pool = |p: [i32; 3]| {
            if p[2].abs() > 1 && p[1] >= -1 {
                BEDROCK
            } else if (3..=6).contains(&p[0]) && p[1] == -1 {
                Cell::Lava
            } else {
                flat(p)
            }
        };
        let t = Made(pool);
        let mut r8 = rules(8);
        r8.dig = false;
        let r = find(&t, [0, 0, 0], Goal::exact([9, 0, 0]), &r8);
        assert!(r.reached);
        for s in &r.steps {
            assert!(t.cell(s.to) != Cell::Lava);
            assert!(t.cell(up(s.to, -1)) != Cell::Lava || s.place == Some(up(s.to, -1)));
        }
        replay(&t, [0, 0, 0], &r, 8);
        // Nothing to place: no way, rather than through the lava.
        let mut stuck = rules(0);
        stuck.dig = false;
        let r = find(&t, [0, 0, 0], Goal::exact([9, 0, 0]), &stuck);
        assert!(!r.reached);
    }

    #[test]
    fn never_digs_next_to_lava() {
        // A wall with lava behind its middle block.
        let wall = |p: [i32; 3]| {
            if p[0] == 5 && p[1] >= 0 && p[1] < 3 {
                STONE
            } else if p[0] == 6 && p[1] == 0 {
                Cell::Lava
            } else if p[2].abs() > 0 && p[1] >= 0 {
                BEDROCK
            } else {
                flat(p)
            }
        };
        let t = Made(wall);
        let r = find(&t, [0, 0, 0], Goal::exact([8, 0, 0]), &rules(0));
        for s in &r.steps {
            for d in &s.dig {
                assert!(lava_free(&t, *d), "dug {d:?} beside lava");
            }
        }
    }

    #[test]
    fn staircases_down_never_straight_into_the_unknown() {
        // Deep stone, the goal 10 below and 10 across.
        let t = Made(flat);
        let r = find(&t, [0, 0, 0], Goal::exact([10, -10, 0]), &rules(0));
        assert!(r.reached);
        replay(&t, [0, 0, 0], &r, 0);
        let unknown = |p: [i32; 3]| if p[1] < 0 { Cell::Unknown } else { Cell::Open };
        let t = Made(unknown);
        let r = find(&t, [0, 0, 0], Goal::exact([10, -10, 0]), &rules(0));
        assert!(r.reached);
        assert!(
            r.steps.iter().all(|s| s.how != Move::DigDown),
            "no digging straight down into the unknown"
        );
    }

    #[test]
    fn gravel_overhead_costs_more_than_stone() {
        // A stone block 3 wide to tunnel through; gravel over the middle lane.
        let t = Made(|p: [i32; 3]| {
            if p[2].abs() > 1 && p[1] >= 0 {
                BEDROCK
            } else if p[1] >= 0 && p[1] < 6 && (2..=8).contains(&p[0]) {
                if p[2] == 0 && p[1] >= 2 {
                    GRAVEL
                } else {
                    STONE
                }
            } else {
                flat(p)
            }
        });
        let r = find(&t, [0, 0, 1], Goal::exact([10, 0, 1]), &rules(0));
        assert!(r.reached);
        assert!(
            r.steps.iter().flat_map(|s| s.dig.iter()).all(|d| d[2] != 0),
            "keeps out from under the gravel: {:?}",
            r.steps
        );
    }

    #[test]
    fn ocean_edge_swims_out_and_back() {
        // Land for x < 3, then a deep ocean to x 20, land again.
        let ocean = |p: [i32; 3]| {
            if (3..20).contains(&p[0]) {
                if p[1] < -12 {
                    STONE
                } else if p[1] < 0 {
                    Cell::Water
                } else {
                    Cell::Open
                }
            } else {
                flat(p)
            }
        };
        let t = Made(ocean);
        let r = find(&t, [0, 0, 0], Goal::exact([22, 0, 0]), &rules(0));
        assert!(r.reached);
        replay(&t, [0, 0, 0], &r, 0);
    }

    #[test]
    fn caves_with_unbreakable_walls_route_round() {
        // Bedrock everywhere but a winding cave.
        let cave = |p: [i32; 3]| {
            let open = (p[1] == 0 || p[1] == 1)
                && (p[2] == 0 && (0..=5).contains(&p[0])
                    || p[0] == 5 && (0..=5).contains(&p[2])
                    || p[2] == 5 && (5..=10).contains(&p[0]));
            if open { Cell::Open } else { BEDROCK }
        };
        let t = Made(cave);
        let r = find(&t, [0, 0, 0], Goal::exact([10, 0, 5]), &rules(0));
        assert!(r.reached);
        assert_eq!(r.digs, 0);
        assert_eq!(r.steps.len(), 15);
    }

    #[test]
    fn a_column_goal_ends_anywhere_in_it() {
        let t = Made(flat);
        let r = find(&t, [0, 0, 0], Goal::column([40, 64, 0], 2.0), &rules(0));
        assert!(r.reached);
        assert!(r.steps.len() <= 40);
    }

    #[test]
    fn the_search_is_bounded() {
        // A goal it can't reach past a bedrock wall: the budget ends it.
        let t = Made(|p: [i32; 3]| {
            if p[1] < 0 || p[0] > 30 {
                BEDROCK
            } else {
                Cell::Open
            }
        });
        let r = find(
            &t,
            [0, 0, 0],
            Goal::exact([60, 0, 0]),
            &Rules {
                budget: 2000,
                ..rules(0)
            },
        );
        assert!(!r.reached);
        assert!(r.nodes <= 2001);
        assert_eq!(r.steps.last().map(|s| s.to[0]), Some(30));
    }
}
