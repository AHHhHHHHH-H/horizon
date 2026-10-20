//! The tech-tree planner (docs/companion-speedrun.md, "What to build", item
//! 2): a goal such as "an iron pickaxe" or "2 buckets" becomes a list of
//! work skills, planned backwards over the recipes, what blocks and mobs
//! drop, smelting and the tools a block needs to drop anything. Each item is
//! got the cheapest way the companion knows: from what it carries, crafted,
//! smelted, mined or hunted; what it knows of the world (its perception)
//! sets what mining and hunting cost. Plans again from scratch whenever a
//! step fails or the inventory changes, so a plan never has to be repaired.
//!
//! The game supplies the data through [`Knowledge`]; the tests use a small
//! made-up tech tree.
use super::work::CraftRecipe;
use std::collections::HashMap;
use std::sync::Arc;

/// A block that drops an item, and the cheapest tool that makes it drop
/// (None: by hand).
#[derive(Clone, Debug, PartialEq)]
pub struct Mined {
    pub block: String,
    pub tool: Option<String>,
    /// Items one block gives on average.
    pub per_block: f32,
}

/// What the planner needs to know of the game and the world.
pub trait Knowledge {
    fn recipes(&self, item: &str) -> &[Arc<CraftRecipe>];
    /// The blocks whose loot gives `item`.
    fn mined_from(&self, item: &str) -> Vec<Mined>;
    /// The mobs that drop `item` when killed.
    fn hunted_from(&self, item: &str) -> Vec<String>;
    /// The items a furnace turns into `item`.
    fn smelted_from(&self, item: &str) -> Vec<String>;
    /// Whether `tool` makes `block` drop.
    fn harvests(&self, tool: &str, block: &str) -> bool;
    /// Estimated ticks to find and break one `block`; None when it knows of
    /// none it could reach (not seen yet, at Fair).
    fn gather_cost(&self, block: &str) -> Option<u32>;
    /// Estimated ticks to find and kill one `mob`; None when none is known.
    fn hunt_cost(&self, mob: &str) -> Option<u32>;
}

/// What a plan does, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Mine `count` of `item` from `block` (its tool already planned).
    Mine {
        item: String,
        count: u32,
        block: String,
    },
    Hunt {
        item: String,
        count: u32,
        mob: String,
    },
    Craft {
        item: String,
        count: u32,
    },
    /// Smelt `count` of `input` into `item`.
    Smelt {
        item: String,
        input: String,
        count: u32,
    },
}

impl Action {
    /// The companion command words for it (`collect 3 raw_iron`).
    pub fn words(&self) -> String {
        let s = |i: &str| super::short_id(i).to_owned();
        match self {
            Action::Mine { item, count, .. } | Action::Hunt { item, count, .. } => {
                format!("collect {count} {}", s(item))
            }
            Action::Craft { item, count } => format!("craft {count} {}", s(item)),
            Action::Smelt { item, count, .. } => format!("smelt {count} {}", s(item)),
        }
    }
    pub fn item(&self) -> &str {
        match self {
            Action::Mine { item, .. }
            | Action::Hunt { item, .. }
            | Action::Craft { item, .. }
            | Action::Smelt { item, .. } => item,
        }
    }
}

/// Stations near enough to use (it remembers them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stations {
    pub crafting_table: bool,
    pub furnace: bool,
}

/// A plan for a goal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TechPlan {
    pub actions: Vec<Action>,
    /// Items it has no known way to get (not seen, can't be crafted).
    pub missing: Vec<String>,
    /// Estimated ticks.
    pub cost: u32,
}

impl TechPlan {
    pub fn complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Fuels, and how many items one smelts.
pub const FUELS: [(&str, f32); 4] = [
    ("minecraft:coal", 8.0),
    ("minecraft:charcoal", 8.0),
    ("minecraft:oak_planks", 1.5),
    ("minecraft:oak_log", 1.5),
];

/// Calls to `need` per plan at most, so a recipe web can't stall a tick.
const BUDGET: u32 = 30_000;
const DEPTH: usize = 10;
/// Ticks one craft takes (open, click, take).
const CRAFT: u32 = 10;
/// Ticks a furnace takes per item.
const SMELT: u32 = 200;

#[derive(Clone)]
struct State {
    stock: HashMap<String, u32>,
    actions: Vec<Action>,
    stations: Stations,
}

/// Plans goals against a stock.
pub struct TechPlanner<'a, K: Knowledge> {
    k: &'a K,
    state: State,
    visiting: Vec<String>,
    budget: u32,
    missing: Vec<String>,
}

