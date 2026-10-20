//! A companion crafting and smelting (docs/companions.md, "Skills"), through
//! the menus a player's client clicks: its inventory's 2x2 grid (window 0), a
//! crafting table it opens with a right click, and a furnace. A craft is
//! planned from what it carries (`work::Planner`: logs → planks → sticks →
//! tools); a crafting table or furnace within 16 blocks is used, else it
//! makes one and places it. Each tick does at most one craft or one visit to
//! the furnace.
use super::{
    Game,
    companion::Body,
    companion_body::{Steer, carried, look_pitch},
    companion_work::{Access, BlockSearch, Outcome, centre, eye, feet, to_block},
    lan::LanHost,
};
use horizon_client::{
    companion::{
        self as c, CompanionCommand, Skill, Want,
        work::{self, BLOCK_REACH, CraftRecipe, Planner, RecipeBook, STATION_RADIUS},
    },
    crafting::Recipe,
    items::ItemStack,
};
use horizon_protocol::packets::play::{self, Serverbound as S};
use std::{collections::HashMap, sync::Arc};

/// Block reads a station search makes per tick.
const SEARCH_BUDGET: u32 = 4 * 4096;
/// ContainerInput: a click, a shift-click.
const PICKUP: i32 = 0;
const QUICK_MOVE: i32 = 1;
/// A furnace cooks one item in this many ticks (`AbstractCookingRecipe` 200).
const COOK_TICKS: f32 = 200.0;
/// Ticks without the furnace moving on before a smelt gives up.
const FURNACE_PATIENCE: u64 = 1200;

/// The crafting recipes by the item they make.
pub(super) struct BookIndex {
    by_result: HashMap<String, Vec<Arc<CraftRecipe>>>,
}

impl RecipeBook for BookIndex {
    fn recipes_for(&self, item: &str) -> &[Arc<CraftRecipe>] {
        self.by_result.get(item).map_or(&[], Vec::as_slice)
    }
}

impl BookIndex {
    pub fn recipes_for_item(&self, item: &str) -> &[Arc<CraftRecipe>] {
        self.by_result.get(item).map_or(&[], Vec::as_slice)
    }
}

/// A craft under way.
pub(super) struct Craft {
    pub item: String,
    pub count: u32,
    pub gather: bool,
    baseline: u32,
    pub made: u32,
    table: Option<[i32; 3]>,
    search: Option<BlockSearch>,
    searched: bool,
    gathers: u32,
    failures: u32,
    tried: Vec<[f64; 3]>,
    place_tries: u32,
    /// The last plan and what it was made from (its stock, a table near):
    /// planned again only when those change.
    planned: Option<(Vec<(String, u32)>, bool, bool, work::Plan)>,
}

impl Craft {
    pub fn new(item: String, count: u32, gather: bool, inventory: &[Option<ItemStack>]) -> Self {
        let baseline = carried(inventory, &item);
        Self {
            item,
            count,
            gather,
            baseline,
            made: 0,
            table: None,
            search: None,
            searched: false,
            gathers: 0,
            failures: 0,
            tried: Vec::new(),
            place_tries: 0,
            planned: None,
        }
    }
}

/// A smelt under way.
pub(super) struct Smelt {
    /// The item it was asked for (input or result), for saving.
    pub asked: String,
    input: String,
    pub output: String,
    pub count: u32,
    baseline: u32,
    pub made: u32,
    furnace: Option<[i32; 3]>,
    search: Option<BlockSearch>,
    searched: bool,
    made_one: bool,
    tried: Vec<[f64; 3]>,
    place_tries: u32,
    /// The furnace's input and result counts last seen, and since when.
    seen: (u32, u32),
    changed: u64,
}

/// A station's slots before the player's: the inventory menu (result, grid,
/// armour), the crafting table (result, grid), the furnace.
const INVENTORY_BASE: usize = 9;
const TABLE_BASE: usize = 10;
const FURNACE_BASE: usize = 3;

/// The menu slot of inventory index `i` in a menu whose own slots come first.
fn menu_slot(base: usize, i: usize) -> usize {
    if i < 9 { base + 27 + i } else { base + i - 9 }
}

