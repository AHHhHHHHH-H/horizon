//! The work skills' rules that need no world (docs/companions.md, "Skills"):
//! what a loot table can drop without a special tool, the crafting plan from
//! what it carries (logs → planks → sticks → tools), how high to aim a bow,
//! and how much fuel a smelt takes. The skills themselves run in the game
//! (game/companion_work.rs, game/companion_craft.rs).
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Collect searches the loaded chunks this far around the companion.
pub const SEARCH_RADIUS: i32 = 48;
/// A crafting table, furnace or bed this close is used; past it one is placed.
pub const STATION_RADIUS: i32 = 16;
/// A player's block interaction range (`Player.blockInteractionRange`).
pub const BLOCK_REACH: f64 = 4.5;

// ---- loot -------------------------------------------------------------------

/// The items a loot table (`minecraft:blocks/stone`) can drop when broken by
/// hand or with an ordinary tool: entries and pools that need a tool match
/// (silk touch, shears) are left out, so stone gives cobblestone and grass
/// blocks dirt.
pub fn loot_items(table: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for pool in table["pools"].as_array().into_iter().flatten() {
        if needs_tool(pool) {
            continue;
        }
        for entry in pool["entries"].as_array().into_iter().flatten() {
            entry_items(entry, &mut out);
        }
    }
    out
}

fn entry_items(entry: &Value, out: &mut Vec<String>) {
    if needs_tool(entry) {
        return;
    }
    match entry["type"].as_str().unwrap_or_default() {
        "minecraft:item" | "item" => {
            if let Some(name) = entry["name"].as_str()
                && !out.iter().any(|o| o == name)
            {
                out.push(name.to_owned());
            }
        }
        "minecraft:alternatives"
        | "minecraft:group"
        | "minecraft:sequence"
        | "alternatives"
        | "group"
        | "sequence" => {
            for child in entry["children"].as_array().into_iter().flatten() {
                entry_items(child, out);
            }
        }
        _ => {}
    }
}

/// Whether an entry's or pool's condition asks for a tool: `match_tool`, a
/// `minecraft:tool/...` predicate reference (26.3's `can_silk_touch`,
/// `can_shear`), alone or among `any_of` / `all_of`; an inverted one asks
/// for its absence. Reads 26.3's single `condition` and the older
/// `conditions` list, whose terms name their kind in `condition`.
fn needs_tool(v: &Value) -> bool {
    fn condition(c: &Value) -> bool {
        if let Some(reference) = c.as_str() {
            return reference
                .trim_start_matches("minecraft:")
                .starts_with("tool/");
        }
        let kind = c["type"]
            .as_str()
            .or_else(|| c["condition"].as_str())
            .unwrap_or_default();
        match kind.trim_start_matches("minecraft:") {
            "match_tool" => true,
            "any_of" | "all_of" => c["terms"]
                .as_array()
                .is_some_and(|t| t.iter().any(condition)),
            _ => false,
        }
    }
    v["conditions"]
        .as_array()
        .is_some_and(|c| c.iter().any(condition))
        || !v["condition"].is_null() && condition(&v["condition"])
}

// ---- crafting plans -----------------------------------------------------------

/// A crafting recipe as the planner sees it: the accepted items per cell.
#[derive(Clone, Debug, PartialEq)]
pub struct CraftRecipe {
    /// `minecraft:oak_planks`.
    pub id: String,
    pub result: String,
    /// Shaped: `width` x `height` cells, row by row; shapeless: one per ingredient.
    pub cells: Vec<Option<Vec<String>>>,
    /// 0 for a shapeless recipe.
    pub width: usize,
    pub height: usize,
    /// Items one craft makes.
    pub count: u32,
}

impl CraftRecipe {
    pub fn shaped(&self) -> bool {
        self.width > 0
    }
    /// Whether it needs a crafting table's 3x3 grid.
    pub fn needs_table(&self) -> bool {
        if self.shaped() {
            self.width > 2 || self.height > 2
        } else {
            self.cells.len() > 4
        }
    }
    /// The grid slot (0-based, row by row in a `size` x `size` grid) of cell `i`.
    pub fn grid_slot(&self, i: usize, size: usize) -> usize {
        if self.shaped() {
            i / self.width * size + i % self.width
        } else {
            i
        }
    }
}

