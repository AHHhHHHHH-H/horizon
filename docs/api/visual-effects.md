# Sandboxed client visual effects

These are Horizon APIs, not ports of other projects. They require a Horizon
build containing the corresponding API levels. Install and enable a `.hmod`
normally; a stock release that predates these functions cannot serve them.

## Permissions and ownership

Declare `render` (experimental). Shipping textures additionally needs API 0.2
content with `registries`; own the assets' namespace. `ui` and `storage` provide
mod-owned settings and persistent values. Effects start off unless a player
installs and enables a mod. Render permission explicitly includes local
lighting, visual filters and world appearance changes.

Nothing here sends server packets, changes saves, propagates block light or
reveals unloaded/server-only world data. Runtime errors/traps remove the owner
and its controls. Worlds/dimensions reset persistent visual controls. Assets
use the existing content resource-reload/restart lifecycle, not a new writable
texture interface.

## Contracts

| API | Level | Boundaries |
| --- | --- | --- |
| `emit-particles` | 8 | Vanilla particle behaviour, local only |
| `emit-configured-particles` | 14 | Vanilla sprite sets with explicit color, alpha, half-size, lifetime, gravity, drag, collision and brightness |
| `set-colored-lights` | 15 | Eight sources per owner; nearest sixteen loaded sources rendered globally |
| `emit-sprite-particles` | 16 | Configured physics with a static image in the caller's content namespace |
| `set-emissive-materials` | 17 | Explicit owned overlays and opt-in resource-pack suffix conventions |
| `set-post-effects` | 18 | Bounded built-in bloom and depth-aware radial light filters |

All three emission functions share the same 32-request/owner/tick and
256-request/global/tick quotas. At most the first 32 entries of a call are
examined. Configured and custom-sprite particles additionally share a live cap
of 256/owner and 1024 globally, live at most 200 ticks, and disappear on owner
stop/reload. Spawns must be within 32 blocks and loaded chunks. Player particle
settings still apply. Queued count is not a promise that a particle will draw.

Custom images live at `assets/<namespace>/textures/particle/<path>.png` and are
addressed as `<namespace>:particle/<path>`. They are static PNGs, at most 64x64
and 1 MiB compressed; atlas admission is limited to 64/namespace, 256 total.
Foreign namespaces, traversal and missing images cannot be used by emission.

Colored lighting is additive, camera-local and unshadowed with linear radius
falloff. Radius is 0.25..16 blocks, intensity 0..1, RGB 0xRRGGBB. Positions must
be finite and within 32 blocks when set. It affects normal terrain and lit
entity rendering, not the world's gameplay light values.

Emissive mappings address base texture ids and owned `emissive/` overlays.
There are at most 32 explicit entries per owner and 128 resolved mappings
globally. First owner wins; explicit entries precede suffix matches. The normal
terrain shader draws overlays full-bright but still fogged. It does not alter
item/entity materials or emit light into nearby blocks. Texture names sharing
a deduplicated layer share its mapping. Overlay images have the same 64x64 /
1 MiB limits; at most 256 additional layers are admitted. Opt-in suffix mode
checks `_e`, `_emissive`, and a safe `suffix.emissive` property in
`assets/minecraft/optifine/emissive.properties` (at most 4 KiB).

Post-effects read the scaled world's color and depth, never the hand or HUD.
Each owner may replace two effects; the first four globally in stable
mod/list order are composited additively. Bloom takes eight samples; radial
filters take sixteen, with depth excluding non-sky samples. Strength/threshold
are 0..1; bloom radius is 0.5..8 target pixels. Godray light coordinates are
normalized viewport positions, not world coordinates. FXAA remains the final
world compositing step. These are bounded built-ins, **not arbitrary WGSL,
user-defined passes or a general shader graph**. They pause while a shader
pack owns post-processing. Shader packs replacing terrain/entity materials
may also replace the lighting/emissive behaviour. Disabling all post owners
releases the extra render target.

## Example and verification

`crates/horizon-mod-sdk/examples/effect-lab` is an original demonstration with
original PNGs, a persistent settings toggle and `/effect_lab on|off`. It is
not any of the six requested mod adaptations. Its `.hmod` is built by the
real-WebAssembly sandbox fixture (or `cargo horizon-mod build`).

Focused tests cover validation, quota sharing, ownership, dimension cleanup,
packaged assets, shader validation, and GPU filter readback/composition/depth.
The optional native fixture drives both single-player paths and a TCP guest:

```sh
python3 tools/visual_mod_harness.py \
  --client target/fast/horizon-client --ctl target/fast/horizon-ctl \
  --resources .local/horizon-client/26.3 \
  --hmod target/mods-test/mods/effect_lab.hmod \
  --output /tmp/opencode/visual-api-fixture
```

It creates scratch game/config directories, captures screenshots and frame
timings, and checks effect activation, clearing, failed reload and post-target
release. No native fixture is considered passed just because its code exists;
actual run results belong in the progress checklist and PR validation report.
