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
  Physics/lifetime controls are implemented by API 14; custom sprite images and
  packaged-asset lifecycle remain unimplemented. This item is not complete.
- [ ] Emissive overlay/material registration and resource-pack suffix conventions.
- [ ] Bounded colored-light sources and renderer integration.
- [ ] Mod-owned post-processing chains, scene/depth inputs and cleanup/composition.
- [ ] Complete effect examples packaged as `.hmod`, including settings/assets.
- [ ] Visual integration fixtures for in-frame, server-thread and multiplayer clients.
- [ ] CPU/GPU profiling and resource-lifetime verification for the remaining effects.
- [ ] Six complete mod/resource-pack adaptations (none is claimed complete).

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