/// The crafting recipes that make an item.
pub trait RecipeBook {
    fn recipes_for(&self, item: &str) -> &[Arc<CraftRecipe>];
}

/// One craft of a plan: the recipe, how many times, and the item put in
/// each cell (None for an empty cell).
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub recipe: Arc<CraftRecipe>,
    pub times: u32,
    pub picks: Vec<Option<String>>,
}

/// What it takes to craft something from a stock of items.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// The crafts in order, what each needs made before it.
    pub steps: Vec<Step>,
    /// What it lacks and can't craft: the items accepted and how many.
    pub missing: Vec<(Vec<String>, u32)>,
}

impl Plan {
    pub fn needs_table(&self) -> bool {
        self.steps.iter().any(|s| s.recipe.needs_table())
    }
}

/// Recursion this deep at most: log → planks → sticks → torch is 3.
const DEPTH: u32 = 5;
/// Recipes tried at most per plan, so that a recipe web can't stall a tick.
const BUDGET: u32 = 4000;

/// Plans crafts from a stock: each cell takes one kind of item, from the
/// stock, or crafted first (a recipe that needs nothing missing is preferred).
pub struct Planner<'a, B: RecipeBook> {
    book: &'a B,
    pub stock: HashMap<String, u32>,
    plan: Plan,
    budget: u32,
    /// Items it already failed to craft so many of from what there is: the
    /// stock only shrinks while planning, so as many or more fail again.
    dead: HashMap<String, u32>,
}

impl<'a, B: RecipeBook> Planner<'a, B> {
    pub fn new(book: &'a B, stock: HashMap<String, u32>) -> Self {
        Self {
            book,
            stock,
            plan: Plan::default(),
            budget: BUDGET,
            dead: HashMap::new(),
        }
    }

    /// Plan `n` more of `item` on top of what is planned; true when nothing
    /// is missing for it.
    pub fn make(&mut self, item: &str, n: u32) -> bool {
        self.craft(item, n, 0)
    }

    /// Keep `n` of `item` aside (a crafting table it will stand at).
    pub fn reserve(&mut self, item: &str, n: u32) -> bool {
        let have = self.have(item);
        if have >= n {
            *self.stock.entry(item.to_owned()).or_default() -= n;
            return true;
        }
        false
    }

    pub fn finish(self) -> Plan {
        let mut plan = self.plan;
        // The same craft twice in a row is one craft done more times.
        let mut steps: Vec<Step> = Vec::new();
        for s in plan.steps {
            match steps.last_mut() {
                Some(last) if last.recipe == s.recipe && last.picks == s.picks => {
                    last.times += s.times
                }
                _ => steps.push(s),
            }
        }
        plan.steps = steps;
        plan
    }

    fn have(&self, item: &str) -> u32 {
        self.stock.get(item).copied().unwrap_or(0)
    }

    fn craft(&mut self, item: &str, n: u32, depth: u32) -> bool {
        if n == 0 {
            return true;
        }
        let mut recipes: Vec<&Arc<CraftRecipe>> = self.book.recipes_for(item).iter().collect();
        // The recipe named after the item first (oak_planks before the
        // bamboo or log variants).
        recipes.sort_by_key(|r| r.id != item);
        for r in &recipes {
            let saved = (self.stock.clone(), self.plan.clone());
            if self.craft_with(r, n, depth) {
                return true;
            }
            (self.stock, self.plan) = saved;
        }
        match recipes.first() {
            Some(r) => {
                self.craft_with(r, n, depth);
                false
            }
            None => false,
        }
    }

