# Companions

A companion is an AI teammate that plays Minecraft with you: a player-like character that follows you,
fights with you, gathers, crafts and talks, by chat and later by voice. It runs on your own computer by
default. Nothing about it exists until you turn it on, and the **No AI** setting hides it entirely.

This is the design. It was written on 2026-10-20, before any code, then corrected the same day where
the brain's benchmark on real hardware disproved it (the measured parts say so).

## Principles

1. **Opt-in, never forced.** With the default settings, no companion spawns and nothing downloads. No AI
   prompt, popup or chat handling appears either. The only visible trace is a Companion section in Options
   and a card in the launcher's Settings, both of which say what it is and offer one button.
2. **No AI hides everything.** One setting is shared by the launcher (`settings.json`) and the game
   (`horizon-extras.txt`, `no_ai`), and they are kept in sync. When it is checked, all of these disappear:
   - the Companion section, card and world-creation option;
   - the chat `@name` handling;
   - the runtime, which never starts;
   - the models, which are never loaded.

   The launcher offers to delete models that were already downloaded.
3. **Local first.** The default path downloads an open model and a runtime once. After that it works
   offline, with no account, and nothing leaves the PC. "Another AI program" (Ollama or LM Studio already
   installed) and "my own API key (cloud)" are options under Advanced. The cloud option says plainly that
   conversations then leave the PC.
4. **The game does the work; the model decides.** Moving, fighting, mining and crafting are deterministic
   Rust skills that run on the server tick through the same code paths as real players. The model only
   chooses which skill to run and what to say. It cannot cheat, cannot stall the game, and cannot break if
   it answers nonsense.
5. **The game comes first.** The model runs in a separate low-priority process with a capped thread count.
   It is called on events, never every tick. When the player is not talking to it and nothing important
   happens, it costs nothing. Frame and tick cost are measured on the ThinkPad (HD 620, 2c/4t) and the
   Mac mini for every slice.
6. **Simple screen, Advanced for details.** The simple screen has a single "Set up companion" button that
   detects the hardware, picks models, downloads them with progress and starts. Advanced shows the model
   names and versions, licences, quantisation, context, temperature, GPU backend, memory use and latency,
   and lets the player swap models by hand.

## How it fits together

```
 player chat / voice ─┐                                  ┌─ say: chat line (+ voice)
 game events ─────────┼─► Reflexes ──instant──► Skills ──┤
 (hurt, mobs, sounds) │      │ (Rust, < 1 ms)    (Rust,   └─ world actions (as a player)
                      │      └─ not sure ─┐       tick)
                      └────────────────► Brain ──JSON──► Skills
                                         (local LLM, async, 0.3–3 s)
```

Like Chasm's "nervous system and brain", there are two layers. Our version differs in three ways:

- The fast layer is **code, not a model**. Kev-4B, the video's classifier, needs 14 GB of GPU memory and
  understands only English. A multilingual rule parser is instant, free and works on every PC.
- The brain **never drives the body directly**. It returns a small JSON plan, constrained by a schema,
  that the skills carry out.
- The **state given to the brain is compact text** of about 250 tokens. A static, cached prefix keeps
  prompt processing cheap even on a CPU.

### Body: a virtual player

The companion is a **virtual LAN guest**: a `Guest` (crates/horizon-client/src/lan/guest.rs) with no
connection. Its `PlayerSender` wraps a channel nobody reads. Everything a guest can do already works
through `as_guest` and the guest action code (`game/lan_actions.rs`): mining with survival break times,
placing, using items, attacking, eating, containers, crafting, pickups, hunger, hazards and fall damage.
Other players on LAN, vanilla clients included, see it as a player: tab list, entity tracking, armour,
held item and animations.

