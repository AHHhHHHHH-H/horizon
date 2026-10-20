//! A companion's work skills (docs/companions.md, "Skills"): collect, give,
//! sleep and the bow, on top of mining. Each runs as a small state machine on
//! the server tick, pressing what a player's client would send: a dig START
//! that the guest's server mines with survival break times, item drops, a
//! right click on a bed. Work comes as a stack of tasks: a craft that lacks
//! logs pushes a collect, which may push the craft of a pickaxe; the top one
//! runs. Crafting and smelting are in game/companion_craft.rs.
//!
//! Searches for blocks are spread over ticks (`BlockSearch`, a section's
//! palette first) so that no tick pays for a whole 48-block scan.
use super::{
    Dim, Game,
    companion::Body,
    companion_body::{Steer, look_pitch, mob_at, threat},
    companion_craft::{Craft, Smelt},
    lan::LanHost,
};
use horizon_client::{
    blocks::Fluid,
    companion::{
        self as c, CompanionCommand, CompanionReport, Skill, Want, gear,
        work::{self, BLOCK_REACH, SEARCH_RADIUS, STATION_RADIUS},
    },
    items::ItemStack,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// Block reads a search makes per tick at most (a whole section is 4096).
const SEARCH_BUDGET: u32 = 6 * 4096;
/// Ticks a target may take before it is given up as out of reach.
const TARGET_PATIENCE: u64 = 900;
/// Ticks one block may take to break before the dig is given up.
const DIG_PATIENCE: u64 = 600;
/// Ticks it walks to one dropped item before leaving it.
const DROP_PATIENCE: u64 = 160;
/// How deep a task stack may grow (make → collect → make a tool → collect).
const STACK: usize = 5;
/// Mobs a collect may hunt for their drops.
const PREY: [&str; 9] = [
    "minecraft:cow",
    "minecraft:mooshroom",
    "minecraft:pig",
    "minecraft:chicken",
    "minecraft:sheep",
    "minecraft:rabbit",
    "minecraft:spider",
    "minecraft:zombie",
    "minecraft:skeleton",
];

// ---- searching -------------------------------------------------------------------

/// A search of the loaded chunks around a point for some block states,
/// nearest sections first, a few sections a tick: a section whose palette
/// holds none of them is passed over unread.
pub(super) struct BlockSearch {
    wanted: Arc<Vec<bool>>,
    centre: [i32; 3],
    radius: i32,
    sections: Vec<(i32, usize, i32)>,
    next: usize,
    pub found: Vec<[i32; 3]>,
}

impl BlockSearch {
    pub fn new(wanted: Arc<Vec<bool>>, centre: [i32; 3], radius: i32) -> Self {
        use horizon_client::chunk::{MIN_Y, SECTIONS};
        let mut sections = Vec::new();
        let low = ((centre[1] - radius - MIN_Y) >> 4).max(0) as usize;
        let high = (((centre[1] + radius - MIN_Y) >> 4) as usize).min(SECTIONS - 1);
        for cx in (centre[0] - radius) >> 4..=(centre[0] + radius) >> 4 {
            for cz in (centre[2] - radius) >> 4..=(centre[2] + radius) >> 4 {
                for s in low..=high {
                    sections.push((cx, s, cz));
                }
            }
        }
        let middle = |(cx, s, cz): &(i32, usize, i32)| {
            let at = [cx * 16 + 8, MIN_Y + *s as i32 * 16 + 8, cz * 16 + 8];
            (0..3).map(|k| (at[k] - centre[k]).pow(2)).sum::<i32>()
        };
        sections.sort_by_key(middle);
        Self {
            wanted,
            centre,
            radius,
            sections,
            next: 0,
            found: Vec::new(),
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.sections.len()
    }

    /// Scan sections until `budget` block reads are spent; true when done.
    pub fn step(&mut self, world: &horizon_client::world::World, mut budget: u32) -> bool {
        use horizon_client::chunk::MIN_Y;
        let r2 = self.radius * self.radius;
        while budget > 0 && !self.done() {
            let (cx, s, cz) = self.sections[self.next];
            self.next += 1;
            budget = budget.saturating_sub(16);
            let Some(chunk) = world.chunks.get(&(cx, cz)) else {
                continue;
            };
            let section = &chunk.sections[s];
            let wanted = &self.wanted;
            if !section
                .blocks
                .may_contain(|v| wanted.get(v as usize).copied().unwrap_or(false))
            {
                continue;
            }
            budget = budget.saturating_sub(4096);
            let base = [cx * 16, MIN_Y + s as i32 * 16, cz * 16];
            for i in 0..4096 {
                if !wanted
                    .get(section.get(i) as usize)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let p = [
                    base[0] + (i & 15) as i32,
                    base[1] + (i >> 8) as i32,
                    base[2] + (i >> 4 & 15) as i32,
                ];
                if (0..3).map(|k| (p[k] - self.centre[k]).pow(2)).sum::<i32>() <= r2 {
                    self.found.push(p);
                }
            }
        }
        self.done()
    }
}

/// What the work skills cache across companions: the recipe index, what
/// each block and mob drops, and what their ticks cost.
#[derive(Default)]
pub(super) struct WorkCache {
    pub book: Option<Arc<super::companion_craft::BookIndex>>,
    /// Block name → the items its loot table gives without a tool match.
    pub block_drops: Option<Arc<HashMap<String, Vec<String>>>>,
    /// Both being built on a worker thread (`companion_prepare_work`).
    pub prepared: Option<
        std::thread::JoinHandle<(
            super::companion_craft::BookIndex,
            HashMap<String, Vec<String>>,
        )>,
    >,
    pub mob_drops: HashMap<String, Arc<Vec<String>>>,
    /// Each block's name and its states' ids (a block's states are listed together).
    pub blocks: std::sync::OnceLock<Vec<(String, std::ops::Range<usize>)>>,
    /// Work ticks since the last `companion cost`, their time and the worst.
    pub cost: (u32, Duration, Duration),
}

// ---- the work state ----------------------------------------------------------------

/// What a body's work skills keep between ticks (`Body::work`).
#[derive(Default)]
pub(super) struct Work {
    /// The tasks, the one running last.
    pub tasks: Vec<Task>,
    /// The block its guest is digging, and since when.
    pub dig: Option<([i32; 3], u64)>,
    /// The tick it started drawing its bow at the fight's target.
    pub draw: Option<u64>,
    /// The menu window it has open (0: its inventory's grid).
    pub menu: Option<i32>,
    /// The night (day number) it last went to bed with its owner.
    pub slept: Option<i64>,
    /// Shut in where walking gets it nowhere (a mine it dug): a staircase
    /// step it digs toward where it goes, until a tick, and how many times.
    tunnel: Option<Stair>,
    tunnel_until: Option<u64>,
    tunnels: u32,
}

impl Work {
    /// Its hands are on a tool, a bow or a menu: its gear stays as it is.
    pub fn busy_hands(&self) -> bool {
        self.dig.is_some() || self.draw.is_some() || self.menu.is_some()
    }
    /// The command that would take up the work again (saved with the world):
    /// the first task, with what is left of it.
    pub fn saved(&self) -> Option<String> {
        self.tasks.first().and_then(Task::left)?.words()
    }
    pub fn status(&self) -> Value {
        match self.tasks.last() {
            Some(t) => {
                let (done, total) = t.progress();
                json!({ "skill": t.skill.name(), "done": done, "total": total, "depth": self.tasks.len() })
            }
            None => Value::Null,
        }
    }
}

/// One task of the stack.
pub(super) struct Task {
    pub skill: Skill,
    pub kind: Kind,
    /// The skill to go back to when the stack is done.
    pub resume: Skill,
    pub dim: Dim,
    /// The progress last reported.
    pub reported: u32,
}

pub(super) enum Kind {
    Collect(Box<Collect>),
    Give(Give),
    Craft(Box<Craft>),
    Smelt(Box<Smelt>),
    Sleep(SleepIn),
}

impl Task {
    fn progress(&self) -> (u32, u32) {
        match &self.kind {
            Kind::Collect(k) => (k.done, k.count),
            Kind::Give(k) => (k.given, k.count),
            Kind::Craft(k) => (k.made, k.count),
            Kind::Smelt(k) => (k.made, k.count),
            Kind::Sleep(_) => (0, 1),
        }
    }
    /// The command for what is left to do.
    fn left(&self) -> Option<CompanionCommand> {
        let (done, total) = self.progress();
        let count = total.saturating_sub(done).max(1);
        Some(match &self.kind {
            Kind::Collect(k) => CompanionCommand::Collect {
                what: k.want.clone(),
                count,
            },
            Kind::Give(k) => CompanionCommand::Give {
                what: k.want.clone(),
                count,
            },
            Kind::Craft(k) => CompanionCommand::Craft {
                item: k.item.clone(),
                count,
                gather: k.gather,
            },
            Kind::Smelt(k) => CompanionCommand::Smelt {
                item: k.asked.clone(),
                count,
            },
            Kind::Sleep(_) => return None,
        })
    }
    fn item(&self) -> String {
        match &self.kind {
            Kind::Collect(k) => k.want.id().to_owned(),
            Kind::Give(k) => k.want.id().to_owned(),
            Kind::Craft(k) => k.item.clone(),
            Kind::Smelt(k) => k.output.clone(),
            Kind::Sleep(_) => "bed".into(),
        }
    }
}

/// What a task's tick came to.
pub(super) enum Outcome {
    Busy,
    Done,
    Failed(String),
    /// A task to do first (it stays below it).
    Push(Task),
}

/// Collect: what counts, where it comes from and where it is at.
pub(super) struct Collect {
    pub want: Want,
    pub count: u32,
    pub done: u32,
    /// The items that count, and how many it carried last tick.
    items: Vec<String>,
    last: u32,
    /// Block states it mines, by state id.
    states: Arc<Vec<bool>>,
    /// Mob kinds whose drops count.
    prey: Vec<String>,
    search: Option<BlockSearch>,
    searches: u32,
    target: Option<Target>,
    bad: Vec<[i32; 3]>,
    /// Drops to pick up around a point until a tick.
    pickup: Option<([f64; 3], u64)>,
    reaching: Option<(u32, u64)>,
    skip: Vec<u32>,
    hunting: Option<(u32, [f64; 3])>,
    /// It left one alone because digging it would let lava in.
    unsafe_seen: bool,
    /// A staircase step toward a drop it can't walk to, and the drop it digs toward.
    reach: Option<Stair>,
    dig_to: Option<u32>,
}

/// The block a collect works on, and where it stands for it.
struct Target {
    pos: [i32; 3],
    spot: Option<[f64; 3]>,
    tried: Vec<[f64; 3]>,
    stair: Option<Stair>,
    since: u64,
}

/// A step of a staircase toward a block below: the blocks to clear (top
/// first) and where its feet go.
struct Stair {
    clear: Vec<[i32; 3]>,
    to: [i32; 3],
}

/// How a staircase step goes.
enum StairGo {
    Busy,
    There,
    Failed,
}

pub(super) struct Give {
    want: Want,
    items: Vec<String>,
    count: u32,
    given: u32,
}

pub(super) struct SleepIn {
    /// Its owner went to bed: it gets up with them.
    with_owner: bool,
    search: Option<BlockSearch>,
    bed: Option<[i32; 3]>,
    tries: u32,
}

/// What reaching a block from where it stands comes to.
pub(super) enum Access {
    /// In reach and in sight: the face its look hits.
    Direct(u8),
    /// In reach, behind a natural block it can dig first.
    Through([i32; 3], u8),
    None,
}

/// What a dig came to this tick.
enum DigStep {
    Busy,
    Broken,
    Failed(String),
}

/// `Direction.get3DDataValue` of a face's normal.
fn face_index(face: [i32; 3]) -> u8 {
    match face {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        _ => 5,
    }
}

pub(super) fn centre(p: [i32; 3]) -> [f64; 3] {
    [
        f64::from(p[0]) + 0.5,
        f64::from(p[1]) + 0.5,
        f64::from(p[2]) + 0.5,
    ]
}

pub(super) fn feet(pos: [f64; 3]) -> [i32; 3] {
    [
        pos[0].floor() as i32,
        (pos[1] + 0.01).floor() as i32,
        pos[2].floor() as i32,
    ]
}

pub(super) fn eye(pos: [f64; 3]) -> [f64; 3] {
    [pos[0], pos[1] + 1.62, pos[2]]
}

/// The distance from `eye` to the nearest point of block `p`.
pub(super) fn to_block(eye: [f64; 3], p: [i32; 3]) -> f64 {
    let q: [f64; 3] = std::array::from_fn(|k| eye[k].clamp(f64::from(p[k]), f64::from(p[k]) + 1.0));
    c::distance(eye, q)
}

/// How many of `items` a stack list holds.
fn carried_any(inventory: &[Option<ItemStack>], items: &[String]) -> u32 {
    inventory
        .iter()
        .flatten()
        .filter(|s| items.iter().any(|i| **i == *s.item))
        .map(|s| s.count)
        .sum()
}

/// Block name → the items its loot table gives without a tool match.
fn build_block_drops(res: &super::Resources) -> HashMap<String, Vec<String>> {
    let items = res.items();
    let mut drops: HashMap<String, Vec<String>> = HashMap::new();
    let mut last = "";
    for f in &res.facts().states {
        // A block's states are listed together.
        if *f.name == *last || drops.contains_key(&*f.name) {
            continue;
        }
        last = &f.name;
        let table = format!("minecraft:blocks/{}", c::short_id(&f.name));
        let got = items
            .loot_json(&table)
            .map(work::loot_items)
            .unwrap_or_default();
        drops.insert(f.name.to_string(), got);
    }
    drops
}

impl Game<'_> {
    // ---- commands -------------------------------------------------------------------

    /// A work command: it replaces the work it had on.
    pub(super) fn companion_start_work(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        command: CompanionCommand,
    ) {
        self.companion_cancel_work(host, b, None);
        let resume = match b.skill {
            s @ (Skill::Follow | Skill::Stay | Skill::Defend | Skill::Idle) => s,
            _ => Skill::Follow,
        };
        let skill = match &command {
            CompanionCommand::Collect { .. } => Skill::Collect,
            CompanionCommand::Give { .. } => Skill::Give,
            CompanionCommand::Craft { .. } => Skill::Craft,
            CompanionCommand::Smelt { .. } => Skill::Smelt,
            _ => Skill::Sleep,
        };
        match self.companion_task(host, b, command, resume) {
            Ok(task) => {
                b.work.tasks.push(task);
                b.target = None;
                b.nav.reset();
                b.skill = skill;
                self.companion_report(b, CompanionReport::SkillStarted(skill));
            }
            Err(reason) => self.companion_report(b, CompanionReport::SkillFailed { skill, reason }),
        }
    }

    /// A task for a command, its item names checked.
    pub(super) fn companion_task(
        &mut self,
        host: &LanHost,
        b: &Body,
        command: CompanionCommand,
        resume: Skill,
    ) -> Result<Task, String> {
        let _span = horizon_client::spans::Span::new("companion_task");
        let g = host.hub.guest(b.conn).ok_or("no body")?;
        let (skill, kind) = match command {
            CompanionCommand::Collect { what, count } => {
                let k = self.companion_collect(what, count, &g.inventory)?;
                (Skill::Collect, Kind::Collect(Box::new(k)))
            }
            CompanionCommand::Give { what, count } => {
                let want = self.resolve_want(what)?;
                let items = self.want_items(&want);
                (
                    Skill::Give,
                    Kind::Give(Give {
                        want,
                        items,
                        count,
                        given: 0,
                    }),
                )
            }
            CompanionCommand::Craft {
                item,
                count,
                gather,
            } => {
                let item = self.companion_item_id(&item)?;
                let k = Craft::new(item, count, gather, &g.inventory);
                if self.companion_book().recipes_for_item(&k.item).is_empty() {
                    return Err(format!("{} can't be crafted", c::short_id(&k.item)));
                }
                (Skill::Craft, Kind::Craft(Box::new(k)))
            }
            CompanionCommand::Smelt { item, count } => {
                let item = self.companion_item_id(&item)?;
                let k = self.companion_smelt_task(&item, count, &g.inventory)?;
                (Skill::Smelt, Kind::Smelt(Box::new(k)))
            }
            _ => (
                Skill::Sleep,
                Kind::Sleep(SleepIn {
                    with_owner: false,
                    search: None,
                    bed: None,
                    tries: 0,
                }),
            ),
        };
        Ok(Task {
            skill,
            kind,
            resume,
            dim: b.dim,
            reported: 0,
        })
    }

    /// Stop all work: the dig, the bow and the menus let go; a report when
    /// `reason` says why.
    pub(super) fn companion_cancel_work(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        reason: Option<&str>,
    ) {
        self.companion_stop_dig(host, b);
        self.companion_close_menu(host, b);
        if b.work.draw.take().is_some() {
            // Another slot selected: the draw stops without a shot.
            let slot = host.hub.guest(b.conn).map_or(0, |g| g.selected as i16);
            self.lan_guest_packet(
                host,
                b.conn,
                S::SetCarriedItem(play::SetCarriedItem { slot }),
            );
        }
        let Some(first) = b.work.tasks.first() else {
            return;
        };
        let (skill, resume) = (first.skill, first.resume);
        let sleeping = b
            .work
            .tasks
            .iter()
            .any(|t| matches!(t.kind, Kind::Sleep(_)));
        b.work.tasks.clear();
        if sleeping && let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) {
            host.hub.wake(gi);
        }
        if b.skill == skill
            || matches!(
                b.skill,
                Skill::Collect | Skill::Give | Skill::Craft | Skill::Smelt | Skill::Sleep
            )
        {
            b.skill = resume;
        }
        if let Some(reason) = reason {
            self.companion_report(
                b,
                CompanionReport::SkillFailed {
                    skill,
                    reason: reason.into(),
                },
            );
        }
    }

    // ---- the tick ---------------------------------------------------------------------

    /// Before it picks a fight: it goes to bed when its owner does.
    pub(super) fn companion_work_reflexes(&mut self, host: &mut LanHost, b: &mut Body) {
        // Work left over from a skill something else ended (a respawn).
        let working = matches!(
            b.skill,
            Skill::Collect
                | Skill::Give
                | Skill::Craft
                | Skill::Smelt
                | Skill::Sleep
                | Skill::Attack
        );
        if !working && !b.work.tasks.is_empty() {
            self.companion_cancel_work(host, b, None);
        }
        let day = self.world.day_time.div_euclid(24000);
        let free = matches!(
            b.skill,
            Skill::Follow | Skill::Stay | Skill::Defend | Skill::Idle
        );
        if self.sleeping.is_some()
            && free
            && b.target.is_none()
            && b.work.tasks.is_empty()
            && b.work.slept != Some(day)
        {
            b.work.slept = Some(day);
            b.work.tasks.push(Task {
                skill: Skill::Sleep,
                kind: Kind::Sleep(SleepIn {
                    with_owner: true,
                    search: None,
                    bed: None,
                    tries: 0,
                }),
                resume: b.skill,
                dim: b.dim,
                reported: 0,
            });
            b.skill = Skill::Sleep;
            b.nav.reset();
            self.companion_report(b, CompanionReport::SkillStarted(Skill::Sleep));
        }
    }

    /// A work skill's tick (`companion_move` for Collect, Give, Craft, Smelt
    /// and Sleep): the top task runs.
    pub(super) fn companion_work(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        pos: [f64; 3],
        owner: [f64; 3],
    ) {
        let started = std::time::Instant::now();
        let _span = horizon_client::spans::Span::new("companion_work");
        let Some(mut task) = b.work.tasks.pop() else {
            b.skill = Skill::Follow;
            return;
        };
        let outcome = if task.dim != b.dim {
            Outcome::Failed("left for another dimension".into())
        } else if b.eating.is_some() {
            Outcome::Busy
        } else {
            match &mut task.kind {
                Kind::Collect(k) => self.collect_tick(host, b, k, pos),
                Kind::Give(k) => self.give_tick(host, b, k, pos, owner),
                Kind::Craft(k) => self.craft_tick(host, b, k, pos),
                Kind::Smelt(k) => self.smelt_tick(host, b, k, pos),
                Kind::Sleep(k) => self.sleep_tick(host, b, k, pos),
            }
        };
        let (done, total) = task.progress();
        if done != task.reported && !matches!(task.kind, Kind::Sleep(_)) {
            task.reported = done;
            let report = CompanionReport::Progress {
                skill: task.skill,
                item: task.item(),
                done: done.min(total),
                total,
            };
            self.companion_report(b, report);
        }
        match outcome {
            Outcome::Busy => b.work.tasks.push(task),
            Outcome::Push(next) if b.work.tasks.len() + 2 > STACK => {
                self.companion_task_failed(
                    host,
                    b,
                    task,
                    format!("too much to do first ({})", next.skill.name()),
                );
            }
            Outcome::Push(next) => {
                let skill = next.skill;
                b.work.tasks.push(task);
                b.work.tasks.push(next);
                b.skill = skill;
                b.nav.reset();
                self.companion_report(b, CompanionReport::SkillStarted(skill));
            }
            Outcome::Done => {
                self.companion_stop_dig(host, b);
                self.companion_close_menu(host, b);
                self.companion_report(b, CompanionReport::SkillFinished(task.skill));
                b.nav.reset();
                match b.work.tasks.last() {
                    Some(parent) => b.skill = parent.skill,
                    None => b.skill = task.resume,
                }
            }
            Outcome::Failed(reason) => self.companion_task_failed(host, b, task, reason),
        }
        let took = started.elapsed();
        let cost = &mut self.companions.work.cost;
        *cost = (cost.0 + 1, cost.1 + took, cost.2.max(took));
    }

    /// A task failed: a gathering craft that lacked a tool makes it first;
    /// else the whole stack fails with the reason.
    fn companion_task_failed(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        task: Task,
        reason: String,
    ) {
        self.companion_stop_dig(host, b);
        self.companion_close_menu(host, b);
        let gathering = b
            .work
            .tasks
            .last()
            .is_some_and(|t| matches!(&t.kind, Kind::Craft(k) if k.gather));
        if let Some(tool) = reason.strip_prefix("needs a ").filter(|_| gathering)
            && matches!(task.kind, Kind::Collect(_))
            && b.work.tasks.len() + 2 <= STACK
        {
            let tool = c::namespaced(&tool.replace(' ', "_"));
            let gi = host.hub.guests.iter().position(|g| g.conn == b.conn);
            let inventory = gi
                .map(|i| host.hub.guests[i].inventory.clone())
                .unwrap_or_default();
            let make = Task {
                skill: Skill::Craft,
                kind: Kind::Craft(Box::new(Craft::new(tool, 1, true, &inventory))),
                resume: task.resume,
                dim: b.dim,
                reported: 0,
            };
            // The collect again once the tool is made.
            let mut again = task;
            if let Kind::Collect(k) = &mut again.kind {
                k.target = None;
                k.bad.clear();
            }
            b.work.tasks.push(again);
            b.work.tasks.push(make);
            b.skill = Skill::Craft;
            self.companion_report(b, CompanionReport::SkillStarted(Skill::Craft));
            return;
        }
        // A failure below fails what it served: the first task says why.
        let first = b.work.tasks.first().map_or(task.skill, |t| t.skill);
        let resume = b.work.tasks.first().map_or(task.resume, |t| t.resume);
        let reason = if first == task.skill && b.work.tasks.is_empty() {
            reason
        } else {
            format!(
                "{} {}: {reason}",
                task.skill.name(),
                c::short_id(&task.item())
            )
        };
        b.work.tasks.clear();
        b.skill = resume;
        b.nav.reset();
        self.companion_report(
            b,
            CompanionReport::SkillFailed {
                skill: first,
                reason,
            },
        );
    }

    // ---- items and wants -------------------------------------------------------------

    /// An item id players type: `oak_log`, `oak_logs`, `torches`.
    pub(super) fn companion_item_id(&self, id: &str) -> Result<String, String> {
        let items = self.res.items();
        let id = c::namespaced(id);
        let candidates = [
            id.clone(),
            id.strip_suffix('s').map(str::to_owned).unwrap_or_default(),
            id.strip_suffix("es").map(str::to_owned).unwrap_or_default(),
        ];
        candidates
            .into_iter()
            .find(|i| !i.is_empty() && items.get(i).is_some())
            .ok_or_else(|| format!("I don't know {}", c::short_id(&id)))
    }

    /// A want with a known item, tag or block (`planks` is the tag).
    fn resolve_want(&self, want: Want) -> Result<Want, String> {
        let items = self.res.items();
        match want {
            Want::Item(id) => match self.companion_item_id(&id) {
                Ok(i) => Ok(Want::Item(i)),
                Err(e) => {
                    let tag = id.clone();
                    let plural = format!("{tag}s");
                    [tag, plural]
                        .into_iter()
                        .find(|t| items.item_tags.contains_key(t))
                        .map(Want::Tag)
                        .ok_or(e)
                }
            },
            Want::Tag(t) if items.item_tags.contains_key(&t) => Ok(Want::Tag(t)),
            Want::Tag(t) => Err(format!("no item tag #{}", c::short_id(&t))),
            Want::Block(id) => {
                let known = self.res.facts().states.iter().any(|f| *f.name == *id);
                if known {
                    Ok(Want::Block(id))
                } else {
                    Err(format!("no block {}", c::short_id(&id)))
                }
            }
        }
    }

    /// The items a want counts.
    fn want_items(&mut self, want: &Want) -> Vec<String> {
        match want {
            Want::Item(i) => vec![i.clone()],
            Want::Tag(t) => self
                .res
                .items()
                .item_tag_order
                .get(t)
                .cloned()
                .unwrap_or_default(),
            Want::Block(b) => self.block_drops().get(b).cloned().unwrap_or_default(),
        }
    }

    /// What each block drops without a tool match (`work::loot_items`):
    /// built on a worker thread when the first body joined, else here.
    fn block_drops(&mut self) -> Arc<HashMap<String, Vec<String>>> {
        self.companion_prepared_work();
        if let Some(d) = &self.companions.work.block_drops {
            return d.clone();
        }
        let _span = horizon_client::spans::Span::new("companion_block_drops");
        let drops = Arc::new(build_block_drops(&self.res));
        self.companions.work.block_drops = Some(drops.clone());
        drops
    }

    /// A world opened (or a body joined): the work skills' indexes of the
    /// game's data (recipes, block drops) are built on a worker thread, not
    /// by the first order's tick (~1–1.5 ms each on an M4 Pro).
    pub(super) fn companion_prepare_work(&mut self) {
        let cache = &mut self.companions.work;
        if cache.book.is_some() || cache.block_drops.is_some() || cache.prepared.is_some() {
            return;
        }
        let res = self.res.clone();
        cache.prepared = std::thread::Builder::new()
            .name("companion work".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let built = (
                    super::companion_craft::build_book(&res),
                    build_block_drops(&res),
                );
                if horizon_client::spans::enabled() {
                    eprintln!(
                        "Horizon: companion work indexes built in {:.2} ms",
                        started.elapsed().as_secs_f32() * 1000.0
                    );
                }
                built
            })
            .ok();
    }

    /// The indexes `companion_prepare_work` built, once they are asked for.
    pub(super) fn companion_prepared_work(&mut self) {
        let cache = &mut self.companions.work;
        let joined = cache.prepared.take().map(|h| {
            let _span = horizon_client::spans::Span::new("companion_work_join");
            h.join()
        });
        let Some(Ok((book, drops))) = joined else {
            return;
        };
        cache.book.get_or_insert_with(|| Arc::new(book));
        cache.block_drops.get_or_insert_with(|| Arc::new(drops));
    }

    /// What a mob kind drops (`minecraft:entities/<kind>`), once per kind.
    fn mob_drops(&mut self, kind: &str) -> Arc<Vec<String>> {
        if let Some(d) = self.companions.work.mob_drops.get(kind) {
            return d.clone();
        }
        let table = format!("minecraft:entities/{}", c::short_id(kind));
        let got = Arc::new(
            self.res
                .items()
                .loot_json(&table)
                .map(work::loot_items)
                .unwrap_or_default(),
        );
        self.companions
            .work
            .mob_drops
            .insert(kind.to_owned(), got.clone());
        got
    }

    /// The block states where `pred` holds of the state's name, by id.
    /// (`pred` runs once per block.)
    pub(super) fn states_where(&self, mut pred: impl FnMut(&str) -> bool) -> Arc<Vec<bool>> {
        let states = &self.res.facts().states;
        let blocks = self.companions.work.blocks.get_or_init(|| {
            let mut blocks: Vec<(String, std::ops::Range<usize>)> = Vec::new();
            for (i, f) in states.iter().enumerate() {
                match blocks.last_mut() {
                    Some((name, ids)) if **name == *f.name => ids.end = i + 1,
                    _ => blocks.push((f.name.to_string(), i..i + 1)),
                }
            }
            blocks
        });
        let mut out = vec![false; states.len()];
        for (name, ids) in blocks {
            if pred(name) {
                out[ids.clone()].fill(true);
            }
        }
        Arc::new(out)
    }

    // ---- collect --------------------------------------------------------------------------

    fn companion_collect(
        &mut self,
        what: Want,
        count: u32,
        inventory: &[Option<ItemStack>],
    ) -> Result<Collect, String> {
        let want = self.resolve_want(what)?;
        let drops = self.block_drops();
        let mut items = self.want_items(&want);
        // An item no block drops plainly but that is a block (iron ore,
        // stone): the block is what's wanted, whatever it drops.
        let want = match want {
            Want::Item(i) if !drops.values().any(|d| d.contains(&i)) && drops.contains_key(&i) => {
                items = drops.get(&i).cloned().unwrap_or_default();
                Want::Block(i)
            }
            w => w,
        };
        let states = match &want {
            Want::Block(id) => {
                // Iron ore is deepslate iron ore too.
                let deep = format!("minecraft:deepslate_{}", c::short_id(id));
                let mut both = vec![id.clone()];
                if id.ends_with("_ore") {
                    both.push(deep);
                    items.extend(drops.get(&both[1]).into_iter().flatten().cloned());
                }
                self.states_where(|n| both.iter().any(|b| b == n))
            }
            _ => self.states_where(|n| {
                drops
                    .get(n)
                    .is_some_and(|d| d.iter().any(|i| items.contains(i)))
            }),
        };
        let prey: Vec<String> = PREY
            .iter()
            .filter(|kind| self.mob_drops(kind).iter().any(|i| items.contains(i)))
            .map(|k| (*k).to_owned())
            .collect();
        if !states.iter().any(|s| *s) && prey.is_empty() {
            return Err(format!("{} can't be collected", c::short_id(want.id())));
        }
        let last = carried_any(inventory, &items);
        Ok(Collect {
            want,
            count,
            done: 0,
            items,
            last,
            states,
            prey,
            search: None,
            searches: 0,
            target: None,
            bad: Vec::new(),
            pickup: None,
            reaching: None,
            skip: Vec::new(),
            hunting: None,
            unsafe_seen: false,
            reach: None,
            dig_to: None,
        })
    }

    fn collect_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Collect,
        pos: [f64; 3],
    ) -> Outcome {
        let Some(g) = host.hub.guest(b.conn) else {
            return Outcome::Busy;
        };
        let now = carried_any(&g.inventory, &k.items);
        if now > k.last && !matches!(k.want, Want::Block(_)) {
            k.done += now - k.last;
        }
        k.last = now;
        if k.done >= k.count && k.pickup.is_none() {
            return Outcome::Done;
        }
        // Buried by falling gravel: the block at its eyes goes first.
        if self.in_wall(pos, 1.62) {
            let head = feet(eye(pos));
            return match self.companion_dig(host, b, head, 1, false) {
                DigStep::Failed(e) => Outcome::Failed(e),
                _ => Outcome::Busy,
            };
        }
        // Back from a hunt: its drops.
        if let Some((id, last)) = k.hunting {
            match self.mobs.iter().find(|m| m.id == id && m.alive()) {
                Some(m) if !b.ticks.is_multiple_of(40) || c::distance(mob_at(m), pos) < 48.0 => {
                    k.hunting = Some((id, mob_at(m)));
                    if b.target.is_none() {
                        Self::hunt(b, id);
                    }
                    return Outcome::Busy;
                }
                _ => {
                    k.hunting = None;
                    k.pickup = Some((last, b.ticks + 100));
                }
            }
        }
        // What it broke or killed dropped: it picks that up.
        if let Some((at, until)) = k.pickup {
            // The drop shows up a tick or two after the block goes: it waits a second for it.
            if b.ticks < until
                && (self.collect_pick_up(host, b, k, at, pos) || b.ticks + 180 < until)
            {
                return Outcome::Busy;
            }
            k.pickup = None;
        }
        if k.done >= k.count {
            return Outcome::Done;
        }
        if k.target.is_some() {
            return self.collect_target(host, b, k, pos);
        }
        // Wanted items lying near: picked up first.
        if self.collect_pick_up(host, b, k, pos, pos) {
            return Outcome::Busy;
        }
        let here = feet(pos);
        let search = k
            .search
            .get_or_insert_with(|| BlockSearch::new(k.states.clone(), here, SEARCH_RADIUS));
        let world = &self.world;
        let states = &k.states;
        let facts = self.res.facts();
        search.found.retain(|p| {
            !k.bad.contains(p)
                && states
                    .get(world.block(p[0], p[1], p[2]).0 as usize)
                    .copied()
                    .unwrap_or(false)
        });
        // The nearest it can harvest, with a little weight against digging.
        let inventory = &host
            .hub
            .guest(b.conn)
            .map(|g| g.inventory.clone())
            .unwrap_or_default();
        let items = self.res.items();
        let mut needs: Option<&'static str> = None;
        let mut best: Option<(f64, [i32; 3])> = None;
        for p in &search.found {
            let f = facts.get(world.block(p[0], p[1], p[2]));
            if f.requires_tool
                && !inventory
                    .iter()
                    .flatten()
                    .any(|s| items.correct_for_drops(Some(s), &f.name))
            {
                needs = needs.or_else(|| gear::tool_needed(items, &f.name));
                continue;
            }
            if !self.safe_to_dig(*p) {
                k.unsafe_seen = true;
                continue;
            }
            let exposed = [
                [0, 1, 0],
                [0, -1, 0],
                [1, 0, 0],
                [-1, 0, 0],
                [0, 0, 1],
                [0, 0, -1],
            ]
            .iter()
            .any(|d| {
                let n = facts.get(world.block(p[0] + d[0], p[1] + d[1], p[2] + d[2]));
                !n.full_collision && n.fluid != Fluid::Lava
            });
            let cost = c::distance(pos, centre(*p))
                + if exposed { 0.0 } else { 6.0 }
                + (f64::from(p[1]) - pos[1]).abs() * 0.5;
            if best.is_none_or(|(b, _)| cost < b) {
                best = Some((cost, *p));
            }
        }
        // Keep looking a little: a nearer one may be in the next sections.
        let enough = best.is_some_and(|(cost, _)| cost < 12.0) || search.found.len() >= 24;
        if !search.done() && !enough {
            search.step(&self.world, SEARCH_BUDGET);
            return Outcome::Busy;
        }
        if let Some((_, p)) = best {
            k.target = Some(Target {
                pos: p,
                spot: None,
                tried: Vec::new(),
                stair: None,
                since: b.ticks,
            });
            return Outcome::Busy;
        }
        // No block: a mob that drops it.
        let prey = self
            .mobs
            .iter()
            .filter(|m| m.alive() && !m.is_baby() && k.prey.iter().any(|p| **p == *m.kind.id))
            .map(|m| (m.id, mob_at(m)))
            .filter(|(_, at)| c::distance(*at, pos) <= f64::from(SEARCH_RADIUS))
            .min_by(|x, y| c::distance(x.1, pos).total_cmp(&c::distance(y.1, pos)));
        if let Some((id, at)) = prey
            && !b.hold_fire
        {
            k.hunting = Some((id, at));
            Self::hunt(b, id);
            return Outcome::Busy;
        }
        if let Some(tool) = needs {
            return Outcome::Failed(format!("needs a {}", c::short_id(tool).replace('_', " ")));
        }
        // Searched out: once more from where it now stands, then it gives up.
        if k.searches < 2 && !k.bad.is_empty() {
            k.searches += 1;
            k.search = None;
            k.bad.clear();
            return Outcome::Busy;
        }
        let name = c::short_id(k.want.id()).replace('_', " ");
        Outcome::Failed(if k.unsafe_seen {
            format!("no {name} within {SEARCH_RADIUS} blocks I can dig safely (lava beside it)")
        } else {
            format!("no {name} within {SEARCH_RADIUS} blocks")
        })
    }

    /// Fight mob `id` for its drops: the attack skill, back to collecting after.
    fn hunt(b: &mut Body, id: u32) {
        b.resume = Skill::Collect;
        b.skill = Skill::Attack;
        b.target = Some(id);
        b.ordered = true;
        b.nav.reset();
    }

    /// Walk to the nearest wanted item lying within 6 blocks of `at`; false
    /// when there is none left to take. A drop in a pit below it, or one it
    /// can't walk to, it digs a staircase to.
    fn collect_pick_up(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Collect,
        at: [f64; 3],
        pos: [f64; 3],
    ) -> bool {
        let item = self
            .dropped
            .iter()
            .filter(|d| !k.skip.contains(&d.id) && d.pickup_delay < 15)
            .filter(|d| k.items.iter().any(|i| **i == *d.stack.item))
            .map(|d| (d.id, [d.position.x, d.position.y, d.position.z]))
            .filter(|(_, p)| c::distance(*p, at) <= 6.0 && c::distance(*p, pos) <= 12.0)
            .min_by(|x, y| c::distance(x.1, pos).total_cmp(&c::distance(y.1, pos)));
        let Some((id, p)) = item else {
            k.reaching = None;
            k.reach = None;
            return false;
        };
        let since = match k.reaching {
            Some((i, t)) if i == id => t,
            _ => {
                k.reaching = Some((id, b.ticks));
                k.reach = None;
                b.nav.reset();
                b.ticks
            }
        };
        if b.ticks - since > DROP_PATIENCE {
            k.skip.push(id);
            k.reaching = None;
            k.reach = None;
            self.companion_stop_dig(host, b);
            return true;
        }
        let (f, ic, h) = (feet(pos), feet(p), c::horizontal(p, pos));
        // Its pickup box (1 block wider, half a block taller) takes it from there.
        let close = h < 1.2 && p[1] > pos[1] - 0.6 && p[1] < pos[1] + 1.5;
        // Near: straight there, digging through natural blocks (the way
        // round can be long from inside a mine).
        if !close && h < 6.0 {
            if k.reach.is_none() {
                k.reach = self.reach_step(f, ic);
            }
            if let Some(stair) = k.reach.take() {
                // Digging its way there now: the rest of the way too.
                k.dig_to = Some(id);
                match self.follow_stair(host, b, &stair, pos) {
                    StairGo::Busy => k.reach = Some(stair),
                    StairGo::There => {}
                    StairGo::Failed => k.skip.push(id),
                }
                return true;
            }
        }
        if self.companion_steer(host, b, p, 0.3, false) == Steer::Stuck {
            if k.dig_to != Some(id) && h < 6.0 {
                k.dig_to = Some(id);
                b.nav.reset();
            } else {
                k.skip.push(id);
            }
        }
        true
    }

    /// One staircase step from `f` (its feet) toward a drop lying in cell
    /// `ic`: down a level while the drop is lower, up one while it is
    /// higher, through natural blocks only; the step over the drop's own
    /// column needs no floor (it falls onto the drop's).
    fn reach_step(&self, f: [i32; 3], ic: [i32; 3]) -> Option<Stair> {
        let (dx, dz) = (ic[0] - f[0], ic[2] - f[2]);
        if dx == 0 && dz == 0 && ic[1] >= f[1] - 1 && ic[1] <= f[1] {
            return None;
        }
        // Straight above or below: a staircase round instead.
        let toward = if dx == 0 && dz == 0 {
            [1, 0, 0]
        } else if dx.abs() >= dz.abs() {
            [dx.signum(), 0, 0]
        } else {
            [0, 0, dz.signum()]
        };
        let dy = (ic[1] - f[1]).signum();
        let dirs = [
            toward,
            [toward[2], 0, toward[0]],
            [-toward[2], 0, -toward[0]],
        ];
        dirs.into_iter().find_map(|d| {
            let to = [f[0] + d[0], f[1] + dy, f[2] + d[2]];
            let over = to[0] == ic[0] && to[2] == ic[2] && to[1] >= ic[1];
            if to[1] - 1 <= horizon_client::chunk::MIN_Y + 1 {
                return None;
            }
            if over {
                // The way down to the drop is open: it lies on a floor.
                if !(ic[1]..to[1]).all(|y| self.passable([ic[0], y, ic[2]])) {
                    return None;
                }
            } else if !self.floor([to[0], to[1] - 1, to[2]]) {
                return None;
            }
            let mut column = vec![[to[0], to[1] + 1, to[2]], to];
            if dy < 0 {
                column.insert(0, [to[0], to[1] + 2, to[2]]);
            }
            if dy > 0 {
                // Head room to jump up.
                column.insert(0, [f[0], f[1] + 2, f[2]]);
            }
            let clear: Vec<[i32; 3]> = column.into_iter().filter(|q| !self.passable(*q)).collect();
            clear
                .iter()
                .all(|q| self.natural(*q))
                .then_some(Stair { clear, to })
        })
    }

    /// Walk to `goal` (`companion_steer`); when walking gets stuck, as in
    /// a mine it dug, it digs a staircase toward the goal through natural
    /// blocks for a while, then walks again. Stuck after a few tries.
    pub(super) fn companion_go(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        goal: [f64; 3],
        near: f64,
        pos: [f64; 3],
    ) -> Steer {
        let there = c::horizontal(pos, goal) <= near && (goal[1] - pos[1]).abs() < 2.5;
        if there {
            b.work.tunnel = None;
            b.work.tunnel_until = None;
            b.work.tunnels = 0;
        }
        if let Some(until) = b.work.tunnel_until.filter(|u| b.ticks < *u && !there) {
            let stair = match b.work.tunnel.take() {
                Some(stair) => Some(stair),
                None => self.reach_step(feet(pos), feet(goal)),
            };
            let go = stair.map(|stair| {
                let go = self.follow_stair(host, b, &stair, pos);
                if let StairGo::Busy = go {
                    b.work.tunnel = Some(stair);
                }
                go
            });
            match go {
                Some(StairGo::Busy | StairGo::There) => return Steer::Moving,
                _ => {
                    b.work.tunnel_until = Some(until.min(b.ticks));
                    b.nav.reset();
                }
            }
        }
        b.work.tunnel = None;
        match self.companion_steer(host, b, goal, near, c::distance(pos, goal) > 10.0) {
            Steer::Stuck if b.work.tunnels < 4 => {
                b.work.tunnels += 1;
                b.work.tunnel_until = Some(b.ticks + 300);
                b.nav.reset();
                Steer::Moving
            }
            s => s,
        }
    }

    /// Take a staircase step: dig what's in the way (top first), then step
    /// in.
    fn follow_stair(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        stair: &Stair,
        pos: [f64; 3],
    ) -> StairGo {
        if let Some(q) = stair.clear.iter().copied().find(|q| !self.passable(*q)) {
            let face = match self.companion_access(eye(pos), q) {
                Access::Direct(face) => face,
                _ => 1,
            };
            return match self.companion_dig(host, b, q, face, false) {
                DigStep::Failed(_) => StairGo::Failed,
                _ => StairGo::Busy,
            };
        }
        let to = stair.to;
        let goal = [
            f64::from(to[0]) + 0.5,
            f64::from(to[1]),
            f64::from(to[2]) + 0.5,
        ];
        if feet(pos) == to || c::horizontal(pos, goal) < 0.25 && (pos[1] - goal[1]).abs() < 0.6 {
            return StairGo::There;
        }
        match self.companion_steer(host, b, goal, 0.2, false) {
            Steer::Stuck => StairGo::Failed,
            _ => StairGo::Busy,
        }
    }

    /// Work on the current target: dig it from where it stands, or go where it can.
    fn collect_target(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Collect,
        pos: [f64; 3],
    ) -> Outcome {
        let Some(t) = k.target.as_mut() else {
            return Outcome::Busy;
        };
        let p = t.pos;
        let state = self.world.block(p[0], p[1], p[2]);
        if !k.states.get(state.0 as usize).copied().unwrap_or(false) {
            // Gone: broken by it (its drops lie there) or by something else.
            if b.work.dig.is_some_and(|(d, _)| d == p) && matches!(k.want, Want::Block(_)) {
                k.done += 1;
            }
            self.companion_stop_dig(host, b);
            k.target = None;
            k.pickup = Some((centre(p), b.ticks + 200));
            return Outcome::Busy;
        }
        if b.ticks - t.since > TARGET_PATIENCE {
            self.companion_stop_dig(host, b);
            k.bad.push(p);
            k.target = None;
            return Outcome::Busy;
        }
        // A staircase step under way.
        if let Some(stair) = t.stair.take() {
            match self.follow_stair(host, b, &stair, pos) {
                StairGo::Busy => {
                    if let Some(t) = k.target.as_mut() {
                        t.stair = Some(stair);
                    }
                }
                StairGo::There => {}
                StairGo::Failed => {
                    k.bad.push(p);
                    k.target = None;
                }
            }
            return Outcome::Busy;
        }
        match self.companion_access(eye(pos), p) {
            Access::Direct(face) => {
                return match self.companion_dig(host, b, p, face, true) {
                    DigStep::Failed(e) => {
                        if e.starts_with("needs a ") {
                            return Outcome::Failed(e);
                        }
                        k.unsafe_seen |= e.ends_with("safely");
                        k.bad.push(p);
                        k.target = None;
                        Outcome::Busy
                    }
                    _ => Outcome::Busy,
                };
            }
            Access::Through(q, face) => {
                if let DigStep::Failed(_) = self.companion_dig(host, b, q, face, false) {
                    k.bad.push(p);
                    k.target = None;
                }
                return Outcome::Busy;
            }
            Access::None => {}
        }
        self.companion_stop_dig(host, b);
        let Some(t) = k.target.as_mut() else {
            return Outcome::Busy;
        };
        if t.spot.is_none() {
            t.spot = self.companion_spot_for(p, pos, &t.tried);
            if t.spot.is_none() {
                // Out of reach from any floor around: a staircase down to it.
                let f = feet(pos);
                match (p[1] <= f[1] + 1).then(|| self.stair_step(f, p)).flatten() {
                    Some(stair) => t.stair = Some(stair),
                    None => {
                        k.bad.push(p);
                        k.target = None;
                    }
                }
                return Outcome::Busy;
            }
        }
        let Some(spot) = t.spot else {
            return Outcome::Busy;
        };
        match self.companion_steer(host, b, spot, 0.3, c::distance(pos, spot) > 10.0) {
            Steer::Moving => {}
            Steer::Arrived | Steer::Stuck => {
                // There and still no way to it, or no way there: another spot.
                t.tried.push(spot);
                t.spot = None;
                if t.tried.len() > 3 {
                    k.bad.push(p);
                    k.target = None;
                }
            }
        }
        Outcome::Busy
    }

    // ---- mining -------------------------------------------------------------------------

    /// Whether a player can be in block `p`: no collision, no lava.
    pub(super) fn passable(&self, p: [i32; 3]) -> bool {
        let f = self.res.facts().get(self.world.block(p[0], p[1], p[2]));
        !f.full_collision && !f.legacy_solid && f.fluid != Fluid::Lava
    }

    /// A floor to stand on: a full block that doesn't hurt.
    pub(super) fn floor(&self, p: [i32; 3]) -> bool {
        let f = self.res.facts().get(self.world.block(p[0], p[1], p[2]));
        f.full_collision
            && !matches!(
                &*f.name,
                "minecraft:magma_block" | "minecraft:cactus" | "minecraft:powder_snow"
            )
    }

    /// Whether `p` may be dug without letting lava in or breaking a
    /// container, bed or workstation someone placed.
    fn safe_to_dig(&self, p: [i32; 3]) -> bool {
        let facts = self.res.facts();
        let f = facts.get(self.world.block(p[0], p[1], p[2]));
        if f.hardness < 0.0 || f.block_entity && !f.name.ends_with("_ore") {
            return false;
        }
        [[0, 1, 0], [1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]]
            .iter()
            .all(|d| {
                facts
                    .get(self.world.block(p[0] + d[0], p[1] + d[1], p[2] + d[2]))
                    .fluid
                    != Fluid::Lava
            })
    }

    /// A natural block it may dig through to reach another: leaves, plants,
    /// earth and stone, never what someone built.
    fn natural(&self, p: [i32; 3]) -> bool {
        let f = self.res.facts().get(self.world.block(p[0], p[1], p[2]));
        let n = c::short_id(&f.name);
        let earth = matches!(
            n,
            "dirt"
                | "grass_block"
                | "coarse_dirt"
                | "podzol"
                | "rooted_dirt"
                | "mycelium"
                | "stone"
                | "deepslate"
                | "tuff"
                | "granite"
                | "diorite"
                | "andesite"
                | "calcite"
                | "gravel"
                | "sand"
                | "red_sand"
                | "sandstone"
                | "clay"
                | "snow"
                | "snow_block"
                | "netherrack"
                | "moss_block"
                | "dripstone_block"
        );
        (earth
            || n.ends_with("_leaves")
            || n.ends_with("_ore")
            || f.replaceable && f.fluid == Fluid::None)
            && f.hardness >= 0.0
            && f.hardness <= 3.0
            && self.safe_to_dig(p)
    }

    /// How it can reach block `p` from `eye`: in reach and in sight, behind
    /// a natural block, or not from here.
    pub(super) fn companion_access(&self, eye: [f64; 3], p: [i32; 3]) -> Access {
        if to_block(eye, p) > BLOCK_REACH - 0.1 {
            return Access::None;
        }
        let to = centre(p);
        let d = c::distance(eye, to).max(1e-6);
        let dir = [
            (to[0] - eye[0]) / d,
            (to[1] - eye[1]) / d,
            (to[2] - eye[2]) / d,
        ];
        match self.clip_ray(eye, dir, BLOCK_REACH) {
            Some(h) if h.pos == p => Access::Direct(face_index(h.face)),
            Some(h) if self.natural(h.pos) => Access::Through(h.pos, face_index(h.face)),
            _ => Access::None,
        }
    }

    /// Where to stand to dig `p`: a floor around it from which it is in
    /// reach and in sight (or behind natural blocks), nearest `from` first.
    pub(super) fn companion_spot_for(
        &self,
        p: [i32; 3],
        from: [f64; 3],
        tried: &[[f64; 3]],
    ) -> Option<[f64; 3]> {
        let mut spots: Vec<(f64, [i32; 3])> = Vec::new();
        for dy in -4..=1 {
            for dx in -3i32..=3 {
                for dz in -3i32..=3 {
                    let s = [p[0] + dx, p[1] + dy, p[2] + dz];
                    let below = [s[0], s[1] - 1, s[2]];
                    if below == p
                        || s == p
                        || !self.passable(s)
                        || !self.passable([s[0], s[1] + 1, s[2]])
                        || !self.floor(below)
                    {
                        continue;
                    }
                    let at = [
                        f64::from(s[0]) + 0.5,
                        f64::from(s[1]),
                        f64::from(s[2]) + 0.5,
                    ];
                    if tried.iter().any(|t| c::distance(*t, at) < 0.5)
                        || to_block(eye(at), p) > BLOCK_REACH - 0.3
                    {
                        continue;
                    }
                    let cost = c::distance(from, at) + f64::from(dy.abs()) * 0.5;
                    spots.push((cost, s));
                }
            }
        }
        spots.sort_by(|a, b| a.0.total_cmp(&b.0));
        spots.into_iter().take(16).find_map(|(_, s)| {
            let at = [
                f64::from(s[0]) + 0.5,
                f64::from(s[1]),
                f64::from(s[2]) + 0.5,
            ];
            (!matches!(self.companion_access(eye(at), p), Access::None)).then_some(at)
        })
    }

    /// One step of a staircase from `f` (its feet) toward `p` below or
    /// level: never straight down, never into lava or the void, a floor
    /// under the step.
    fn stair_step(&self, f: [i32; 3], p: [i32; 3]) -> Option<Stair> {
        let (dx, dz) = (p[0] - f[0], p[2] - f[2]);
        let toward = if dx.abs() >= dz.abs() && dx != 0 {
            [dx.signum(), 0, 0]
        } else if dz != 0 {
            [0, 0, dz.signum()]
        } else {
            [1, 0, 0]
        };
        let dy = if p[1] < f[1] - 1 { -1 } else { 0 };
        let dirs = [
            toward,
            [toward[2], 0, toward[0]],
            [-toward[2], 0, -toward[0]],
        ];
        dirs.into_iter().find_map(|d| {
            let to = [f[0] + d[0], f[1] + dy, f[2] + d[2]];
            if to[1] - 1 <= horizon_client::chunk::MIN_Y + 1
                || !self.floor([to[0], to[1] - 1, to[2]])
            {
                return None;
            }
            let mut column = vec![[to[0], to[1] + 1, to[2]], to];
            if dy < 0 {
                column.insert(0, [to[0], to[1] + 2, to[2]]);
            }
            let clear: Vec<[i32; 3]> = column.into_iter().filter(|q| !self.passable(*q)).collect();
            clear
                .iter()
                .all(|q| self.natural(*q) || *q == p)
                .then_some(Stair { clear, to })
        })
    }

    /// Dig block `p` as a player: the best tool in hand, a START its guest
    /// mines with survival break times, a STOP once it broke. `drops`: the
    /// block must drop (a tool it lacks fails).
    fn companion_dig(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        p: [i32; 3],
        face: u8,
        drops: bool,
    ) -> DigStep {
        let facts = self.res.facts();
        let f = facts.get(self.world.block(p[0], p[1], p[2]));
        if facts.is_air(self.world.block(p[0], p[1], p[2]))
            || f.fluid != Fluid::None && !f.full_collision
        {
            if b.work.dig.is_some_and(|(d, _)| d == p) {
                self.companion_stop_dig(host, b);
            }
            return DigStep::Broken;
        }
        if f.hardness < 0.0 || !self.safe_to_dig(p) {
            return DigStep::Failed(format!("can't dig {} safely", c::short_id(&f.name)));
        }
        let (name, requires) = (f.name.to_string(), f.requires_tool);
        let Some(g) = host.hub.guest(b.conn) else {
            return DigStep::Busy;
        };
        let res = Arc::clone(&self.res);
        let items = res.items();
        if drops
            && requires
            && !g
                .inventory
                .iter()
                .flatten()
                .any(|s| items.correct_for_drops(Some(s), &name))
        {
            let tool = gear::tool_needed(items, &name).unwrap_or("minecraft:iron_pickaxe");
            return DigStep::Failed(format!("needs a {}", c::short_id(tool).replace('_', " ")));
        }
        let tool = gear::best_tool(items, &g.inventory, &name, requires);
        // It looks at what it digs.
        let at = g.position;
        let to = centre(p);
        b.yaw = c::yaw_toward(at, to) as f32;
        b.pitch = look_pitch(eye(at), to);
        if b.work.dig.is_some_and(|(d, _)| d == p) {
            let since = b.work.dig.map_or(b.ticks, |(_, t)| t);
            let still = host
                .play
                .get(&b.conn)
                .and_then(super::lan_actions::GuestPlay::digging)
                == Some(p);
            if b.ticks - since > DIG_PATIENCE || !still {
                self.companion_stop_dig(host, b);
                return DigStep::Failed(format!("couldn't break {}", c::short_id(&name)));
            }
            return DigStep::Busy;
        }
        self.companion_stop_dig(host, b);
        if let Some(i) = tool {
            self.companion_hold_slot(host, b, i);
        }
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = (b.yaw, b.pitch);
        }
        self.lan_guest_packet(
            host,
            b.conn,
            S::PlayerAction(play::PlayerAction {
                action: 0,
                pos: p,
                direction: face,
                sequence: 0,
            }),
        );
        b.work.dig = Some((p, b.ticks));
        DigStep::Busy
    }

    /// The dig stops: STOP once the block is gone (its client saw it break),
    /// else ABORT.
    pub(super) fn companion_stop_dig(&mut self, host: &mut LanHost, b: &mut Body) {
        let Some((p, _)) = b.work.dig.take() else {
            return;
        };
        let gone = self.res.facts().is_air(self.world.block(p[0], p[1], p[2]));
        self.lan_guest_packet(
            host,
            b.conn,
            S::PlayerAction(play::PlayerAction {
                action: if gone { 3 } else { 2 },
                pos: p,
                direction: 1,
                sequence: 0,
            }),
        );
    }

    /// Hold inventory slot `i` in its main hand: a hotbar slot is selected, a
    /// stack from the main inventory changes places with the held one.
    pub(super) fn companion_hold_slot(&mut self, host: &mut LanHost, b: &Body, i: usize) {
        let Some(g) = host.hub.guest_mut(b.conn) else {
            return;
        };
        let slot = if i < 9 {
            i
        } else {
            let selected = g.selected;
            g.inventory.swap(i, selected);
            selected
        };
        if g.selected != slot || i >= 9 {
            self.lan_guest_packet(
                host,
                b.conn,
                S::SetCarriedItem(play::SetCarriedItem { slot: slot as i16 }),
            );
        }
    }

    // ---- give -----------------------------------------------------------------------------

    fn give_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Give,
        pos: [f64; 3],
        owner: [f64; 3],
    ) -> Outcome {
        if k.given >= k.count {
            return Outcome::Done;
        }
        let Some(g) = host.hub.guest(b.conn) else {
            return Outcome::Busy;
        };
        let have = carried_any(&g.inventory, &k.items);
        if have == 0 {
            return Outcome::Failed(match k.given {
                0 => format!("I have no {}", c::short_id(k.want.id()).replace('_', " ")),
                n => format!("I only had {n}"),
            });
        }
        let d = c::horizontal(pos, owner);
        if c::distance(pos, owner) > c::FOLLOW_TELEPORT {
            if !self.companion_place_near(host, b, owner) {
                return Outcome::Failed("can't get to you".into());
            }
            return Outcome::Busy;
        }
        if d > 2.5 || (owner[1] - pos[1]).abs() > 1.5 {
            if self.companion_steer(host, b, owner, 2.0, d > 8.0) == Steer::Stuck
                && !self.companion_place_near(host, b, owner)
            {
                return Outcome::Failed("can't get to you".into());
            }
            return Outcome::Busy;
        }
        // Face its owner, the stack in hand, and toss (Player.drop).
        b.yaw = c::yaw_toward(pos, owner) as f32;
        b.pitch = look_pitch(eye(pos), [owner[0], owner[1] + 1.0, owner[2]]);
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return Outcome::Busy;
        };
        let Some(i) = host.hub.guests[gi].inventory.iter().position(|s| {
            s.as_ref()
                .is_some_and(|s| k.items.iter().any(|i| **i == *s.item))
        }) else {
            return Outcome::Busy;
        };
        self.companion_hold_slot(host, b, i);
        let g = &mut host.hub.guests[gi];
        (g.yaw, g.pitch) = (b.yaw, b.pitch);
        let stack = g.inventory[g.selected].as_ref().map_or(0, |s| s.count);
        let n = (k.count - k.given).min(stack);
        let before: Vec<u32> = self.dropped.iter().map(|d| d.id).collect();
        if n == stack {
            self.lan_guest_packet(
                host,
                b.conn,
                S::PlayerAction(play::PlayerAction {
                    action: 4,
                    pos: [0; 3],
                    direction: 0,
                    sequence: 0,
                }),
            );
        } else {
            for _ in 0..n {
                self.lan_guest_packet(
                    host,
                    b.conn,
                    S::PlayerAction(play::PlayerAction {
                        action: 5,
                        pos: [0; 3],
                        direction: 0,
                        sequence: 0,
                    }),
                );
            }
        }
        // What it threw is its owner's: it doesn't go after it.
        for d in self.dropped.iter().filter(|d| !before.contains(&d.id)) {
            if b.out_of_reach.len() >= 16 {
                b.out_of_reach.remove(0);
            }
            b.out_of_reach.push(d.id);
        }
        k.given += n;
        Outcome::Busy
    }

    // ---- sleep ----------------------------------------------------------------------------

    fn sleep_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut SleepIn,
        pos: [f64; 3],
    ) -> Outcome {
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return Outcome::Busy;
        };
        let time = self.world.day_time.rem_euclid(24000);
        let night = (12542..23460).contains(&time) || self.world.weather.thundering;
        if host.hub.guests[gi].bed.is_some() {
            // In bed: up with its owner, or in the morning.
            if k.with_owner && self.sleeping.is_none() || !night {
                host.hub.wake(gi);
                return Outcome::Done;
            }
            return Outcome::Busy;
        }
        if k.with_owner && self.sleeping.is_none() {
            return Outcome::Done;
        }
        if !night {
            return Outcome::Failed("I can sleep only at night".into());
        }
        let owner_bed = self.sleeping;
        if k.bed.is_none() {
            // A bed near its owner's when the owner sleeps, else near it.
            let here = owner_bed.unwrap_or(feet(pos));
            let search = k.search.get_or_insert_with(|| {
                let beds = self.states_where(|n| n.ends_with("_bed"));
                BlockSearch::new(beds, here, STATION_RADIUS)
            });
            if !search.step(&self.world, SEARCH_BUDGET) {
                return Outcome::Busy;
            }
            let registry = &self.pack.assets.registry;
            let free = |p: &[i32; 3]| {
                let props = registry
                    .state(self.world.block(p[0], p[1], p[2]))
                    .map(|s| s.properties.clone())
                    .unwrap_or_default();
                props.get("part").map(String::as_str) == Some("head")
                    && props.get("occupied").map(String::as_str) != Some("true")
                    && Some(*p) != owner_bed
            };
            k.bed = search
                .found
                .iter()
                .filter(|p| free(p))
                .min_by(|x, y| {
                    c::distance(centre(**x), pos).total_cmp(&c::distance(centre(**y), pos))
                })
                .copied();
            if k.bed.is_none() {
                return Outcome::Failed(format!("no free bed within {STATION_RADIUS} blocks"));
            }
        }
        let Some(bed) = k.bed else {
            return Outcome::Busy;
        };
        if !self
            .res
            .facts()
            .get(self.world.block(bed[0], bed[1], bed[2]))
            .name
            .ends_with("_bed")
        {
            k.bed = None;
            k.search = None;
            return Outcome::Busy;
        }
        let at = [
            f64::from(bed[0]) + 0.5,
            f64::from(bed[1]),
            f64::from(bed[2]) + 0.5,
        ];
        if c::horizontal(pos, at) > 2.0 || (pos[1] - at[1]).abs() > 1.5 {
            if self.companion_go(host, b, at, 1.5, pos) == Steer::Stuck {
                return Outcome::Failed("can't get to the bed".into());
            }
            return Outcome::Busy;
        }
        // A right click on the bed (BedBlock.useWithoutItem).
        b.yaw = c::yaw_toward(pos, at) as f32;
        b.pitch = look_pitch(eye(pos), [at[0], at[1] + 0.5, at[2]]);
        self.lan_guest_packet(
            host,
            b.conn,
            S::UseItemOn(play::UseItemOn {
                hand: 0,
                pos: bed,
                face: 1,
                cursor: [0.5, 0.5625, 0.5],
                inside: false,
                world_border: false,
                sequence: 0,
            }),
        );
        if host.hub.guests[gi].bed.is_some() {
            return Outcome::Busy;
        }
        k.tries += 1;
        if k.tries >= 3 {
            let monsters = self.mobs.iter().any(|m| {
                threat(m)
                    && (m.position.x - pos[0]).abs() < 8.0
                    && (m.position.z - pos[2]).abs() < 8.0
            });
            return Outcome::Failed(if monsters {
                "monsters nearby".into()
            } else {
                "couldn't sleep in the bed".into()
            });
        }
        Outcome::Busy
    }

    // ---- bows -----------------------------------------------------------------------------

    /// A fight at range with its bow: drawn for a full shot (20 ticks), aimed
    /// above the target for the arrow's drop, released. False when the bow
    /// isn't right (no bow or arrows, too close, out of sight): it fights
    /// in melee then, the bow put away.
    pub(super) fn companion_bow(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        id: u32,
        pos: [f64; 3],
    ) -> bool {
        let Some(m) = self.mobs.iter().find(|m| m.id == id && m.alive()) else {
            return false;
        };
        let (at, height, velocity) = (mob_at(m), f64::from(m.height()), m.velocity);
        let Some(gi) = host.hub.guests.iter().position(|g| g.conn == b.conn) else {
            return false;
        };
        let g = &host.hub.guests[gi];
        let bow = g
            .inventory
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| &*s.item == "minecraft:bow"));
        let arrows = g.inventory.iter().chain([&g.offhand]).flatten().any(|s| {
            matches!(
                &*s.item,
                "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:tipped_arrow"
            )
        });
        let h = c::horizontal(pos, at);
        let from = eye(pos);
        let target = [at[0], at[1] + height * 0.5, at[2]];
        let in_sight = || {
            let d = c::distance(from, target).max(1e-6);
            let dir = [
                (target[0] - from[0]) / d,
                (target[1] - from[1]) / d,
                (target[2] - from[2]) / d,
            ];
            self.clip_ray(from, dir, d).is_none()
        };
        let usable = bow.is_some()
            && arrows
            && b.eating.is_none()
            && (5.0..=28.0).contains(&h)
            && in_sight();
        let Some(bow) = bow.filter(|_| usable) else {
            if b.work.draw.take().is_some() {
                // Put away: the melee weapon in hand again.
                let slot = host.hub.guests[gi].selected as i16;
                self.lan_guest_packet(
                    host,
                    b.conn,
                    S::SetCarriedItem(play::SetCarriedItem { slot }),
                );
                self.companion_gear(host, b);
            }
            return false;
        };
        // Lead a moving target by the arrow's flight time.
        let flight = h / 2.7;
        let lead = [
            target[0] + velocity.x * flight,
            target[1],
            target[2] + velocity.z * flight,
        ];
        b.yaw = c::yaw_toward(pos, lead) as f32;
        b.pitch = work::bow_pitch(c::horizontal(from, lead), lead[1] - (from[1] - 0.1), 3.0)
            .unwrap_or(-30.0);
        if let Some(g) = host.hub.guest_mut(b.conn) {
            (g.yaw, g.pitch) = (b.yaw, b.pitch);
        }
        let drawing = host
            .play
            .get(&b.conn)
            .and_then(super::lan_actions::GuestPlay::using);
        match (b.work.draw, drawing) {
            (Some(since), Some(_)) if b.ticks - since >= u64::from(work::FULL_DRAW) => {
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
                self.companions.struck.insert(id, self.world.game_time);
                host.mob_players.insert(id, b.conn);
            }
            (Some(_), Some(_)) => {}
            _ => {
                b.work.draw = None;
                if host.hub.guests[gi].inventory[host.hub.guests[gi].selected]
                    .as_ref()
                    .is_none_or(|s| &*s.item != "minecraft:bow")
                {
                    self.companion_hold_slot(host, b, bow);
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
                if host
                    .play
                    .get(&b.conn)
                    .and_then(super::lan_actions::GuestPlay::using)
                    .is_some()
                {
                    b.work.draw = Some(b.ticks);
                }
            }
        }
        true
    }

    /// `companion cost`'s work numbers, then cleared.
    pub(super) fn companion_work_cost(&mut self) -> Value {
        let (n, total, max) = std::mem::take(&mut self.companions.work.cost);
        json!({
            "work_ticks": n,
            "work_mean_ms": total.as_secs_f64() * 1000.0 / f64::from(n.max(1)),
            "work_max_ms": max.as_secs_f64() * 1000.0,
        })
    }
}
