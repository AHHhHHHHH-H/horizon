# Client protocol coverage

A successful join or zero `decode_errors` does not establish complete client support.
A packet must be decoded, applied to client state, and displayed or acted on. A typed
decoder alone proves none of the later steps.

For the contribution index, known limits, unfinished fixes and a wider audit
process, see [the client parity handoff](client-parity-handoff.md).

## Inspect a session

`horizon-ctl --socket PATH status` includes `server.protocol_gaps`:

- `unsupported_*_packet`: no typed decoder or raw handler applied the packet.
- `unhandled_*_packet`: a decoded packet reached a path without a handler.
- `undecodable_play_packet`: decoding failed; `decode_errors` also counts these failures.
- `unsupported_entity`: a spawn was tracked without a model or object/projectile handler.

The log warns once per category and registry/packet id per connection. The status keeps
counts for up to 128 distinct keys and counts excess events in `omitted_events`. No
packet bodies, chat, skin properties or player names are retained by these diagnostics.
Dimension changes do not reset the report; a new connection does.

These are omission counters, **not a completeness certificate**. They do not yet measure
unapplied entity metadata fields, partially interpreted item components, missing skin
downloads, or styling lost after decoding. An entity handler can exist but draw incorrectly.
Intentional non-visual entities (`interaction`, `marker`) are not missing renderers.
Optional non-brand plugin channels are not treated as missing vanilla features.

## Inventory decoders

```sh
cargo run -p horizon-protocol --example client_packet_audit
```

This emits every pinned clientbound login, configuration and play packet with its id
and `typed_decoder` flag. It is generated from the packet enum and vanilla id tables,
not a manually maintained support percentage. Command suggestions currently have a
raw handler: their lack of a typed decoder is not an unhandled feature.

The login, configuration and game-thread play matches are exhaustive. A newly modeled packet must get a handler
or an explicit omission arm; it cannot disappear into a blanket wildcard. Network-owned
packets, chunk-batch starts and optional custom payloads have documented no-op arms.

## Follow-up areas found in the October 2026 audit

- Server UI: boss events, display entities, dialogs and server links.
- Controls: forced camera/look direction, cooldowns, entity sounds and stopping sounds.
- Connection transitions: play-phase cookies and transfers.
- World updates: biome-only chunk updates, tags, minecart paths and projectile power.
- Client state: chat deletion/completions, recipe updates and advancement tab selection.
- Data presentation: structured item names, skin lifecycle and item-component interpretation.

Several have existing upstream PRs. Check those before starting overlapping work.
For each fix, compare vanilla's codec **and** client listener/render path, add an oracle
fixture, and test a real Rust client driven by a controlled Java server. Server-output
parity and Rust-to-Rust LAN tests alone cannot establish Java-server-to-Rust-client parity.
Keep captured game data and private session reports out of commits.