    fn craft_with(&mut self, r: &Arc<CraftRecipe>, n: u32, depth: u32) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        let times = n.div_ceil(r.count.max(1));
        let mut clean = true;
        let mut picks = Vec::with_capacity(r.cells.len());
        for cell in &r.cells {
            picks.push(match cell {
                Some(options) => {
                    let (pick, ok) = self.obtain(options, times, depth);
                    clean &= ok;
                    pick
                }
                None => None,
            });
        }
        self.plan.steps.push(Step {
            recipe: Arc::clone(r),
            times,
            picks,
        });
        *self.stock.entry(r.result.clone()).or_default() += times * r.count;
        clean
    }

    /// `n` of one of `options` for a cell: from the stock, else crafted.
    fn obtain(&mut self, options: &[String], n: u32, depth: u32) -> (Option<String>, bool) {
        let mut order: Vec<&String> = options.iter().collect();
        order.sort_by_key(|o| std::cmp::Reverse(self.have(o)));
        if let Some(o) = order.first().filter(|o| self.have(o) >= n) {
            let o = (*o).clone();
            *self.stock.entry(o.clone()).or_default() -= n;
            return (Some(o), true);
        }
        if depth < DEPTH {
            for o in &order {
                let need = n - self.have(o);
                if self.book.recipes_for(o).is_empty()
                    || self.dead.get(*o).is_some_and(|d| need >= *d)
                {
                    continue;
                }
                let saved = (self.stock.clone(), self.plan.clone());
                if self.craft(o, need, depth + 1) {
                    *self.stock.entry((*o).clone()).or_default() -= n;
                    return (Some((*o).clone()), true);
                }
                (self.stock, self.plan) = saved;
                let d = self.dead.entry((*o).clone()).or_insert(need);
                *d = (*d).min(need);
            }
            // Nothing works out: a raw material is what's missing (logs,
            // not the oak wood they would make); else the first craftable.
            let raw = order.iter().any(|o| self.book.recipes_for(o).is_empty());
            if !raw && let Some(o) = order.first() {
                let need = n - self.have(o);
                self.craft(o, need, depth + 1);
                let left = self.have(o).min(n);
                *self.stock.entry((*o).clone()).or_default() -= left;
                return (Some((*o).clone()), false);
            }
        }
        // Nothing to craft it from: take what there is, the rest is missing.
        let best = order.first().map(|o| (*o).clone());
        let have = best.as_deref().map_or(0, |o| self.have(o));
        if let Some(o) = &best {
            *self.stock.entry(o.clone()).or_default() -= have;
        }
        self.plan.missing.push((options.to_vec(), n - have));
        (best, false)
    }
}

// ---- bows ---------------------------------------------------------------------------

/// Arrow gravity and drag per tick (`AbstractArrow`).
const ARROW_GRAVITY: f64 = 0.05;
const ARROW_DRAG: f64 = 0.99;

/// The pitch (degrees; negative looks up) that sends an arrow leaving at
/// `speed` blocks a tick to a point `distance` blocks away horizontally and
/// `rise` blocks higher, on the flattest path; None when it can't reach.
pub fn bow_pitch(distance: f64, rise: f64, speed: f64) -> Option<f32> {
    let height_at = |angle: f64| -> Option<f64> {
        let (mut x, mut y) = (0.0, 0.0);
        let (mut vx, mut vy) = (speed * angle.cos(), speed * angle.sin());
        for _ in 0..200 {
            if x + vx >= distance {
                return Some(y + vy * (distance - x) / vx);
            }
            x += vx;
            y += vy;
            vx *= ARROW_DRAG;
            vy = vy * ARROW_DRAG - ARROW_GRAVITY;
            if vx < 1e-3 {
                return None;
            }
        }
        None
    };
    let (mut lo, mut hi) = (-std::f64::consts::FRAC_PI_4, std::f64::consts::FRAC_PI_4);
    if height_at(hi).is_none_or(|h| h < rise) {
        return None;
    }
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if height_at(mid).is_some_and(|h| h >= rise) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(-hi.to_degrees() as f32)
}

/// The ticks a full draw takes (`BowItem.getPowerForTime` reaches 1 at 20).
pub const FULL_DRAW: u32 = 20;

// ---- furnaces -------------------------------------------------------------------------

