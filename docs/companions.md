# Companions

A companion is an AI teammate that plays Minecraft with you: a player-like character that follows you,
fights with you, gathers, crafts and talks, by chat and later by voice. It runs on your own computer by
default. Nothing about it exists until you turn it on, and the **No AI** setting hides it entirely.

This is the design. It was written on 2026-10-20, before any code.

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
  day. On the integrated server thread (#731), it lives in that thread's hub.
- **Movement** is simulated on the server. The skill picks inputs (forward, strafe, jump, sneak, sprint,
  look) and `MotionWorld::controlled_simulation_tick` moves it with player physics. A path comes from the
  vanilla PathFinder port (`mobs/path.rs`, a player-sized walk evaluator) and is followed with `mobs/nav.rs`
  logic. The companion moves like a player and can't fly through walls.
- **Looks.**
  - It gets one of vanilla's default skins (Steve, Alex, Ari, Efe, Kai, Makena, Noor, Sunny, Zuri), read
    from the player's game files, never shipped. This needs per-entity skin selection, which also fixes
    real players all rendering with the default skin today.
  - Its name tag carries a small "AI" marker, so nobody mistakes it for a person.
- **Life.** It has health, hunger and armour like a player. It dies, drops its items (keepInventory is
  respected), and respawns next to its owner 10 seconds later, then comments on it.
- **Saving.** `companions/<uuid>.json` in the world folder holds its name, personality, level, memory
  notes and owner. Its inventory and position are saved like a guest's player data.
- **Portals.** If the companion is within 16 blocks when its owner changes dimension, it comes along. It
  either uses the portal itself on multi-level servers, or arrives with the owner in the in-window mode.

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

### Reflexes (Rust, instant)

- **Commands.** Commands are parsed in English, French, Spanish, German and Portuguese. Item names come
  from the game's language files. The commands are:
  - follow, stay / wait here, come here, stop, go there;
  - attack, defend me, hold fire, fire, jump, crouch, stand up;
  - get / collect <n> <item>, give me <n> <item>, eat, sleep, craft <item>.

  A matched command runs at once, with an instant acknowledgement taken from the personality's phrase
  bank. The brain may add a remark afterwards, asynchronously. Anything else, or an unclear command, goes
  to the brain.
- **Callouts.** Hostiles and sounds near the owner produce callouts relative to the owner's facing, such
  as "Creeper, 2 o'clock, close!", without the LLM. Each callout type has its own cooldown.
- **Self-preservation.** It retreats at low health, avoids lava and big falls through pathfinding, and
  backs away from a primed creeper.

### Brain (local LLM)

- **When it is called.**
  - When the owner addresses it with something the reflexes don't take.
  - On important events: its death, the owner's death, a new dimension, an advancement, a boss, low
    health, a skill finishing or failing.
  - Unprompted remarks are limited to one every 20 s by default. A chattiness setting goes from Quiet to
    Talkative.
- **What it is given:**
  1. A static system prompt: game rules, safety rules, the personality, the level and the action list.
     This stays the same and is reused from the runtime's prompt cache.
  2. Memory notes, at most about 300 tokens: who the owner is, what happened, and live persona changes
     such as "you're a cat now".
  3. State, at most about 250 tokens: position, dimension, biome, time, weather, health and hunger of
     both, an inventory summary, nearby entities with clock bearing and distance, notable blocks nearby
     (ores, chests, water, lava, beds), the owner's crosshair target, the current skill and recent events.
  4. The last six exchanges.
- **What it returns** is JSON constrained by a schema (llama-server `response_format: json_schema`):
  `{"say": "…", "actions": [{"skill": "collect", "item": "oak_log", "count": 16}, …], "remember": "…"}`.
  `say` comes first, so the text streams to chat while the rest is generated. There are at most three
  actions. Unknown items and impossible actions are rejected by the skills, and the brain hears why.
- **Thinking is off** (Qwen3.5's `enable_thinking: false`), to keep latency low.
- **Latency targets:** a reflex under 1 ms; a brain's first word under 1 s on the Standard tier with a
  GPU, under 3 s on Light on a CPU. The whole reply is usually under 40 tokens.
- **Memory** is a short rolling summary in the world folder. "Forget everything" in the Companion screen
  clears it.

### Levels and personalities

- **Levels:**
  - **Helper** does what you ask, defends itself and you, and otherwise stays back.
  - **Super strong** fights everything near you, protects you and gathers gear fast. It is visibly
    boosted: Strength II, Resistance II and Regeneration I, shown as effects and particles.
  - **Partner to beat the game** plays alongside you through the progression, using a milestone graph
    read from advancements and inventories: wood → stone tools → iron → diamonds → Nether → blaze rods
    and pearls → stronghold → End → dragon. It picks the next milestone, works on it when you don't
    command it, and tells you the plan.
- **Personalities:**
  - Veteran: grumpy and sarcastic.
  - Coach: friendly and explains things.
  - Expert: calm and precise.
  - Joker.
  - Quiet.
  - A custom description field.

  It talks in the game's language. Personality and level can be changed live from the Companion screen,
  or by telling it ("you're a pirate now").
- **Safety:**
  - Models are the official instruction-tuned ones. Uncensored models are never offered.
  - The system prompt keeps it inside Minecraft, with no real-world harmful instructions and no sexual
    content. It refuses in character.
  - **Language: Clean** is the default, and **Salty** allows mild swearing.
  - **Kid-friendly** softens the humour.
  - Nothing is recorded or sent anywhere on the local path.

### Multiplayer rules

- In version 1, the single-player player or the LAN host can have one companion. Guests see it, can talk
  to it, and it can help them, but it obeys its owner first.
- Every companion chat line is tagged. A guest with **No AI** checked doesn't see those lines. The
  companion still exists in the world, because the world is shared, but its name tag shows only the AI
  marker.
- A host with No AI never spawns one.
- Dedicated servers: later, behind a server setting that is off by default.

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
  - It runs at low priority: nice 10, or BELOW_NORMAL on Windows.
  - It uses `--threads` = physical cores − 1 (at least 1; 2 on a 2-core machine), `--parallel 1`, an
    8K context and prompt-cache reuse.
  - GPU layers are all of them on Apple Silicon or a discrete GPU with enough free VRAM. On an iGPU the
    model runs on the CPU, because the game needs that GPU.
  - It listens on 127.0.0.1 only, on a random port, with a random API key.

**The models.** All are open-weight and Apache 2.0, downloaded by the player straight from Hugging Face,
ungated and without an account, then checked against pinned SHA-256:

| Tier (plain label) | Model | File | Picked when |
|---|---|---|---|
| **Light**: for older PCs | Qwen3.5-2B Instruct, Q4_K_M (`unsloth/Qwen3.5-2B-GGUF`) | 1.28 GB | Fewer than the thresholds below; runs on the CPU. |
| **Standard**: most PCs | Qwen3.5-4B, Q4_K_M (`unsloth/Qwen3.5-4B-GGUF`) | 2.74 GB | ≥ 5 GB free VRAM, or Apple Silicon ≥ 16 GB, or ≥ 16 GB RAM and ≥ 6 cores |
| **Strong**: good graphics card | Qwen3.5-9B, Q4_K_M (`unsloth/Qwen3.5-9B-GGUF`) | 5.68 GB | ≥ 8 GB free VRAM, or Apple Silicon ≥ 24 GB |
| **Best**: needs a strong GPU | Gemma 4 12B instruct, Google's official QAT Q4_0 (`google/gemma-4-12B-it-qat-q4_0-gguf`) | 6.98 GB | ≥ 11 GB free VRAM, or Apple Silicon ≥ 32 GB |

- **How free VRAM is measured:** `llama-server --list-devices` after the runtime download, minus about
  1.5 GB for the game itself.
- **Why these models:**
  - Qwen3.5 has the best tool use per gigabyte at small sizes (BFCL-V4 66.1 for the 9B) and covers about
    200 languages.
  - Gemma 4 12B is the brain Chasm's video used, with strong role-play and multilingual quality. It is now
    Apache 2.0, and Google's QAT build keeps more quality at 4 bits.
  - The JSON schema makes even the 2B reliable at choosing actions.
- **Rejected:**
  - kev-4b: 14 GB of VRAM, English-only, a classifier.
  - Liquid LFM2.5: licence limited to companies under $10M revenue.
  - Uncensored fine-tunes.
  - Models gated behind a Hugging Face login.

**Voice (slice 6):**
- **Engine.** Voice runs in a second small sidecar, `horizon-voice`, built by our release workflow with
  sherpa-onnx (Apache 2.0). It is downloaded only when voice is turned on.
- **Speech to text** uses NVIDIA Parakeet TDT 0.6B v3 (CC-BY-4.0, 25 European languages, auto-detect),
  as int8 ONNX of about 0.6 GB. Silero VAD trims silence, and a push-to-talk key, V by default, controls
  the mic. The microphone is opened (and macOS asks for permission) only once voice is on.
- **Text to speech** uses Kokoro-82M v1.0 (Apache 2.0, ungated, about 0.3 GB, real-time on a CPU). Each
  personality gets a voice. It speaks en, fr, es, it, pt, ja, zh and hi; German is text-only until a
  licence-clean German voice is picked.
- **Playback.** The voice plays positionally from the companion's head through the game's mixer. Past 48
  blocks, or in another dimension, it comes over a "radio" (band-pass filter and static), with subtitles.
- **Rejected:**
  - Breeze-TTS-2: research and non-commercial licence, 12 GB NVIDIA only, English and Chinese.
  - Kyutai Pocket TTS: gated download. It could be an Advanced option later.

## Code layout

- `crates/horizon-companion`: no game dependency, unit-tested. It holds:
  - the catalogue (models, runtime builds, URLs, SHA-256, sizes, licences);
  - hardware detection and tier choice;
  - download and verify with resume and progress;
  - the runtime supervisor (start, stop, health, priority);
  - the OpenAI-compatible streaming client (`ureq`, server-sent events), for local, Ollama, LM Studio and
    cloud;
  - the prompt builder and the JSON schema;
  - the reflex parser and phrase banks;
  - presets.

  Both the game and the launcher use it.
- `crates/horizon-client/src/companion/`: the virtual guest, the skills, perception (the state summary),
  the chat hook, the Companion options section and persistence. The brain is called from a std-thread
  worker over channels, polled once per tick, never blocking (the `skins::Downloader` pattern).
- The launcher holds the Settings card (No AI, Set up companion, progress, Advanced) and passes
  `--no-ai` / extras to the game.

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
6. **Voice:** speech out (positional, radio), then push-to-talk in.
7. **Polish:** cost numbers on the ThinkPad and the Mac mini, demo recordings, licences in
   THIRD_PARTY_NOTICES and the launcher's Advanced section.
