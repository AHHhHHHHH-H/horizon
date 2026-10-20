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

### Reflexes: the nervous system (instant)

Ludovic asked for a classification model, like Kev-4B in the video. Every line the companion hears goes
through three stages, and the first one that is sure wins:

1. **Rules (under 1 ms).** Commands are parsed in English, French, Spanish, German and Portuguese. Item
   names come from the game's language files. The commands are:
   - follow, stay / wait here, come here, stop, go there;
   - attack, defend me, hold fire, fire, jump, crouch, stand up;
   - get / collect <n> <item>, give me <n> <item>, eat, sleep, craft <item>.

   Negations count: "don't follow me" and "ne me suis pas" are not "follow".
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
    so `tone` is only a playback gain, but it clones a voice from a clip, so it speaks as the cast
    too (below). Its built-in voices: 22 per language (23 in French, with daan), from VCTK
    (CC-BY-4.0), LibriVox (public domain), Common Voice, Kyutai's voice donations and recordings
    (CC0) and Alba MacKenna (CC-BY-4.0). Left out: cosette (Expresso) and jean (EARS), which are
    CC-BY-NC, and juergen and rafael, whose recordings have no stated source. All of them make a pack
    of 269 MB per language.
  - **Expressive voice (GPU, "Best quality"): Qwen3-TTS-12Hz-1.7B-CustomVoice.** Apache 2.0, ungated,
    Q8 GGUF of 2.8 GB, ten languages including the five above, nine built-in voices. It takes a
    natural-language instruction for each line ("Shouting loudly, urgent and intense.", "Whispering
    very softly…", "Dry and sarcastic…"). On the M4 Pro's GPU it starts speaking in 60–90 ms and runs
    2.2× faster than real time, with 6 GB resident. It evens out loudness (a shout and a whisper came
    out within 0.2 dB RMS), so the instruction changes the delivery (the whisper is less voiced, the
    shout lower and tenser) and the game's tone gain makes it loud or quiet, as for Pocket TTS. There
    is no 0.6B CustomVoice GGUF in audio.cpp (only the 0.6B Base cloning model). Chasm's own project
    uses Qwen3-TTS as well.
  - **The voice cast (Ludovic 2026-10-20: several voices to choose from).** Twelve designed characters,
    six women and six men, `crates/horizon-senses/src/cast.rs`. Each personality has its default
    (Veteran, Coach, Expert, Joker, Quiet one), and any character can be picked for any personality:
    Veteran (gruff, deep, battle-worn), Coach (cheerful, warm), Expert (calm, precise), Joker
    (cocky, cheeky), Quiet one (soft-spoken, shy), Old captain (raspy sea dog), Speedrunner (hyped
    streamer), Nana (warm older woman), Teen (sarcastic, bored), Knight (deep, stoic), Sunny
    (bright, bubbly), Butler (dry, refined, British).
    - **How they are made.** Qwen3-TTS 1.7B VoiceDesign (Apache-2.0) turns a written description
      and a seed into a voice, so no real person is recorded or cloned. One English neutral take is
      the character's anchor (the take closest to the character's other takes). Every other clip
      (shout, whisper, laugh, sarcastic; English and French; 5–8 s each, with an exact transcript) is
      the best of two VoiceDesign takes and one VoxCPM2 clone of the anchor with a style prefix. The
      best is the one most similar to the anchor (WeSpeaker speaker embeddings) whose Parakeet
      transcript matches (≤12% word errors, 30% for laughs) and that sounds like its emotion: a
      shout at least 2 dB louder or 25% higher, a whisper at least 40% less voiced or 4 dB quieter.
      Redesigning each emotion drifts (seeds of one description scored 0.54–0.67 against each
      other); cloning the anchor holds it better. 8 of the 120 clips (seven whispers, one shout) had
      no take that passed every check and are marked `checked: false` in the pack's manifest.
      Tools: `tools/companion_voices/`.
    - **At run time** a cloning model gets, for each line, the clip of its tone (`Emotion::for_tone`:
      shout and scared → shout, calm and free-form → neutral), so the delivery comes from the clip
      and the timbre stays the character's. Clips are selected by name from audio.cpp's `voice_dir`
      (`hz-<character>-<language>-<emotion>`, transcripts in its `prompt_text`). A clip is encoded
      the first time a line uses it (0.7 s on the M4 Pro), then kept: `SpeechClient::prepare`
      encodes a character's clips when the voice is chosen, and the server keeps 10 prepared.
    - **One clip per language is needed.** French lines cloned from the English clips had 32% word
      errors (an English accent and wrong words); from the French clips, 1%. Languages without clips
      (es, de, pt, it…) use the English neutral clip in `x_vector_only_mode` (speaker embedding
      only): 0–8% errors and the same voice, but a flat delivery, with the tone gain only. Clips in
      those languages come next (60 per language).
    - **The voice pack.** The clips are generated by us and small: 120 WAVs (24 kHz mono 16-bit),
      47 MB, plus `prompt_text` and `manifest.json` (what made each clip, its seed and similarity).
      They are not in the repository. They ship as one archive (`cast::VOICE_PACK`), hosted with the
      release that turns the cast on and pinned by SHA-256 like the models, downloaded with the
      expressive or the natural voice into `<data>/companion/voices/v1/`. It is built at
      `~/builds/horizon-companion-voices/pack` on the Mac mini until then.
  - **Engines per tier** (Mac mini M4 Pro; 12 characters × the same 10 short lines, 6 English and
    4 French; streamed; all GPU runs shared the GPU with a Gemma 12B benchmark of another session,
    so their speeds are pessimistic and only comparable with each other):

    | | consistency (similarity to the anchor) | word errors | shout vs neutral | whisper voicing | first audio (clip prepared / new) | memory |
    |---|---|---|---|---|---|---|
    | Qwen3-TTS 1.7B Base + clips | 0.69 (worst line 0.53) | 0.9% | +3.6 dB | 0.81× | 0.27–0.33 s / 0.87 s | 3.8 GB (peak 4.3) |
    | Qwen3-TTS 0.6B Base + clips | 0.70 (0.52) | 2.3% (fr 4.7%) | +3.9 dB | 0.75× | 0.23–0.24 s / 0.82 s | 2.8 GB (peak 3.3) |
    | VoiceDesign per line (4 characters) | 0.67 (0.48) | 3.9% (fr 7.4%) | +3.6 dB | 1.15× (not whispered) | 0.19 s | 3.0 GB (peak 3.5) |
    | VoxCPM2 clone + style (4 characters) | 0.48 (0.43) | 30% | +3.3 dB | 1.14× | 0.48 s | 3.5 GB (peak 4.8) |
    | Pocket TTS + clips (CPU, 2 threads) | 0.63 (0.45) | 2.2% | +4.9 dB | 0.77× | 0.04 s / 0.7 s | 0.33–0.38 GB (peak 0.76) |

    On the same 4 characters, 1.7B Base scored 0.685 and VoiceDesign-per-line 0.669, with VoiceDesign
    drifting more on its worst lines and never whispering. `mem_saver` saved 165 MB of the 1.7B's
    footprint, not its peak, and changed no latency: left off. Speeds while the GPU was shared:
    real-time factor 1.47 (1.7B), 1.35 (0.6B), 1.31 (VoiceDesign), 0.95 (VoxCPM2).
    **With the GPU to itself** (3 characters × the 10 lines, clips prepared first): 1.7B Base starts
    speaking in 0.16 s (at most 0.19 s) and runs 2.1× faster than real time (RTF 0.49), 3.8 GB
    (peak 4.5 GB); 0.6B Base 0.13 s and RTF 0.43, 2.8 GB (peak 3.4 GB); VoiceDesign per line
    0.09 s and RTF 0.49, 3.6 GB. Preparing a clip takes 1.2 s, so a character's 5 clips in one
    language are ready 6 s after the voice is chosen. On the M4 Pro the 0.6B is barely faster
    (generation is bound by its per-step latency): its gain is the 1 GB of memory.
    - **High end (an RTX 4080 class card): Qwen3-TTS 1.7B Base with the clips.** The fewest errors,
      the emotions of the clips, steady across lines.
    - **Mid-range GPUs: Qwen3-TTS 0.6B Base with the same clips.** As consistent, 1 GB less, a few
      more errors in French.
    - **Processor: Pocket TTS**, cloning the same clips (each pack speaks its language, so English
      and French have the cast's own clips) or one of its built-in voices.
    - Rejected for run time: VoiceDesign per line (drifts, no whisper; it stays the design tool) and
      VoxCPM2 (inconsistent and 30% errors when streamed, which forbids its retry of bad takes).
    - CustomVoice stays in the catalogue for its instruction control, but its presets are Chinese,
      Japanese or Korean apart from two English men.
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
  "Standard brain on your RTX 3060, natural voice on the processor: 3.4 GB to download".
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
  - the catalogue (models, runtime builds, URLs, SHA-256, sizes, licences);
  - hardware detection and tier choice;
  - download and verify with resume and progress;
  - the runtime supervisor (start, stop, health, priority);
  - the OpenAI-compatible streaming client (`ureq`, server-sent events), for local, Ollama, LM Studio and
    cloud;
  - the prompt builder and the JSON schema;
  - the reflex pipeline (rules → classifier → brain) and phrase banks;
  - `plan()` and `CompanionSettings` (simple and Advanced);
  - presets.

  Both the game and the launcher use it.
- `crates/horizon-senses`: no game dependency either. The senses' catalogue (audio.cpp builds, speech
  to text, voices, classifier models, with their needs and measured latency; the same `ModelFile`
  shape as the brain's catalogue, so `plan()` can take them as more parts), the sidecar supervisor,
  the streaming voice and push-to-talk clients, the voice cast (`cast.rs`: characters, clips, the
  tone → clip choice, the voice pack), the classifier and its example bank, the radio and the
  resampler. `examples/senses_bench.rs` measures all of it (`--voices <pack> --cast <id>` for the
  cast). `tools/companion_voices/` designs the cast and builds the voice pack (dev only). It is a crate of its own, not part of
  `horizon-companion`, so the senses and the brain were built side by side; the classifier runs on the
  brain's pinned llama.cpp (the same b11756).
- `crates/horizon-client/src/companion_voice.rs` (the microphone, push-to-talk, the speakers) and
  `game/companion_voice.rs` (the key, the mic icon, routing what was heard); the mixer's voice streams
  are in `audio.rs`. `HORIZON_COMPANION_VOICE=<file.json>` turns voice on without the setup screens.
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
6. **Senses:** the classifier model, voice out (Pocket / Qwen3-TTS through audio.cpp, positional and
   radio) and push-to-talk in (Parakeet v3). This is part of the first version.
7. **Polish:** cost numbers on the ThinkPad and the Mac mini, demo recordings, licences in
   THIRD_PARTY_NOTICES and the launcher's Advanced section.