impl<'a, K: Knowledge> TechPlanner<'a, K> {
    pub fn new(k: &'a K, stock: HashMap<String, u32>, stations: Stations) -> Self {
        Self {
            k,
            state: State {
                stock,
                actions: Vec::new(),
                stations,
            },
            visiting: Vec::new(),
            budget: BUDGET,
            missing: Vec::new(),
        }
    }

    /// Plan `n` of `item` on top of what is planned; its cost, None when it
    /// can't be had.
    pub fn want(&mut self, item: &str, n: u32) -> Option<u32> {
        let saved = self.state.clone();
        match self.need(item, n) {
            Some(c) => Some(c),
            None => {
                self.state = saved;
                if !self.missing.iter().any(|m| m == item) {
                    self.missing.push(item.to_owned());
                }
                None
            }
        }
    }

    /// A tool (not used up): planned unless one it carries or plans already
    /// does the job (`harvests` the block, when given).
    pub fn want_tool(&mut self, tool: &str, for_block: Option<&str>) -> Option<u32> {
        self.ensure_tool(tool, for_block)
    }

    /// The plan: the same mine, hunt, craft or smelt done once for all
    /// (at its first place, which comes before every use of it).
    pub fn finish(self) -> TechPlan {
        let mut actions: Vec<Action> = Vec::new();
        for a in self.state.actions {
            let same = actions.iter_mut().find(|b| match (&**b, &a) {
                (
                    Action::Mine { item, block, .. },
                    Action::Mine {
                        item: i, block: bl, ..
                    },
                ) => item == i && block == bl,
                (
                    Action::Hunt { item, mob, .. },
                    Action::Hunt {
                        item: i, mob: m, ..
                    },
                ) => item == i && mob == m,
                (Action::Craft { item, .. }, Action::Craft { item: i, .. }) => item == i,
                (
                    Action::Smelt { item, input, .. },
                    Action::Smelt {
                        item: i, input: n, ..
                    },
                ) => item == i && input == n,
                _ => false,
            });
            match same {
                Some(
                    Action::Mine { count, .. }
                    | Action::Hunt { count, .. }
                    | Action::Craft { count, .. }
                    | Action::Smelt { count, .. },
                ) => {
                    *count += match &a {
                        Action::Mine { count, .. }
                        | Action::Hunt { count, .. }
                        | Action::Craft { count, .. }
                        | Action::Smelt { count, .. } => *count,
                    }
                }
                None => actions.push(a),
            }
        }
        TechPlan {
            actions,
            missing: self.missing,
            cost: 0,
        }
    }

    fn have(&self, item: &str) -> u32 {
        self.state.stock.get(item).copied().unwrap_or(0)
    }

    fn take(&mut self, item: &str, n: u32) {
        let e = self.state.stock.entry(item.to_owned()).or_default();
        *e = e.saturating_sub(n);
    }

    fn put(&mut self, item: &str, n: u32) {
        *self.state.stock.entry(item.to_owned()).or_default() += n;
    }

