//! Loading and calling mods with wasmtime's component model.
use crate::manifest::{Capability, Manifest, Package, Side};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use wasmtime::{
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Component, HasSelf, Linker},
};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../horizon-mod-sdk/wit",
        world: "horizon-mod",
    });
}
/// API 0.4's block entities: a second world, so mods built before it (no
/// `block-entities` export) still instantiate as `horizon-mod`.
mod machine_bindings {
    wasmtime::component::bindgen!({
        path: "../horizon-mod-sdk/wit",
        world: "machine-host",
        additional_derives: [PartialEq],
    });
}
pub use bindings::horizon::mods::{
    game::{Clock, Player},
    hud::{Corner, Line},
};
use bindings::{
    HorizonMod,
    exports::horizon::mods::events::Registration,
    horizon::mods::{chat, game, hud, log as mod_log, storage},
};
use machine_bindings::{MachineHost, horizon::mods::machine};
pub use machine_bindings::{
    exports::horizon::mods::block_entities::BlockEntityType,
    horizon::mods::machine::{CookingKind, CookingRecipe, ItemStack as Stack},
};

/// A mod's linear memory cap.
const MEMORY: usize = 64 << 20;
/// Fuel (about one unit per WebAssembly instruction) per call: a bound on
/// runaway loops, far above the hooks' time budgets.
const FUEL_INIT: u64 = 2_000_000_000;
const FUEL_TICK: u64 = 50_000_000;
const FUEL_FRAME: u64 = 10_000_000;
const FUEL_EVENT: u64 = 100_000_000;
/// Chat lines a mod may show per call, HUD lines and their length.
const CHAT_PER_CALL: usize = 8;
const HUD_LINES: usize = 16;
const LINE_CHARS: usize = 256;
/// Total bytes a mod's `storage` may hold on disk, across all its keys.
const STORAGE_QUOTA: u64 = 4 << 20;
/// Distinct keys a mod's `storage` may hold: `STORAGE_QUOTA` alone doesn't
/// bound a flood of zero- or near-zero-byte files (each still a file name,
/// a directory entry and an inode).
const STORAGE_MAX_KEYS: usize = 64;
/// Bytes `storage::set` may write across one hook call: fuel bounds a
/// mod's CPU time per call, not the time a synchronous disk write takes, so
/// a cheap-in-fuel loop replacing one key with large payloads needs its own
/// cap, separate from the on-disk quota it would otherwise stay under.
const STORAGE_WRITE_BUDGET: u64 = 1 << 20;
/// A mod's data, beside its `horizon-mods` folder (`find`'s `path.parent()`
/// never has a manifest there, so `find` never mistakes it for a mod).
const STORAGE_DIR: &str = ".horizon-mod-data";

/// The export a block-entity mod adds (`horizon:mods/block-entities`).
const BLOCK_ENTITIES_EXPORT: &str = "horizon:mods/block-entities@0.1.0";
/// Slots a mod's block entity may ask the game to keep.
const MAX_SLOTS: u32 = 256;

/// The game's item and recipe facts that block entities query (`machine`):
/// set once with [`Mods::set_item_facts`] after the data packs load.
pub trait ItemFacts: Send + Sync {
    /// 0 for an unknown item.
    fn max_stack_size(&self, id: &str) -> u32;
    /// Ticks the item burns as furnace fuel, 0 if it is no fuel.
    fn burn_time(&self, id: &str) -> u32;
    /// What is left once the item is used up (a bucket from a lava bucket).
    fn remainder(&self, id: &str) -> Option<String>;
    fn cooking_recipe(&self, kind: CookingKind, input: &str) -> Option<CookingRecipe>;
}

/// A block state property of a block entity's block, as `machine` sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: String,
    /// The values the block allows for it; `set-property` refuses others.
    pub values: Vec<String>,
}

/// What the game lends a mod's block entity during a tick: its slots and its
/// block's properties. `changed` is set when the mod wrote either, so the
/// game saves the chunk and updates the block and open menus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockEntityView {
    pub handle: u32,
    pub slots: Vec<Option<Stack>>,
    pub properties: Vec<Property>,
    pub changed: bool,
}

/// What mods may read of the game, copied once per tick by the game.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub player: Option<Player>,
    pub clock: Option<Clock>,
}

