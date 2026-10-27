# Visual-effect mod API implementation status

Tracking issue: https://github.com/ludovic111/horizon/issues/2884
Implementation PR: https://github.com/ludovic111/horizon/pull/2888

This is an implementation checkpoint, not a claim that all six requested mods
are ported or that every API is complete. No custom client was installed.

## Implemented in this branch

- [x] API 8: bounded local vanilla particles, quotas and failed-owner cleanup.
- [x] API 9: loaded client block states, fluids, biomes and light sampling.
- [x] API 10: permission-gated positional audio, validation and quotas.
- [x] API 11: optional atmosphere fields, transitions and owner/dimension cleanup.
- [x] API 12: bounded nearby entity snapshots for the documented client types.
- [x] API 13: bounded queues of locally observed gameplay events.
- [x] API 14: owner-scoped configurable particles using vanilla sprite sets,
  explicit color/size/lifetime/physics and live caps (256/owner, 1024 total).
  Pending and live particles are removed on owner failure, disable or reload.
- [x] API 15: bounded colored-light owners and normal terrain/entity shaders.
- [x] API 16: owned static particle images and bounded packaged-asset admission.
- [x] API 17: owned terrain emissive overlays and resource-pack suffix conventions.
- [x] API 18: owner-scoped bounded bloom and depth-aware radial filters,
  additive composition, world color/depth inputs and target cleanup.
- [x] SDK bindings, API-level detection and generated reference for these functions.
- [x] Real WebAssembly sandbox regression tests for these functions.

## Existing infrastructure to reuse

- Mod-owned screens/settings widgets (`ui`) and persistent values (`storage`).
- API 0.2 packaged assets (`registries`) and namespace/path/size validation.
- Native WGSL shader-pack validation and scene/depth/frame inputs.

These are existing facilities. API 18 uses separate bounded built-in filters;
it does not grant sandboxed owners arbitrary shader-pack programs.

## Contract and validation boundaries

The implemented contracts and caps are in [visual-effects.md](visual-effects.md).
These are bounded client APIs, not an unrestricted shader framework:

- Static custom particle images, not arbitrary animated particle scripts.
- Terrain emissive overlays, not entity/item material replacement.
- Camera-local unshadowed colored illumination, not world light propagation.
- Built-in bloom/radial filters, not arbitrary WGSL or user-defined shader graphs.
- Post filters yield while a shader pack owns post-processing.

- [x] Original Effect Lab example, packaged with assets and persistent settings.
- [x] Native integration fixture code for in-frame, server-thread and TCP guests.
- [x] GPU readback test for filter composition, depth and resource cleanup.
- [ ] Run the final native fixtures and CPU/GPU profiling on the final build.
- [ ] Final full repository checks after the API 18 changes.
- [ ] Six complete mod/resource-pack adaptations (separate work; none claimed complete).

## Validation checkpoint

At the API 14 checkpoint:

- Real WebAssembly sandbox suite: 40 passed, 1 pre-existing ignored.
- Generated API-reference checks: 4 passed.
- Client particle unit tests with mods enabled: 14 passed.
- Client binary check with mods enabled: passed.
- Clippy, all targets of client/runtime/SDK/developer tool with mods: passed.
- Native visual, collision/lifetime integration and GPU profiling checks:
  not run. No claim of visual equivalence or complete custom-particle support.

The focused sandbox suite at the event-API checkpoint passed 39 tests, with one
pre-existing ignored test. Each new API also passed its focused sandbox test and
the client compiled with `mods` enabled. The generated API reference passed.
The broader repository self-check is recorded separately in the PR; do not reuse
the initial particle-only PR's old passing report for this expanded branch.

Run final validation from the branch root:

```sh
python3 tools/pr_ready.py --base origin/main --quick
```

All game/launcher integration runs must use scratch player/config directories as
specified in AGENTS.md. The user's live worlds and installed binaries are not test
fixtures. Original mod source/assets must not be copied into this implementation.
