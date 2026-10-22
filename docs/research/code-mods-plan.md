# Code mods in modpacks: the gaps, the ranking, a first port

ATM10's world now generates well in Horizon (structures, ores, decoration blocks), but **333 of its
496 mods are still "not yet"**. Almost all of them have code: machines, magic, mobs, tools. This page
is the research spike that gives Ludovic the facts to choose the path. It answers four questions:
what the mod API carries today, which ATM10 mods each missing piece unlocks, what a first real code
port looks like, and what porting costs.

The decision from [modpacks.md](../modpacks.md) stands: **porting, not emulation**. Java mods become
native Horizon mods (Rust compiled to WebAssembly, against the Horizon mod API), and each port is
checked against the Java mod running in a dev-only NeoForge oracle. Mod jars and decompiled code stay
on the machine that ports them and never enter this repository.

## 1. What the mod API carries today

| Line | What a mod can do | Where |
|---|---|---|
| API 0.1 (code) | client side only: read the player and the clock, show chat lines and a HUD text panel, keep small files, own chat commands, hooks per tick, frame and chat line | `horizon.wit`, `crates/horizon-mods` |
| API 0.2 (content) | blocks and items declared as data, each copying a vanilla *template* block or item with its numbers changed; recipes, loot, tags, models, textures, lang and worldgen JSON in the mod's namespace | `content.rs`, the content root |
| **API 0.4 preview (this spike)** | block entities for the mod's own blocks: game-owned slots and block state, a batched tick per mod, a saved state, the menu's data values, and the game's item, fuel and cooking-recipe facts | `machine` and `block-entities` in `horizon.wit`, [mods.md](../mods.md#block-entities-api-04-preview) |

Code still cannot touch the world. API 0.1 runs on the client and only reads the player; the runtime
refuses `side = "server"` mods. API 0.2 is pure data: a template gives a mod block every behaviour
Horizon already has for the vanilla block it copies, but nothing of its own.

The sandbox is good and stays: a memory cap, fuel per call (a runaway loop stops the mod, not the
game), no files, sockets or processes, and capabilities the player sees before turning a mod on.

### The gaps for code mods

Each gap is a piece of API plus the game wiring behind it. Codes are used in the ranking below.

| Code | Gap | What it means | State |
|---|---|---|---|
| **T** | ticking block entities with saved state | per-block state, a tick, save and load | **runtime and SDK done here**; the game does not create or save them yet |
| **V** | inventories | slots that hoppers, pipes, menus and drops see; slot rules per side (`WorldlyContainer`) | slots done here (game-owned); slot rules missing |
| **M** | menus and screens | a slot layout, progress bars, buttons, the server-side click logic | data values (`data()`) done here; layout and screen missing |
| **N** | client↔server networking | a mod's own packets between its two halves | missing (`net`, planned in mods.md) |
| **C** | energy, fluid and item transfer between blocks | NeoForge capabilities: FE, fluid tanks, item handlers on neighbours | missing |
| **R** | custom recipe types | machine recipes in the mod's own JSON, and their recipe-viewer layout | missing (vanilla cooking recipes are readable now) |
| **B** | block behaviour hooks | use, place, break, neighbour change, random and scheduled ticks on mod blocks | missing (API 0.3) |
| **I** | item behaviour hooks | use, use on block, inventory tick, attributes, tools and armour of the mod | missing (API 0.3) |
| **E** | server events, player data, config | join, death, spawn, chat, commands with types; data attached to players and chunks; a config schema | missing (API 0.3) |
| **H** | new hooks into vanilla behaviour | what a Mixin does: a named hook added to the engine for everyone | per hook |
| **X** | entities and AI | entity types, attributes, goals, brains, spawning | missing (API 0.5) |
| **D** | custom rendering | block-entity renderers, animated models (GeckoLib), custom baked models, item renderers | missing (API 0.5) |
| **W** | dimensions and worldgen code | dimension types, chunk generators, portals, structure code | worldgen JSON carries; code missing |
| **U** | client UI interfaces | `keys`, `target`, `inventory-read`, `ui`, `recipes` | missing (planned in mods.md) |
| **A** | accessory slots | what Curios does | missing |
| **S** | an embedded scripting VM | CC: Tweaked's Lua computers, Integrated Dynamics, SFM programs | missing |

## 2. ATM10's code mods, ranked by the gaps they hit