    /// Get `n` of `item` into the stock and take them out of it; the cost.
    fn need(&mut self, item: &str, n: u32) -> Option<u32> {
        if n == 0 {
            return Some(0);
        }
        if self.budget == 0
            || self.visiting.len() >= DEPTH
            || self.visiting.iter().any(|v| v == item)
        {
            return None;
        }
        self.budget -= 1;
        let have = self.have(item);
        if have >= n {
            self.take(item, n);
            return Some(0);
        }
        let short = n - have;
        self.take(item, have);
        self.visiting.push(item.to_owned());
        let start = self.state.clone();
        let mut best: Option<(u32, State)> = None;
        let mut consider = |me: &mut Self, cost: Option<u32>| {
            if let Some(c) = cost
                && best.as_ref().is_none_or(|(b, _)| c < *b)
            {
                best = Some((c, me.state.clone()));
            }
            me.state = start.clone();
        };
        // Crafted.
        let recipes: Vec<Arc<CraftRecipe>> = self.k.recipes(item).to_vec();
        for r in &recipes {
            let c = self.craft(item, r, short);
            consider(self, c);
        }
        // Smelted.
        for input in self.k.smelted_from(item) {
            let c = self.smelt(item, &input, short);
            consider(self, c);
        }
        // Mined.
        for m in self.k.mined_from(item) {
            let c = self.mine(item, &m, short);
            consider(self, c);
        }
        // Hunted.
        for mob in self.k.hunted_from(item) {
            let c = self.k.hunt_cost(&mob).map(|per| {
                self.state.actions.push(Action::Hunt {
                    item: item.to_owned(),
                    count: short,
                    mob: mob.clone(),
                });
                per.saturating_mul(short)
            });
            consider(self, c);
        }
        self.visiting.pop();
        let (cost, state) = best?;
        self.state = state;
        Some(cost)
    }

    fn craft(&mut self, item: &str, r: &CraftRecipe, short: u32) -> Option<u32> {
        let times = short.div_ceil(r.count.max(1));
        let mut cost = CRAFT * times;
        if r.needs_table() && !self.state.stations.crafting_table {
            cost += self.ensure_station("minecraft:crafting_table")?;
        }
        // Cells asking for the same items are got together.
        let mut groups: Vec<(&Vec<String>, u32)> = Vec::new();
        for cell in r.cells.iter().flatten() {
            match groups.iter_mut().find(|(o, _)| *o == cell) {
                Some(g) => g.1 += times,
                None => groups.push((cell, times)),
            }
        }
        for (options, n) in groups {
            cost += self.one_of(options, n)?;
        }
        self.state.actions.push(Action::Craft {
            item: item.to_owned(),
            count: times * r.count,
        });
        self.put(item, times * r.count - short);
        Some(cost)
    }

    /// `n` of one of `options` (a tag's items): what it carries first, else
    /// the cheapest.
    fn one_of(&mut self, options: &[String], n: u32) -> Option<u32> {
        if let Some(o) = options.iter().find(|o| self.have(o) >= n) {
            let o = o.clone();
            self.take(&o, n);
            return Some(0);
        }
        let start = self.state.clone();
        let mut best: Option<(u32, State)> = None;
        // Few options are worth a look: those it has some of, then the
        // first few (the tag's own order, oak first).
        let mut order: Vec<&String> = options.iter().collect();
        order.sort_by_key(|o| std::cmp::Reverse(self.have(o)));
        for o in order.into_iter().take(4) {
            if let Some(c) = self.need(o, n)
                && best.as_ref().is_none_or(|(b, _)| c < *b)
            {
                best = Some((c, self.state.clone()));
            }
            self.state = start.clone();
        }
        let (c, s) = best?;
        self.state = s;
        Some(c)
    }

    fn smelt(&mut self, item: &str, input: &str, short: u32) -> Option<u32> {
        let mut cost = SMELT * short / 2;
        if !self.state.stations.furnace {
            cost += self.ensure_station("minecraft:furnace")?;
        }
        cost += self.need(input, short)?;
        cost += self.fuel(short)?;
        self.state.actions.push(Action::Smelt {
            item: item.to_owned(),
            input: input.to_owned(),
            count: short,
        });
        Some(cost)
    }

    /// Fuel for `items` smelts: what it carries, else the cheapest to get.
    fn fuel(&mut self, items: u32) -> Option<u32> {
        let start = self.state.clone();
        let mut best: Option<(u32, State)> = None;
        // Planks and logs of any wood burn: what it carries counts.
        let carried: Vec<(String, f32)> = self
            .state
            .stock
            .iter()
            .filter(|(_, n)| **n > 0)
            .filter_map(|(i, _)| super::work::fuel_value(i).map(|v| (i.clone(), v)))
            .collect();
        let mut fuels: Vec<(String, f32)> =
            FUELS.iter().map(|(i, v)| ((*i).to_owned(), *v)).collect();
        fuels.extend(carried);
        for (item, value) in fuels {
            let n = (items as f32 / value).ceil() as u32;
            if let Some(c) = self.need(&item, n)
                && best.as_ref().is_none_or(|(b, _)| c < *b)
            {
                best = Some((c, self.state.clone()));
            }
            self.state = start.clone();
        }
        let (c, s) = best?;
        self.state = s;
        Some(c)
    }