/// What placing a crafting table or furnace came to.
pub(super) enum Place {
    Busy,
    Placed([i32; 3]),
    Failed(String),
}

/// What going to a station came to.
enum Reach {
    /// In reach and sight: the face to click.
    Ready(u8),
    Walking,
    Failed,
}

impl Game<'_> {
    /// The recipe index, built once.
    pub(super) fn companion_book(&mut self) -> Arc<BookIndex> {
        if let Some(b) = &self.companions.work.book {
            return b.clone();
        }
        let mut by_result: HashMap<String, Vec<Arc<CraftRecipe>>> = HashMap::new();
        for (name, r) in &self.res.recipes().crafting {
            let (cells, width, height, result) = match r {
                Recipe::Shaped {
                    width,
                    height,
                    cells,
                    result,
                } => (
                    cells
                        .iter()
                        .map(|c| c.as_ref().map(|i| i.1.clone()))
                        .collect(),
                    *width,
                    *height,
                    result,
                ),
                Recipe::Shapeless {
                    ingredients,
                    result,
                } => (
                    ingredients.iter().map(|i| Some(i.1.clone())).collect(),
                    0,
                    0,
                    result,
                ),
                Recipe::Special(_) => continue,
            };
            let Some(item) = result.item.as_deref().filter(|_| result.extra.is_none()) else {
                continue;
            };
            by_result
                .entry(item.to_owned())
                .or_default()
                .push(Arc::new(CraftRecipe {
                    id: format!("minecraft:{name}"),
                    result: item.to_owned(),
                    cells,
                    width,
                    height,
                    count: result.count,
                }));
        }
        let book = Arc::new(BookIndex { by_result });
        self.companions.work.book = Some(book.clone());
        book
    }

    /// `3 sticks`, `2 planks` for a reason.
    fn describe_missing(&self, missing: &[(Vec<String>, u32)]) -> String {
        let items = self.res.items();
        missing
            .iter()
            .map(|(options, n)| {
                let name = items
                    .item_tag_order
                    .iter()
                    .find(|(_, members)| members.len() > 1 && **members == *options)
                    .map(|(tag, _)| c::short_id(tag).to_owned())
                    .unwrap_or_else(|| {
                        c::short_id(options.first().map_or("?", String::as_str)).to_owned()
                    });
                format!("{n} {}", name.replace('_', " "))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What it collects for a missing ingredient: any log for planks.
    fn gather_want(&self, options: &[String]) -> Want {
        let items = self.res.items();
        let in_tag = |tag: &str| {
            items
                .item_tags
                .get(tag)
                .is_some_and(|t| options.iter().all(|o| t.contains(o)))
        };
        if in_tag("minecraft:logs") {
            return Want::Tag("minecraft:logs".into());
        }
        match items
            .item_tag_order
            .iter()
            .find(|(_, m)| m.len() > 1 && **m == *options)
        {
            Some((tag, _)) => Want::Tag(tag.clone()),
            None => Want::Item(options.first().cloned().unwrap_or_default()),
        }
    }

    /// The menu window it has open, if it is still open.
    fn open_window(&self, host: &LanHost, b: &Body) -> Option<i32> {
        let w = b.work.menu?;
        if w == 0 {
            return Some(0);
        }
        let table = host
            .play
            .get(&b.conn)
            .and_then(|p| p.crafting.table_window());
        let other = host
            .hub
            .guest(b.conn)
            .and_then(|g| g.menu.as_ref())
            .map(|m| m.window);
        (table == Some(w) || other == Some(w)).then_some(w)
    }

    /// Close the menu it has open (`ServerboundContainerClosePacket`): a
    /// grid gives its items back.
    pub(super) fn companion_close_menu(&mut self, host: &mut LanHost, b: &mut Body) {
        if let Some(container) = b.work.menu.take() {
            self.lan_guest_packet(
                host,
                b.conn,
                S::ContainerClose(play::ContainerClose { container }),
            );
        }
    }

    fn companion_click(
        &mut self,
        host: &mut LanHost,
        b: &Body,
        window: i32,
        slot: usize,
        button: i8,
        input: i32,
    ) {
        self.lan_guest_packet(
            host,
            b.conn,
            S::ContainerClick(play::ContainerClick {
                container: window,
                state: 0,
                slot: slot as i16,
                button,
                input,
                changed: Vec::new(),
                carried: None,
            }),
        );
    }

    /// Put `n` of `item` into menu slot `to`, one right click each, taking
    /// stacks from its inventory; false when it runs out.
    fn companion_put(
        &mut self,
        host: &mut LanHost,
        b: &Body,
        window: i32,
        base: usize,
        item: &str,
        to: usize,
        mut n: u32,
    ) -> bool {
        for _ in 0..8 {
            if n == 0 {
                return true;
            }
            let Some(i) = host.hub.guest(b.conn).and_then(|g| {
                g.inventory
                    .iter()
                    .position(|s| s.as_ref().is_some_and(|s| &*s.item == item))
            }) else {
                return false;
            };
            let from = menu_slot(base, i);
            self.companion_click(host, b, window, from, 0, PICKUP);
            let held = host
                .hub
                .guest(b.conn)
                .and_then(|g| g.carried.as_ref())
                .map_or(0, |s| s.count);
            let k = n.min(held);
            for _ in 0..k {
                self.companion_click(host, b, window, to, 1, PICKUP);
            }
            n -= k;
            // The rest back where it was.
            self.companion_click(host, b, window, from, 0, PICKUP);
            if k == 0 {
                return false;
            }
        }
        n == 0
    }

    /// Walk to where it can click block `p`.
    fn companion_reach(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        p: [i32; 3],
        pos: [f64; 3],
        tried: &mut Vec<[f64; 3]>,
    ) -> Reach {
        if let Access::Direct(face) = self.companion_access(eye(pos), p) {
            return Reach::Ready(face);
        }
        let Some(spot) = self.companion_spot_for(p, pos, tried) else {
            return Reach::Failed;
        };
        match self.companion_go(host, b, spot, 0.3, pos) {
            Steer::Moving => {}
            Steer::Arrived | Steer::Stuck => {
                tried.push(spot);
                if tried.len() > 4 {
                    return Reach::Failed;
                }
            }
        }
        Reach::Walking
    }

    /// A right click on block `p` (a crafting table or furnace opens).
    pub(super) fn companion_use_block(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        p: [i32; 3],
        face: u8,
        pos: [f64; 3],
    ) {
        let to = centre(p);
        b.yaw = c::yaw_toward(pos, to) as f32;
        b.pitch = look_pitch(eye(pos), to);
        let d = [
            [0.5, 0.0, 0.5],
            [0.5, 1.0, 0.5],
            [0.5, 0.5, 0.0],
            [0.5, 0.5, 1.0],
            [0.0, 0.5, 0.5],
            [1.0, 0.5, 0.5],
        ];
        self.lan_guest_packet(
            host,
            b.conn,
            S::UseItemOn(play::UseItemOn {
                hand: 0,
                pos: p,
                face: i32::from(face),
                cursor: d[usize::from(face).min(5)],
                inside: false,
                world_border: false,
                sequence: 0,
            }),
        );
    }

    /// Place a crafting table or furnace it carries on the floor beside it.
    pub(super) fn companion_place(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        item: &str,
        pos: [f64; 3],
        tries: &mut u32,
    ) -> Place {
        *tries += 1;
        if *tries > 4 {
            return Place::Failed(format!(
                "couldn't place a {}",
                c::short_id(item).replace('_', " ")
            ));
        }
        let f = feet(pos);
        let owner = self
            .player
            .as_ref()
            .map(|p| feet([p.position.x, p.position.y, p.position.z]));
        let ring = [
            [1, 0],
            [-1, 0],
            [0, 1],
            [0, -1],
            [1, 1],
            [1, -1],
            [-1, 1],
            [-1, -1],
            [2, 0],
            [-2, 0],
            [0, 2],
            [0, -2],
        ];
        // Each try takes the next spot: one that wouldn't take the block (an
        // entity in the way) is passed over.
        let spots: Vec<[i32; 3]> = ring
            .iter()
            .flat_map(|d| [0, 1, -1].map(|dy| [f[0] + d[0], f[1] + dy, f[2] + d[1]]))
            .filter(|p| {
                let s = self.res.facts().get(self.world.block(p[0], p[1], p[2]));
                let floor = [p[0], p[1] - 1, p[2]];
                (s.replaceable || self.res.facts().is_air(self.world.block(p[0], p[1], p[2])))
                    && s.fluid == horizon_client::blocks::Fluid::None
                    && self.floor(floor)
                    && owner.is_none_or(|o| [o, [o[0], o[1] + 1, o[2]]].iter().all(|q| q != p))
                    && to_block(eye(pos), floor) < BLOCK_REACH - 0.2
            })
            .collect();
        let Some(&p) = spots.get((*tries as usize - 1) % spots.len().max(1)) else {
            return Place::Failed("no room to place it".into());
        };
        let Some(i) = host.hub.guest(b.conn).and_then(|g| {
            g.inventory
                .iter()
                .position(|s| s.as_ref().is_some_and(|s| &*s.item == item))
        }) else {
            return Place::Failed(format!("no {}", c::short_id(item)));
        };
        self.companion_hold_slot(host, b, i);
        let floor = [p[0], p[1] - 1, p[2]];
        self.companion_use_block(host, b, floor, 1, pos);
        if *self
            .res
            .facts()
            .get(self.world.block(p[0], p[1], p[2]))
            .name
            == *item
        {
            *tries = 0;
            return Place::Placed(p);
        }
        Place::Busy
    }

    /// The nearest block of `name` within 16 blocks, searched a few sections
    /// a tick; None while searching or when there is none (`searched`).
    fn companion_station(
        &mut self,
        b: &Body,
        search: &mut Option<BlockSearch>,
        searched: &mut bool,
        name: &str,
        pos: [f64; 3],
    ) -> Option<[i32; 3]> {
        let wanted = self.states_where(|n| n == name);
        let found =
            self.companion_known(b, &wanted, feet(pos), STATION_RADIUS, search, SEARCH_BUDGET)?;
        *searched = true;
        let found = found
            .iter()
            .filter(|p| {
                *self
                    .res
                    .facts()
                    .get(self.world.block(p[0], p[1], p[2]))
                    .name
                    == *name
            })
            .min_by(|x, y| c::distance(centre(**x), pos).total_cmp(&c::distance(centre(**y), pos)))
            .copied();
        *search = None;
        found
    }

    // ---- craft ------------------------------------------------------------------------

    pub(super) fn craft_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Craft,
        pos: [f64; 3],
    ) -> Outcome {
        let Some(g) = host.hub.guest(b.conn) else {
            return Outcome::Busy;
        };
        let have = carried(&g.inventory, &k.item);
        k.baseline = k.baseline.min(have);
        k.made = have - k.baseline;
        if k.made >= k.count {
            return Outcome::Done;
        }
        // Leftovers in its 2x2 grid go back first.
        if g.craft_grid.iter().any(Option::is_some) && b.work.menu != Some(0) {
            self.lan_guest_packet(
                host,
                b.conn,
                S::ContainerClose(play::ContainerClose { container: 0 }),
            );
            return Outcome::Busy;
        }
        let mut stock: HashMap<String, u32> = HashMap::new();
        for s in g.inventory.iter().flatten() {
            *stock.entry(s.item.to_string()).or_default() += s.count;
        }
        if let Some(t) = k.table
            && *self
                .res
                .facts()
                .get(self.world.block(t[0], t[1], t[2]))
                .name
                != *"minecraft:crafting_table"
        {
            k.table = None;
            k.searched = false;
        }
        let book = self.companion_book();
        let left = k.count - k.made;
        let mut key: Vec<(String, u32)> = stock.iter().map(|(i, n)| (i.clone(), *n)).collect();
        key.push((k.item.clone(), left));
        key.sort_unstable();
        let plan = match &k.planned {
            Some((s, table, searched, plan))
                if *s == key && *table == k.table.is_some() && *searched == k.searched =>
            {
                plan.clone()
            }
            _ => {
                let mut plan = {
                    let mut p = Planner::new(&*book, stock.clone());
                    p.make(&k.item, left);
                    p.finish()
                };
                if plan.needs_table() && k.table.is_none() && k.searched {
                    // No table near: one is made first (and kept), then placed.
                    let mut p = Planner::new(&*book, stock);
                    if !p.reserve("minecraft:crafting_table", 1) {
                        p.make("minecraft:crafting_table", 1);
                    }
                    p.make(&k.item, left);
                    plan = p.finish();
                }
                k.planned = Some((key, k.table.is_some(), k.searched, plan.clone()));
                plan
            }
        };
        if plan.needs_table() && k.table.is_none() && !k.searched {
            k.table = self.companion_station(
                b,
                &mut k.search,
                &mut k.searched,
                "minecraft:crafting_table",
                pos,
            );
            return Outcome::Busy;
        }
        if !plan.missing.is_empty() {
            let what = self.describe_missing(&plan.missing);
            if !k.gather || k.gathers >= 8 {
                return Outcome::Failed(format!("missing {what}"));
            }
            let (options, n) = plan.missing[0].clone();
            let want = self.gather_want(&options);
            k.gathers += 1;
            return match self.companion_task(
                host,
                b,
                CompanionCommand::Collect {
                    what: want,
                    count: n,
                },
                Skill::Follow,
            ) {
                Ok(task) => Outcome::Push(task),
                Err(_) => Outcome::Failed(format!("missing {what}")),
            };
        }
        let Some(step) = plan.steps.first().cloned() else {
            return Outcome::Failed(format!("can't craft {}", c::short_id(&k.item)));
        };
        let table_window = host
            .play
            .get(&b.conn)
            .and_then(|p| p.crafting.table_window());
        let open = self.open_window(host, b);
        let (window, base, size) = if step.recipe.needs_table() {
            let table = match k.table {
                Some(t) => t,
                None => {
                    return match self.companion_place(
                        host,
                        b,
                        "minecraft:crafting_table",
                        pos,
                        &mut k.place_tries,
                    ) {
                        Place::Placed(p) => {
                            k.table = Some(p);
                            Outcome::Busy
                        }
                        Place::Busy => Outcome::Busy,
                        Place::Failed(e) => Outcome::Failed(e),
                    };
                }
            };
            match open.filter(|w| Some(*w) == table_window) {
                Some(w) => (w, TABLE_BASE, 3),
                None => {
                    self.companion_close_menu(host, b);
                    match self.companion_reach(host, b, table, pos, &mut k.tried) {
                        Reach::Ready(face) => {
                            self.companion_use_block(host, b, table, face, pos);
                            b.work.menu = host
                                .play
                                .get(&b.conn)
                                .and_then(|p| p.crafting.table_window());
                            if b.work.menu.is_none() {
                                k.failures += 1;
                            }
                        }
                        Reach::Walking => {}
                        Reach::Failed => {
                            k.table = None;
                            k.searched = true;
                            k.tried.clear();
                            k.failures += 1;
                        }
                    }
                    return if k.failures > 4 {
                        Outcome::Failed("can't get to a crafting table".into())
                    } else {
                        Outcome::Busy
                    };
                }
            }
        } else {
            match open.filter(|w| Some(*w) == table_window) {
                Some(w) => (w, TABLE_BASE, 3),
                None => {
                    self.companion_close_menu(host, b);
                    b.work.menu = Some(0);
                    (0, INVENTORY_BASE, 2)
                }
            }
        };
        // The grid, cell by cell, then a shift-click on the result.
        let mut filled = true;
        for (cell, pick) in step.picks.iter().enumerate() {
            let Some(item) = pick else { continue };
            let to = 1 + step.recipe.grid_slot(cell, size);
            if !self.companion_put(host, b, window, base, item, to, step.times) {
                filled = false;
                break;
            }
        }
        if filled {
            self.companion_click(host, b, window, 0, 0, QUICK_MOVE);
        }
        if window == 0 {
            // The 2x2 grid gives back what wasn't used.
            self.companion_close_menu(host, b);
        }
        if !filled {
            self.companion_close_menu(host, b);
            k.failures += 1;
            if k.failures > 4 {
                return Outcome::Failed(format!(
                    "couldn't craft {}",
                    c::short_id(&step.recipe.result)
                ));
            }
        }
        Outcome::Busy
    }

    // ---- smelt ------------------------------------------------------------------------

    /// A smelt of `item` (what goes in, or what comes out of what it carries).
    pub(super) fn companion_smelt_task(
        &self,
        item: &str,
        count: u32,
        inventory: &[Option<ItemStack>],
    ) -> Result<Smelt, String> {
        let recipes = self.res.recipes();
        let smelting = |r: &&horizon_client::crafting::Cooking| r.kind.ends_with("smelting");
        let (input, output) = match recipes.cook("minecraft:smelting", &ItemStack::new(item, 1)) {
            Some(r) => (item.to_owned(), r.result.item.to_string()),
            None => {
                let r = recipes
                    .cooking
                    .iter()
                    .filter(smelting)
                    .filter(|r| *r.result.item == *item)
                    .find_map(|r| {
                        inventory
                            .iter()
                            .flatten()
                            .find(|s| r.ingredient.test(s))
                            .map(|s| (s.item.to_string(), r))
                    })
                    .ok_or_else(|| {
                        format!("can't smelt anything I carry into {}", c::short_id(item))
                    })?;
                (r.0, item.to_owned())
            }
        };
        Ok(Smelt {
            asked: item.to_owned(),
            baseline: carried(inventory, &output),
            input,
            output,
            count,
            made: 0,
            furnace: None,
            search: None,
            searched: false,
            made_one: false,
            tried: Vec::new(),
            place_tries: 0,
            seen: (0, 0),
            changed: 0,
        })
    }

    pub(super) fn smelt_tick(
        &mut self,
        host: &mut LanHost,
        b: &mut Body,
        k: &mut Smelt,
        pos: [f64; 3],
    ) -> Outcome {
        let Some(g) = host.hub.guest(b.conn) else {
            return Outcome::Busy;
        };
        let have = carried(&g.inventory, &k.output);
        let has_input = carried(&g.inventory, &k.input);
        let has_furnace = carried(&g.inventory, "minecraft:furnace") > 0;
        k.baseline = k.baseline.min(have);
        k.made = have - k.baseline;
        if k.made >= k.count {
            return Outcome::Done;
        }
        if let Some(p) = k.furnace
            && !matches!(
                self.world.block_entities.get(&(p[0], p[1], p[2])),
                Some(horizon_client::containers::BlockEntity::Furnace(f)) if f.kind.recipe_type().ends_with("smelting")
            )
        {
            k.furnace = None;
        }
        let Some(p) = k.furnace else {
            if !k.searched {
                k.furnace = self.companion_station(
                    b,
                    &mut k.search,
                    &mut k.searched,
                    "minecraft:furnace",
                    pos,
                );
                return Outcome::Busy;
            }
            if has_furnace {
                return match self.companion_place(
                    host,
                    b,
                    "minecraft:furnace",
                    pos,
                    &mut k.place_tries,
                ) {
                    Place::Placed(p) => {
                        k.furnace = Some(p);
                        Outcome::Busy
                    }
                    Place::Busy => Outcome::Busy,
                    Place::Failed(e) => Outcome::Failed(e),
                };
            }
            if !k.made_one {
                k.made_one = true;
                let command = CompanionCommand::Craft {
                    item: "minecraft:furnace".into(),
                    count: 1,
                    gather: false,
                };
                return match self.companion_task(host, b, command, Skill::Follow) {
                    Ok(task) => Outcome::Push(task),
                    Err(e) => Outcome::Failed(e),
                };
            }
            return Outcome::Failed(format!("no furnace within {STATION_RADIUS} blocks"));
        };
        let Some(horizon_client::containers::BlockEntity::Furnace(f)) =
            self.world.block_entities.get(&(p[0], p[1], p[2]))
        else {
            return Outcome::Busy;
        };
        let count_of = |slot: usize, item: &str| {
            f.items[slot]
                .as_ref()
                .filter(|s| &*s.item == item)
                .map_or(0, |s| s.count)
        };
        let (cooking, done) = (count_of(0, &k.input), count_of(2, &k.output));
        let foreign = [0, 2].iter().any(|&s| {
            f.items[s]
                .as_ref()
                .is_some_and(|s| *s.item != *k.input && *s.item != *k.output)
        });
        let fuel_ticks = f.lit_remaining as f32
            + f.items[1]
                .as_ref()
                .and_then(|s| work::fuel_value(&s.item).map(|v| v * COOK_TICKS * s.count as f32))
                .unwrap_or(0.0);
        let fuel_item = f.items[1].as_ref().map(|s| s.item.to_string());
        if foreign {
            return Outcome::Failed("the furnace is busy with something else".into());
        }
        if (cooking, done) != k.seen {
            k.seen = (cooking, done);
            k.changed = b.ticks;
        }
        let left = k.count - k.made;
        let to_load = left.saturating_sub(cooking + done).min(has_input);
        let short_of_fuel = fuel_ticks < (cooking + to_load) as f32 * COOK_TICKS - 1.0;
        let visit = done > 0 && (done >= left || cooking == 0)
            || to_load > 0
            || short_of_fuel && cooking + to_load > 0;
        if !visit {
            self.companion_close_menu(host, b);
            if cooking == 0 && done == 0 && has_input == 0 {
                return Outcome::Failed(format!(
                    "I only had {} {}",
                    k.made,
                    c::short_id(&k.input).replace('_', " ")
                ));
            }
            if b.ticks - k.changed > FURNACE_PATIENCE {
                return Outcome::Failed("the furnace stopped".into());
            }
            // Waiting by the furnace.
            let at = centre(p);
            if c::horizontal(pos, at) > 4.0 {
                let _ = self.companion_steer(host, b, at, 3.0, false);
            } else {
                b.yaw = c::yaw_toward(pos, at) as f32;
                b.pitch = look_pitch(eye(pos), at);
            }
            return Outcome::Busy;
        }
        let window = host
            .hub
            .guest(b.conn)
            .and_then(|g| g.menu.as_ref())
            .filter(|m| m.pos == Some((p[0], p[1], p[2])))
            .map(|m| m.window);
        let Some(window) = window else {
            self.companion_close_menu(host, b);
            match self.companion_reach(host, b, p, pos, &mut k.tried) {
                Reach::Ready(face) => {
                    self.companion_use_block(host, b, p, face, pos);
                    b.work.menu = host
                        .hub
                        .guest(b.conn)
                        .and_then(|g| g.menu.as_ref())
                        .filter(|m| m.pos == Some((p[0], p[1], p[2])))
                        .map(|m| m.window);
                }
                Reach::Walking => {}
                Reach::Failed => return Outcome::Failed("can't get to the furnace".into()),
            }
            return Outcome::Busy;
        };
        b.work.menu = Some(window);
        // The results out, the input in, then fuel for all of it.
        if done > 0 {
            self.companion_click(host, b, window, 2, 0, QUICK_MOVE);
        }
        if to_load > 0 {
            let input = k.input.clone();
            self.companion_put(host, b, window, FURNACE_BASE, &input, 0, to_load);
        }
        let queued = (cooking + to_load) as f32 * COOK_TICKS;
        if fuel_ticks < queued - 1.0 {
            let inventory = host
                .hub
                .guest(b.conn)
                .map(|g| g.inventory.clone())
                .unwrap_or_default();
            let fuel = inventory
                .iter()
                .flatten()
                .filter(|s| *s.item != *k.input)
                .filter(|s| fuel_item.as_deref().is_none_or(|f| *f == *s.item))
                .filter_map(|s| work::fuel_value(&s.item).map(|v| (s.item.to_string(), v)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match fuel {
                Some((item, value)) => {
                    let n = ((queued - fuel_ticks) / (value * COOK_TICKS))
                        .ceil()
                        .max(1.0) as u32;
                    let n = n.min(carried(&inventory, &item));
                    self.companion_put(host, b, window, FURNACE_BASE, &item, 1, n);
                }
                None if f_lit_or_fuel(fuel_ticks) => {}
                None => {
                    self.companion_close_menu(host, b);
                    return Outcome::Failed("no fuel (coal, charcoal, planks or logs)".into());
                }
            }
        }
        self.companion_close_menu(host, b);
        k.changed = b.ticks;
        Outcome::Busy
    }
}

/// Whether some fuel still burns or waits in the furnace.
fn f_lit_or_fuel(fuel_ticks: f32) -> bool {
    fuel_ticks > 0.0
}