/// What mods asked the game to show since the last [`Mods::drain`].
#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    /// `chat.show`: a line after the mod's name.
    Chat { name: String, text: String },
    /// A mod command's feedback, red when `error`.
    Feedback { text: String, error: bool },
    /// The mod was turned off: it trapped, overran or failed to start.
    Stopped { name: String, reason: String },
}

/// A mod's text panel, drawn every frame until it changes.
#[derive(Clone, Debug)]
pub struct Panel {
    pub corner: Corner,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Installed and turned off by the player.
    Off,
    Running,
    /// Could not load or start, or stopped while running.
    Failed(String),
}

/// One installed mod, for the Mods page.
pub struct Entry {
    pub path: PathBuf,
    /// None when the manifest could not be read (`status` says why).
    pub manifest: Option<Manifest>,
    pub status: Status,
    /// The mod's average time per call of its busiest hook.
    pub cost: Duration,
    run: Option<Running>,
    modified: Option<SystemTime>,
}

struct Running {
    store: Store<State>,
    exports: Exports,
    hooks: Registration,
    /// Its block entity types (API 0.4), checked against its namespaces.
    types: Vec<BlockEntityType>,
}

struct Exports {
    events: HorizonMod,
    /// Present when the component exports `block-entities`.
    machine: Option<MachineHost>,
}

struct State {
    name: String,
    id: String,
    snapshot: Arc<Snapshot>,
    out: Vec<Output>,
    panel: Option<Panel>,
    /// Capability `storage`'s folder; made on first `set`.
    data_dir: PathBuf,
    /// Bytes `storage::set` may still write before this hook call ends
    /// (reset per call in [`Mods::call`]): fuel bounds a mod's CPU time but
    /// not disk I/O time, so a loop of small-fuel, large-payload `set`s
    /// needs its own budget.
    storage_budget: u64,
    limits: StoreLimits,
    /// Capability `block-entities`: the game's facts, and the block entities
    /// lent to the current `tick` call (none outside it).
    facts: Option<Arc<dyn ItemFacts>>,
    views: Vec<BlockEntityView>,
}

/// The installed mods and the engine running them. `Mods::default()` is
/// the state with none: no engine, and every hook a no-op.
#[derive(Default)]
pub struct Mods {
    engine: Option<(Engine, Linker<State>)>,
    entries: Vec<Entry>,
    snapshot: Arc<Snapshot>,
    out: Vec<Output>,
    facts: Option<Arc<dyn ItemFacts>>,
    /// Each live block entity's handle: the entry of the mod that owns it.
    handles: HashMap<u32, usize>,
    next_handle: u32,
    /// The block entities a mod held when it stopped mid-call, saved from
    /// its dropped store so the game gets their slots back.
    stranded: Vec<BlockEntityView>,
}

/// The `.hmod` files and mod folders in `dir`, by name.
pub fn find(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "hmod")
                || p.join(crate::manifest::MANIFEST).is_file()
        })
        .collect();
    found.sort();
    found
}

impl Mods {
    /// Reads every mod at `paths` and starts those `enabled` says are on.
    /// A command that `reserved` (vanilla's commands) or an earlier mod
    /// already uses is refused.
    pub fn load(
        paths: &[PathBuf],
        enabled: impl Fn(&str) -> bool,
        reserved: impl Fn(&str) -> bool,
    ) -> Mods {
        let mut mods = Mods::default();
        for path in paths {
            let modified = modified(path);
            let mut entry = Entry {
                path: path.clone(),
                manifest: None,
                status: Status::Off,
                cost: Duration::ZERO,
                run: None,
                modified,
            };
            match Package::read(path) {
                Err(e) => entry.status = Status::Failed(e),
                Ok(package) => {
                    let on = enabled(&package.manifest.id);
                    entry.manifest = Some(package.manifest.clone());
                    if on {
                        // Content is read before the world loads; a mod whose
                        // content is refused stays off, with the reason.
                        let content = crate::Content::read(path, &package.manifest);
                        entry.status =
                            match content.and_then(|_| mods.start(path, &package, &reserved)) {
                                Ok(run) => {
                                    entry.run = run;
                                    Status::Running
                                }
                                Err(e) => Status::Failed(e),
                            };
                    }
                }
            }
            if let (Status::Failed(reason), Some(m)) = (&entry.status, &entry.manifest) {
                log::warn!("Horizon: mod {} did not start: {reason}", m.id);
            }
            mods.entries.push(entry);
        }
        mods
    }