    fn mine(&mut self, item: &str, m: &Mined, short: u32) -> Option<u32> {
        let per = self.k.gather_cost(&m.block)?;
        let blocks = (short as f32 / m.per_block.max(0.05)).ceil() as u32;
        let mut cost = per.saturating_mul(blocks);
        if let Some(tool) = &m.tool {
            cost += self.ensure_tool(tool, Some(&m.block))?;
        }
        self.state.actions.push(Action::Mine {
            item: item.to_owned(),
            count: short,
            block: m.block.clone(),
        });
        // More may drop than counted on (gravel's flint): the rest is extra.
        Some(cost)
    }

    /// A tool it keeps: one it carries that harvests the block does; else
    /// `tool` is planned and kept in the stock.
    fn ensure_tool(&mut self, tool: &str, block: Option<&str>) -> Option<u32> {
        let k = self.k;
        let carried = self.state.stock.iter().any(|(i, n)| {
            *n > 0
                && match block {
                    Some(b) => k.harvests(i, b),
                    None => i == tool,
                }
        });
        if carried {
            return Some(0);
        }
        let c = self.need(tool, 1)?;
        self.put(tool, 1);
        Some(c)
    }

    /// A crafting table or furnace: carried, else made and kept (it is
    /// placed where it is used, and then counts as near).
    fn ensure_station(&mut self, station: &str) -> Option<u32> {
        if self.have(station) > 0 {
            return Some(0);
        }
        let c = self.need(station, 1)?;
        match station {
            "minecraft:crafting_table" => self.state.stations.crafting_table = true,
            _ => self.state.stations.furnace = true,
        }
        Some(c + CRAFT)
    }
}

