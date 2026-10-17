# Collision shape identity indexing: validation and measurements

Compared `f535d1b4` (original) with `7a4a15a8` (indexed; implementation introduced
at `56b5a87a`) using the same 26.3 client data. The benchmarked operation is
the shape deduplication inside `Behaviours::load`, invoked by the native client
and by the server/world loader. No changes were made to resource exports, shape
geometry, dynamic fallback (`-1`), state order, or mod/resource-pack loading.

## Identity and compatibility contract

`VoxelShape::same_identity` is `Arc::ptr_eq` on the private `ShapeIdentity`.
The new `identity_key` returns that allocation address while the registry and
interned clones keep it alive. Different shape objects can have identical
coordinates, occupancy and shared boxes but **must retain different IDs**.
The collision decoder deliberately uses `fresh_identity` when reusing equal
grid storage, and recognizes canonical singleton empty/block identities.
Hash lookup maintains the exact first-seen order of the old linear scan.
Uncached/dynamic states retain shape ID `-1`. Pack-derived data uses the same
loader and the same exact identity relation; there is no hash of geometric data
and no normalization of user-supplied shapes. These invariants are exercised
by the collision importer and indexed-vs-linear regression tests.

## Focused deduplication benchmark (existing result)

With 35,723 block states, 6,093 distinct identities and twelve samples per
algorithm, the warm-process collision-registry benchmark reported **93.326 ms**
median for linear search and **0.992 ms** for hash indexing (approximately
94 times faster for the deduplication loop). `collision_shape_dedup_bench`
asserts that all state IDs from the two implementations match. This measures
the inner loop alone, **not** resource loading or client startup.

Reproduce:

```sh
CARGO_TARGET_DIR=/path/to/dedup-target cargo run --offline --profile fast \
  -p horizon-world --example collision_shape_dedup_bench -- /path/to/26.3
```

## Paired loader-process measurements

The original requested `tools/profile_client_startup.py` did not exist in this
revision. It was implemented and then extended to use separately prebuilt
binaries and alternating order, with no build, checksum or resource hashing in
the timed process launches. Build from the two clean worktrees:

```sh
git worktree add --detach .worktrees/shape-identity-baseline-20261017 f535d1b4
cd .worktrees/shape-identity-baseline-20261017
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/path/to/baseline-target \
  cargo build --offline --profile fast -p horizon-world --example level_shape_fixture

cd ../shape-identity-index-20261017
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/path/to/optimized-target \
  cargo build --offline --profile fast -p horizon-world --example level_shape_fixture
python3 tools/profile_client_startup.py --resources /path/to/26.3 \
  --baseline-binary /path/to/baseline-target/fast/examples/level_shape_fixture \
  --optimized-binary /path/to/optimized-target/fast/examples/level_shape_fixture \
  --warmups 3 --runs 15 --output benchmarks/shape-dedup-2026-10-17/paired-isolated.json
```

The reference machine was Linux 7.2.8-zen1-2-zen, AMD Ryzen 5 5600X
(12 logical CPUs), Python 3.14.7. The full SHA-256 hashes for both binaries
and the five input resources, order of execution, 15 individual timings per
revision, three warmups each, load average per pair, and exact UTC measurement
window are retained in [`paired-isolated.json`](paired-isolated.json).

| Measurement (ms) | Original | Indexed |
| --- | ---: | ---: |
| Minimum | 1,318.510 | 1,265.935 |
| Median | 1,608.751 | 1,560.048 |
| Mean | 2,099.426 | 1,939.263 |
| Maximum | 3,152.716 | 3,045.165 |
| Standard deviation | 769.168 | 717.266 |

Median of paired indexed-minus-original differences: **-58.685 ms**.
Machine one-minute load increased from **7.8** to **14.33** during the run;
both series became much slower in the latter half. External compilation and
other desktop workloads could not be completely controlled, so **these figures
do not establish a reliable full-loader or client startup improvement**.
The earlier unpaired medians (1,517.992 ms original and 1,972.990 ms indexed)
were also confounded by concurrent compiler jobs and are retained only as
historical observations in `before.json` and `after.json`.

Even this paired test includes process creation, JSON reading, collision-NBT
parsing, geometry analysis and other behaviour construction. It does not run
the renderer or measure full native-client time-to-title. An idle host with
controlled CPU scheduling and additional repeated sessions would be needed
for a defensible whole-loader estimate.

## Validation

The following completed successfully on the optimised worktree, with separate
Cargo targets to avoid build directory conflicts:

```sh
CARGO_TARGET_DIR=/path/to/validation-target cargo test --offline -p horizon-world --lib --tests
CARGO_TARGET_DIR=/path/to/validation-target cargo test --offline -p horizon-core --lib
cargo fmt --all --check
python3 -m py_compile tools/profile_client_startup.py
git diff --check
```

`horizon-world`: 89 library tests and all integration suites passed, including
the six `collision_data` tests. `horizon-core`: 84 library tests passed. The
loader ran successfully against the pinned export on both revisions. Canonical
and legacy collision identity cases, repeated alias lookups, equal grids with
separate identities, and first-seen ID ordering were checked. Dynamic/modded
resources outside the pinned export have not been run through a live client.