    fn engine(&mut self) -> Result<&(Engine, Linker<State>), String> {
        if self.engine.is_none() {
            let mut config = Config::new();
            config.wasm_component_model(true).consume_fuel(true);
            let engine = Engine::new(&config).map_err(|e| e.to_string())?;
            let mut linker = Linker::new(&engine);
            HorizonMod::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)
                .map_err(|e| e.to_string())?;
            MachineHost::add_to_linker::<State, HasSelf<State>>(&mut linker, |s| s)
                .map_err(|e| e.to_string())?;
            self.engine = Some((engine, linker));
        }
        Ok(self.engine.as_ref().expect("made above"))
    }

    /// Starts a mod's code; `Ok(None)` for a content-only mod, which has
    /// nothing to run (the game reads its content before the world loads).
    /// `path` is the mod's own `.hmod` or folder, for capability `storage`'s
    /// per-mod directory.
    fn start(
        &mut self,
        path: &Path,
        package: &Package,
        reserved: &dyn Fn(&str) -> bool,
    ) -> Result<Option<Running>, String> {
        let m = &package.manifest;
        let Some(wasm) = &package.wasm else {
            return Ok(None);
        };
        if m.side == Side::Server {
            return Err("server mods do not run yet".into());
        }
        // Two running mods with the same id would share one `data_dir`
        // (capability `storage`): the first loaded keeps the id.
        if self
            .entries
            .iter()
            .any(|e| e.run.is_some() && e.manifest.as_ref().is_some_and(|other| other.id == m.id))
        {
            return Err(format!("another running mod already uses id {}", m.id));
        }
        let taken: Vec<String> = self
            .entries
            .iter()
            .filter_map(|e| e.run.as_ref())
            .flat_map(|r| r.hooks.commands.iter().cloned())
            .collect();
        let snapshot = self.snapshot.clone();
        let facts = self.facts.clone();
        let (engine, linker) = self.engine()?;
        let component =
            Component::new(engine, wasm).map_err(|e| format!("not a component: {e}"))?;
        let ty = component.component_type();
        let imports: Vec<String> = ty
            .imports(engine)
            .map(|(name, _)| name.to_string())
            .collect();
        m.check_imports(imports.iter().map(String::as_str))?;
        let has_machine = ty
            .exports(engine)
            .any(|(name, _)| name == BLOCK_ENTITIES_EXPORT);
        let data_dir = path
            .parent()
            .unwrap_or(Path::new("."))
            .join(STORAGE_DIR)
            .join(&m.id);
        let state = State {
            name: m.name.clone(),
            id: m.id.clone(),
            snapshot,
            out: Vec::new(),
            panel: None,
            data_dir,
            storage_budget: STORAGE_WRITE_BUDGET,
            limits: StoreLimitsBuilder::new()
                .memory_size(MEMORY)
                .instances(16)
                .build(),
            facts,
            views: Vec::new(),
        };
        let mut store = Store::new(engine, state);
        store.limiter(|s| &mut s.limits);
        store.set_fuel(FUEL_INIT).map_err(|e| e.to_string())?;
        let instance = linker
            .instantiate(&mut store, &component)
            .map_err(|e| reason(&e))?;
        let events = HorizonMod::new(&mut store, &instance).map_err(|e| reason(&e))?;
        let machine = match has_machine {
            true => Some(MachineHost::new(&mut store, &instance).map_err(|e| reason(&e))?),
            false => None,
        };
        let mut hooks = events
            .horizon_mods_events()
            .call_init(&mut store)
            .map_err(|e| reason(&e))??;
        let mut types = Vec::new();
        if let Some(machine) = &machine {
            let declared = machine
                .horizon_mods_block_entities()
                .call_types(&mut store)
                .map_err(|e| reason(&e))?;
            if !declared.is_empty() && !m.capabilities.contains(&Capability::BlockEntities) {
                return Err(format!(
                    "has block entities but not capability block-entities in {}",
                    crate::manifest::MANIFEST
                ));
            }
            types = check_types(m, declared)?;
        }
        hooks.commands.retain(|c| {
            let free = !reserved(c) && !taken.contains(c);
            if !free {
                log::warn!("Horizon: mod {} cannot take /{c}: already a command", m.id);
            }
            free
        });
        self.out.append(&mut store.data_mut().out);
        Ok(Some(Running {
            store,
            exports: Exports { events, machine },
            hooks,
            types,
        }))
    }

    /// True when at least one mod runs: the game skips its mod work otherwise.
    pub fn is_running(&self) -> bool {
        self.entries.iter().any(|e| e.run.is_some())
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    /// What mods read until the next snapshot.
    pub fn set_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshot = Arc::new(snapshot);
    }

    /// Calls `f` on every running mod whose hooks `wants`, with `fuel`.
    fn each(
        &mut self,
        fuel: u64,
        wants: impl Fn(&Registration) -> bool,
        mut f: impl FnMut(&Exports, &mut Store<State>) -> wasmtime::Result<()>,
    ) {
        for i in 0..self.entries.len() {
            if self.entries[i]
                .run
                .as_ref()
                .is_some_and(|r| wants(&r.hooks))
            {
                self.call(i, fuel, &mut f);
            }
        }
    }

    /// One call into mod `i`; a trap or an overrun turns the mod off.
    fn call<R>(
        &mut self,
        i: usize,
        fuel: u64,
        f: impl FnOnce(&Exports, &mut Store<State>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        let entry = &mut self.entries[i];
        let run = entry.run.as_mut()?;
        run.store.data_mut().snapshot = self.snapshot.clone();
        run.store.data_mut().storage_budget = STORAGE_WRITE_BUDGET;
        run.store.data_mut().facts = self.facts.clone();
        let started = Instant::now();
        let result = run
            .store
            .set_fuel(fuel)
            .and_then(|()| f(&run.exports, &mut run.store));
        // An average over about the last 64 calls.
        entry.cost = (entry.cost * 63 + started.elapsed()) / 64;
        self.out.append(&mut run.store.data_mut().out);
        match result {
            Ok(r) => Some(r),
            Err(e) => {
                let reason = reason(&e);
                log::warn!("Horizon: mod {} stopped: {reason}", run.store.data().id);
                let name = run.store.data().name.clone();
                self.out.push(Output::Stopped {
                    name,
                    reason: reason.clone(),
                });
                entry.status = Status::Failed(reason);
                self.stranded = std::mem::take(&mut run.store.data_mut().views);
                entry.run = None;
                None
            }
        }
    }

    /// A game tick.
    pub fn tick(&mut self, tick: u64) {
        self.each(
            FUEL_TICK,
            |h| h.tick,
            |m, s| m.events.horizon_mods_events().call_tick(s, tick),
        );
    }
    /// A rendered frame, `seconds` after the last.
    pub fn frame(&mut self, seconds: f32) {
        self.each(
            FUEL_FRAME,
            |h| h.frame,
            |m, s| m.events.horizon_mods_events().call_frame(s, seconds),
        );
    }
    /// A chat line the player received, as plain text.
    pub fn chat(&mut self, text: &str) {
        self.each(
            FUEL_EVENT,
            |h| h.chat,
            |m, s| m.events.horizon_mods_events().call_chat(s, text),
        );
    }
    /// Whether a running mod owns `/name`.
    pub fn has_command(&self, name: &str) -> bool {
        self.command_owner(name).is_some()
    }
    fn command_owner(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| {
            e.run
                .as_ref()
                .is_some_and(|r| r.hooks.commands.iter().any(|c| c == name))
        })
    }
    /// Runs `line` (without its slash) if a mod owns its command; false
    /// when none does.
    pub fn command(&mut self, line: &str) -> bool {
        let (name, args) = line.split_once(' ').unwrap_or((line, ""));
        let Some(i) = self.command_owner(name) else {
            return false;
        };
        let result = self.call(i, FUEL_EVENT, |m, s| {
            m.events.horizon_mods_events().call_command(s, name, args)
        });
        match result {
            Some(Ok(Some(text))) => self.out.push(Output::Feedback { text, error: false }),
            Some(Err(text)) => self.out.push(Output::Feedback { text, error: true }),
            Some(Ok(None)) | None => {}
        }
        true
    }
    /// The commands running mods own, for suggestions.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter_map(|e| e.run.as_ref())
            .flat_map(|r| r.hooks.commands.iter().map(String::as_str))
    }

    /// The running mods' HUD panels.
    pub fn panels(&self) -> impl Iterator<Item = &Panel> {
        self.entries
            .iter()
            .filter_map(|e| e.run.as_ref()?.store.data().panel.as_ref())
    }
    /// What mods asked to show since the last call.
    pub fn drain(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.out)
    }

    /// The game's item and recipe facts, for capability `block-entities`.
    pub fn set_item_facts(&mut self, facts: Arc<dyn ItemFacts>) {
        self.facts = Some(facts);
    }

    /// The block entity types of the running mods (API 0.4).
    pub fn block_entity_types(&self) -> impl Iterator<Item = &BlockEntityType> {
        self.entries
            .iter()
            .filter_map(|e| e.run.as_ref())
            .flat_map(|r| r.types.iter())
    }

    /// The running mod whose block entity type is `block`'s, and the type.
    fn block_entity_type(&self, block: &str) -> Option<(usize, &BlockEntityType)> {
        self.entries.iter().enumerate().find_map(|(i, e)| {
            let t = e.run.as_ref()?.types.iter().find(|t| t.block == block)?;
            Some((i, t))
        })
    }

    /// A block entity for `block`, placed (`saved` empty) or loaded (what
    /// [`Mods::save_block_entity`] returned): its handle, and the slots the
    /// game must keep for it. None when no running mod has `block`, or the
    /// mod stopped.
    pub fn create_block_entity(&mut self, block: &str, saved: &[u8]) -> Option<(u32, u32)> {
        let (i, ty) = self.block_entity_type(block)?;
        let slots = ty.slots;
        let handle = self.next_handle;
        self.next_handle = self.next_handle.wrapping_add(1);
        self.call(i, FUEL_EVENT, |m, s| {
            machine_of(m)?
                .horizon_mods_block_entities()
                .call_create(s, handle, block, saved)
        })?;
        self.handles.insert(handle, i);
        Some((handle, slots))
    }

    /// One server tick of the ticking block entities in `views`, one call
    /// per mod. Each mod sees only its own; their slots and properties come
    /// back changed. Views of unknown handles or stopped mods are left alone.
    pub fn tick_block_entities(&mut self, views: &mut [BlockEntityView]) {
        for i in 0..self.entries.len() {
            let Some(run) = self.entries[i].run.as_ref() else {
                continue;
            };
            let ticking = |v: &BlockEntityView| self.handles.get(&v.handle) == Some(&i);
            if !run.types.iter().any(|t| t.ticks) || !views.iter().any(ticking) {
                continue;
            }
            let mine: Vec<usize> = (0..views.len()).filter(|&v| ticking(&views[v])).collect();
            let lent: Vec<BlockEntityView> = mine
                .iter()
                .map(|&v| std::mem::take(&mut views[v]))
                .collect();
            let handles: Vec<u32> = lent.iter().map(|v| v.handle).collect();
            if let Some(run) = self.entries[i].run.as_mut() {
                run.store.data_mut().views = lent;
            }
            self.call(i, FUEL_TICK, |m, s| {
                machine_of(m)?
                    .horizon_mods_block_entities()
                    .call_tick(s, &handles)
            });
            // A mod that stopped in the call left its views in `stranded`,
            // with what it wrote before it stopped.
            let returned = match self.entries[i].run.as_mut() {
                Some(run) => std::mem::take(&mut run.store.data_mut().views),
                None => std::mem::take(&mut self.stranded),
            };
            for (slot, view) in mine.into_iter().zip(returned) {
                views[slot] = view;
            }
        }
    }

    /// What the game saves for the block entity beside its slots.
    pub fn save_block_entity(&mut self, handle: u32) -> Option<Vec<u8>> {
        let i = *self.handles.get(&handle)?;
        self.call(i, FUEL_EVENT, |m, s| {
            machine_of(m)?
                .horizon_mods_block_entities()
                .call_save(s, handle)
        })
    }

    /// The values the block entity's menu shows (progress, fuel).
    pub fn block_entity_data(&mut self, handle: u32) -> Vec<i32> {
        let Some(&i) = self.handles.get(&handle) else {
            return Vec::new();
        };
        self.call(i, FUEL_EVENT, |m, s| {
            machine_of(m)?
                .horizon_mods_block_entities()
                .call_data(s, handle)
        })
        .unwrap_or_default()
    }

    /// The block entity was broken or unloaded.
    pub fn remove_block_entity(&mut self, handle: u32) {
        if let Some(i) = self.handles.remove(&handle) {
            self.call(i, FUEL_EVENT, |m, s| {
                machine_of(m)?
                    .horizon_mods_block_entities()
                    .call_remove(s, handle)
            });
        }
    }

    /// Restarts the mods whose file changed since they loaded (mod
    /// development: rebuild, and the game picks it up). True when any did.
    pub fn reload_changed(&mut self, reserved: impl Fn(&str) -> bool) -> bool {
        let mut any = false;
        for i in 0..self.entries.len() {
            let entry = &self.entries[i];
            let now = modified(&entry.path);
            let was_on = entry.status != Status::Off;
            if now == entry.modified || !was_on {
                continue;
            }
            any = true;
            self.entries[i].modified = now;
            self.entries[i].run = None;
            // The new instance knows none of the old one's block entities:
            // the game creates them again from their saves.
            self.handles.retain(|_, owner| *owner != i);
            let path = self.entries[i].path.clone();
            let started = Package::read(&path).and_then(|p| {
                self.entries[i].manifest = Some(p.manifest.clone());
                self.start(&path, &p, &reserved)
            });
            let entry = &mut self.entries[i];
            let id = entry.manifest.as_ref().map_or("?", |m| m.id.as_str());
            match started {
                Ok(run) => {
                    log::info!("Horizon: mod {id} reloaded");
                    entry.run = run;
                    entry.status = Status::Running;
                }
                Err(e) => {
                    log::warn!("Horizon: mod {id} did not reload: {e}");
                    entry.status = Status::Failed(e);
                }
            }
        }
        any
    }
}