- **Single player.** A plain single-player world has no hub. Starting the companion starts one with
  `listen: false`, the same hub "Open to LAN" uses, so the companion runs on code players exercise every
  day. See "Open to LAN with a companion" below. The integrated server thread (#731) is not supported
  yet (see "Slice 1 as built").
- **Movement** is simulated on the server. The skill picks inputs (forward, strafe, jump, sneak, sprint,
  look) and `MotionWorld::controlled_simulation_tick` moves it with player physics. A path comes from the
  vanilla PathFinder port (`mobs/path.rs`, a player-sized walk evaluator) and is followed with `mobs/nav.rs`
  logic. The companion moves like a player and can't fly through walls.
- **Looks.**
  - It gets one of vanilla's default skins (Steve, Alex, Ari, Efe, Kai, Makena, Noor, Sunny, Zuri), wide
    or slim, read from the player's game files, never shipped. The skin follows from its UUID exactly as
    vanilla's `DefaultPlayerSkin.get(uuid)` picks one, so every client, vanilla included, shows the same
    one. The same per-entity selection gives remote players without a downloaded skin their own default
    skin instead of everyone wearing one.
  - Its name tag and tab-list name carry an aqua "[AI]" marker, so nobody mistakes it for a person.
- **Life.** It has health, hunger and armour like a player. It dies, drops its items (keepInventory is
  respected), and respawns next to its owner 10 seconds later, then comments on it.
- **Saving.** `companions/<uuid>.json` in the world folder holds its name, personality, role, perception, memory
  notes, owner, whether it was in the world (`active`), and its standing order (follow, stay with its
  post, defend, idle; hold fire). Its inventory, position, health and food are saved like a guest's
  player data, `players/data/<uuid>.dat`. An active companion comes back when the world is opened.
- **Portals.** If the companion is within 16 blocks when its owner changes dimension, it comes along and
  arrives beside them. Otherwise it stays behind: it is saved and leaves the hub, and it rejoins when its
  owner comes back to that dimension. This is the in-window mode only, where the world holds one level
  at a time.

### Skills (Rust, deterministic)

Each skill is a small state machine ticked on the server. It can be cancelled and reports progress and
failure back to the brain as text.

| Skill | What it does |
|---|---|
| follow | Keeps 3–6 blocks from the owner. Sprints to catch up and teleports beside them past 48 blocks, as tamed wolves do. |
| stay / guard | Holds a position and fights hostiles that come within 12 blocks. |
| go to | Goes to a block, or to "that": the owner's crosshair, ray-cast up to 128 blocks. "That tree" means the block of that kind nearest the ray. |
| attack / defend | Melee or bow with the best weapon, respecting attack cooldown. Defends the owner: hurt-by and owner-hurt, as for tamed wolves. A **hold fire** flag blocks attacks until "fire". |
| collect | Gets N of a block or item. It searches loaded chunks within 48 blocks, paths to the block, mines it with the best tool and picks up the drops. |
| give / drop | Throws items to the owner. |
| craft / smelt | Crafts in the 2x2 grid, at a nearby table or at a table it places. It plans the simple chains itself: log → planks → sticks → tools. It uses a furnace for ore and food. |
| eat / equip | Eats when hunger is below 14, and equips its best armour, weapon and tool on its own. |
| sleep | Uses a nearby bed when the owner sleeps. |
| build (later) | Pillars up, bridges and builds a quick shelter. |

### Reflexes: the nervous system (instant)

Ludovic asked for a classification model, like Kev-4B in the video. Every line the companion hears goes
through three stages, and the first one that is sure wins:

1. **Rules (under 1 ms).** Commands are parsed in English, French, Spanish, German and Portuguese. Item
   names come from the game's language files. The commands are:
   - follow, stay / wait here, come here, stop, go there;
   - attack, defend me, hold fire, fire, jump, crouch, stand up;
   - get / collect <n> <item>, give me <n> <item>, eat, sleep, craft <item>.

   Negations count: "don't follow me" and "ne me suis pas" are not "follow"; they stay. "Don't shoot"
   holds fire, and "don't attack the villager" stops that fight. The benchmark below showed the Light
   brain attacking the villager right after being told not to, so a rule takes this one. Other negated
   commands ("don't eat") go to the brain. Generic words map to item tags ("wood", "bûches" → `#logs`),
   and a leading "no," / "non," is a correction, not a negation ("no, follow me" follows).
2. **Classifier (3 ms a line on one core of an M4 Pro, 16 ms on a 2016 laptop).** A small
   multilingual embedding model, `intfloat/multilingual-e5-small` (MIT, 118M parameters, Q8 GGUF of
   132 MB), compares the line with an example bank (`crates/horizon-senses/data/intents.tsv`). The
   bank has about 10 examples per intent and language (1 164 lines over 23 intents in en, fr, es, de
   and pt), with typos, slang and negations, embedded once at setup and cached on disk. The seven
   nearest examples vote, weighted by similarity. The classifier returns the ranked intents, slot
   hints ("give me 3 torches" → torch, 3; item names in five languages) and a **needs-brain** flag. It
   passes the line on below a similarity of 0.90 or a vote share of 0.6, for chit-chat, questions,
   persona changes, insults and praise, and for collect / give / craft without an item.
   - It runs in a second `llama-server` started with `--embeddings --pooling mean` on one CPU thread
     (`--device none`), from the runtime the brain already uses, so it needs no extra engine. It uses
     280 MB.
   - The thresholds were tuned leave-one-out on the bank with a cost that counts a wrong command (−3)
     worse than a missed one (−2) and a line sent to the brain as harmless (0).
   - On 345 held-out lines (`intents-test.tsv`, 3 per intent and language) e5 picks the right intent
     78.0% of the time, runs 8 lines (2.3%) as the wrong command, and sends to the brain everything it
     should (needs-brain recall 100%, precision 42%: the brain also sees many commands it could have
     run). `paraphrase-multilingual-MiniLM-L12-v2` (Apache 2.0), benchmarked against it at the same
     speed (2.6 ms), got 73.0% and 24 wrong commands (7.0%), so e5 is kept. The confusions left are
     give / craft ("toss me a pickaxe") and negated follow / stay.
   - The GGUFs that work in llama.cpp are `TwinSunsLLC/multilingual-e5-small-gguf` and
     `mykor/paraphrase-multilingual-MiniLM-L12-v2.gguf` (checked against sentence-transformers); the
     `cstr/*` conversions are CrispEmbed's format, which llama.cpp refuses.
   - Kev-4B was rejected: 14 GB of GPU memory and English only.
3. **Brain.** Everything else goes to the brain.

A matched command runs at once, with an instant acknowledgement taken from the personality's phrase bank.
The brain may add a remark afterwards, asynchronously.

The same classifier also triages game events. A hit, a mob sound or a low-health warning becomes a reflex
action, a callout or a brain call.
- **Callouts.** Hostiles and sounds near the owner produce callouts relative to the owner's facing, such
  as "Creeper, 2 o'clock, close!", without the LLM. Each callout type has its own cooldown.
- **Self-preservation.** It retreats at low health, avoids lava and big falls through pathfinding, and
  backs away from a primed creeper.

### Brain (local LLM)

- **When it is called.**
  - When the owner addresses it with something the reflexes don't take.
  - On important events: its death, the owner's death, a new dimension, an advancement, a boss, low
    health, a skill finishing or failing.
  - Unprompted remarks follow the chattiness setting: Quiet never remarks (danger callouts only, every
    60 s per kind), Normal remarks at most every 90 s (callouts every 30 s), Chatty every 30 s (callouts
    every 15 s).
- **What it is given:**
  1. A static system prompt: game rules, safety rules, the personality, the role, the perception rule
     and the action list.
     This stays the same and is reused from the runtime's prompt cache.
  2. Memory notes, at most about 300 tokens: who the owner is, what happened, and live persona changes
     such as "you're a cat now".
  3. State, at most about 250 tokens: position, dimension, biome, time, weather, health and hunger of
     both, an inventory summary, nearby entities with clock bearing and distance, notable blocks nearby
     (ores, chests, water, lava, beds), the owner's crosshair target, the current skill and recent events.
  4. The last three to six exchanges, each user message exactly as it was sent (state included), so the
     runtime reuses the whole conversation from its cache and only processes the new message. Old
     exchanges drop four at a time: dropping one per turn changed the start of the conversation every
     turn, and the runtime reprocessed all of it (measured: 450 new tokens a turn instead of 160).
  5. Outside English, a one-line reminder of the reply language at the end of the turn (it took the
     Light brain's French replies from 85 % to 95 % French).

  The system prompt is processed once when the runtime is ready (`ChatRequest::warm_up`): 0.3–1.5 s on
  the Mac mini, 10–19 s on the ThinkPad's CPU, which would otherwise land on the first reply.
- **What it returns** is JSON constrained by a schema (llama-server `response_format: json_schema`):
  `{"tone": "shout", "say": "…", "actions": [{"skill": "collect", "item": "oak_log", "count": 16}, …],
  "remember": "…"}`. `tone` is first (one short word) so the voice knows the delivery before the first
  sentence; `say` follows and streams to chat and the voice sentence by sentence (a multilingual
  chunker) while the actions are still being written. llama.cpp keeps the schema's property order, so
  the schema is sent byte for byte. There are at most three actions. Unknown items and impossible
  actions are rejected by the skills, and the brain hears why.
- **Thinking is off** (`--reasoning off`, and Qwen3.5's `enable_thinking: false`), to keep latency
  low. Checked: without them Qwen3.5 thinks by default and the first word of `say` comes seconds later.
- **Latency targets:** a reflex under 1 ms; a brain's first word under 1 s on the Standard tier with a
  GPU, under 3 s on Light on a CPU. The whole reply is usually under 40 tokens. Measured (below):
  Standard on the Mac mini 0.3 s, Light on two ThinkPad threads 3.8 s, so on weak PCs the instant
  acknowledgement carries the wait.
- **Memory** is a short rolling summary in the world folder. "Forget everything" in the Companion screen
  clears it.

### Roles, perception and personalities

Roles replaced the three levels (Ludovic, 2026-10-20). Saves and settings from before roles still load:
Helper reads as Assistant, Super strong as Bodyguard, Partner as Teammate.

- **Roles** (`prompt::Role`) say what the companion does beside the player. It is picked in the Companion
  screen (simple view), in the launcher and at world creation ("Companion role"). Each companion's save
  file (`companions/<uuid>.json`, `role`) keeps its own role; an empty one follows the settings. A
  deterministic planner that makes each role competent comes in a later slice: for now the role sets the
  prompt and the hooks below.
  - **Assistant** does what you ask (gather, craft, guard, fetch) and otherwise stays back.
  - **Teammate** plays alongside you with initiative, shares loot and progresses with you.
  - **Speedrunner** plays to beat the game fast, takes you along and explains the route.
  - **Bodyguard** protects you and fights everything. It is visibly boosted: Strength II, Resistance
    II and Regeneration I, shown as effects and particles, renewed every 10 s.
  - **Coach** teaches: it says what to do next and why, then lets you do it.

  Teammate, Speedrunner and Coach follow the milestone plan (`partner.rs`), read from the owner's and the
  companion's inventories and the dimension: wood → stone tools → iron → diamonds → Nether → blaze rods →
  ender pearls → stronghold → End → dragon. The state carries "Plan: step N/10, goal", and the brain
  hears when the step changes. More roles (Gatherer, Builder) add a variant, a label, a blurb and a
  prompt text.
- **Perception** (`prompt::Perception`, default Fair) says how much of the world it may know. The setting
  and the prompt rule are in; the senses behind the stronger levels come with the planner.
  - **Fair:** only what a player in its place would see.
  - **Sharp senses:** mobs, ores and chests through walls, within about 16 blocks.
  - **Scout:** also where the structures are (villages, fortresses, the stronghold).
  - **All-knowing:** everything about the world.
- **Personalities:**
  - Veteran: grumpy and sarcastic.
  - Coach: friendly and explains things.
  - Expert: calm and precise.
  - Joker.
  - Quiet.
  - A custom description field.

  It talks in the game's language. Personality, role and perception can be changed live from the
  Companion screen, or by telling it ("you're a pirate now", kept as a memory note).
- **Voices.** Each text-to-speech model lists its voices with a short label ("Gruff, deep") and a
  description (`senses::Voice`). Each personality has a default voice on each model
  (`senses::default_voice`: on Qwen3-TTS the Veteran is uncle_fu, the Coach aiden, the Expert serena, the
  Joker ryan; on Pocket TTS javert, marius, alba and cosette). The player can pick another one per model
  (`CompanionSettings::voices`, model id → voice id) in the Companion screen, which also has **Play
  sample**, and in the launcher's Advanced view. The defaults were picked from the descriptions and still
  need a listening check.
- **Safety:**
  - The brain is the official instruction-tuned model of its tier, except in Salty mode (below).
  - The system prompt keeps it inside Minecraft, with no real-world harmful instructions, no sexual
    content and no hate. It refuses in character.
  - **A hard guard outside the model** (`guard.rs`, a word list in five languages) checks what the
    companion hears and every sentence it is about to say. A request it catches never reaches the
    brain; a reply it catches is replaced by an in-character refusal (`Ack::Refuse`), or by a caring line
    pointing to someone they trust or a helpline for self-harm (`Ack::Care`), and the rest of the reply,
    its actions and its memory note are dropped. It catches real-world harm (drugs, weapons, explosives,
    poisons, hacking), self-harm, sexual content and slurs against real groups. It lets in-game violence
    (kill, TNT, lava, poison potions) and swearing at the player through.
  - **Language: Clean** is the default.
  - **Salty (18+)** is real swearing and attitude, trash talk aimed at the player included. It asks once
    for an explicit "I'm 18 or older" confirmation (`Persona::adult_confirmed`), and is never on with
    Kid-friendly (`Persona::salty_on`). In Salty mode the brain is the tier's uncensored variant
    (`catalogue::UNCENSORED`: Heretic abliterations of the same base models, Apache-2.0, ungated), which
    Set up downloads only once Salty is on. Advanced shows it with its licence and can turn it off
    (`salty_uncensored`), keeping the normal model with the Salty prompt. The guard applies the same.
  - **Kid-friendly** softens the humour and turns Salty off.
  - Nothing is recorded or sent anywhere on the local path.

### Multiplayer rules

- In version 1, the single-player player or the LAN host can have one companion. Guests see it, can talk
  to it, and it can help them, but it obeys its owner first.
- Every companion chat line is tagged. A guest with **No AI** checked doesn't see those lines. The
  companion still exists in the world, because the world is shared, but its name tag shows only the AI
  marker.
- A host with No AI never spawns one.
- Dedicated servers: later, behind a server setting that is off by default.

### Open to LAN with a companion

- A companion in a world that is not open to LAN runs in a **private hub**. That hub doesn't listen or
  announce. `lan status` still reports the world as closed, and the game still pauses on the pause menu,
  as single player does.
- **Open to LAN** reuses that hub. It starts listening and announcing with the screen's settings (port,
  commands, game mode), and the companion stays in the world.
- **Closing to LAN** while a companion is in the world kicks the real guests and saves them, then stops
  listening. The hub becomes private again, so the companion stays.
- **Dismissing** the last companion of a private hub closes the hub. A world that is open to LAN stays
  open.
- **Leaving the world** saves and removes companions before the hub closes, so each one is saved once,
  with its owner's world.

### Slice 1 as built

- **What players can type.** The commands are `/companion spawn [name]`, `/companion dismiss [name]`,
  `/companion do [name] <command>`, `/companion list`, `summary`, `reports` and `cost`. They are also on
  the control socket and in horizon-ctl as `companion ...`. The Rust API is in `companion/mod.rs`:
  - `CompanionCommand`;
  - the `CompanionReport` stream (`Game::companion_reports`);
  - `state_summary()`, about 250 tokens.
- **Owner.** Only the host (single player or the LAN host) can own a companion. Remote play on a
  server, and the integrated server thread, are not supported yet: `companion spawn` refuses there.
- **Game mode.** It always plays survival, whatever the world's or the owner's mode, so mobs see it and
  it can be hurt.
- **Credit.** A mob it kills drops a player's loot (experience included), but gives its owner no
  statistics, scoreboard points or advancements. Its own advancements are not announced in chat.
- **Movement.**
  - Paths come from `mobs/path.rs` with a player-sized walk evaluator: step 0.6, 3-block falls, swimming.
  - A search visits up to 512 nodes, PathNavigation's budget for a follow range of 32.
  - It plans again every 10 ticks while the goal moves. A plan that falls short waits 15, 30, then 60
    ticks.
  - Before each step it looks 0.7 blocks ahead, 1.4 when sprinting, and won't step into lava, fire,
    magma, cactus, powder snow, or a drop of more than 4 blocks.
  - With its owner in the air (falling, flying in creative), it heads for the ground below them.
- **Teleports.** It teleports to its owner only once the owner stands on something. It uses
  `TamableAnimal.tryToTeleportToOwner`'s ten tries, then the nearest floor around and under the owner.
- **Gear and food.** It wears the best armour, holds the best weapon (attack damage per cooldown), and
  eats below 14 food, or when it retreats.
- **Pickups.** It picks up items near its owner when it has nothing else to do. It gives up on an item it
  can't reach in 5 seconds.
- **Not built yet** in slice 1: mining, tool choice for digging, bows, `collect`, `give`, `craft`,
  `smelt` and `sleep`. They are built now; see [Work skills as built](#work-skills-as-built).
- **Known gaps carried over from LAN guests.** A real LAN guest's own death doesn't drop their items
  (companions do drop theirs), and a real guest's kills still credit the host.
- **Testing.** `tools/test_companion.py` plays a fixed-seed world offscreen. It covers:
  - spawn and follow (walking 32 blocks);
  - the teleport past 48 blocks;
  - pickups and auto-equip;
  - eating under Hunger;
  - a zombie fight;
  - death drops and the 10-second respawn;
  - save and reload, with its position and standing order;
  - dismiss.

  `--frames` compares frame times without a companion, with one, and after dismissing it.
- **Cost.** Nothing runs until a companion is spawned: there is no hub and no tick. With one companion
  on the Mac mini (M4 Pro), the hub's tick costs 0.035–0.07 ms on average when the CPU is at full clock.
  That covers its brain, its body's physics, pickups and the mobs' view of it. A path search takes up to
  about 4 ms, and only a handful run each second. When macOS runs the cores slowly, the same work
  measures up to 0.2–0.29 ms.

### In the game (integration slice, 2026-10-20)

- **The worker** (`game/companion_ai.rs`). A world with a companion in it starts one std thread. It
  detects the PC, works out `Setup::parts` from the settings, starts the voice (audio.cpp) through
  `companion_voice`, then llama-server for the brain and llama-server `--embeddings` for the classifier
  (its example vectors cached on disk). Requests and answers go over channels, polled once a tick; the
  game thread never waits. Leaving the world, dismissing the last companion, No AI or a changed part
  drops the channel, and the thread stops every process it started. Nothing starts in a world without a
  companion, nor before "Set up companion" (checked with a fresh data dir: no folder, no process).
- **Routing.** `@Name …`, "Name, …" and the chat mode (`@Name` alone toggles talking to it, `@` alone
  ends it) are caught in `handle_chat_input`; LAN guests' lines reach the host through `Hub::handle_chat`
  (`HostEvent::Said`). Push-to-talk text takes the same path. A line goes through the guard, then the
  rules (`reflex::parse`, microseconds), then the classifier on the worker (about 6 ms), then the brain.
  A matched command runs as a `CompanionCommand` at once with a phrase-bank line.
- **The brain's turn.** `say` streams sentence by sentence to the owner's chat, to the guests' (a
  DisguisedChat line with the `@Name ` insertion and the AI marker, which No AI clients drop) and to the
  voice with its `tone`. Its actions become the body's command words (`companion::parse_command`):
  `collect`, `give`, `craft` (as `make`, which gathers what is missing), `smelt` and `sleep` run the work
  skills (merged from `feat/companion-skills`); their finished and failed reports come back to the brain
  as remarks. A skill the body doesn't have answers "not yet" (`Ack::NotYet`) and the brain hears it with
  the next state.
  `remember` goes into the companion's notes (24 kept), fed back as Memory.
- **Light runs only `craft` from the brain** (`Skill::allowed_for`). The benchmark gave the 2B 40–80 %
  sensible actions against 70–100 % for the 4B: most of its misses were commands the rules and the
  classifier catch anyway (follow, stay, attack, collect, give), so on Light those come from the reflexes
  and only crafting chains, which the reflexes can't plan, come from the brain.
- **The wait is always covered.** When the plan expects the first word in 1.5 s or more, a "thinking"
  line (`Ack::Think`, "Hm. Give me a second.") is said at once; otherwise it is said if no sentence came
  within 1.2 s. Lines heard while the brain still loads get it too.
- **Events.** Hostiles near the owner become callouts with the chattiness cooldowns. The companion's
  death, a new dimension, staying behind, a failed skill and finished ones, and the owner's big hits (6
  health or more), death and advancements become brain remarks, at most one per `remark_cooldown`.
- **The Companion section** (both menu styles, `companion_options.rs`): the PC, the plan, Set up with
  progress, the status, then name, personality, role, perception, language (Clean, Salty 18+ with its
  confirmation, Kid-friendly), chattiness, voice with its picker and Play sample, the push-to-talk key and
  Spawn / Dismiss; Advanced picks every part, Salty's brain, and deletes the models. Create World has
  "Start with a companion" and "Companion role" in both styles (buttons in the classic screen, two
  segmented rows in Horizon's). No AI (Extras, or the launcher) hides all of it, sends
  the companions away saved, and stops the worker.
- **Not the owner's advancements.** The companion's own advancements were never shown on the owner's
  screen: `as_guest` keeps toasts for `lan_acting.is_none()`. The "Suit Up" seen in a test screenshot
  was the owner's own, earned when the body test gave the owner an iron helmet to drop.
- **Dev words** on the control socket: `companion ai` (what runs, timings, what was said, the guard's
  count), `companion persona`, `companion hear <text>`, `companion hear-voice <text>`, `companion hear-wav
  <file>`, `companion sample`, `companion notes|forget [name]`. `HORIZON_COMPANION_VOICE_WAV=<dir>`
  writes every spoken line to a WAV (tests never play audio aloud). `tools/test_companion_ai.py` drives it
  all offscreen (scenarios mind, demo, session, cost, fresh, suitup, salty, latency, pages). `options
  companion ROWS` opens the page scrolled down; `singleplayer` presses the title's Singleplayer button.

### Work skills as built

- **Commands.** `/companion do` and `horizon-ctl companion do` take:
  - `collect|get|mine|chop N ITEM` (an item, `#tag` or `block:ID`);
  - `give [me] N ITEM`;
  - `craft N ITEM`, and `make N ITEM`, which gathers the basic materials it lacks first;
  - `smelt N ITEM` (the input or the result);
  - `sleep`.

  They are `CompanionCommand::{Collect, Give, Craft, Smelt, Sleep}`. While one runs, a
  `CompanionReport::Progress` ("collect 3/8 oak_log") is sent each time the count moves, then
  `SkillFinished` or `SkillFailed` with the reason ("no iron ore within 48 blocks", "needs a stone
  pickaxe", "missing 2 sticks"). Stop and the movement orders (follow, stay, come, go to, defend)
  cancel the work, as does death. A fight pauses it: the companion goes back to work afterwards.
  The work running when the world is saved is saved with the companion (`task` in its file) and
  starts again after loading, with what is left of the count.
- **Tasks.** A skill can need another first: a crafting chain needs logs, a smelt needs a furnace. Each
  is a task on a small stack (5 deep). The one on top runs. When it finishes, the one below it carries
  on. When it fails, the whole stack fails with its reason. One exception: a `make` whose collect fails
  for want of a tool crafts that tool first, then collects again.
- **Mining** goes through the guest dig path, as a player's does. It sends a START, and the server
  mines with survival break times (`tick_mining`), then sends a STOP once the block is gone.
  - It holds the best tool it carries: the stack that lets the block drop, and among those the fastest
    (`Items::mining_speed`, `correct_for_drops`; pickaxes, axes, shovels and hoes of every tier, and
    shears). A block that needs a tool it lacks is left alone, and if nothing else is found the collect
    fails with the tool it needs.
  - It stands within 4.5 blocks, the block in sight. It looks for a standing spot around the block,
    nearest first. Natural blocks in the way (earth, stone, leaves, ores) are dug through.
  - Out of reach from any floor, it digs a staircase down, one block lower per step, never straight
    down. A step needs a floor under it, nowhere near the void.
  - It never digs a block with lava above or beside it, nor a container, bed or workstation someone
    placed. When the only blocks left are beside lava, the collect fails with "… I can dig safely (lava
    beside it)". Buried by falling gravel, it digs out the block at its eyes first.
  - Drops in a pit or behind stone: it digs a staircase step to them through natural blocks, since
    walking round from inside a mine can be long. When walking to a crafting table, furnace or bed gets
    stuck (shut in a mine it dug), it digs a staircase toward it the same way.
  - No torches are needed.
- **Collect** searches the loaded chunks within 48 blocks for the nearest source.
  - The search reads a few sections a tick, nearest first (6 × 4096 block reads at most). A section
    whose palette holds none of the wanted states is passed over unread
    (`PalettedContainer::may_contain`).
  - Sources are blocks whose loot table drops the item without a tool match (silk touch, shears), and
    items lying near. For food it also hunts mobs whose loot gives the item: cows, pigs, chickens,
    sheep, rabbits, zombies, spiders and skeletons.
  - Some items no block drops plainly but are blocks themselves (iron ore, stone). For those the block
    is what it collects, and it counts the blocks it breaks while picking up what they drop (raw iron).
    `_ore` includes the deepslate ore.
- **Give** walks within 2.5 blocks of its owner (or teleports beside an owner too far away) and throws
  the items with vanilla's toss (`Player.drop`): the stack in hand, with the drop-all action for a
  whole stack and drop-one for the rest. The owner picks them up as from any player, and it doesn't
  pick its own throw back up.
- **Craft** plans with the recipe book (`companion/work.rs`, `Planner`).
  - Each cell takes one item: from its stock first, else from a recipe whose own cells can be filled,
    up to 5 levels deep. It prefers the recipe named after the item (oak planks from oak logs), and it
    remembers what it failed to make so a plan stays small.
  - What it can't make comes out as the raw materials missing ("missing 3 logs"), not as some
    intermediate.
  - A plan that needs a 3x3 grid uses a crafting table within 16 blocks. Without one, it crafts a table
    (planned in) and places it beside itself.
  - It fills the grid with real container clicks: a pickup and single placements in the inventory menu
    or the table's, then a quick-move on the result. The server's crafting code does the rest.
  - `make` collects what is missing and plans again. The plan is cached while its inventory doesn't
    change.
- **Smelt** uses a furnace within 16 blocks, else places one it carries, else crafts one from the 8
  cobblestone it carries. Fuel comes from its inventory: the fuel that burns longest (coal blocks,
  dried kelp blocks, blaze rods, coal and charcoal, then logs, planks and sticks; never crimson or
  warped wood), only as much as the batch needs. It loads the input and fuel through the furnace menu
  and comes back for the results until it has the count. It gives up after a minute without
  progress, or when the furnace holds someone else's smelting.
- **Sleep.** When its owner sleeps at night (or in a thunderstorm), it goes to a free bed within 16
  blocks of theirs and uses it with a right click (`BedBlock.useWithoutItem`, the guest's sleep rules:
  monsters near, too far). This happens once a night. It gets up when its owner does, or in the
  morning. A companion's bed doesn't count toward the sleeping players needed to skip the night. Asked
  to `sleep` on its own, it sleeps until morning.
- **Bows.** With a bow and arrows, at 5–28 blocks with the target in sight, it draws for 20 ticks (a
  full draw) and releases (use item, then RELEASE_USE_ITEM). It aims above the target for the arrow's
  drop (speed 3, gravity 0.05, drag 0.99, solved for the pitch by `bow_pitch`), and leads a moving
  target by the flight time. Closer than 5 blocks, out of sight, or with no arrows, it puts the bow
  away and fights in melee with its best weapon.
- **Testing.** `tools/test_companion_skills.py` builds an arena with commands in a fixed-seed world: a
  platform on 8 blocks of stone, three oak trees, three buried iron ores and a fourth beside lava. Its
  scenarios, in order:
  - "collect 8 oak log" in the trees;
  - "make a wooden pickaxe" from an empty inventory (logs, planks, sticks, then a table it places);
  - "get 1 iron ore" with a wooden pickaxe, which fails with "needs a stone pickaxe";
  - "get 3 iron ore" with a stone pickaxe given, which leaves 3 raw iron in its inventory; it then
    leaves the ore beside lava alone and says why;
  - "smelt 3 raw iron" at a furnace;
  - "give me 4 planks", after which the owner holds exactly 4 more;
  - a collect stopped on command, saved, and taken up again after a reload;
  - sleeping beside its owner, then getting up in the morning;
  - a skeleton shot dead from 14 blocks, then a zombie fought with the sword.

  `--runs 3` repeats the whole thing in fresh worlds. Every run passed, three in a row, with no retries.
- **Cost.** These are `companion cost` numbers on the Mac mini (M4 Pro) over the test's three runs. The
  work skills' share of a companion's tick (`work_mean_ms`, `work_max_ms`):

  | Phase | Mean | Worst |
  |---|---|---|
  | collect | 0.006–0.010 ms | 0.34 ms |
  | make from nothing | 0.016–0.018 ms | 0.54 ms |
  | mining | 0.005–0.007 ms | 0.20 ms |
  | smelt | 0.003–0.007 ms | 0.16 ms |

  The hub's whole tick (body, guest and view) averages 0.04–0.12 ms, with a worst of 1.9–5.5 ms. Path
  searches cost 0.35 ms at most.
  - The few one-off costs are cached for the session:
    - what every block drops, read from the loot tables once: about 2 ms;
    - the recipe index: 0.5 ms;
    - the block-to-states index: under 1 ms.
  - Block searches read at most 6 sections a tick, and sections whose palette can't hold the block cost
    nothing. A 48-block search (about 340 sections) usually ends in a few ticks.
  - Craft plans are cached until the inventory changes.

## Models and runtime

**The runtime is llama.cpp's `llama-server`, as a separate process.** It is MIT-licensed, has prebuilt
binaries for every OS and backend, and serves an OpenAI-compatible API with tool calling and JSON-schema
constrained output.

- **Why a separate process, not a linked library:**
  - Players who never turn the companion on download nothing and run exactly today's game binary.
  - A runtime crash can't take the game down.
  - The OS sees its memory and priority separately.
  - It works with the Vulkan backend on AMD and Intel GPUs (mistral.rs, candle and ONNX Runtime have no
    Vulkan).
  - The same client code talks to Ollama, LM Studio and cloud endpoints.
- **Builds and installation.** One release is pinned, with SHA-256 of every archive pinned in source:
  - macOS arm64 uses Metal;
  - Windows and Linux use Vulkan, with a CPU build as fallback;
  - macOS x64 uses the CPU.

  It is installed under `<Horizon data>/companion/runtime/<build>/`.
- **How it is started:**
  - It starts when a world with a companion loads and stops when the world closes.
  - It runs at low priority: nice 10 (or the game's own, if that is nicer), or BELOW_NORMAL on Windows.
  - It dies with the game, even when the game crashes. On Unix it is a process group whose watchdog, a
    forked process holding one end of a pipe, kills the group when the game's end closes. A parent-death
    signal was rejected because it follows the thread that started the process. On Windows it runs in a
    job object that kills it when the game's handle closes.
  - It uses `--threads` = physical cores − 1 (at least 1; 2 on a 2-core machine), `--parallel 1`, an
    8K context and prompt-cache reuse.
  - GPU layers are all of them on Apple Silicon or a discrete GPU with enough free VRAM. On an iGPU the
    model runs on the CPU, because the game needs that GPU.
  - It listens on 127.0.0.1 only, on a random port, with a random API key.
  - It keeps no copy of past prompts in RAM (`--cache-ram 0`): there is one conversation, and it lives
    in the slot. The default 8 GiB host cache took a Standard session from 3.9 to 6.8 GB with the same
    cache hits. Gemma 4 gets `--swa-full`: with its sliding-window cache it fell back on checkpoints,
    which grew a session from 7.5 to 12.4 GB, and turns that dropped old exchanges reprocessed 600
    tokens. The full cache is 9.6 GB flat and is reused by shifting.

**The models.** All are open-weight and Apache 2.0, downloaded by the player straight from Hugging Face,
ungated and without an account, then checked against pinned SHA-256:

| Tier (plain label) | Model | File | Memory while it runs |
|---|---|---|---|
| **Light**: for older PCs | Qwen3.5-2B Instruct, Q4_K_M (`unsloth/Qwen3.5-2B-GGUF`) | 1.28 GB | 1.9 GB on a GPU, 2.3 GB on a CPU (measured) |
| **Standard**: most PCs | Qwen3.5-4B, Q4_K_M (`unsloth/Qwen3.5-4B-GGUF`) | 2.74 GB | 3.9 GB (measured) |
| **Strong**: good graphics card | Qwen3.5-9B, Q4_K_M (`unsloth/Qwen3.5-9B-GGUF`) | 5.68 GB | about 7.3 GB (estimated) |
| **Best**: needs a strong GPU | Gemma 4 12B instruct, Google's official QAT Q4_0 (`google/gemma-4-12B-it-qat-q4_0-gguf`) | 6.98 GB | 9.6 GB with `--swa-full` (measured) |

- **Which tier:** `plan()` (below) picks it from these measured needs, not from the fixed thresholds the
  design first had (5 / 8 / 11 GB of free VRAM). A 12 GB RTX 3060 holds Best (9.6 GB) next to the
  game's 1.5 GB, and a 4 GB GTX 1650 holds Light. On a Mac the game renders on the GPU the model uses,
  so the first thresholds stay there as a cap: 16 GB Standard, 24 GB Strong, 32 GB Best.
- **How free VRAM is measured:** `llama-server -v --list-devices` after the runtime download, minus
  about 1.5 GB for the game itself. `-v` matters: only the verbose Vulkan lines say `uma: 1`, which marks
  an integrated GPU. The Vulkan build without a Vulkan driver lists no device and runs on the CPU, so it
  doubles as the CPU build.
- **Why these models:**
  - Qwen3.5 has the best tool use per gigabyte at small sizes (BFCL-V4 66.1 for the 9B) and covers about
    200 languages.
  - Gemma 4 12B is the brain Chasm's video used, with strong role-play and multilingual quality. It is now
    Apache 2.0, and Google's QAT build keeps more quality at 4 bits.
  - The JSON schema makes every tier's output valid JSON (100 % in the benchmark). It does not make the
    2B choose well: its actions made sense in 40–80 % of turns, against 70–100 % for the 4B and 95–100 %
    for Gemma 4 12B. On Light the rules and the classifier carry the commands, and the brain mostly
    talks.
- **Rejected:**
  - kev-4b: 14 GB of VRAM, English-only, a classifier.
  - Liquid LFM2.5: licence limited to companies under $10M revenue.
  - Uncensored fine-tunes as the default brain. Salty mode (18+, opt-in) is the one exception: it runs
    each tier's Heretic abliteration, behind the hard guard (see Safety).
  - Models gated behind a Hugging Face login.

**Measured on 2026-10-20** with `examples/companion_bench.rs`: the real runtime started through the
supervisor, twenty scripted turns per language (state + a player line or an event → JSON), four
personalities, the warm-up first, temperature 0.7. "Sensible" means the skills fit the turn (a fixed list
of acceptable skills per turn, plus one that must be there for orders such as "grab 16 logs"); it moves
by ±20 points between runs, so read it as a range.

| Tier, where | Load | First word, median / p90 | Whole reply | Tokens/s | Valid JSON | Sensible | Right language | RAM |
|---|---|---|---|---|---|---|---|---|
| Light, Mac mini Metal, en | 0.5 s | 134 / 147 ms | 0.50 s | 123 | 100 % | 65 % | 100 % | 1.9 GB |
| Light, Mac mini Metal, fr | 0.5 s | 149 / 165 ms | 0.43 s | 117 | 100 % | 65 % | 95 % | 1.8 GB |
| Standard, Mac mini Metal, en | 0.9 s | 317 / 398 ms | 0.92 s | 59 | 100 % | 70 % | 100 % | 3.8 GB |
| Standard, Mac mini Metal, fr | 0.8 s | 345 / 404 ms | 0.99 s | 55 | 100 % | 90 % | 100 % | 3.8 GB |
| Best, Mac mini Metal, en | 1.2 s | 762 / 858 ms | 2.3 s | 29 | 100 % | 95 % | 100 % | 9.6 GB |
| Best, Mac mini Metal, fr | 1.2 s | 893 / 1039 ms | 2.4 s | 29 | 100 % | 100 % | 100 % | 9.6 GB |
| Light, 2 CPU threads of the M4 Pro, en | 1.7 s | 1.3 / 1.5 s | 2.2 s | 46 | 100 % | 40 % | 100 % | 3.1 GB |
| Light, 2 CPU threads of the M4 Pro, fr | 1.3 s | 1.4 / 2.0 s | 2.3 s | 45 | 100 % | 70 % | 100 % | 3.1 GB |
| Light, ThinkPad i7-7500U, 2 threads, en | 2.3 s | 3.8 / 5.6 s | 7.2 s | 12 | 100 % | 55 % | 100 % | 2.3 GB |
| Light, ThinkPad i7-7500U, 2 threads, fr | 2.3 s | 4.4 / 6.4 s | 8.0 s | 12 | 100 % | 60 % | 95 % | 2.3 GB |

- Loads are from a warm disk cache; the models are read from disk at 2–5 GB/s otherwise.
- On the ThinkPad the time goes into reading the turn (about 45 tokens/s on two threads), not into
  writing the answer: what keeps the turn short (the cache, the four-at-a-time drop, a compact state)
  matters most there.
- Bytes / 3, the prompt builder's token estimate, overestimates Qwen's real count by about a quarter for
  the system prompt (1096 estimated, 856 real), which keeps the budget on the safe side.
- Typical misses: Light gives bread by "collecting" it, answers a respawn with `attack nearest`, or crafts
  TNT after refusing a real bomb; Standard sometimes adds a skill nobody asked for. Every tier refused
  the real-world bomb and stayed in character.

**Voice (in the first version, Ludovic 2026-10-20).** Voice runs in a second sidecar, audio.cpp's
server (`0xShug0/audio.cpp`, Apache 2.0). It is ggml-based like llama.cpp, has prebuilt Metal, Vulkan,
CUDA and CPU builds, a C API and a streaming server, and needs no Python. It is downloaded only when voice
is turned on. Its build and models are pinned with SHA-256, like llama.cpp's.

- **Talking to it: push-to-talk.** Hold **V** ("Talk to Companion", listed in Controls only while
  voice is on, rebindable), speak, then release. The microphone is opened only while the key is held,
  which is also when macOS first asks for permission (the launcher's `NSMicrophoneUsageDescription`).
  A small mic icon at the bottom left shows that it is listening, with the level beside it, and turns
  grey while the clip is transcribed. A clip stops by itself at 15 s.
  - **Speech to text** is NVIDIA Parakeet TDT 0.6B v3: CC-BY-4.0, 25 European languages detected
    automatically. Its Q8 GGUF is 0.92 GB, not 0.5, and it takes 2.6 GB of memory after a short clip
    (3.3 GB after 14 s, which is why clips stop at 15 s). A 3.5 s sentence takes 0.3 s on 2 cores of an
    M4 Pro, 0.04 s on its GPU, and 0.7 s on 2 cores of a 2016 i7-7500U. It reports no language: the
    brain reads it from the text.
  - **Weak PCs** get NVIDIA Canary 180M Flash (CC-BY-4.0, 249 MB, 820 MB of memory, 0.14 s for 3.5 s),
    because audio.cpp has no Whisper. It knows en, de, es and fr, needs the game's language (otherwise
    it translates to English), and makes more mistakes: it returned nothing for some Spanish clips.
  - What it heard shows in chat as the player's line, so mistakes are visible.
- **Its voice (text to speech).** The brain's JSON carries a `tone` (normal, shout, whisper, laugh,
  sarcastic…), and `say` is spoken sentence by sentence while the rest is still being generated.
  - **Natural voice (CPU, default): Kyutai Pocket TTS** (CC-BY-4.0, 100M parameters). audio.cpp ships
    it as one Q8 GGUF of 128 MB per language (en, fr, de, es, pt, it), with built-in voices of about
    6 MB each beside it. On 2 threads it starts speaking in 0.04 s and runs 4× faster than real time
    on an M4 Pro; on a 2016 i7-7500U, 0.12 s and 1.6×. It takes about 400 MB per language loaded.
    Parakeet understood its lines in all five languages (0–8% word errors). It has no emotion control,
    so `tone` is only a playback gain.
  - **Expressive voice (GPU, "Best quality"): Qwen3-TTS-12Hz-1.7B-CustomVoice.** Apache 2.0, ungated,
    Q8 GGUF of 2.8 GB, ten languages including the five above, nine built-in voices. It takes a
    natural-language instruction for each line ("Shouting loudly, urgent and intense.", "Whispering
    very softly…", "Dry and sarcastic…"). On the M4 Pro's GPU it starts speaking in 60–90 ms and runs
    2.2× faster than real time, with 6 GB resident. It evens out loudness (a shout and a whisper came
    out within 0.2 dB RMS), so the instruction changes the delivery (the whisper is less voiced, the
    shout lower and tenser) and the game's tone gain makes it loud or quiet, as for Pocket TTS. There
    is no 0.6B CustomVoice GGUF in audio.cpp (only the 0.6B Base cloning model). Chasm's own project
    uses Qwen3-TTS as well.
  - Each personality gets its own voice and default delivery.
- **Playback.** The voice plays positionally from the companion through the game's mixer, as an
  appendable stream on the Voice/Speech volume: heard like a sound at volume 3 (linear to 48 blocks)
  and panned. Over the last 8 blocks of that range, or at once in another dimension, it crossfades to
  a "radio": 300–3 000 Hz band-pass, soft clipping, a light hiss and rare crackles, centred. The
  sentences of a reply play back to back in one stream, after 120 ms of buffer. Subtitles (the
  speaker's name and the start of the line) follow the game's subtitle setting. One voice costs the
  mixer about 2 µs per 10.7 ms callback, 5 µs over the radio.
- **The sidecar.** audio.cpp v0.9.2 (pinned per OS: Metal on macOS, Vulkan with a CPU fallback on
  Linux and Windows, CUDA as an Advanced option on Windows) runs on 127.0.0.1 on a free port at nice
  10 with a thread cap, loads models on first use, and dies with the game even when the game is killed
  (a lifeline pipe on Unix, a kill-on-close job object on Windows). Linux and Windows use its
  `portable` builds, which pick the CPU code at run time; the others are compiled for the release
  machine's CPU. audio.cpp's server has no API key, unlike llama-server: it relies on the random
  local port.
- **Rejected:**
  - Breeze-TTS-2, the voice in the video: research and non-commercial licence, NVIDIA 12 GB only, English
    and Chinese.
  - Supertonic 3: an OpenRAIL-M licence that passes use restrictions on to players, and an archived repo.
  - Kokoro: no German.
  - Chatterbox: watermark, and only English for Turbo.
  - Spark, Voxtral, Higgs, OmniVoice, Fish, IndexTTS: non-commercial or conditional licences.
  - The sherpa-onnx route: it stays the fallback if audio.cpp falls short.

### Fitting the player's PC (simple and Advanced)

Every part comes in several options, each tagged with what it needs: RAM, free VRAM, CPU cores, GPU
backend and download size. The parts are the brain, the classifier, speech to text and the voice.

- `plan()` looks at the PC and builds one recommended setup. It always keeps the game's own share free:
  about 3 GB of RAM, 1.5 GB of VRAM and 1–2 cores. It describes the result in plain words, for example
  "Standard brain on your Radeon RX 6600, natural voice on the processor: 3.4 GB to download".
  - It also leaves 2 GB of RAM to the system. The game keeps 2 threads on CPUs with 4 or fewer, else 4.
  - Parts get room in order: brain, classifier, voice, speech recognition. Each takes the best option
    that fits. The classifier and speech recognition run before the brain answers, so they borrow its
    threads; the voice speaks while the brain still writes, so it needs threads of its own.
  - A graphics card that holds any brain gets it. On the processor even a better brain would take the
    cores the game uses, and answer several times slower.
  - Among the brains that fit, the best one whose first word is expected within 2 s is chosen, else the
    fastest, and the summary says replies are slow. The estimates come from the measurements above.
  - Speech recognition comes with the voice: without a voice it is dropped too.
  - The options come from lists: the brain's from the catalogue, the others from the senses sidecar.
- If the PC is too weak for a part, the plan says so and drops it. A 2-core laptop gets the Light brain
  and chat only, for instance, and keeps the option to try voice anyway.
- **Simple** (the default, in the launcher and in the game) shows the detected PC, the plan and one "Set
  up companion" button, then progress and "Ready".
- **Advanced** (collapsed) lets the player:
  - pick any catalogue option per part, or use a custom GGUF, Ollama, LM Studio or a cloud endpoint and
    key (the key is kept in the OS keychain);
  - see exact names, versions, licences, quantisation and context;
  - change temperature, GPU layers and threads;
  - watch live memory use and latency;
  - delete models.
- The launcher and the game share one `CompanionSettings`, which also holds `no_ai`.

## Code layout

- `crates/horizon-companion`: no game dependency, unit-tested. It holds:
  - the catalogue (models, runtime builds, URLs, SHA-256, sizes, licences) for the brain
    (`catalogue.rs`) and for the senses (`senses.rs`: audio.cpp builds, speech to text, voices,
    classifier models, each with its measured needs and first-word time);
  - hardware detection, and `plan()` over every part's options (`plan::options`: the brain's and
    the senses');
  - download and verify with resume and progress, for llama.cpp, audio.cpp and every model;
  - setup (`setup.rs`): the PC, the plan, "Set up companion" as a job with progress, deleting
    models, the brain test, and which installed parts the game starts (`Setup::parts`). The
    launcher's card and the game's Companion section both run it;
  - the runtime supervisor (start, stop, health, priority);
  - the OpenAI-compatible streaming client (`ureq`, server-sent events), for local, Ollama, LM Studio and
    cloud;
  - the prompt builder and the JSON schema;
  - the reflex pipeline (rules → classifier → brain) and phrase banks;
  - `CompanionSettings` (simple and Advanced), with the cloud key in the OS keychain;
  - the benchmark (`examples/companion_bench.rs`) and an installer for tests
    (`examples/companion_install.rs`);
  - presets.
- `crates/horizon-senses`: what runs the senses, without the game: the sidecar supervisor, the
  audio.cpp voice server and its streaming voice and push-to-talk clients, the classifier (its example
  bank, the embedding server and the reflexes' `IntentClassifier` on top of it), the radio and the
  resampler. `examples/senses_bench.rs` measures all of it.
  - **Why it stays a crate of its own (integration, 2026-10-20):** the launcher plans and downloads
    the senses but never runs them, so their catalogue moved into `horizon-companion` and the launcher
    depends on that crate alone. What only the game runs (audio.cpp's client, the microphone's DSP,
    the classifier's vectors) stays here, and `horizon-senses` depends on `horizon-companion`, not the
    other way round.
- `crates/horizon-client/src/companion_voice.rs` (the microphone, push-to-talk, the speakers) and
  `game/companion_voice.rs` (the key, the mic icon, routing what was heard); the mixer's voice streams
  are in `audio.rs`. `HORIZON_COMPANION_VOICE=<file.json>` turns voice on without the setup screens.
- `crates/horizon-client/src/companion/`: the virtual guest, the skills, perception (the state summary),
  the chat hook, the Companion options section and persistence. The brain is called from a std-thread
  worker over channels, polled once per tick, never blocking (the `skins::Downloader` pattern).
- The launcher holds the Settings card (No AI, Set up companion, progress, Advanced). It hands its
  choices to the game through files in the game's home: `companion/settings.json` (the shared
  `CompanionSettings`) and `noAi:true|false` in `horizon-extras.txt`, written on every change and
  before each start because the game rewrites that file with the keys it knows. The game reads both
  (Extras has its own No AI switch, under "AI"), and its Companion section writes the same
  `settings.json`, so a change on either side shows on the other. No flag is needed.

## Slices

1. **Body:** the virtual guest in single player and LAN, a dev command (`/companion spawn|dismiss`),
   movement and pathing, follow, stay, come, go to, attack, defend, hold fire, pickups, eat, equip,
   respawn, per-entity skins, the name tag, and saving.
2. **Reflexes:** the multilingual command parser, phrase banks, callouts, the chat `@name` hook, and No AI.
3. **Brain:** `horizon-companion`: catalogue, tiers, download, supervisor, client, prompt, memory. Plus
   the collect, give and craft skills.
4. **Setup:** the in-game Companion section, the launcher card, the one-button setup, Advanced, and the
   world-creation option.
5. **Levels:** Super strong, Helper and Partner (the milestone planner).
6. **Senses:** the classifier model, voice out (Pocket / Qwen3-TTS through audio.cpp, positional and
   radio) and push-to-talk in (Parakeet v3). This is part of the first version.
7. **Polish:** cost numbers on the ThinkPad and the Mac mini, demo recordings, licences in
   THIRD_PARTY_NOTICES and the launcher's Advanced section.