/// Fuels it burns, best first, and how many items one smelts (a furnace
/// cooks one item in 200 ticks; coal burns 1600).
pub fn fuel_value(item: &str) -> Option<f32> {
    let name = item.trim_start_matches("minecraft:");
    Some(match name {
        "coal" | "charcoal" => 8.0,
        "coal_block" => 80.0,
        "blaze_rod" => 12.0,
        "dried_kelp_block" => 20.0,
        "stick" => 0.5,
        _ if name.starts_with("crimson_") || name.starts_with("warped_") => return None,
        _ if name.ends_with("_planks") || name.ends_with("_log") || name.ends_with("_wood") => 1.5,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loot_without_a_tool_match() {
        // blocks/stone: silk touch gives stone, else cobblestone.
        let stone = json!({"pools": [{"entries": [{"type": "minecraft:alternatives", "children": [
            {"type": "minecraft:item", "name": "minecraft:stone", "conditions": [
                {"condition": "minecraft:match_tool", "predicate": {}}]},
            {"type": "minecraft:item", "name": "minecraft:cobblestone"}]}]}]});
        assert_eq!(loot_items(&stone), vec!["minecraft:cobblestone"]);
        // Glass drops only with silk touch.
        let glass = json!({"pools": [{"conditions": [{"condition": "minecraft:match_tool"}],
            "entries": [{"type": "minecraft:item", "name": "minecraft:glass"}]}]});
        assert!(loot_items(&glass).is_empty());
        // Leaves: shears or silk touch for the block, sticks otherwise.
        let leaves = json!({"pools": [
            {"entries": [{"type": "minecraft:alternatives", "children": [
                {"type": "minecraft:item", "name": "minecraft:oak_leaves", "conditions": [{"condition": "minecraft:any_of",
                    "terms": [{"condition": "minecraft:match_tool"}, {"condition": "minecraft:match_tool"}]}]},
                {"type": "minecraft:item", "name": "minecraft:oak_sapling"}]}]},
            {"conditions": [{"condition": "minecraft:inverted", "term": {"condition": "minecraft:match_tool"}}],
             "entries": [{"type": "minecraft:item", "name": "minecraft:stick"}]}]});
        assert_eq!(
            loot_items(&leaves),
            vec!["minecraft:oak_sapling", "minecraft:stick"]
        );
        // 26.3's shape: one `condition`, a predicate reference or a typed term.
        let ore = json!({"pools": [{"entries": [{"type": "minecraft:alternatives", "children": [
            {"type": "minecraft:item", "condition": "minecraft:tool/can_silk_touch", "name": "minecraft:iron_ore"},
            {"type": "minecraft:item", "name": "minecraft:raw_iron"}]}]}]});
        assert_eq!(loot_items(&ore), vec!["minecraft:raw_iron"]);
        let glass = json!({"pools": [{"condition": {"type": "minecraft:any_of",
            "terms": ["minecraft:tool/can_silk_touch", {"type": "minecraft:match_tool"}]},
            "entries": [{"type": "minecraft:item", "name": "minecraft:glass_pane"}]}]});
        assert!(loot_items(&glass).is_empty());
        let leaves = json!({"pools": [{"condition": {"type": "minecraft:inverted", "term": "minecraft:tool/can_shear"},
            "entries": [{"type": "minecraft:item", "name": "minecraft:stick"}]}]});
        assert_eq!(loot_items(&leaves), vec!["minecraft:stick"]);
    }

    struct Book(HashMap<String, Vec<Arc<CraftRecipe>>>);
    impl RecipeBook for Book {
        fn recipes_for(&self, item: &str) -> &[Arc<CraftRecipe>] {
            self.0.get(item).map_or(&[], Vec::as_slice)
        }
    }
    impl Book {
        fn new(recipes: Vec<CraftRecipe>) -> Self {
            let mut by: HashMap<String, Vec<Arc<CraftRecipe>>> = HashMap::new();
            for r in recipes {
                by.entry(r.result.clone()).or_default().push(Arc::new(r));
            }
            Self(by)
        }
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| format!("minecraft:{x}")).collect()
    }
    fn book() -> Book {
        let planks = s(&["oak_planks", "birch_planks"]);
        let recipe =
            |id: &str, cells: Vec<Option<Vec<String>>>, width, height, count| CraftRecipe {
                id: format!("minecraft:{id}"),
                result: format!("minecraft:{id}"),
                cells,
                width,
                height,
                count,
            };
        Book::new(vec![
            recipe(
                "oak_planks",
                vec![Some(s(&["oak_log", "oak_wood"]))],
                0,
                0,
                4,
            ),
            recipe("birch_planks", vec![Some(s(&["birch_log"]))], 0, 0, 4),
            recipe("oak_wood", vec![Some(s(&["oak_log"])); 4], 2, 2, 3),
            recipe(
                "stick",
                vec![Some(planks.clone()), Some(planks.clone())],
                1,
                2,
                4,
            ),
            recipe("crafting_table", vec![Some(planks.clone()); 4], 2, 2, 1),
            recipe(
                "wooden_pickaxe",
                vec![
                    Some(planks.clone()),
                    Some(planks.clone()),
                    Some(planks.clone()),
                    None,
                    Some(s(&["stick"])),
                    None,
                    None,
                    Some(s(&["stick"])),
                    None,
                ],
                3,
                3,
                1,
            ),
        ])
    }

    #[test]
    fn a_pickaxe_from_logs() {
        let book = book();
        let stock = HashMap::from([("minecraft:oak_log".to_owned(), 3)]);
        let mut p = Planner::new(&book, stock);
        assert!(p.make("minecraft:crafting_table", 1));
        assert!(p.make("minecraft:wooden_pickaxe", 1));
        let plan = p.finish();
        assert!(plan.missing.is_empty(), "{plan:?}");
        assert!(plan.needs_table());
        let crafts: Vec<(&str, u32)> = plan
            .steps
            .iter()
            .map(|s| (&s.recipe.id[10..], s.times))
            .collect();
        // 4 planks for the table, 3 for the head, 2 for the sticks: 3 logs.
        assert_eq!(
            crafts,
            vec![
                ("oak_planks", 1),
                ("crafting_table", 1),
                ("oak_planks", 2),
                ("stick", 1),
                ("wooden_pickaxe", 1)
            ]
        );
        assert_eq!(plan.steps[4].picks[4].as_deref(), Some("minecraft:stick"));
    }

    #[test]
    fn what_is_missing_from_nothing() {
        let book = book();
        let mut p = Planner::new(&book, HashMap::new());
        assert!(!p.make("minecraft:wooden_pickaxe", 1));
        let plan = p.finish();
        let logs: u32 = plan
            .missing
            .iter()
            .filter(|(o, _)| o.contains(&"minecraft:oak_log".to_owned()))
            .map(|(_, n)| n)
            .sum();
        // Three planks cells and a stick: the planks come 4 a log.
        assert!((1..=4).contains(&logs), "{plan:?}");
        // Birch logs do: the planner picks the planks they make.
        let stock = HashMap::from([("minecraft:birch_log".to_owned(), 2)]);
        let mut p = Planner::new(&book, stock);
        assert!(p.make("minecraft:wooden_pickaxe", 1));
        let plan = p.finish();
        assert_eq!(plan.steps[0].recipe.id, "minecraft:birch_planks");
    }

    #[test]
    fn grid_slots_of_shaped_and_shapeless() {
        let book = book();
        let pick = &book.recipes_for("minecraft:wooden_pickaxe")[0];
        assert_eq!(pick.grid_slot(4, 3), 4);
        let stick = &book.recipes_for("minecraft:stick")[0];
        assert!(!stick.needs_table());
        assert_eq!(stick.grid_slot(1, 2), 2);
        assert_eq!(stick.grid_slot(1, 3), 3);
    }

    #[test]
    fn bows_aim_above_far_targets() {
        let near = bow_pitch(5.0, 0.0, 3.0).unwrap();
        let far = bow_pitch(30.0, 0.0, 3.0).unwrap();
        assert!(near < 0.0 && near > -2.0, "{near}");
        assert!(far < near, "{far} {near}");
        // Down a cliff it aims below the line; a weak shot can't reach far.
        assert!(bow_pitch(10.0, -6.0, 3.0).unwrap() > 20.0);
        assert!(bow_pitch(200.0, 0.0, 0.5).is_none());
    }

    #[test]
    fn fuels() {
        assert_eq!(fuel_value("minecraft:coal"), Some(8.0));
        assert_eq!(fuel_value("minecraft:oak_planks"), Some(1.5));
        assert_eq!(fuel_value("minecraft:crimson_planks"), None);
        assert_eq!(fuel_value("minecraft:stone"), None);
    }
}