/// A mod's `block-entities` export; an error (the call fails, the mod goes
/// on) for a component without one, which then has no types to call it for.
fn machine_of(m: &Exports) -> wasmtime::Result<&MachineHost> {
    m.machine
        .as_ref()
        .ok_or_else(|| wasmtime::Error::msg("no block-entities export"))
}

/// The block entity types a mod declared, if every one is for a block in its
/// own namespaces, declared once, with at most [`MAX_SLOTS`] slots.
fn check_types(m: &Manifest, types: Vec<BlockEntityType>) -> Result<Vec<BlockEntityType>, String> {
    for (n, t) in types.iter().enumerate() {
        let owned = t
            .block
            .split_once(':')
            .is_some_and(|(ns, path)| !path.is_empty() && m.owned_namespaces().any(|o| o == ns));
        if !owned {
            return Err(format!(
                "declares a block entity for {}, which is not one of its blocks",
                t.block
            ));
        }
        if types[..n].iter().any(|o| o.block == t.block) {
            return Err(format!("declares {} twice", t.block));
        }
        if t.slots > MAX_SLOTS {
            return Err(format!("{} asks for over {MAX_SLOTS} slots", t.block));
        }
    }
    Ok(types)
}

fn modified(path: &Path) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() {
        // A folder mod changes when its component does.
        return std::fs::metadata(path.join(crate::manifest::WASM))
            .ok()?
            .modified()
            .ok();
    }
    meta.modified().ok()
}