/// Plan `goals` (item, count) from a stock: one plan for all of them, in
/// order. A goal whose item is a tool is had once.
pub fn plan<K: Knowledge>(
    k: &K,
    stock: HashMap<String, u32>,
    stations: Stations,
    goals: &[(&str, u32)],
) -> TechPlan {
    let mut p = TechPlanner::new(k, stock, stations);
    let mut cost = 0;
    for (item, n) in goals {
        cost += p.want(item, *n).unwrap_or(0);
    }
    let mut out = p.finish();
    out.cost = cost;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up tech tree: wood, stone, iron, food, buckets.
    struct Tree {
        recipes: HashMap<String, Vec<Arc<CraftRecipe>>>,
        /// Blocks it knows of and what one costs to get.
        known: HashMap<String, u32>,
        mobs: HashMap<String, u32>,
    }

    fn i(s: &str) -> String {
        format!("minecraft:{s}")
    }

    fn shaped(
        result: &str,
        count: u32,
        w: usize,
        h: usize,
        cells: &[Option<&[&str]>],
    ) -> Arc<CraftRecipe> {
        Arc::new(CraftRecipe {
            id: i(result),
            result: i(result),
            cells: cells
                .iter()
                .map(|c| c.map(|o| o.iter().map(|x| i(x)).collect()))
                .collect(),
            width: w,
            height: h,
            count,
        })
    }

    const PLANKS: &[&str] = &["oak_planks", "birch_planks"];

    impl Tree {
        fn new() -> Self {
            let mut r: HashMap<String, Vec<Arc<CraftRecipe>>> = HashMap::new();
            let mut add = |c: Arc<CraftRecipe>| r.entry(c.result.clone()).or_default().push(c);
            add(shaped("oak_planks", 4, 1, 1, &[Some(&["oak_log"])]));
            add(shaped("birch_planks", 4, 1, 1, &[Some(&["birch_log"])]));
            add(shaped("stick", 4, 1, 2, &[Some(PLANKS), Some(PLANKS)]));
            add(shaped("crafting_table", 1, 2, 2, &[Some(PLANKS); 4]));
            let pick = |m: &'static [&'static str], out: &str| {
                shaped(
                    out,
                    1,
                    3,
                    3,
                    &[
                        Some(m),
                        Some(m),
                        Some(m),
                        None,
                        Some(&["stick"]),
                        None,
                        None,
                        Some(&["stick"]),
                        None,
                    ],
                )
            };
            add(pick(PLANKS, "wooden_pickaxe"));
            add(pick(&["cobblestone"], "stone_pickaxe"));
            add(pick(&["iron_ingot"], "iron_pickaxe"));
            let ring = [Some(&["cobblestone"][..]); 9];
            let mut furnace = ring.to_vec();
            furnace[4] = None;
            add(shaped("furnace", 1, 3, 3, &furnace));
            add(shaped(
                "bucket",
                1,
                3,
                2,
                &[
                    Some(&["iron_ingot"]),
                    None,
                    Some(&["iron_ingot"]),
                    None,
                    Some(&["iron_ingot"]),
                    None,
                ],
            ));
            add(shaped(
                "flint_and_steel",
                1,
                2,
                1,
                &[Some(&["iron_ingot"]), Some(&["flint"])],
            ));
            add(shaped(
                "iron_block",
                1,
                3,
                3,
                &[Some(&["iron_ingot"][..]); 9],
            ));
            add(shaped("iron_ingot", 9, 1, 1, &[Some(&["iron_block"])]));
            add(shaped("bread", 1, 3, 1, &[Some(&["wheat"][..]); 3]));
            let mut known = HashMap::new();
            for (b, c) in [
                ("oak_log", 40),
                ("stone", 30),
                ("iron_ore", 400),
                ("gravel", 30),
            ] {
                known.insert(i(b), c);
            }
            let mut mobs = HashMap::new();
            mobs.insert(i("cow"), 200);
            Self {
                recipes: r,
                known,
                mobs,
            }
        }
    }

    impl Knowledge for Tree {
        fn recipes(&self, item: &str) -> &[Arc<CraftRecipe>] {
            self.recipes.get(item).map_or(&[], |v| v)
        }
        fn mined_from(&self, item: &str) -> Vec<Mined> {
            let m = |b: &str, t: Option<&str>, per: f32| Mined {
                block: i(b),
                tool: t.map(i),
                per_block: per,
            };
            match item.trim_start_matches("minecraft:") {
                "oak_log" => vec![m("oak_log", None, 1.0)],
                "birch_log" => vec![m("birch_log", None, 1.0)],
                "cobblestone" => vec![m("stone", Some("wooden_pickaxe"), 1.0)],
                "raw_iron" => vec![m("iron_ore", Some("stone_pickaxe"), 1.0)],
                "coal" => vec![m("coal_ore", Some("wooden_pickaxe"), 1.0)],
                "flint" => vec![m("gravel", None, 0.1)],
                "wheat" => vec![m("wheat", None, 1.0)],
                _ => vec![],
            }
        }
        fn hunted_from(&self, item: &str) -> Vec<String> {
            match item.trim_start_matches("minecraft:") {
                "beef" | "leather" => vec![i("cow")],
                _ => vec![],
            }
        }
        fn smelted_from(&self, item: &str) -> Vec<String> {
            match item.trim_start_matches("minecraft:") {
                "iron_ingot" => vec![i("raw_iron")],
                "cooked_beef" => vec![i("beef")],
                "iron_nugget" => vec![i("iron_pickaxe")],
                _ => vec![],
            }
        }
        fn harvests(&self, tool: &str, block: &str) -> bool {
            let tier = |t: &str| match t.trim_start_matches("minecraft:") {
                "wooden_pickaxe" => 1,
                "stone_pickaxe" => 2,
                "iron_pickaxe" => 3,
                _ => 0,
            };
            let needs = match block.trim_start_matches("minecraft:") {
                "stone" | "coal_ore" => 1,
                "iron_ore" => 2,
                _ => 0,
            };
            tier(tool) >= needs && (needs > 0 || tool == block)
        }
        fn gather_cost(&self, block: &str) -> Option<u32> {
            self.known.get(block).copied()
        }
        fn hunt_cost(&self, mob: &str) -> Option<u32> {
            self.mobs.get(mob).copied()
        }
    }

    fn stock(items: &[(&str, u32)]) -> HashMap<String, u32> {
        items.iter().map(|(k, n)| (i(k), *n)).collect()
    }

    fn position(p: &TechPlan, pred: impl Fn(&Action) -> bool) -> usize {
        p.actions
            .iter()
            .position(pred)
            .unwrap_or_else(|| panic!("no such action in {:#?}", p.actions))
    }

    fn crafts(item: &str) -> impl Fn(&Action) -> bool + '_ {
        move |a| matches!(a, Action::Craft { item: x, .. } if x.ends_with(item))
    }

    fn mines(item: &str) -> impl Fn(&Action) -> bool + '_ {
        move |a| matches!(a, Action::Mine { item: x, .. } if x.ends_with(item))
    }

    #[test]
    fn wooden_pickaxe_from_nothing() {
        let t = Tree::new();
        let p = plan(
            &t,
            HashMap::new(),
            Stations::default(),
            &[(&i("wooden_pickaxe"), 1)],
        );
        assert!(p.complete(), "{:?}", p.missing);
        // Logs, planks, sticks, a table, the pickaxe.
        let logs = position(&p, mines("oak_log"));
        let planks = position(&p, crafts("oak_planks"));
        let table = position(&p, crafts("crafting_table"));
        let pick = position(&p, crafts("wooden_pickaxe"));
        assert!(logs < planks && planks < table && table < pick);
        let Action::Mine { count, .. } = &p.actions[logs] else {
            unreachable!()
        };
        // 3 planks + 2 sticks' planks (1 craft of 4 sticks) + 4 for the table = 9 planks: 3 logs.
        assert_eq!(*count, 3);
        assert_eq!(
            p.actions.iter().filter(|a| crafts("oak_planks")(a)).count(),
            1,
            "{:#?}",
            p.actions
        );
    }

    #[test]
    fn iron_pickaxe_goes_through_stone_and_a_furnace() {
        let t = Tree::new();
        let p = plan(
            &t,
            HashMap::new(),
            Stations::default(),
            &[(&i("iron_pickaxe"), 1)],
        );
        assert!(p.complete(), "{:?}", p.missing);
        let wooden = position(&p, crafts("wooden_pickaxe"));
        let cobble = position(&p, mines("cobblestone"));
        let stone = position(&p, crafts("stone_pickaxe"));
        let furnace = position(&p, crafts("furnace"));
        let iron = position(&p, mines("raw_iron"));
        let smelt = position(&p, |a| matches!(a, Action::Smelt { .. }));
        let pick = position(&p, crafts("iron_pickaxe"));
        assert!(wooden < cobble && cobble < stone && stone < iron && iron < smelt && smelt < pick);
        assert!(furnace < smelt);
        let Action::Mine { count, .. } = &p.actions[iron] else {
            unreachable!()
        };
        assert_eq!(*count, 3);
        let cobbles: u32 = p
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Mine { item, count, .. } if item.ends_with("cobblestone") => Some(*count),
                _ => None,
            })
            .sum();
        assert_eq!(cobbles, 3 + 8, "a stone pickaxe and a furnace");
        // Never through iron blocks or nuggets.
        assert!(!p.actions.iter().any(|a| a.item().ends_with("iron_block")));
    }

    #[test]
    fn what_it_carries_is_used_and_tools_kept() {
        let t = Tree::new();
        let p = plan(
            &t,
            stock(&[
                ("stone_pickaxe", 1),
                ("oak_planks", 10),
                ("raw_iron", 2),
                ("furnace", 1),
            ]),
            Stations {
                crafting_table: true,
                furnace: false,
            },
            &[(&i("bucket"), 2)],
        );
        assert!(p.complete(), "{:?}", p.missing);
        // 6 ingots: 2 raw carried, 4 mined with the pickaxe it has.
        let iron = position(&p, mines("raw_iron"));
        let Action::Mine { count, .. } = &p.actions[iron] else {
            unreachable!()
        };
        assert_eq!(*count, 4);
        assert!(
            !p.actions.iter().any(|a| crafts("pickaxe")(a)),
            "{:#?}",
            p.actions
        );
        assert!(
            !p.actions.iter().any(|a| crafts("furnace")(a)),
            "it carries one"
        );
        let Some(Action::Smelt { count, .. }) =
            p.actions.iter().find(|a| matches!(a, Action::Smelt { .. }))
        else {
            unreachable!()
        };
        assert_eq!(*count, 6);
        assert!(
            !p.actions.iter().any(|a| mines("oak_log")(a)),
            "planks burn: no logs"
        );
    }

    #[test]
    fn several_goals_share_their_tools() {
        let t = Tree::new();
        let p = plan(
            &t,
            HashMap::new(),
            Stations::default(),
            &[
                (&i("iron_pickaxe"), 1),
                (&i("bucket"), 2),
                (&i("flint_and_steel"), 1),
            ],
        );
        assert!(p.complete(), "{:?}", p.missing);
        assert_eq!(
            p.actions
                .iter()
                .filter(|a| crafts("stone_pickaxe")(a))
                .count(),
            1
        );
        assert_eq!(p.actions.iter().filter(|a| crafts("furnace")(a)).count(), 1);
        let raw: u32 = p
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::Mine { item, count, .. } if item.ends_with("raw_iron") => Some(*count),
                _ => None,
            })
            .sum();
        assert_eq!(raw, 3 + 6 + 1, "10 iron for the speedrun kit");
        // Flint from gravel: 10 gravel on average for 1 flint.
        assert!(p.actions.iter().any(mines("flint")));
    }

    #[test]
    fn food_is_hunted_and_cooked() {
        let t = Tree::new();
        let p = plan(
            &t,
            stock(&[("wooden_pickaxe", 1), ("oak_log", 8)]),
            Stations::default(),
            &[(&i("cooked_beef"), 6)],
        );
        assert!(p.complete(), "{:?}", p.missing);
        let hunt = position(&p, |a| matches!(a, Action::Hunt { .. }));
        let smelt = position(&p, |a| matches!(a, Action::Smelt { .. }));
        assert!(hunt < smelt);
    }

    #[test]
    fn what_it_has_not_seen_is_missing() {
        let mut t = Tree::new();
        t.known.remove(&i("iron_ore"));
        let p = plan(
            &t,
            stock(&[("stone_pickaxe", 1)]),
            Stations::default(),
            &[(&i("bucket"), 1)],
        );
        assert!(!p.complete());
        assert_eq!(p.missing, vec![i("bucket")]);
        assert!(
            p.actions.is_empty(),
            "nothing half-planned: {:?}",
            p.actions
        );
        // Once it sees some, it plans.
        t.known.insert(i("iron_ore"), 900);
        let p = plan(
            &t,
            stock(&[("stone_pickaxe", 1)]),
            Stations::default(),
            &[(&i("bucket"), 1)],
        );
        assert!(p.complete());
    }

    #[test]
    fn cheaper_sources_win() {
        let mut t = Tree::new();
        // Birch is known and cheap, oak far away: birch planks.
        t.known.insert(i("birch_log"), 10);
        t.known.insert(i("oak_log"), 500);
        let p = plan(
            &t,
            HashMap::new(),
            Stations::default(),
            &[(&i("crafting_table"), 1)],
        );
        assert!(p.actions.iter().any(mines("birch_log")), "{:#?}", p.actions);
    }

    #[test]
    fn plans_have_words_for_the_skills() {
        let a = Action::Mine {
            item: i("raw_iron"),
            count: 3,
            block: i("iron_ore"),
        };
        assert_eq!(a.words(), "collect 3 raw_iron");
        assert_eq!(
            Action::Smelt {
                item: i("iron_ingot"),
                input: i("raw_iron"),
                count: 3
            }
            .words(),
            "smelt 3 iron_ingot"
        );
    }
}
