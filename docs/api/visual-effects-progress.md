# Visual-effect mod API implementation status

Tracking issue: https://github.com/ludovic111/horizon/issues/2884
Implementation PR: https://github.com/ludovic111/horizon/pull/2888

This tracks the bounded API contracts, not six completed mod ports. All validation
runs use scratch homes; a separate opt-in local test installation is not used as
a fixture. Mod adaptations are separate from this API PR's merge criteria.

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
- [x] Run native fixtures and capture CPU/GPU timings in-frame, server-thread and over TCP.
- [x] Repository self-check after the API 18 changes (`pr_ready --quick`); CI parity remains a separate gate.

The separate original Emissive Ores mod has been built and user-tested. The other
five adaptations remain separate work; they do not block review of these APIs.

## Validation checkpoint

After merging main at `b6c936f82`, with the control-chat routing fix:

- Real WebAssembly sandbox suite: 44 passed, 1 pre-existing ignored.
- Generated API-reference checks: 4 passed.
- Client library tests with mods: 1110 passed, 33 ignored. Includes GPU readback
  for filter composition/depth/cleanup and shader globals/HLSL validation.
- Python suite: 401 passed, including the additional fixture regression tests.
- Native Effect Lab fixtures: in-frame, server-thread and TCP guest passed effect
  admission, disable cleanup and failed-hot-reload cleanup. On/off PNGs captured.
- CPU/GPU timestamp summaries recorded for each mode. These are one-second smoke
  profiles on one Linux integrated GPU, not a statistical performance guarantee.
  Post-processing targets used 1,509,120 bytes in these runs and returned to zero
  on disable and failed reload. Built-in effects have an opt-in CPU/GPU cost.
- Full workspace clippy (mods enabled and no-default-features server): passed.
- Clean-cache repository self-check passed: 2355 workspace tests, 2 doc tests,
  5 full content-root checks and 204 launcher tests; formatting, workspace/server
  Clippy and launcher Clippy passed too. Relocated debug artifacts were rebuilt
  because stale pre-move manifest paths affected preparation/release-signing
  tests; no test assertions were changed or skipped to obtain this pass.
- `--quick` does not execute the 348 affected vanilla parity harnesses locally.
  Upstream CI/review gates remain applicable. Windows/macOS interactive runs and
  broad performance guarantees are not claimed. See the PR for the full report.

The fixture exposed a control-socket chat bug: remote chat bypassed client-mod
commands. It now uses the same routing as normal player chat. TCP guests use the
established `connect` flow with a distinct guest name. Fixture processes inherit a
scratch working directory, and failures retain their command output and logs.

Run final validation from the branch root:

```sh
python3 tools/pr_ready.py --base origin/main --quick
```

All game/launcher integration runs must use scratch player/config directories as
specified in AGENTS.md. The user's live worlds and installed binaries are not test
fixtures. Original mod source/assets must not be copied into this implementation.