/// A trap or error, as the player reads it.
fn reason(e: &wasmtime::Error) -> String {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "it took too long (an endless loop?)".into(),
        Some(Trap::UnreachableCodeReached) => "it crashed (a panic)".into(),
        Some(trap) => format!("it crashed: {trap}"),
        None => format!("{e:#}"),
    }
}

fn clip(text: String) -> String {
    match text.char_indices().nth(LINE_CHARS) {
        Some((at, _)) => text[..at].to_string(),
        None => text,
    }
}

impl mod_log::Host for State {
    fn write(&mut self, level: mod_log::Level, message: String) {
        let level = match level {
            mod_log::Level::Debug => log::Level::Debug,
            mod_log::Level::Info => log::Level::Info,
            mod_log::Level::Warn => log::Level::Warn,
            mod_log::Level::Error => log::Level::Error,
        };
        log::log!(level, "[mod {}] {}", self.id, clip(message));
    }
}
impl game::Host for State {
    fn get_player(&mut self) -> Option<Player> {
        self.snapshot.player.clone()
    }
    fn get_clock(&mut self) -> Option<Clock> {
        self.snapshot.clock
    }
}
impl chat::Host for State {
    fn show(&mut self, text: String) {
        let shown = self
            .out
            .iter()
            .filter(|o| matches!(o, Output::Chat { .. }))
            .count();
        if shown < CHAT_PER_CALL {
            let name = self.name.clone();
            self.out.push(Output::Chat {
                name,
                text: clip(text),
            });
        }
    }
}
impl hud::Host for State {
    fn set_panel(&mut self, corner: Corner, lines: Vec<Line>) {
        let lines = lines
            .into_iter()
            .take(HUD_LINES)
            .map(|l| Line {
                text: clip(l.text),
                color: l.color & 0xFF_FFFF,
            })
            .collect();
        self.panel = Some(Panel { corner, lines });
    }
    fn clear(&mut self) {
        self.panel = None;
    }
}
impl storage::Host for State {
    fn get(&mut self, key: String) -> Option<Vec<u8>> {
        storage_key_ok(&key)
            .then(|| std::fs::read(self.data_dir.join(&key)).ok())
            .flatten()
    }
    fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        if !storage_key_ok(&key) {
            return Err(format!("{key:?} is not [a-z0-9_.-]{{1,64}}"));
        }
        let len = value.len() as u64;
        if len > self.storage_budget {
            return Err("too much storage traffic in one call".into());
        }
        // Everything already on disk but the file this call replaces.
        let existing: Vec<std::fs::DirEntry> = std::fs::read_dir(&self.data_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy() != key)
            .collect();
        if existing.len() >= STORAGE_MAX_KEYS {
            return Err(format!("would hold over {STORAGE_MAX_KEYS} keys"));
        }
        let used: u64 = existing
            .iter()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        if used + len > STORAGE_QUOTA {
            return Err(format!(
                "would use over {} MiB of storage",
                STORAGE_QUOTA >> 20
            ));
        }
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        std::fs::write(self.data_dir.join(&key), value).map_err(|e| e.to_string())?;
        self.storage_budget -= len;
        Ok(())
    }
    fn delete(&mut self, key: String) {
        if storage_key_ok(&key) {
            let _ = std::fs::remove_file(self.data_dir.join(&key));
        }
    }
}

