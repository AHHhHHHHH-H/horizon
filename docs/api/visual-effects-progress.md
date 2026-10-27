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
- [x] SDK bindings, API-level detection and generated reference for these functions.
- [x] Real WebAssembly sandbox regression tests for these functions.

## Existing infrastructure to reuse

- Mod-owned screens/settings widgets (`ui`) and persistent values (`storage`).
- API 0.2 packaged assets (`registries`) and namespace/path/size validation.
- Native WGSL shader-pack validation and scene/depth/frame inputs.

These are existing facilities. Wiring shader packs to sandboxed mod owners and
proving the full asset/settings lifecycle for the requested effects is still work.

## Not implemented yet

- [ ] Custom particle sprite/physics/lifetime declarations and their asset lifecycle.
- [ ] Emissive overlay/material registration and resource-pack suffix conventions.
- [ ] Bounded colored-light sources and renderer integration.
- [ ] Mod-owned post-processing chains, scene/depth inputs and cleanup/composition.
- [ ] Complete effect examples packaged as `.hmod`, including settings/assets.
- [ ] Visual integration fixtures for in-frame, server-thread and multiplayer clients.
- [ ] CPU/GPU profiling and resource-lifetime verification for the remaining effects.
- [ ] Six complete mod/resource-pack adaptations (none is claimed complete).

## Validation checkpoint

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
