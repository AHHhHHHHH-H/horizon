//! Bounded, payload-free diagnostics for server features the client cannot apply.
//! These counters describe omissions, not successful protocol coverage.
use horizon_protocol::{Direction, State, ids};
use std::collections::BTreeMap;

const LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gap {
    LoginPacket(i32),
    ConfigurationPacket(i32),
    UnhandledConfigurationPacket(i32),
    PlayPacket(i32),
    UndecodablePlayPacket(i32),
    UnhandledPlayPacket(i32),
    Entity(i32),
}

impl Gap {
    pub fn id(self) -> i32 {
        match self {
            Self::LoginPacket(id)
            | Self::ConfigurationPacket(id)
            | Self::UnhandledConfigurationPacket(id)
            | Self::PlayPacket(id)
            | Self::UndecodablePlayPacket(id)
            | Self::UnhandledPlayPacket(id)
            | Self::Entity(id) => id,
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            Self::LoginPacket(_) => "unsupported_login_packet",
            Self::ConfigurationPacket(_) => "unsupported_configuration_packet",
            Self::UnhandledConfigurationPacket(_) => "unhandled_configuration_packet",
            Self::PlayPacket(_) => "unsupported_play_packet",
            Self::UndecodablePlayPacket(_) => "undecodable_play_packet",
            Self::UnhandledPlayPacket(_) => "unhandled_play_packet",
            Self::Entity(_) => "unsupported_entity",
        }
    }

    pub fn name(self) -> Option<&'static str> {
        let state = match self {
            Self::LoginPacket(_) => State::Login,
            Self::ConfigurationPacket(_) | Self::UnhandledConfigurationPacket(_) => {
                State::Configuration
            }
            Self::PlayPacket(_) | Self::UndecodablePlayPacket(_) | Self::UnhandledPlayPacket(_) => {
                State::Play
            }
            Self::Entity(_) => return None,
        };
        ids::name(state, Direction::Clientbound, self.id())
    }
}

#[derive(Default)]
pub struct Gaps {
    counts: BTreeMap<Gap, u64>,
    omitted_events: u64,
}

impl Gaps {
    /// True only for the first occurrence of a retained key: log once per kind.
    /// A peer cannot grow the map without limit by sending arbitrary packet ids.
    pub fn record(&mut self, gap: Gap) -> bool {
        if let Some(count) = self.counts.get_mut(&gap) {
            *count = count.saturating_add(1);
            return false;
        }
        if self.counts.len() == LIMIT {
            self.omitted_events = self.omitted_events.saturating_add(1);
            return false;
        }
        self.counts.insert(gap, 1);
        true
    }

    /// No payloads, chat, player names, profile properties or server addresses.
    pub fn report(&self, entity_name: impl Fn(i32) -> Option<String>) -> serde_json::Value {
        let entries: Vec<_> = self
            .counts
            .iter()
            .map(|(&gap, &count)| {
                let name = gap.name().map(str::to_owned).or_else(|| {
                    matches!(gap, Gap::Entity(_))
                        .then(|| entity_name(gap.id()))
                        .flatten()
                });
                serde_json::json!({
                    "category": gap.category(), "id": gap.id(), "name": name, "count": count,
                })
            })
            .collect();
        serde_json::json!({"entries": entries, "omitted_events": self.omitted_events, "limit": LIMIT})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_are_deduplicated_but_counts_continue() {
        let mut gaps = Gaps::default();
        let gap = Gap::PlayPacket(ids::play::clientbound::BOSS_EVENT);
        assert!(gaps.record(gap));
        assert!(!gaps.record(gap));
        let report = gaps.report(|_| None);
        assert_eq!(report["entries"][0]["name"], "boss_event");
        assert_eq!(report["entries"][0]["count"], 2);
    }

    #[test]
    fn arbitrary_ids_cannot_grow_the_report_without_limit() {
        let mut gaps = Gaps::default();
        for id in 0..LIMIT as i32 + 10 {
            gaps.record(Gap::PlayPacket(id));
        }
        assert!(!gaps.record(Gap::PlayPacket(0)));
        let report = gaps.report(|_| None);
        assert_eq!(report["entries"].as_array().unwrap().len(), LIMIT);
        assert_eq!(report["entries"][0]["count"], 2);
        assert_eq!(report["omitted_events"], 10);
    }

    #[test]
    fn an_unknown_packet_id_is_not_resolved_as_an_entity_type() {
        let mut gaps = Gaps::default();
        gaps.record(Gap::PlayPacket(i32::MAX));
        gaps.record(Gap::UndecodablePlayPacket(
            ids::play::clientbound::CONTAINER_SET_CONTENT,
        ));
        let report = gaps.report(|_| Some("minecraft:pig".into()));
        assert!(report["entries"][0]["name"].is_null());
        assert_eq!(report["entries"][1]["name"], "container_set_content");
    }

    #[test]
    fn phases_and_entities_do_not_share_packet_keys() {
        let mut gaps = Gaps::default();
        for gap in [
            Gap::LoginPacket(0),
            Gap::ConfigurationPacket(0),
            Gap::PlayPacket(0),
            Gap::UnhandledPlayPacket(0),
            Gap::Entity(0),
        ] {
            assert!(gaps.record(gap));
        }
        let report = gaps.report(|_| Some("minecraft:text_display".into()));
        assert_eq!(report["entries"].as_array().unwrap().len(), 5);
        assert_eq!(report["entries"][4]["name"], "minecraft:text_display");
    }
}
