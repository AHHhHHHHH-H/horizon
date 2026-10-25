//! Inventory the pinned clientbound packet names and their typed decoders.
//! `cargo run -p horizon-protocol --example client_packet_audit`
//! A raw handler (e.g. command suggestions) may exist without a typed decoder.
use horizon_protocol::{GAME_VERSION, PROTOCOL_VERSION, ids, packets};
use serde_json::json;

fn main() {
    let phases = [
        (
            "login",
            ids::login::clientbound::NAMES,
            packets::login::Clientbound::modeled_ids(),
        ),
        (
            "configuration",
            ids::configuration::clientbound::NAMES,
            packets::configuration::Clientbound::modeled_ids(),
        ),
        (
            "play",
            ids::play::clientbound::NAMES,
            packets::play::Clientbound::modeled_ids(),
        ),
    ];
    let phases: Vec<_> = phases
        .into_iter()
        .map(|(state, names, modeled)| {
            let packets: Vec<_> = names.iter().enumerate().map(|(id, name)| {
            json!({"id": id, "name": name, "typed_decoder": modeled.contains(&(id as i32))})
        }).collect();
            json!({"state": state, "packets": packets})
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "minecraft": GAME_VERSION, "protocol": PROTOCOL_VERSION, "phases": phases,
        }))
        .unwrap()
    );
}
