# Companion speedrun: the Speedrunner role's any% route

The Speedrunner role should beat Minecraft alone, from a fresh world to the Ender Dragon's death, reliably
and fast. This document covers four things:

- what the engine can already do for that route;
- the route itself, as a state machine a fair bot can follow;
- the skills, the planner and the harness to build, in order;
- an honest effort estimate.

It was written on 2026-10-20 from the companion body branch (`feat/companion-body` @ 13f80741), the brief
(`companions-planner.md`) and headless test runs on the Mac mini. It contains no gameplay code.

**Perception levels.** Ludovic decided these on 2026-10-20. Perception is a player setting that every role
uses, and the default is Fair:

1. **Fair:** it sees only what a player could see.
2. **Sharp senses:** Fair, plus blocks within about 16 blocks through walls.
3. **Scout:** Fair, plus where structures are, as /locate would say.
4. **All-knowing:** structures, plus every block.

The planner works through one perception interface (see [Perception](#perception-one-interface-four-levels)).
Each route step lists its variant per level where the levels differ, and the harness reports results per
level.

## Summary

- **The engine can already play the whole route in single player**, for the local player. These all work
  headless:
  - gravel drops flint;
  - buckets pick up and place lava and water, and water turns a lava source into obsidian;
  - a nether portal lights and takes you through;
  - fortresses have blaze spawners;
  - piglins barter and attack a player without gold;
  - blazes drop rods and endermen drop pearls;
  - an eye of ender flies toward the stronghold and drops;
  - the portal room has its silverfish spawner, and 12 eyes open the End portal;
  - the dragon circles, strafes and lands;
  - a bed explodes in the End;
  - killing the dragon builds the exit portal and the egg, and the exit portal takes you home.

  `/tick sprint` works and drives the companion: 1,200–1,500 ticks per second, 60–75 times real time.
- **Three engine gaps block a companion going solo.**
  1. **A companion never goes through a portal by itself.** In the single-player window, a companion
     standing in a lit portal stays there; it only changes dimension when its owner does, within 16 blocks.
     On the server thread, guests do travel, but companions refuse to spawn there.
  2. **The dragon only sees the local player.** It targets, strafes, charges, knocks back and breathes on
     `self.player`, and ignores guests, so it would never fight the companion.
  3. **On a server level (the server thread or a dedicated server) the dragon never spawns.** The fight
     ticks only when there is a local player in the End. This is also a public multiplayer bug and can be
     fixed publicly.
- **The route chosen for a bot:**
  1. Wood, stone tools and food.
  2. About 10 iron from caves and strip mining.
  3. Two buckets.
  4. Cast the nether portal in a mould at a lava pool.
  5. Fortress, then about 7 blaze rods at a spawner.
  6. About 13 pearls from endermen in a warped forest. Bartering is too poor in 26.3.
  7. Back to the Overworld, triangulate the stronghold with two eye throws (exact bearings), and dig down
     by staircase.
  8. Find the portal room and fill the frame.
  9. In the End: shoot the 8 open crystals with a bow, pillar up and shoot the 2 caged ones, then kill the
     dragon with a sword while it perches.

  A bed one-cycle is the later, faster variant.
- **Effort:**
  - **A first unassisted dragon kill on All-knowing, on one seed:** about 30–36 agent-days.
  - **At least 50% success on Fair over 10 seeds:** about 55–65 agent-days in total.

  Most of it is skills: movement that breaks and places blocks, and fighting. The engine part is about
  8 agent-days. With three or four agents working in parallel this is 2.5–4 weeks of calendar time.

## 1. Engine gap audit

How each finding was checked:

- **Code:** read in this tree.
- **Test:** run headless with `--offscreen`, `HORIZON_NO_AUDIO=1` and the control socket. The scripts are in
  [Appendix: tests run](#appendix-tests-run).
- **Vanilla:** read in the 26.3 sources under `.local/minecraft/26.3/sources`.

Status values:

- **works:** right for the local player.
- **works (guests):** also right for a companion.
- **partial:** works, with the gap named in the row.
- **missing:** not there.

| Mechanic | Status | Evidence | What to build |
|---|---|---|---|
| Tree, ore and stone drops | works | Loot tables are data-driven (`items.rs` `loot_table`). Guests mine with survival break times (`game/lan_actions.rs`). | Nothing. |
| Gravel → flint (10% without Fortune) | works | Test: 7 flint from 40 `loot … mine` rolls with an iron shovel. The table is `blocks/gravel` with `table_bonus` 0.1. | Nothing. |
| Crafting (tools, bucket, flint and steel, shield, bow, arrows, eyes, blaze powder, beds) | works | The recipes are vanilla data (`data/minecraft/recipe/*.json` all present). Guests craft through menus (`game/lan_crafting.rs`). The skills branch adds `companion_craft.rs`. | A planner over recipes (S1). |
| Smelting | works | Furnace block entity. The skills branch adds the smelt skill. | Nothing in the engine. |
| Water and lava buckets | works | Test: pick up a lava source, then place it. Dispenser paths are in `horizon-world/src/level/buckets.rs`. | Nothing. |
| Flowing liquids | works | Test: flow, then obsidian or cobblestone. `fluid_flow` and `fluid` parity harnesses. | Nothing. |
| Obsidian from water reaching a lava source | works | Test: lava source plus water beside it or above it → `minecraft:obsidian`. `horizon-world/src/level/fluids.rs:609`. | Nothing. |
| Nether portal: frame detection, lighting, travel (local player) | works | Test: a 4×5 obsidian frame lit with flint and steel on the top face of the bottom block → `nether_portal[axis=x]`. In 5 s the player is in the Nether. `game/nether_portal.rs`, `test_nether_portal_parity.py`. | Nothing. |
| Nether portal: a companion alone | missing | Test: a companion sent into a lit portal stayed in the Overworld. It changes dimension only with its owner, within 16 blocks (`companion_body.rs:117`). `tick_server_travel` returns unless the game is a server (`server_travel.rs:59`). | Watch mode moves the owner with the companion (E2a). Later, companions on the server thread (E2b). |
| Nether portal: a companion with its owner | works (guests) | Test: the owner walked through and the companion arrived beside them in the Nether. | Nothing. |
| End portal: a companion alone | missing | `tick_end` checks only `self.player` (`game/end.rs:347`). | Same as above (E2). |
| Ruined portals | works | Test: `/locate` finds one 495 blocks from spawn on seed 20261020. The generator is `worldgen/structure/ruined_portal.rs`. The chest table is vanilla data: obsidian, flint and steel, fire charge and flint, each 40 of 398 per roll, 4–8 rolls. | Nothing. |
| Lava lakes, surface and underground | works | Lake feature (`worldgen/feature_graph.rs`), aquifer lava below y −54. | Nothing. |
| Nether generation: wastes, soul sand valley, warped forest | works | Test: `/locate biome` finds each within 71–214 blocks of the portal. | Nothing. |
| Fortresses with blaze spawners | works | Test: `/locate` finds one 427 blocks away. It holds two spawners whose `SpawnData` is a blaze. `worldgen/structure/fortress.rs:805`. | Nothing. |
| Bastions | works | Test: `/locate` finds one 794 blocks away. Jigsaw generation. Block-level parity not checked here. | Nothing for this route. |
| Blazes: AI, fireballs, drops | works | `mobs/nether.rs` (`BlazeAttackGoal`, volleys, rising). Test: a kill by the player dropped a rod (`killed_by_player`). | Nothing. |
| Spawners activate for guests | works (guests) | `alive_player_near` counts every non-spectator player (`game/natural_spawning.rs:273`), and natural spawning sees guests. | Nothing. |
| Piglins: bartering | works | Test: 4 gold ingots bought leather, obsidian and blackstone. Uses the vanilla `gameplay/piglin_bartering` table (`game/piglins.rs`). | Nothing. |
| Piglins: gold armour rule | partial | Test: a player without gold was killed by a piglin, which is correct. The AI is a goal-based approximation of `PiglinAi`, not the vanilla Brain (`mobs/piglin.rs`). | Nothing for this route, since it avoids piglins. |
| Endermen and pearls | works | `mobs/enderman.rs`: stare anger with the 0.025 cone, carved pumpkin, teleports. Test: a kill dropped a pearl (0–1 per kill). | Nothing. |
| Eyes of ender: crafting | works | `ender_eye.json` and `blaze_powder.json`. | Nothing. |
| Eyes of ender: throw | works | `game/end.rs:983`: the nearest stronghold ring chunk, `signalTo` (12 blocks horizontally, 8 up), survives 4 times in 5. Test: the eye dropped about 10 blocks away, on the bearing to the stronghold. | The companion reads the eye's flight (S8). |
| Strongholds: generation, portal room, silverfish spawner | works | Test: 12 frames at y −4, the room's spawner holding a silverfish, lava under the portal (`worldgen/structure/stronghold.rs:832-867`). | Nothing. |
| End portal frames, eyes, activation | works | Test: 12 eyes placed with `interact` produced 9 `end_portal` blocks, and the player arrived in the End. Guests do it through `as_guest`; not tested with a companion. | Test it with the companion in E2. |
| End: arrival platform | works | Test: arrived at (100.5, 49, 0.5) with 25 obsidian (`end.rs` `END_SPAWN`). | Nothing. |
| End: obsidian spikes, crystals, cages | works | `dragon.rs:542` places crystals on `spikes_for_seed`, and `rebuild_spike` places the bars. Test: 10 crystals. Vanilla: the caged spikes are sizes 1 and 2 (heights 79 and 82, radius 2). | Nothing. |
| Dragon AI phases | works for the local player | Test, sampled every 1.5 s: holding (0), strafe (1), landing approach (2). All eleven phases are ported (`dragon.rs:104-1320`), including the landing chance `1/(crystals+3)`. | — |
| Dragon targets guests | missing | The target, strafe, scanning, charge, wing contacts, breath and fireballs all read `self.player` (`dragon.rs:521, 949, 1559, 1751, 1809`). A companion is never targeted or hurt. | E1: the nearest valid player, guests included, through `lan_mobs::player_for`. |
| Dragon on server levels | missing | Test on the server thread: in the End, crystals but no dragon after 20 s. `tick_fight` returns when `self.player` is None (`dragon.rs:521`). Public bug, not companion-specific. | E1b: tick the fight from the level's players. |
| Crystal healing | works | `nearest_crystal`, +1 HP every 10 ticks, matching vanilla `checkCrystals`. | Nothing. |
| Crystal destruction | works | Arrows and fireballs (`creatures.rs:1858`), explosions (`dragon.rs:2330`) and melee (`attack_dragon`). Power-6 explosion. Breaking the healing crystal does 10 damage to the dragon, and breaking any crystal while it is holding makes it strafe. | Nothing. |
| Dragon breath and fireballs | works for the local player | `tick_breath` and `tick_dragon_fireballs` hurt `self.player` only. | Part of E1. |
| Damage to the dragon: melee and arrows | partial | Parts take a quarter plus one. **Arrows still hurt a perched dragon**, where vanilla's `AbstractDragonSittingPhase.onHurt` deflects them. **Invulnerability** drops any hit while `invulnerable > 10`, where vanilla lets the excess over the last hit through. | E3. |
| Damage to the dragon: explosions and beds | partial | Test: a bed in the End explodes (death by Intentional Game Design). Explosions hit dragon parts, but **without exposure** (vanilla's `seenPercent`), and only the strongest part counts. | E3: exposure, per-part hits. |
| Dragon death, exit portal, egg, return | works | Test: after `dragon-hurt` came the death flight, the egg at (0, 67, 0), 20 exit portal blocks, and the exit portal led to the Overworld spawn. | — |
| Credits (win screen) | missing | Leaving the End skips the win screen; `seenCredits` is always 0 (`save.rs:2615`). | E5, cosmetic. The run ends at the dragon's death. |
| Companion respawn | partial (for solo) | A dead companion respawns beside its owner after 10 s (`companion.rs`). A solo run should respawn it at its bed or the world spawn, as vanilla does. | E4. |
| `/tick sprint` | works | `clock.rs:344`: up to 40 ticks per frame (`SPRINT_PER_FRAME`). The hub ticks inside `Game::tick` (`game/lan.rs:964`), so the companion speeds up too. Test: 8,343 ticks/s in an empty world; 1,515 then 1,240 ticks/s at night with mobs and a companion. | E6: make sure a guest walking at 60× never paths into unloaded chunks. |
| Chunk generation under sprint | partial | Test: about 150–170 chunks generated in 0.5 s after each 1,000-block teleport (render distance 8, M4 Pro), so roughly 300 chunks/s. A companion sprinting at 60× crosses about 21 new chunk columns a second (about 360 chunks/s at render distance 8), so exploration is limited by generation at around 30–60×. | E6: stall the companion, not the clock, while its chunks generate. |
| Companion movement | partial | The vanilla PathFinder walk evaluator: 512 nodes, 3-block falls, swimming (`mobs/path.rs`). **It cannot break or place blocks while it paths**: no pillaring, bridging, tunnelling or stairs. | S2, the biggest skill. |
| Companion block search | partial | `BlockSearch` on the skills branch scans the palettes of loaded chunks. That is x-ray: allowed on All-knowing, not on Fair. | P: the perception interface. |

**What a guest does through a portal, in detail.**

- **In the window (one level at a time).** The level in place is the owner's.
  - A companion inside a portal gets no portal process.
  - When the owner's dimension changes, each companion within `COME_ALONG` (16 blocks) comes along:
    `tick_companion` sees `b.dim != self.dim` while `near_owner`, and places it beside the owner.
  - Every other companion is parked: saved to `players/data` and removed from the hub. It rejoins when the
    owner comes back to that dimension.
- **On the server thread or a dedicated server (`options.dedicated` is set; `integrated.rs:194`).**
  - Every guest has its own `PortalState`. `tick_server_travel` runs `handle_server_portal` for each one,
    and a `Trip` moves it to the destination level, opening that level if nobody is there
    (`server_travel.rs`, `levels.rs`).
  - So a guest there travels alone. A companion cannot be spawned there yet (`companions_supported`,
    `companion.rs:266`).
  - The End fight does not run on those levels (E1b).

### Engine work, estimated

| # | Work | Agent-days |
|---|---|---|
| E1 | The dragon sees every player: holding → strafe on the nearest player by `NEW_TARGET_TARGETING`, scanning, attacking, charge, wing knockback and head damage, breath and fireballs. These run on the guest through `as_guest`, as mob hits do (`lan_mobs.rs`). Crystal kills by any player. | 2 |
| E1b | The fight on server levels: spawn and tick the dragon from the level's players, not `self.player`. The exit portal, eye throws and End portal entry for network players. Public. | 1 |
| E2a | Watch mode's dimension link. The owner is a spectator following the companion. When the companion's portal wait completes (`PortalState`, 80 ticks), the owner's level changes and the companion arrives through its own portal, at the exit vanilla finds for it. | 1.5 |
| E2b | (Later.) Companions on the server thread: an owner is optional, the hub is the integrated server's, each companion travels on its own. Needs E1b. | 4–6 |
| E3 | Dragon fidelity: arrow and wind charge deflection while perched; `LivingEntity.hurt` invulnerability (damage above the last hit); `seenPercent` exposure and per-part explosion hits. | 1 |
| E4 | Solo respawn: at its bed or the world spawn (`findRespawnPositionAndUseSpawnBlock`), in the Overworld, as vanilla does. | 0.5 |
| E5 | Win screen and `seenCredits` (cosmetic). | 0.5 |
| E6 | Sprint safety: a guest waits at the edge of generated chunks, and the companion stalls while its chunks generate. Measure sprint ticks per second with exploration. | 1 |
|  | **Total for the in-window route (E1, E1b, E2a, E3, E4, E6)** | **≈ 7** |

## Perception: one interface, four levels

The route and the planner never read the world directly. They ask a `Perception` object:

```rust
trait Perception {
    /// Blocks of these kinds it knows of, nearest first (seen, or allowed by the level).
    fn known_blocks(&self, kinds: &BlockSet, around: BlockPos, radius: i32) -> Vec<BlockPos>;
    /// Whether it knows the state at `pos` (for path costs: unknown = assume stone).
    fn block(&self, pos: BlockPos) -> Option<BlockStateId>;
    /// Structures it knows of: (kind, position, how it knows: seen | located).
    fn structures(&self, kind: StructureKind) -> Vec<KnownStructure>;
    /// Entities in view (all levels: entities are only ever the visible ones).
    fn entities(&self) -> &[SeenEntity];
    /// Biome at a position it has been near (F3 shows the current one).
    fn biome(&self, pos: BlockPos) -> Option<BiomeId>;
    fn level(&self) -> PerceptionLevel;
}
```

| Level | Blocks | Structures | Entities |
|---|---|---|---|
| Fair | **Seen memory.** A block is seen when one of its faces touches air or liquid and a ray from the companion's eye reaches that face within render distance. The memory keeps it until the chunk unloads or the block changes in view. | Only once seen: nether bricks, stone bricks and the like. | In view (a ray to the bounding box, within render distance), plus sounds within 16 blocks. |
| Sharp senses | Fair, plus every block within 16 blocks, through walls. | As Fair. | As Fair. |
| Scout | As Fair. | Plus `locate` results for villages, fortresses, bastions, strongholds and ruined portals, through the same code as `/locate` (`ChunkGenerator.findNearestMapStructure`). | As Fair. |
| All-knowing | Every block in loaded chunks, plus generator queries for unloaded areas (`/locate biome`). | Scout's list, plus each structure's pieces (the portal room's bounding box). | As Fair. |

How Fair seeing is computed:

- **Seeing is a budget, not a scan.** Each tick casts about 1,500 rays across the 360° around the eye,
  rotating through directions, plus rays to each block the plan wants to know about.
- **Rays walk the section palettes.** Air and liquid pass, and the first solid face is recorded. This
  costs about as much as the visibility already in the mesher.
- **The cost goal is under 0.05 ms per tick.** Rays can run on the companion's worker thread from a
  snapshot.
- **A test proves Fair is fair.** On fixed seeds it checks that every block in the memory had a clear ray
  from some position the companion occupied. It also checks that All-knowing and Fair agree on every block
  Fair says it saw.

The Watch HUD and run summaries show the level, and the saved companion file records it.

## 2. The route

### Shape

The route is a deterministic state machine of **phases**, each made of **steps**. Each step has:

- a goal the planner can expand (for example "have 10 obsidian"), and a precondition check;
- a time budget;
- failure handlers.

A failure re-plans from the current inventory and world memory; it never goes back to a fixed point.

```
O1 wood ─ O2 stone kit ─ O3 food ─ O4 iron ─ O5 kit ─ O6 lava & portal ─┐
                                                                        │
┌─ N1 enter & anchor ─ N2 find fortress ─ N3 blaze rods ─ N4 pearls ─ N5 home portal
│
└─ S1 eyes ─ S2 triangulate ─ S3 travel ─ S4 dig & room ─ S5 fill frame ─ S6 enter
   ─ E1 reach island ─ E2 crystals ─ E3 kill dragon ─ done
```

Global rules run on every tick, ahead of the current step:

- Eat at 14 food or below, and at 17 or below before a fight.
- Retreat and heal (block in, eat) below 8 HP.
- Shelter or keep working underground at night on the surface.
- Never stand in a block next to lava without blocks to place.
- Keep at least 32 throwaway blocks: cobblestone, or netherrack in the Nether.
- Water-bucket a fall of 4 or more blocks when the bucket is free.

### Overworld

| Step | Goal and preconditions | Skills (✓ built, ◐ skills branch, ○ new) | Failure modes → recovery | Time (game) |
|---|---|---|---|---|
| O1 Wood | 1. Logs ×8–12 → planks, sticks, crafting table. 2. Wooden pickaxe. | collect ◐, craft ◐, go to ✓, perception ○ | No tree in view → explore toward green (leaves or log faces), spiralling out. Fair uses the seen memory; All-knowing goes to the nearest log. | 0.5–1.5 min |
| O2 Stone kit | Cobblestone ×20+ → stone pickaxe, axe, sword, shovel, furnace. 32 blocks kept. | mine ◐, craft ◐, staircase dig ○ | No exposed stone → dig down by staircase (never straight down). | 1 min |
| O3 Food | At least 6 cooked meat, or bread. Food above 14 throughout. | hunt ○ (combat ✓ basic), smelt ◐, eat ✓ | No animals in view → widen the search. Fall back on sweet berries, apples from oak leaves, bread from village wheat if a village is seen. | 1.5–3 min |
| O4 Iron | Raw iron ×10: pickaxe 3, two buckets 6, flint and steel 1. Optionally 12 for a shield or 14 for shears. Iron sword optional. | ore finder ○, cave walk ○, strip mine ○, smelt ◐ | No iron in view → explore exposed caves and ravines, then staircase to y 16 and strip mine (vanilla's iron peaks at y 16; there is also a mountain band). Scout: a village blacksmith chest, or a shipwreck or buried treasure if close. Sharp senses and All-knowing: the nearest ore. | 4–10 min |
| O5 Kit | Iron pickaxe, 2 buckets, flint and steel, shield; one bucket filled with water. Gravel ×~40 mined on the way for flint (arrows). | craft ◐, collect ◐ | Short on iron → back to O4. | 0.5 min |
| O6 Lava and portal | 10 obsidian in a 4×5 frame (no corners), lit. | lava finder ○, portal casting ○, place ✓ | No lava pool → keep looking: surface lakes are visible from far and glow; then caves; then the lava below y −54. A ruined portal in view: use its frame and chest (obsidian, flint and steel) and cast only the missing blocks. Casting fails (water spreads, lava flows) → rebuild the mould and retry. | 2–6 min |

**O6, the portal.** The candidates were:

- **Ruined portal:** needs luck and usually casting anyway.
- **Mining obsidian:** needs 3 diamonds, which is slow and luck-bound.
- **Lava casting with two buckets:** chosen.

Casting is deterministic and works wherever there is lava, which every seed has below y −54. The step:

1. Pick a pool with at least 10 still sources, a solid floor beside it, and no fall into the pool.
2. Build the frame's mould from cobblestone beside the pool: a 4-wide, 5-tall frame, 10 slots without
   corners.
3. For each slot: take lava with the empty bucket, pour it into the slot, pour water from the second
   bucket one block above and beside the slot, then pick the water back up. Each slot turns to obsidian
   in under a second, as the test showed.
4. Light the frame with flint and steel on the top face of the bottom block, as the test did, and record
   the portal's position.

The simpler speedrunner method of pouring water over the pool's edge works too, but a bot can't predict
where the water will spread as reliably as it can predict this mould.

Variants by level:

- Fair and Sharp senses find lava through the seen memory.
- Scout prefers a ruined portal within 300 blocks.
- All-knowing goes to the nearest lava with 10 or more sources.

### Nether

| Step | Goal and preconditions | Skills | Failure modes → recovery | Time |
|---|---|---|---|---|
| N1 Enter and anchor | In the Nether. Portal position recorded in both dimensions. Gold boots if it has gold (optional; piglins). | portal use (E2a) | Arriving in lava or a closed room → place blocks, dig out. | 0.1 min |
| N2 Find a fortress | A fortress in the memory, with a route to a spawner. | long-range navigation ○, Nether bridging ○, structure memory ○ | Lava sea → bridge with netherrack at least 2 blocks above it. Ghast → step back behind blocks, shield. Lost → return toward the anchor and pick a new heading. **Scout and All-knowing** skip the search and go to the located fortress. | Fair 3–8 min; Scout 1–3 min |
| N3 Blaze rods | At least 7 rods (14 powder; 12 eyes plus spares). | spawner camp ○, shield ○, melee crits ○ | **Fair heuristic:** nether brick faces in the seen memory give the fortress's bounding volume. Search its interior (open bridges, corridors) for a spawner: a block with its flames and particles, or a blaze in view. Blazes come in groups → fight from a 1-wide cover beside the spawner, block fireballs with the shield, hit only when a blaze is within reach, never chase into the open. Low HP → retreat into cover and eat. Spawner out of reach → place blocks. | 4–7 min |
| N4 Pearls | At least 13 pearls (12 eyes plus spare), minus eyes already in the frames (unknown until S5, so assume none). | biome finder ○, enderman fighting ○ | **Endermen in a warped forest.** Find it by its blue fog and warped nylium (Fair) or with `/locate biome` (All-knowing). Fight under a 2-block ceiling the companion builds (endermen are 2.9 tall): aggro them by looking at them (the stare cone), hit when they come. No warped forest within 400 blocks → soul sand valley, or Overworld nights on the way to the stronghold. Too few after 10 minutes → carry on and also loot the stronghold's corridor chests (`stronghold_corridor` has an `ender_pearl` entry). | 5–9 min |
| N5 Home | Back through the first portal (it remembered it). | navigation ○ | Portal broken by a ghast → relight it with flint and steel. | 1–2 min |

**Why not bartering.** In 26.3 the barter table gives 2–4 pearls with weight 10 out of 469, about 0.064
pearls per gold ingot. Thirteen pearls would need about 200 ingots. Endermen drop 0–1 pearls (0.5 on
average), so about 26 kills. A warped forest has dense enderman spawns and little else hostile. A bot with
an iron sword (6 damage, 9 with a crit, against 40 HP) needs about 5 hits a kill. Bartering stays a
fallback for teams with gold.

**Why blazes before pearls.** The fortress is the uncertain search. Pearls work in any warped forest,
soul sand valley or Overworld night, so they come second, wherever the route is by then.

### Stronghold

| Step | Goal and preconditions | Skills | Failure modes → recovery | Time |
|---|---|---|---|---|
| S1 Eyes | Blaze powder, then 12+ eyes. | craft ◐ | Short of rods or pearls → back to N3 or N4, or spend the spares. | 0.2 min |
| S2 Triangulate | The stronghold's start chunk within ±16 blocks. | eye reading ○, triangulation ○ | An eye breaks (1 in 5) → throw another. Two rays almost parallel → walk further perpendicular. Inside the first ring (1,280–2,816 blocks from the origin), the first ray already narrows it down. **Scout and All-knowing:** skip it; one throw confirms. | 1–2 min |
| S3 Travel | Within 16 blocks of the target in the Overworld. | long-range navigation ○ | Night, mobs → global rules. Water → boat (optional) or swim. | 3–8 min (about 1,500 blocks at sprint, with stops) |
| S4 Dig and find the room | Standing in the portal room. | staircase dig ○, stronghold search ○ | Silverfish (infested stone) → don't break unknown stone bricks; kill silverfish. The room is not in the start chunk → walk the corridors breadth-first, preferring unexplored ends. The room is found by exposed end portal frames, the spawner, or the lava under the frame. All-knowing walks straight to the room's piece. | 2–5 min |
| S5 Fill the frame | 12 frames with eyes. Portal open. | interact ✓ | The spawner is active (silverfish) → break it with the pickaxe first (it drops nothing, so the room is safe). Not enough eyes → only possible if earlier steps fell short; go back to N3/N4 through the portal. | 0.3 min |
| S6 Enter | In the End. | portal use (E2a) | — | — |

**S2, triangulation.** The bot needs one thing from each throw: the direction the eye flies. Vanilla's
`EyeOfEnder.signalTo` aims at `start + 12·d̂` (plus 8 up), where `d̂` is the horizontal unit vector to the
stronghold's start chunk corner (`getLocatePos`). Horizon does the same (`end.rs:983`). So the eye's
horizontal position always lies on the ray from the throw point A along `d̂`, and two samples give the
bearing:

```
u = normalize(eye_xz(t) − A_xz)     for any tick t ≥ 2 of the flight
```

A player reads the same bearing on screen, as speedrunners do with F3+C. The bot reads entity positions at
the precision of the tracking packets (1/4096 block). That is fair: no seed or structure data, only what it
sees.

From throw points A and B with bearings u and v, using `×` for the 2-D cross product
(`p × q = pₓq_z − p_zqₓ`):

```
t = ((B − A) × v) / (u × v)        target ≈ A + t·u
```

- **Rejecting a bad pair.** If |u × v| < sin 3° the rays are nearly parallel: walk further perpendicular
  and throw again.
- **Baseline.** At 1,500 blocks with a 150-block baseline perpendicular to u, the angle between the rays is
  about 5.7°. Bearing error from packet precision is under 0.01°, so the target error is about 1–3 blocks.
- **Wasted travel.** Walking 150 blocks sideways costs about 30 s. Doing it on the way, at an angle, wastes
  less.
- **First throw where the route is.** The first throw happens next to the home portal. If |A| < 1,280 the
  bot travels along u before the second throw, since the target is at least 1,280 − |A| blocks out along it.
- **Recovering eyes.** Each eye drops after 80 ticks (4 in 5) about 12 blocks away. The bot picks it up and
  reuses it. Three throws cost about 0.6 eyes on average.

### The End

| Step | Goal and preconditions | Skills | Failure modes → recovery | Time |
|---|---|---|---|---|
| E1 Reach the island | On the main island's end stone, 20–40 blocks from the fountain. 64 or more blocks in hand. | bridge and pillar ○ | The arrival platform (100, 48, 0) is often below or beside the island: pillar up, or bridge toward (0, 0). Endermen: don't look at them (the bot aims its view away from enderman heads). | 0.5–1 min |
| E2 Crystals | 0 crystals, or only caged ones left in plan B. | bow ballistics ◐/○, pillar ○, cage break ○ | Arrow misses → re-aim (the crystal hitbox is 2×2; aim at its centre with the arrow's arc). Dragon strafing → shield toward its fireballs; move. Out of arrows → craft (flint, stick, feather), or pillar up and hit the crystal from below with a block between (plan C). | 3–6 min |
| E3 Kill the dragon | `fight.killed`. | perch fighting ○, breath dodge ○, knockback safety ○ | **v1, sword at perch.** Wait in a safe spot 6–10 blocks from the fountain, away from the edge. When it perches (landing, then sitting), circle to the side of the head, out of the breath cloud, and land crit hits on the head (full damage; the other parts take ¼ + 1). Vanilla takes off after 50 sitting damage, so about 4 perches. Knocked up by its wings → water bucket, or fall onto blocks; in the void → death (E4 respawn, back through the portal). **v2, beds (later).** On the first perch, place a bed under the lowered head with a block between bed and bot, and right-click it. Repeat with 4–5 beds about 0.5 s apart. This needs E3's exposure rule and 15 wool from sheep (shears or kills) in O3. | v1 3–6 min; v2 1–3 min |

**E2, crystals.**

- **The 8 open crystals:** bow from the ground.
  - The bot computes the arc exactly: an arrow at full charge starts at 3.0 blocks per tick, falls 0.05
    per tick and keeps 99% of its speed each tick.
  - The crystals sit on spikes of height 76–103, 42 blocks from the centre, so 15–40 blocks up from 15–45
    blocks away.
  - About 12–16 arrows. The bot gathers about 20 on the way: skeleton drops at night, or crafted from
    gravel flint, sticks and chicken feathers (4 arrows a craft).
- **The 2 caged crystals** (spike sizes 1 and 2, heights 79 and 82, radius 2). The cage is a 5×5 ring of
  bars with a roof. Plan C:
  1. Pillar up beside the spike to the roof plus 1.
  2. Break one roof bar above the crystal.
  3. Bridge 12 blocks away at that height and shoot down through the hole. At 12 blocks a power-6
     explosion does under 3 damage.
- **Plan B (a fallback, and v2's choice):** leave the caged crystals.
  - The dragon heals only while it is within 32 blocks of a living crystal, at +1 HP every 10 ticks. It
    does not heal while perched at the fountain, since the spikes are 42 blocks out.
  - So a one-cycle (v2) ignores every crystal: it waits longer for the first perch (landing chance
    1/(crystals+3) per holding path) but avoids the most dangerous part.

**Why this order for the first kill.** Bow plus sword at the perch is slow but forgiving: each mistake costs
time, not the run. A bed one-cycle is fast but needs:

- placement and explosion timing;
- explosion damage control;
- 15 wool.

For a bot with exact geometry it is very achievable, and it is the second milestone: it brings the End from
about 6–12 minutes down to 2–4.

### Variants by perception level

| Step | Fair | Sharp senses | Scout | All-knowing |
|---|---|---|---|---|
| O4 iron | Caves, ravines, strip mine at y 16 | Nearest ore within 16 blocks while strip mining (much faster) | Fair, plus a blacksmith chest or buried treasure if close | Nearest ore in loaded chunks |
| O6 lava | Seen pools, then caves, then y −54 | Plus lava behind walls | Ruined portal under 300 blocks, else Fair | Nearest pool with 10 or more sources |
| N2 fortress | Heading and search with nether brick cues | As Fair | Located | Located, path to the spawner piece |
| N4 pearls | Biome cues (fog, nylium) | As Fair | As Fair (biomes are not structures) | `/locate biome warped_forest` |
| S2 stronghold | Two-throw triangulation | As Fair | Located (one confirming throw) | Located |
| S4 portal room | Corridor search | Frames and spawner within 16 blocks | Corridor search from the located start | Straight to the room's piece |

### Expected times (game time, v1)

| Phase | Fair | All-knowing |
|---|---|---|
| Overworld (O1–O6) | 10–22 min | 7–12 min |
| Nether (N1–N5) | 13–26 min | 8–14 min |
| Stronghold (S1–S6) | 7–17 min | 5–10 min |
| End (E1–E3) | 7–13 min | 7–13 min |
| **Total** | **≈ 40–75 min** | **≈ 30–50 min** |

At 30–60 times real time under `/tick sprint`, a Fair run is about 1–2.5 minutes of wall time on the
Mac mini, while generation keeps up (E6). For scale, a good human random-seed runner takes 15–30 minutes.
With v2's beds, more efficient navigation and tuning, a Fair bot could reach about 25–35 minutes.

### Risks

1. **Movement is the long pole.** Most failures will be:
   - getting stuck: two-high gaps, water currents, leaves, powder snow;
   - falls, and lava in caves and the Nether.

   A path search that can dig, place and estimate unknown blocks (S2) needs heavy harness time.
2. **Combat deaths.** Creepers in caves, skeletons at range, blaze groups, ghasts on bridges. Vanilla
   survival rules (a shield, retreat, eating) cover most of it. Each death costs the inventory unless it is
   recovered: the recovery step goes back to the drop within 5 minutes of game time, the item despawn time.
3. **Fair search can run long.** No fortress within 500 blocks of the portal, or iron-poor terrain. The
   per-step time budgets end the run as a classified failure ("no fortress in 12 min") rather than looping,
   so the harness shows where Fair loses.
4. **The dragon's AI only fights the companion after E1.** Without E1 the fight is too easy and unfair.
   Harness numbers before E1 don't count.
5. **Watch mode couples the owner to the companion.** If the owner leaves Watch mode in another dimension,
   the in-window hub parks the companion. E2b removes the coupling.
6. **Sprint and generation (E6).** At 60× the companion can outrun chunk generation. Untreated, its paths
   fail at the edge of loaded terrain and it looks stuck.
7. **Hunger and time budgets drift with sprint:** none expected, since everything is in game ticks. But
   the LLM brain must not be called per tick under sprint. The Speedrunner runs with the brain muted, and it
   narrates afterwards from the split log.
8. **Piglin AI is an approximation.** The route avoids piglins (no gold, routes around bastions). Any
   piglin anger that differs from vanilla is a fairness risk, not a blocker.

## 3. What to build, in order

| # | Item | Needs from the engine | Agent-days |
|---|---|---|---|
| 1 | **Perception** (interface, All-knowing first, then Fair seen memory, Sharp senses, Scout, the fairness audit test) | Section palettes; ray walk; `/locate` code | 3 (All-knowing alone: 1) |
| 2 | **Tech-tree planner**: goal regression over recipes, block and mob loot tables (the skills branch's `work::loot_items`), smelting, tool tiers, counts. Unit-tested against fake worlds. | Recipe and loot data (present) | 3 |
| 3 | **Movement 2.0**: a path search with dig, place, pillar, bridge, staircase and swim moves, block costs from perception (unknown blocks cost stone), lava, void and fall safety, long-range waypoints over chunk-level costs, Nether bridging. | E6 | 8 |
| 4 | **Survival loop**: hunt and cook, eat thresholds, retreat and block-in, night policy, armour and tool durability. | — | 2 |
| 5 | **Combat 2.0**: shield blocking, crit timing, keeping range from skeletons, creeper retreat, bow ballistics against mobs, endermen under a 2-high ceiling, blaze cover. | — | 3 |
| 6 | **Resource strategies**: trees, stone, iron (caves, strip mining), gravel, sheep and chickens. | 1, 3 | 3 |
| 7 | **Portal casting**: lava finder, mould, two-bucket cast, light, record. | Buckets, fluids (present) | 2.5 |
| 8 | **Watch mode** (dimension link E2a, spectator camera, HUD: step, timer, perception level, splits). | E2a | 1.5 + E2a |
| 9 | **Nether**: fortress search, spawner camp, warped forest pearls, home. | E1b not needed in-window | 4 |
| 10 | **Stronghold**: eye reading, triangulation, travel, staircase dig, corridor search, frame fill. | — | 3 |
| 11 | **End**: island approach, crystal sniping, caged crystals, perch fighting, knockback safety. Beds as v2. | E1, E3 | 4 (+2 for v2) |
| 12 | **Route machine and recovery**: phases, budgets, death recovery, re-plan, split log, narration hooks. | E4 | 2.5 |
| 13 | **Harness** (below) | `/tick sprint`, events | 2 |

Engine items E1, E1b, E2a, E3, E4 and E6 come first or alongside items 1–3: about 7 agent-days.

### The harness

`tools/test_speedrun.py`, private repo only, like `tools/test_companion.py`:

- **Launch.** `horizon-client --offscreen --render-distance 8`, `HORIZON_NO_AUDIO=1`, a temporary
  `HORIZON_HOME`, the control socket.
- **World.** A new world per seed, default game rules, difficulty Easy at first (Normal later, reported
  separately).
- **The run.**
  1. The owner enters Watch mode.
  2. `/companion spawn Runner`, role Speedrunner, perception level from `--perception`.
  3. The harness then runs `/tick sprint 72000` in slices and polls `companion reports`, `status.fight` and
     `events` between slices. Slices are 6,000 ticks, so a stuck run is noticed within one.
- **Milestones**, as game ticks: logs, wooden pickaxe, stone pickaxe, food stocked, 10 iron, bucket,
  10 obsidian, portal lit, Nether, fortress seen, 7 rods, 13 pearls, back in the Overworld, 12 eyes,
  stronghold located (within 32 blocks of the truth, checked with `/locate` after the fact), portal room,
  frame filled, End, crystals cleared, dragon killed. The brief's survival and diamond milestones are added
  for the other roles.
- **Limits.** 72,000 game ticks (one hour of game time). Wall-time cap of 15 minutes.
- **Metrics per run:**
  - success, end tick, and the tick of each split;
  - deaths, minimum HP, blocks placed and broken;
  - path failures, re-plans;
  - the failure class: the step and the reason from its budget or recovery;
  - sprint ticks per second, peak RSS.
- **Seeds:**
  - 10 fixed seeds chosen for variety (plains, forest, desert, ocean, mountain and taiga spawns; some with
    a lava pool or ruined portal near spawn, some without);
  - 10 more drawn per run of the full suite, recorded.

  The fixed seeds are compared over time; the fresh ones are the honest success rate.
- **Report.** A per-level table: success rate, median and p90 time, the failure classes in order, and
  per-split medians. Written to `.local/speedrun/<date>.json` and the Markdown table to the PR or branch.
- **Running.**
  - **The Mac mini:** 4 seeds in parallel (about 3 GB each, within `$FLEET_LOAD_MEM_MB`).
  - **The ThinkPad:** 1 seed at `nice 19`.
  - **Never the desktop** while Ludovic is at it.
  - **Cost.** A 10-seed Fair suite should take about 10–15 minutes of wall time on the Mac mini once E6
    holds.
- **Development segments.** `--from <step>` starts from a saved world at the end of a step, for example
  "in the Nether with the kit", built by an earlier run or by commands. This is for debugging only. Results
  from segments are never reported as runs.

## 4. Recommendation

**The fastest path to a first unassisted dragon kill.**

1. **Engine first:** E1, E2a, E3, E4, E6 (about 6 agent-days). E1b in parallel, published as the public
   bug fix it is (the dragon on servers).
2. **Build the skills against All-knowing:** perception's All-knowing level, the planner, Movement 2.0,
   survival, combat, resources, casting, Nether, stronghold (All-knowing variant), End v1, the route
   machine, the harness.
3. **Pick one seed** whose route is short (lava near spawn, a fortress within 300 blocks, the stronghold in
   the first ring) and iterate on the harness until it kills the dragon.

This comes to about **30–36 agent-days**, with 3 or 4 agents in parallel after the first week: about
2.5 weeks of calendar time. Movement and the End are the uncertain parts.

**Then at least 50% on Fair over 10 seeds:**

- the Fair seen memory, fortress search, triangulation and stronghold corridor search (about 6
  agent-days);
- then harness-driven fixing of failure classes (about 15–25 agent-days).

That is about **55–65 agent-days in total**. The bed one-cycle (v2) adds about 2 agent-days and makes the
run faster, not more reliable.

**Options to decide.** These are Ludovic's calls; nothing here decides fairness for him.

| Option | What it saves | What it costs |
|---|---|---|
| A. Reveal on All-knowing (shown on the HUD) first, Fair later | About 6 agent-days and much of the Fair tuning | The "it sees what you see" claim waits |
| B. Reveal on Scout (structures known, sight fair) | About 4 agent-days (fortress search, triangulation, room search) | Smaller than A; Scout is a real setting players can pick |
| C. A curated seed for the reveal recording, plus the harness table on random seeds | Nothing to build; avoids a bad-luck recording | The recording is not a random seed (the table says so) |
| D. Easy difficulty for the first numbers | Fewer combat deaths | Normal must be reported before claiming "beats Minecraft" |
| E. Skip the caged crystals (plan B) in v1 | About 1 agent-day of pillar and cage work | A slower dragon kill (healing between perches) |
| F. Defer E2b (server-thread companions) | 4–6 agent-days | Watch mode is the only way to run solo until then |
| G. Defer credits (E5) and the Watch HUD polish | About 1.5 agent-days | The reveal ends on the exit portal, not the poem |

My suggestion, if useful:

- Build for Fair from the start, since the perception interface makes the level a switch and costs
  little.
- Get the first kill on All-knowing (A) on a curated seed (C).
- Show the harness table per level, including Fair, at the reveal, whatever it is by then.

## 5. Phase 1 as built (overworld, up to a lit portal)

Branch `feat/companion-planner`. What exists, how it fits, what the harness measured, and what
still fails.

### The pieces

- **Perception** (`companion/perception.rs`, `game/companion_senses.rs`): `Senses` holds the
  level (Fair, Sharp, Scout, All-knowing) and a `Memory` of what it has seen. Sight casts 64 rays a
  tick from the eyes. Notable blocks (ores, logs, water and lava sources, crafting stations) are
  kept in their own map. `companion_known` merges memory with a through-walls `BlockSearch` for the
  levels that have x-ray. Structures (ruined portals and so on) are known only to Scout and
  All-knowing. Memory is saved with the companion.
- **Movement 2.0** (`companion/moves.rs`, `game/companion_walk.rs`): A* over cells (Open, Water,
  Lava, Hazard, Solid with dig ticks and falling, Unknown). Moves are walk, up, fall, down, dig down,
  bridge, pillar and swim. Each search is capped at 5000 nodes, and long trips go in 40-block legs.
  A search is resumable (`moves::Search`): it runs 1.5 ms per tick and goes on the next tick, so a
  plan never holds up the game thread. Cells are cached per block state and per position within
  one search. On Fair, solid blocks it never saw and doesn't touch are Unknown (planned as stone).
- **Walker**:
  - **Steps**: patience scales with the blocks a step digs. It goes back to the middle of its row
    while still moving forward. When off the route, it replans from where it is. In water, it swims
    up toward the step.
  - **Bad blocks**: blocks it can't break or stand on become walls for the next plans, for a
    minute (a floor gets built, lava goes). Cells it means to stand in are never bad, and a stand
    holding a block is dug out.
  - **Falls**: a first step that drops two or three blocks counts as on the route.
  - **Cliffs**: where its way ends or turns at a drop of more than 4, it sneaks and doesn't
    sprint.
  - **Blocks it keeps**: it never digs the portal mould's blocks on its way.
  - **Planning rate**: one plan every 5 ticks at most (20 after a plan that found nothing).
  - **Cost of water**: a move fully under water costs twelve walks, a surface swim three, so it
    goes around or bridges a lake rather than swim under it. It doesn't dig while its head is
    under water off a floor; with a floor, digging there costs 25 times more, and digging a cell
    beside water costs 60 more. It swims up into air only where a side step leads out, and treads
    water (holds jump) while it plans or digs.
- **Tech-tree planner** (`companion/techtree.rs`, `GameKnowledge`): turns stage goals into mine,
  hunt, craft and smelt actions. Gather cost comes from the remembered distance. The x-ray levels
  also use flat guesses for natural blocks. Actions run as work commands (`collect`, `make`,
  `smelt`).
- **Run supervisor** (`game/companion_run.rs`, every tick):
  - **Stages**: wood, stone tools, food, iron, kit, portal, armour. Each stage has a time budget;
    food and armour are optional.
  - **Armour** (after the portal is lit, before it goes through): an iron helmet, chestplate and
    leggings and a shield (21 more iron, 3 coal mined by the ore for the smelt), then gold boots
    for piglins, given 5 minutes after the iron set. The body wears the best armour it carries.
    It is made once: lost in a death, it isn't made again. Building it before the cast instead
    cost the cast its time (0/5 lit in two batches).
  - **Splits**: logged when reached, and shown on the Watch HUD (step, timer, perception).
  - **Air**: it goes up for air before 140 ticks under water. Its eyes are low when it swims under a
    ceiling. It stays up until its air is back, and heads for the nearest air to the side when the
    way up is blocked.
  - **Night**: under open sky it digs a capped pit two blocks deep. It stays in the pit while mobs
    gather above. From the iron stage on, carrying a sword with 8 health or more, it works through
    the night; in a pit, it comes out once healed (18). The kit includes a stone sword when it has
    none.
  - **Threats**: it backs off from creepers (5 blocks) and pillagers (20 blocks); a creeper it has
    backed off from for 30 s is ignored.
  - **Damage causes**: the body's hurt sound names the cause (drowning, fire, freezing, berry
    bush). A plain hurt goes to the nearest mob in reach (4.5 blocks, 16 for ranged kinds), else it
    is a fall or a wall. Drowning was reported as "hurt by squid/fish": that was the companion's
    own attribution (nearest mob within 16 blocks), not the game's death message, and is fixed.
  - **Places it died**: kept as dangers; leads and pools within 12 blocks of one are skipped.
  - **Death recovery**: it goes back for its things for up to 2 minutes, unless it died there
    the last time too.
  - **Hunger**: hungry (10 or less) with nothing to eat, it hunts first whatever the stage. A
    runner below 18 health eats to heal when nothing is close. The food stage cooks the meat as soon
    as it has hunted (furnace, fuel it carries); the iron stage tops its logs up to 4 under open
    sky so the kit's furnace has fuel. A smelt loads only what its fuel can cook, takes another
    kind of fuel out of the slot to load what it carries, and may go 6000 ticks without an item
    changing (21 iron cook for 4200).
  - **Exploring**: legs run nearly straight. It follows leads from memory first: for wood, the
    nearest tree it saw (a desert spawn walks to the tree line); for iron on Fair, iron ore it
    saw however far (not ore under water), then cave and ravine floors seen well below it (the
    nearest dry ones, not lake beds). Wood leads and wood targets are logs in a tree (open or
    leaves above), not the logs of a mineshaft or a house. Mine legs go down in short staircase
    steps (6 across, 6 down); a surface search under a roof goes up first. The goal it looks for
    is the planner's missing item, not a crafted one it can make from it.
  - **Watchdogs**:
    - work that changes no item for 3000 ticks;
    - the route's own moves going nowhere for 3000 ticks;
    - a smelt is let finish rather than cut off by the stage budget;
    - after a stalled smelt, the meat stays raw;
    - a craft that hasn't finished in 1200 ticks fails, and the table it was stuck at is not
      used again;
    - an exploring leg (or lead) not reached in 1500 ticks: another heading (one walked to and
      fro in a tunnel for 35,000 ticks);
    - stalled twice within 4 blocks, it walks 12 blocks away and 4 up ("getting unstuck");
    - a task that keeps failing waits 30 s before it is planned again.
- **Portal cast**: lava and water are found in memory, through walls on Sharp and up, or (lava)
  near ruined portals on Scout and up.
  - **Which pool**: by cost, not distance: distance plus twice the blocks down and three times
    the blocks up, plus twice the solid blocks over it, less a bonus for size. Only sources on top
    (open air above) count. A pool not come 2 blocks nearer in 75 s, or not reached in 4 minutes,
    is given up and the next one tried; the chosen pool stays chosen while it is there. With no
    pool known, All-knowing digs down to y -40 soon, Fair to y -50 after a few legs.
  - **Site and mould**: the site is two blocks from a pool. It builds a mould of floors, a back
    wall and three corners, then fills the ten frame blocks one at a time.
  - **Each frame block**: water first, lava poured on the back wall's face, water beside or
    above it, then the water taken back. A pour that misses is scooped back up.
  - **Finding water and lava**: it digs a line of sight to buried sources and takes lava from the
    pool's near edge first. Sources and pools it can't reach are given up after 30 s, and a frame
    block that won't cast in 5 minutes moves it to another pool. Water left in the frame is taken
    back from the ground before it; water it can't reach is left there (lava poured beside it
    sets), never buried under a block. A water source it can't fill a bucket from in 2 minutes
    is given up. Once a frame block is cast, the site is kept: a new pool doesn't move it.
  - **Lighting**: before the flint and steel, it drains the inside of the frame and the cells in
    front of it, giving up a source it can't take in 30 s.
  - **After a death**: it keeps the site and the blocks cast, and looks for a pool again.

### Harness results

`tools/test_speedrun.py`: 5 fixed seeds (20261020, 1, 42, 123456, 987654321), Easy, 48,000 game
ticks (40 game minutes) per run, `/tick sprint`, Watch mode, on the Mac mini (3 or 5 clients in
parallel, fewer when other work loaded the machine). Times are game min:s from the start of the
run; success counts the seeds that reached the milestone. The last two batches of round 3: r8b
is the code of this commit, r7y the same but for the last two fixes (the portal as a wall, the
water scan only when needed):

| Milestone | All-knowing r8b | r7y | Fair r8b | r7y |
| --- | --- | --- | --- | --- |
| wooden pickaxe | 5/5 0:32 | 5/5 0:17 | 5/5 0:40 | 5/5 0:14 |
| stone tools | 5/5 1:03 | 5/5 0:47 | 5/5 1:12 | 5/5 0:51 |
| food (4 cooked) | 4/5 3:15 | 5/5 3:46 | 3/5 2:53 | 2/5 2:53 |
| 10 iron | 4/5 5:19 | 5/5 5:19 | 5/5 13:38 | 4/5 16:12 |
| iron pickaxe | 4/5 11:19 | 5/5 9:32 | 3/5 23:14 | 2/5 9:45 |
| buckets | 4/5 11:35 | 5/5 10:13 | 4/5 24:46 | 3/5 10:53 |
| 10 obsidian | 2/5 20:31 | 3/5 14:54 | 0/5 | 0/5 |
| portal lit | 1/5 22:21 | 2/5 12:22 | 0/5 | 0/5 |
| iron armour + shield | 0/5 | 2/5 24:31 | 0/5 | 0/5 |
| gold boots | 0/5 | 0/5 | 0/5 | 0/5 |
| first night survived | 4/5 19:06 | 4/5 19:08 | 4/5 19:06 | 2/5 29:08 |

(median game min:s of the seeds that reached it.)

Runs are not deterministic (mobs, drops, timing): the same code gives different counts from one
batch to the next. Portal lit, over the round 3 batches (5 seeds each):

| Batches | change | All-knowing | Fair |
| --- | --- | --- | --- |
| r7e-r7h | pools by cost, give-ups | 1/5, 0/5, 2/5 | 0/5, 1/5, 0/5 |
| r7k-r7m | water, lack choice, night | 2/5, 3/5 | 0/5, 0/5 |
| r7n-r7r | fuel, sticky frame, drain | 1/5, 2/5, 1/5, 2/5, 0/5 | 0/5 each |
| r7s | sticky pool | 1/5 | 0/5 |
| r7t-r7v | armour before the cast | 1/5, 0/5, 0/5 | 0/5 each |
| r7w-r7y | armour after the portal | 2/5, 2/5, 2/5 | 0/5 each |
| r8a | water scan only when needed | 2/5 (one lost in the Nether) | 0/5 |
| r8b | the portal is a wall to the walker | 1/5 | 0/5 |

The 3/5 All-knowing target was reached once (r7m), the 1/5 Fair target once (r7g, seed 1). From r7w
to r8a All-knowing lit 2/5 in every batch; r8b lit 1/5 (a second seed had 10 obsidian at 20 minutes
and ran out of time). In r8a one seed lit its portal at 10:13, then walked into it fetching logs for
the armour and died in the Nether, losing the run: portal blocks are now walls to the walker. Fair
reaches iron on 3-5 seeds of 5 and buckets on 2-4, and has not cast a frame since r7g.

**Armour's time cost.** The iron set and shield take 21 more iron, 3 coal and four crafts: from 8:01
to 13:44 after the portal is lit (6 runs: 8:01, 10:33, 12:35, 12:48, 13:42, 13:44; median 12:41).
Made before the cast (r7t-r7v), it took 10-11 minutes after the buckets, and the cast then ran out
of time (0/5 lit in two of those three batches): it is made after the portal is lit, before it goes
through. Gold boots were never made: no gold ore within 48 blocks of where the iron set was
finished, and the 5 minutes ran out on the way down to y -16. On Fair the iron set was made once
(r7v, before the cast, 15:36 after the buckets).

On a prepared platform, `tools/test_companion_portal.py` (kit given, a 5x5 lava pool, one water
source) casts all ten blocks and lights the portal (round 2: 9 of 10 runs, 1,180-2,410 ticks; round
3, with a sword in the kit: 7 of 7 on the final code and the two before it, 1,600-1,800 ticks on
r8a). A pool list cached for 20 ticks broke it (2 of 3 runs gave the pool up after 6 blocks) and was
dropped.

### Hard failures and their causes

- **Deaths in the portal stage lose the kit.** On the seeds that didn't light, the run died
  under ground by a pool (drowning, lava, zombies at night), went back for its things too late,
  and spent the rest of the run making buckets and flint and steel again (flint once took 7,000
  ticks). Seed 42 died 3-11 times a run.
- **Getting to lava.** Pools are chosen by path cost and given up after 75 s without coming
  nearer, but deep pools (y -40 to -56) still take minutes, and a mould is sometimes built
  twice. The cast itself works once at a pool.
- **Fair iron and lava.** Fair finds iron by caves it has seen and y 16 tunnels, on 3-5 seeds of
  5, late (median 16 minutes). Lava without x-ray means going down to y -50 and opening caves;
  the one Fair portal (r7g) came from a surface pool.
- **Desert (seed 123456).** Some runs find no tree within reach for many minutes ("no poplar log
  within 48 blocks"); others walk to the tree line and light the portal (r7s, r7w).
- **Gold.** "no raw gold within 48 blocks": gold boots need a gold lead (Fair) or a deeper
  search (x-ray reaches 48 blocks), and more than 5 minutes.
- **Fixed this round**: drowning reported as "hurt by squid/fish" (the companion's own guess,
  the nearest mob within 16 blocks; the body's hurt sound now names drowning); the Fair "looking
  for stripped acacia log" when raw iron was missing (the lack choice); the zombie horde at the
  spawn that killed a respawned runner up to 10 times a night (it digs in when unarmed); a
  furnace holding another kind of fuel ("no fuel" with fuel in the pack); a stuck crafting table
  used again and again; an exploring leg walked to and fro for 35,000 ticks.

### Tick cost and spikes

Round 3 adds a cost: the portal stage's pool choice scans its memory for lava every tick, 2.2-2.5 ms
of CPU on some ticks (919 cast sections over 2 ms of CPU in the 10 runs of the r8b batch, 6.5 ms at
most). Spreading that scan out (cached safely, or resumable like the block searches) is the next
performance fix. Round 2's measurement:

`companion cost` over whole runs, one client alone (24,000 ticks, seed 1, other agents' work still
running on the machine). Wall time includes preemption; CPU time is the companion thread's own:

| | All-knowing | Fair |
| --- | --- | --- |
| CPU p99 (ms) | 0.35 | 0.48 |
| CPU p99.9 (ms) | 1.77 | 0.73 |
| CPU max (ms) | 2.05 | 1.47 |
| ticks over 2 ms CPU | 2 of 24,107 | 0 of 24,145 |
| wall p99 / p99.9 (ms) | 0.39 / 3.04 | 0.51 / 2.77 |
| wall max (ms) | 9.9 | 67 |

Every section over 2 ms of wall time used under 1.3 ms of CPU: the wall peaks (the 67 ms one is a
sight section with 0.4 ms of CPU) are the thread being descheduled, not companion work. Round 1's
26-107 ms ticks had these causes, now fixed:

1. **Advancements.** Each award scanned all 1,739 `recipe_unlocked` criteria, and each inventory
   change tested every `inventory_changed` criterion. Both are now indexed (by recipe id and by
   plain item id) and the indexes are built when the tree loads, on the loader thread.
2. **Planner state.** The tech planner cloned its stock map and action list on every node; it now
   uses `Rc<str>` keys and a shared linked list of actions.
3. **Walk plans.** The A\* search is resumable and runs in 1.5 ms slices, one per tick, with the
   plan applied when it finishes.
4. **Block searches** are spread over ticks (`BlockSearch`, one section per tick) and stop at a
   cap of finds; the harvest checks and the memory scan are memoized.

### Next, toward the Nether and the End

1. **Keep the kit through deaths**: go back for its things at once (it now waits out the
   night), and never take a pool route through deep water or past lava edges at night.
2. **Shorter trips to lava**: cast at surface pools and ruined portals first (Scout and up),
   and make the mould where the lava is with the cobble it carries.
3. **Fair**: a cave finder that follows open air down from cave mouths and ravines, then lava
   by caves below y 0; village chests for iron and food.
4. **Armour**: smelt in two furnaces at once, and look for gold while tunnelling for iron (it
   is often in the same caves).
5. **Merge the Nether and End branches**: done in round 4 (section 6), with the armour fix;
   `tools/test_speedrun.py --route` follows the run into the Nether and the End.

## 6. Round 4: one route (`feat/companion-route`)

The three segments are one branch: this one (f2dfd7ce), `feat/companion-end` @ 6bb5c9ec and
`feat/companion-nether` @ 9b43f2ad, then dce981dc. The Nether's engine fixes are kept (guest armour
e0549f88, explosions 0782a598, fireball fire 5b7235c2) and the End's own armour and explosion
workarounds dropped. Chaining: the Overworld run's Done starts the Nether run (Enter → Fill), and a
Nether run that ends "done" starts the End once. While the Overworld run is on, portal blocks are
walls to the walker; the Nether's Enter leg has 6,000 ticks (it walks back from where it made the
armour).

Overworld changes in this round:

- **Night**: in the portal stage it digs in for the night unless it wears two pieces of armour or
  more (the cast used to go on through the night, and zombies killed it there).
- **Lava**: flowing lava at its feet or head, beside or below, is walled off at once (not within 2
  blocks of the mould).
- **Out of the portal**: standing in its own lit portal during the Overworld run, it steps out.
- **Explore**: three leg time-outs within 6 blocks send it 12 blocks on and 6 up.
- **Water clock**: two minutes for a bucket of water, whichever source it is after (two it couldn't
  reach took turns for 33,000 ticks).
- **Mobs out of reach**: a mob it was fighting when it got stuck is left alone for two minutes (it
  took the same one back for 130,000 ticks).
- **A new body keeps the run**: a body that dies in another dimension now takes the Overworld run
  back, as it did the Nether run (before, it began again from wood).
- **Armour order**: `HORIZON_ARMOUR_FIRST=1` makes the armour before the cast. Armour after the
  portal stays the default:

| Batch (All-knowing, 5 seeds, 48,000 ticks) | portal lit | armour | gold boots | deaths by seed |
| --- | --- | --- | --- | --- |
| r9a, armour after | 2/5 (9:14, 22:12) | 1/5 | 1/5 | 0, 0, 0, 15, 2 |
| r9b, armour first | 0/5 | 3/5 | 1/5 | 22, 1, 1, 9, 1 |
| r9c, armour after, water clock | 1/5 (23:46) | 0/5 | 0/5 | 3, 0, 0, 0, 0 |
| r9d, armour after, all fixes | 2/5 (9:05, 30:28) | 1/5 | 1/5 | 2, 0, 5, 0, 7 |

(seeds 20261020, 1, 42, 123456, 987654321.) No death in the portal stage over r9a and r9c (r7w-r8b:
8 zombies, 5 drownings there); the seeds that didn't light ran out of time. Seed 123456 starts in a
desert with no tree within 48 blocks: all its deaths are husks, zombies and spiders while it looks
for wood.

**The chained route** (`--route`, seed 987654321, All-knowing, 200,000 ticks; times game min:s):

| Try | build | how far | where it ended |
| --- | --- | --- | --- |
| 1 | merge | portal lit, making armour | stood in its own portal, died in the Nether, the new body began again from wood (fixed: steps out, keeps the run) |
| 2 | merge | iron kit | never lit: 11 deaths (6 skeletons), kit lost each time |
| 3 | merge | portal stage | "looking for a lava pool" in one place for 190,000 ticks (fixed: explore escape) |
| 4 | water clock | lit 12:58, armour 27:49 | the Nether's Enter leg ran out (1,200 ticks) walking back down 80 blocks to the portal (fixed: 6,000) |
| 5 | Enter 6,000 | frame block 6 of 10 | chased a mob it couldn't reach for 130,000 ticks (fixed: left alone) |
| 6 | Enter 6,000 | iron kit | 11 deaths, then starving in a water hole it couldn't pillar out of (not fixed) |
| 7 | final | lit 10:21, armour 22:07, Nether 23:04 | fortress located; killed by a hoglin 12 min into the Nether |
| 8 | final | lit 54:35 (6 deaths), armour 70:01 | in the Nether after a 4.5 min walk back; killed by a piglin on the way to a fortress 833 blocks off |

Both of the last two tries reach the Nether; the next deaths are the Nether leg's (hoglins and
piglins on the way to the fortress).

## Appendix: tests run

All were run on the Mac mini (Ludovic away), with `target/fast/horizon-client --offscreen`,
`HORIZON_NO_AUDIO=1`, seed 20261020, a temporary `HORIZON_HOME`, through the control socket. The scripts
were throwaway Python in `/tmp` and are not committed. They used `command`, `give`, `setblock`,
`look-at block … up`, `interact`, `use-item`, `find-block`, `entities`, `status`, `companion` and `events`.

1. **Overworld.**
   - `/tick sprint 2400` in an empty world: 8,343 ticks/s.
   - `/locate`: stronghold 1,421 blocks, ruined portal 495, village 594, shipwreck 1,235.
   - Obsidian: lava plus water above and beside → `obsidian` (both ways).
2. **Flint.** 40 × `loot give @s mine … iron_shovel` on gravel → 7 flint, 33 gravel.
3. **Portal.**
   - Obsidian frame lit with flint and steel → `nether_portal[axis=x]`; in the Nether after 5 s.
   - Nether `/locate`: fortress 427, bastion 794, warped forest 214, soul sand valley 71.
4. **Companion and Nether.**
   - A companion sent into a lit portal alone stayed in the Overworld. With its owner, it arrived in the
     Nether.
   - The fortress's two spawners hold blazes.
   - 4 gold ingots → leather, obsidian, blackstone.
   - A piglin killed a player wearing no gold.
   - Kills by the player: a blaze rod and an ender pearl.
5. **Stronghold and End.**
   - An eye dropped about 10 blocks away on the stronghold bearing (6 → 5 eyes, then picked up).
   - 12 frames at y −4, the silverfish spawner, lava in the room.
   - 12 eyes placed → 9 portal blocks → End arrival at (100.5, 49, 0.5).
   - Dragon phases sampled: holding, strafe, landing approach.
   - A bed exploded.
   - `dragon-hurt` → death flight, egg at (0, 67, 0), exit portal (20 blocks), back to the Overworld spawn;
     no credits.
6. **Sprint with a companion and mobs at night:** 1,515 and 1,240 ticks/s. Companion hub cost: 0.015 ms per
   tick on average, 2.8 ms at most.
7. **Generation:** about 150–170 chunks generated within 0.5 s of each 1,000-block teleport, render distance 8.
8. **Server thread** (`HORIZON_SERVER_THREAD=1`):
   - `companion spawn` refused ("companions play in your own world, not on a server").
   - In the End (by `execute in`), 10 crystals and endermen, but no dragon after 20 s.
9. **Buckets:** an empty bucket aimed at a lava source → lava bucket; placed back → `lava[level=0]`.