**Method.** Each of the 333 not-yet mods in ATM10's coverage report
(`horizon-port-pack report`, 2026-10-19) got the set of gaps above it needs and a "players notice it"
score: 3 for the pack-defining mods (Create, Mekanism, AE2, Refined Storage, Sophisticated Storage and
Backpacks, Waystones, JEI, Jade, JourneyMap, FTB Quests, Ars Nouveau, Iron's Spells, Twilight Forest,
Apotheosis, Farmer's Delight, Mystical Agriculture, Supplementaries, Allthemodium), 2 for mods most
players meet, 1 for the rest. The gap sets are a judgement from what each mod does (its Modrinth
categories, tier and, for the big ones, knowledge of the mod), not from reading 333 jars, so treat any
single row as approximate and the totals as the signal. The table is
[`docs/modpacks/atm10-code-gaps.csv`](../modpacks/atm10-code-gaps.csv) (metadata only: ids, names,
licences, downloads, gaps, required mods).

Of the 333: **24 are data only** (AllTheOres, AllTheCompressed, Pam's crops, XTones...: their jars
were not downloaded yet, no code needed), **1 is a library** (Kotlin for Forge, mislabelled), and
**307 need code**: 221 content mods, 30 integrations, 27 client-UI mods, 18 unclassified and 11
server-only.

### How many mods need each gap

| Gap | Mods that need it | Weighted by "players notice" |
|---|---:|---:|
| T ticking block entities | **162** | 233 |
| N networking | 148 | 231 |
| D custom rendering | 145 | 229 |
| M menus and screens | 140 | 217 |
| V inventories | 124 | 183 |
| I item hooks | 93 | 154 |
| B block hooks | 71 | 115 |
| C energy/fluid/item transfer | 69 | 98 |
| X entities and AI | 69 | 113 |
| E server events, data, config | 47 | 70 |
| R custom recipe types | 44 | 77 |
| U client UI | 42 | 58 |
| H vanilla hooks | 33 | 49 |
| W dimensions | 18 | 32 |
| A accessory slots | 13 | 23 |
| S scripting VM | 8 | 9 |

**Block entities are the most common gap**: 162 of the 307 code mods need them. Of those 162, 123 also
need inventories, 121 menus, 110 custom rendering, 69 transfer between blocks and 43 their own recipe
types. The most common exact sets are "client UI only" (26 mods), **"block entity + inventory + menu +
networking" (17)** and the same plus rendering and transfer (17).

### Unlocked by gap X

A mod is unlocked when every gap it needs is filled *and* the mods it requires are unlocked (AE2's 15
add-ons wait for AE2). Filling the gaps cheapest first, then the machine stack in the order that
unlocks the most at each step:

| Step | Adds | Mods unlocked (total) | Players notice (score 3) |
|---|---|---:|---|
| 1 | U client UI | 17 | |
| 2 | N networking | 28 | JEI, Jade, JourneyMap |
| 3 | B, I, E, H (API 0.3: logic) | 67 | |
| 4 | T, V, M (machines core) | 91 | FTB Quests |
| 5 | D custom rendering | 120 | Sophisticated Storage, Waystones |
| 6 | C transfer | 145 | |
| 7 | R recipe types | 189 | AE2, Refined Storage, Farmer's Delight, Mystical Agriculture |
| 8 | X entities | 264 | Create, Mekanism, Ars Nouveau, Apotheosis, Supplementaries |
| 9 | W dimensions | 283 | Allthemodium, Twilight Forest |
| 10 | A accessory slots | 298 | Iron's Spells, Sophisticated Backpacks |
| 11 | S scripting | 307 | |

What this says:

- **The cheap gaps unlock small mods.** Client UI, networking and the 0.3 logic hooks together unlock
  67 mods, but only JEI, Jade and JourneyMap among the ones players notice, and those three are better
  built into Horizon (a recipe viewer, a look-at tooltip, a map) than ported.
- **Block entities are the backbone of the big tier.** Machines core (T, V, M) plus rendering (D),
  transfer (C) and recipe types (R) take the count from 67 to 189 and bring AE2, Refined Storage,
  Sophisticated Storage, Waystones, Farmer's Delight and Mystical Agriculture.
- **Rendering is the hidden gate.** 110 block-entity mods also draw something of their own (a
  rotating shaft, items on a pedestal, a fluid level). A data-driven first step (animated models in a
  declared format, items and fluids drawn at declared spots) covers most of them without arbitrary
  draw code.
- **Entities are the last big step to the flagships.** Create, Mekanism, Ars Nouveau and Apotheosis
  all need entities (contraptions, the Robit, familiars, bosses) on top of everything else.
- Licences of the 307: 84 permissive, 33 copyleft, 39 All Rights Reserved, 16 custom, 7 restricted
  (non-commercial or no-derivatives) and 128 unknown (CurseForge-only, read from the jar at import).
  Ports stay local whatever the licence; only permissive or copyleft ones could ever be published.

## 3. Proof of concept: Iron Furnaces' iron furnace

**The pick.** Iron Furnaces (10.1 million Modrinth downloads, **Apache-2.0**) is the most downloaded
ATM10 mod whose gaps are exactly the most common machine set: block entity, inventory, menu,
networking. Its iron furnace is a ticking block entity with an inventory, a block state that follows
it, fuel and cooking-recipe lookups, and menu data. The slice ported: **the iron furnace in furnace
mode** (no augments, factory or generator modes, redstone settings, auto input/output or experience).

**The API pieces added** (generic, nothing Iron Furnaces-specific; behind tests):

- `horizon.wit`: interface `machine` (capability `block-entities`): slots and block-state properties
  of the block entities lent to the current tick; the game's max stack sizes, fuel burn times,
  remainders (a bucket from a lava bucket) and cooking recipes. Export `block-entities`: `types`,
  `create`, `tick(handles)`, `save`, `remove`, `data`. A second world, so mods built before keep
  loading.
- `horizon-mods`: the runtime side. Types checked at start (own namespaces, once each, at most 256
  slots); one fuel-budgeted call per mod per tick for all its block entities; slot writes of unknown
  items refused, stacks cut to the item's maximum, property values the block does not allow refused;
  a mod sees only the handles lent to it. Manifest `api = "0.4"`, capability `block-entities`.
- `horizon-mod-sdk`: the `Mod` trait gains `block_entity_types`, `create_block_entity`,
  `tick_block_entities`, `save_block_entity`, `remove_block_entity`, `block_entity_data`, all
  defaulted, so existing mods compile unchanged.
- Tests: a test mod with a small smelter block entity (`crates/horizon-mods/tests/machine`) and three
  sandbox tests (tick, save and load; only its own handles and known items; refused without the
  capability or for a block it does not own).

**The port** is about 200 lines of Rust (local, in the dev-only modports folder; see below). It keeps
the furnace's private state (burn time left, cooking progress and total) by handle, and reads and
writes the game's slots and its `lit` property.

**The harness** runs the same scenarios on both sides and compares every tick:

- Java: `tools/neoforge_oracle.py machines` starts the dev-only NeoForge 21.1.259 server with the
  mod's jar and our exporter mod, places the block, fills its container, runs the block entity's own
  ticker N times and writes the slots, the block-state properties and the saved tag's fields after
  every tick (`MachineTrace.java`).
- Horizon: `crates/horizon-client/examples/mod_machine_probe.rs` loads the `.hmod` through
  `horizon-mods` with **Horizon's own** item, fuel and recipe tables (26.3), and writes the same.
- `tools/mod_machine_harness.py` compares the two.

| Scenario | Ticks | Equal |
|---|---:|---:|
| 10 raw iron, 2 coal (one coal smelts 8 items, 160 ticks each) | 1,800 | 1,800 |
| cobblestone on a lava bucket (the bucket stays) | 500 | 500 |
| a stick's fuel runs out mid-item (progress falls back 2 a tick) | 200 | 200 |
| output slot fills up at 64 | 600 | 600 |
| input swapped to kelp, taken out, raw gold put in, fuel taken | 400 | 400 |
| no recipe: the coal is not burnt | 60 | 60 |
| a wet sponge dried over an empty bucket fills it | 200 | 200 |
| **all** | **3,760** | **3,760** |

The harness fails as it should: the same port with the tier speed set to vanilla's 200 instead of
160 differs from the first tick in six of the seven scenarios.

**What the slice does not show yet.** The game does not create, tick or save mod block entities, so
the furnace does not run in a world yet; its block and model come over through the content carry-over
as any other block. That wiring (place and break, chunk save and load, hoppers through the slots,
drops) is the next step and is generic. The menu needs the screen layout (M), and the other furnace
modes need the gaps they name (energy for the generator: C).

**Licence.** Iron Furnaces is Apache-2.0, which allows publishing a derivative with its notice. The
port still stays local (`~/code/horizon-parity/modports/atm10/codemods/ironfurnaces-port`, with a
NOTICE) until Ludovic decides to publish ports and, if he wants, asks the authors (Qelifern and
XenoMustache).

## 4. What porting costs

**This slice, measured** (one agent on the Mac mini, wall clock from its logs; the whole spike,
ranking included, took under two hours, a third of it waiting for release builds):

| Step | Time | Once or per mod |
|---|---|---|
| Gap ranking of 333 mods (a judgement per mod, see section 2) | about 15 min | once |
| API piece (WIT, runtime, SDK, tests) | about 15 min to write, 2 min of tests | once per gap |
| Oracle machine trace, probe and harness | about 20 min, plus a 10 min release build | once |
| Download, decompile, read the mod's furnace path | about 5 min | per mod |
| Write the port (200 lines) | about 5 min | per mod |
| Scenarios (7) | about 5 min | per mod (partly automatable) |
| Harness: oracle 31 s (NeoForge start included), probe 2 s | 1 min | per run |

So for a machine of this size, **once the API exists, a port slice costs well under an hour of agent
time**, and the harness passed on the first run. Small API pieces are cheap to write; what costs is
wiring each into the game and proving it with harnesses, which the estimates below count. A whole small mod (Iron Furnaces with all its tiers,
augments, modes, menus and recipe viewer pages) is an estimated 1–2 agent-days. A big system is a
campaign: Mekanism, AE2 or Create are hundreds of thousands of lines of Java with their own
networks, renderers and entities, so weeks each even with the API complete.

**The API gaps cost more than the ports.** Estimates in agent-days (build, game wiring, harness,
review), from this spike and from comparable vanilla work in the repo:

| Gap | Agent-days | Notes |
|---|---:|---|
| T/V game wiring: create, tick, save, drop, hoppers, slot rules | 3–4 | builds on this PR |
| M menus and screens (declared layouts the game draws, server click logic) | 4–6 | the container screens exist for vanilla |
| N networking (single player is free; LAN and servers need packets) | 2–3 | |
| B, I, E, H logic hooks (0.3) | 4–6, then about 0.5 per new vanilla hook | |
| C capabilities (FE, fluids, item handlers on neighbours) | 3–5 | |
| R custom recipe types and their viewer layout | 3–4 | |
| U client UI interfaces | 5–8 | or build JEI/Jade/map natively |
| D rendering: data-driven animated models first, then a budgeted draw list | 10–15 | the biggest |
| X entities and AI | 10–15 | Horizon's mob AI exists for vanilla |
| W dimensions and worldgen code | 8–12 | |
| A accessory slots | 2–3 | |

**What agents can automate:** inventory and classification (done), fetching the player's jars,
decompiling, carrying data and assets, writing the port from the decompiled reference and the
harness, generating scenarios from the mod's own recipes and blocks, running the oracle and the
harness, and fixing until green. The harness loop is what makes this safe to hand to agents: they
cannot claim a port works without a tick-by-tick match.

**What agents cannot do alone:** decide to publish ports or contact authors (licences; Ludovic's
call), design each new engine hook a Mixin needed (they can draft it, a reviewer must keep the engine
clean and vanilla-exact), judge visual parity of custom renderers (image diffs help, a human looks),
and follow mod updates (each update reruns the harness, and the port is fixed again).

## 5. Options for Ludovic

| Option | Gaps | Unlocks (of 307) | Cost (agent-days, gaps only) |
|---|---|---:|---:|
| **A. Machines first** | the 0.3 logic hooks (B, I, E, H), then T/V wiring, M, N, D, C, R | 148, incl. AE2, Refined Storage, Sophisticated Storage, Waystones, Farmer's Delight, Mystical Agriculture | 30–45 |
| B. Cheap gaps first | U, N, B, I, E, H | 67, mostly small QoL mods (and JEI, Jade, JourneyMap) | 12–20 |
| A and B | all of the above | 189 | 35–50 |
| C. One flagship vertical | everything Create needs (B, C, D, H, I, N, R, T, V, X), then Create | Create + 12 add-ons, and on the way most of A | 35–55, then weeks for the port |
| D. Stop at data | none: ATM10-like packs stay worldgen, decoration and ores | 0 more | 0 |

The machine stack needs the logic hooks: on its own (T, V, M, N, C, R, D without B, I, E, H) it
unlocks only 53 mods, because most machines also have items and blocks that do something when used.
The ports themselves come on top: under an hour per machine slice like the one above, days per small
mod, weeks per big system.

**Recommendation: A, with JEI, Jade and a minimap built into Horizon itself.** Block entities are the
backbone of the tech and magic mods players build their game around, the first piece is in, and its
first port matched the Java mod on the first run. Building the three most-noticed client mods natively
(a recipe viewer, a look-at tooltip, a minimap) gives ATM10 players what they use every minute without
porting them. Rendering (D) comes right after machines because it gates 110 of the 162 block-entity
mods; entities (X) and dimensions (W) come last, and with them Create, Mekanism, Ars Nouveau and
Twilight Forest.

Next steps if Ludovic picks A: wire mod block entities into the game (create, tick, save, hoppers) and
run the iron furnace in a world; then declared menus; then port a second mod of the same set
(Cobblegen Galore, Step Crafter or Treetap) to measure the per-mod cost again.

## Decisions for Ludovic

1. **The path** (A, B, C or D above).
2. **Publishing ports**: whether permissively licensed ports (Iron Furnaces is Apache-2.0) may ever
   be published, and whether to ask authors first. Until then every port stays local.
3. **Who runs port agents**: only us (players download published ports) or players too, with their
   own AI login.
