# Client parity contribution and follow-up plan

This is a maintainer handoff, not a claim of complete multiplayer parity.
The work below was developed against pinned Java Edition 26.3. Tests cover the
listed cases; they do not prove every server, metadata value or render path.
Recheck current main and open PRs before continuing: upstream changes quickly.

## Existing contributions

- [#1696](https://github.com/ludovic111/horizon/pull/1696): bounded omission
  diagnostics and a generated clientbound decoder inventory. It reports missing
  handling; it does not implement the reported features.
- [#1702](https://github.com/ludovic111/horizon/pull/1702): structured custom item
  names in saves, tooltips and held-item labels. Thirteen native Java/Rust name
  and tooltip fixtures matched. This is not all item-component presentation.
- [#1718](https://github.com/ludovic111/horizon/pull/1718): text-display metadata.
  The native entity accessors and packet codec supplied 513 comparison cases.
- [#1725](https://github.com/ludovic111/horizon/pull/1725): text-display rendering;
  depends on #1718. There were 108 native client-renderer matrix comparisons
  (absolute float tolerance 0.00002), an interpolation regression test and a
  real Java-server/Rust-client rendering fixture. Block/item displays, entity
  glow outlines and nonzero entity ground-shadow settings are not covered.
- [#1729](https://github.com/ludovic111/horizon/pull/1729): stop-sound decoding and
  dispatch to the existing sound engine. All 36 native source/event selections
  consumed and re-encoded exactly, including UI source 10. This was not an
  audible hardware-output test; agent game runs have audio disabled.
- [#1731](https://github.com/ludovic111/horizon/pull/1731): remote living-entity
  attribute snapshots, scale consumers and simulation-distance state. A Java
  server delivered pig scale 2, maximum health 40 and simulation distance 3.
  Player scale 1.5 was checked in both single-player paths. The shared attribute
  evaluator also passed 1,404,798 output-line comparisons against each Java jar;
  that count is evaluator coverage, not new end-to-end consumer coverage.

Existing upstream work was reused, not reimplemented: inventory component
decoding [#1631](https://github.com/ludovic111/horizon/pull/1631), skin handling
[#1652](https://github.com/ludovic111/horizon/pull/1652), and boss overlays
[#1590](https://github.com/ludovic111/horizon/pull/1590).

### Before accepting the contributions

Review dependencies and current CI, rebase where needed, and test the assembled
candidate. The later patches were tested separately, not installed as one final
public-server client. Some PRs remain drafts. Local stock Clippy reported
`boxed_local` in untouched `streaming.rs`; an exception used for local targeted
checks is not a clean stock-check result. The normal client-library harness was
also blocked by a missing pinned jtracy native jar; reported CPU comparisons
used verified game jars and available dependencies. Resolve or report these
conditions rather than turning exceptions into a full-pass claim.

## Unfinished implementation, in importance order

### 1. Server tags and enabled features

The protocol already types `UpdateTags` and `UpdateEnabledFeatures`, but the
client paths audited here did not apply them. Implement this in small slices:

1. Preserve registry bindings and replacement/clearing semantics in each
   connection, including configuration, play and reconfiguration updates.
   Resolve numeric ids through the correct builtin or dynamic registry.
2. Route item tag membership and ordering to actual item/menu/recipe consumers.
   Route block tags to client prediction, including climbable and fall-damage
   reset checks. `Items` contains item/block tags; `MotionResources` contains
   climbable/fall-reset sets. Audit their callers, not just the packet handler.
3. Keep session overrides separate from `Resources`, which are immutable and
   shared with an integrated server. Test disconnect, dimension change and
   reconnect so one server's tags cannot leak into another session or a local
   world. Do not mutate shared defaults to make one fixture pass.
4. Inspect native feature-name validation and each item's/block's actual feature
   requirements. Wire creative/menu availability and affected behaviour to the
   negotiated set. Test absent, unknown and replaced flags. Storing a string
   list alone does not implement feature flags or experimental gameplay.

Use a Java-server datapack fixture that changes tag membership and removes a
previous tag. Check the resulting client behaviour. Retain diagnostics for any
registry or consumer not yet supported; do not declare all tags implemented
after fixing two named tags.

### 2. Server recipe properties and stonecutter data

`lan/recipes.rs::update_recipes` already writes the raw packet. It contains
property-set names with item-id arrays, then selectable stonecutter recipes.
`packets/recipes.rs` already has `HolderSet` and `SlotDisplay` codecs.

1. Add a typed `ClientboundUpdateRecipesPacket` codec and compare it with native
   bytes: empty sets, tag/list ingredients, results with components, malformed
   counts and trailing bytes. This is only the decoder milestone.
2. Apply the server's data to per-session menu/recipe state. `menu::Env` currently
   receives item and station-recipe references; do not keep using prepared
   vanilla defaults when the server supplied different data.
3. Preserve server ordering and selection indices. Test a datapack with changed
   stonecutter results/order and recipe properties, then select a result from a
   real Rust client and check the Java server's resulting inventory.
4. Cover replacement while a menu is open, reconnect and tag-dependent results.
   Keep this distinct from recipe-book add/remove packets; decoding one does
   not implement the other.

### 3. Server links and dialogs

Inspect overlapping [#1720](https://github.com/ludovic111/horizon/pull/1720)
before starting UI work; the delegated investigation found that overlap but
produced no patch. Verify its current scope and status first.

Use `ClientboundServerLinksPacket` and `ServerLinks.UNTRUSTED_LINKS_STREAM_CODEC`
as references. Cover configuration and play, empty/replacement updates, every
known link type, structured custom labels and invalid input. Do not guess the
`Either` discriminator. Add a usable list and explicit URL confirmation, not
only storage or a status field. Accept safe schemes only, never launch on
receipt, and never put server text into a shell command. Tests must not open a
real browser or visit a URL. A focused links screen is not full dialog support.

## Find gaps beyond the reported examples

Maintain three separate inventories rather than one support percentage:

- **Packets:** every pinned id, typed or raw decoding, network/game-thread
  owner, state effect, presentation effect, intentional no-op or missing work,
  and the native/end-to-end test that covers it.
- **Entity types and metadata:** every spawn type and serializer, inherited
  metadata indices, partial updates, actual field consumers, model/texture
  availability, movement, removal and dimension/reset behaviour.
- **Item components and text:** every component's exact wire consumption,
  retained data, client interpretation, save/click encoding and presentation.
  Read-past/raw round trips are not proof of gameplay or display support.

After #1696, use `client_packet_audit` and `server.protocol_gaps` as inputs to
these inventories. They do not measure all metadata fields, skin failures or
lost styling. Exhaustive Rust enum matches do not detect a missing field
consumer or a newly registered packet absent from the enum. Add a reviewed
classification check against the pinned id table, not a source-code search for
an arm that happens to contain a variant name.

Further areas found by the audit include biome-only chunk updates, forced
camera/look direction, cooldowns, entity-bound sounds, minecart paths,
projectile power, chat deletion/completions, advancement tab selection, cookies,
transfers and dialogs. These are investigation targets, not assertions that
current main still lacks every one. Search existing contributions first.

Exercise the boundaries that hide whole classes of failures:

- Join, login/configuration/play transitions, reconfiguration, respawn,
  dimensions, disconnect and reconnect; include reused entity ids.
- A supported update after an unsupported or malformed update; prove correct
  frame consumption. Item components without length prefixes cannot be skipped
  by guessing their size: implement the exact codec or reject explicitly.
- Empty updates that clear old state, partial updates that must retain it,
  repeated updates, capacity limits and diagnostic overflow/deduplication.
- Skin failures/eviction, structured text inheritance and explicit false
  styles, non-ASCII text, unknown metadata and components, and multiple players.

## Acceptance process for each small fix

1. Record the exact pinned Java class/method and intended observable effect.
2. Add a native oracle that invokes that implementation, not a second copy of
   the proposed Rust algorithm. Compare decoded meaning as well as bytes.
   A Rust reader/writer round trip alone proves only internal consistency.
3. Add boundary, clearing, replacement and lifecycle regression cases.
4. Drive a real Rust client from a controlled Java server and assert the effect:
   rendered glyphs, changed menu results, updated dimensions or stopped sound
   instances. A decoded-packet count or tracked entity count is insufficient.
5. For gameplay changes, test the applicable in-frame/server-thread and LAN
   paths too. Server-output parity and Rust-to-Rust LAN tests cannot substitute
   for Java-server-to-Rust-client checks.
6. Run `pr_ready.py` against the intended base and relevant parity harnesses.
   Record head SHA, resources/version, skipped checks, failures and tolerances.
   Compare screenshots at the same position, item and UI scale.

Keep patches small and publish a ledger of what is verified, partial and still
unsupported. Do not silence a diagnostic merely because a decoder was added.
Keep private captures, chat, profile properties, account data, device names and
game artifacts out of commits and public reports. Scrub machine names from
generated check summaries before posting them. Follow the repository's fresh
test-home and agent-attribution rules.