/// A storage key: one path segment of `[a-z0-9_.-]{1,64}`, not `.` or `..`.
/// No `/`, so a key can never leave the mod's own directory.
fn storage_key_ok(key: &str) -> bool {
    let chars_ok = (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b));
    chars_ok && key != "." && key != ".."
}

impl State {
    /// The block entity `handle` lent to this call.
    fn view(&mut self, handle: u32) -> Option<&mut BlockEntityView> {
        self.views.iter_mut().find(|v| v.handle == handle)
    }
}
impl machine::Host for State {
    fn get_slot(&mut self, handle: u32, slot: u32) -> Option<Stack> {
        self.view(handle)?
            .slots
            .get(slot as usize)
            .cloned()
            .flatten()
    }
    fn set_slot(&mut self, handle: u32, slot: u32, stack: Option<Stack>) -> bool {
        let stack = match stack {
            Some(s) if s.count > 0 => {
                // Unknown items are refused; with no facts set (a test host)
                // nothing is known about items, so any id passes uncut.
                let max = match &self.facts {
                    Some(facts) => match facts.max_stack_size(&s.id) {
                        0 => return false,
                        max => max,
                    },
                    None => u32::MAX,
                };
                Some(Stack {
                    count: s.count.min(max),
                    id: s.id,
                })
            }
            _ => None,
        };
        let Some(view) = self.view(handle) else {
            return false;
        };
        let Some(cell) = view.slots.get_mut(slot as usize) else {
            return false;
        };
        if *cell != stack {
            *cell = stack;
            view.changed = true;
        }
        true
    }
    fn get_property(&mut self, handle: u32, name: String) -> Option<String> {
        let view = self.view(handle)?;
        let p = view.properties.iter().find(|p| p.name == name)?;
        Some(p.value.clone())
    }
    fn set_property(&mut self, handle: u32, name: String, value: String) -> bool {
        let Some(view) = self.view(handle) else {
            return false;
        };
        let Some(p) = view.properties.iter_mut().find(|p| p.name == name) else {
            return false;
        };
        if !p.values.contains(&value) {
            return false;
        }
        if p.value != value {
            p.value = value;
            view.changed = true;
        }
        true
    }
    fn max_stack_size(&mut self, id: String) -> u32 {
        self.facts.as_ref().map_or(0, |f| f.max_stack_size(&id))
    }
    fn burn_time(&mut self, id: String) -> u32 {
        self.facts.as_ref().map_or(0, |f| f.burn_time(&id))
    }
    fn remainder(&mut self, id: String) -> Option<String> {
        self.facts.as_ref()?.remainder(&id)
    }
    fn cooking(&mut self, kind: CookingKind, input: String) -> Option<CookingRecipe> {
        self.facts.as_ref()?.cooking_recipe(kind, &input)
    }
}
