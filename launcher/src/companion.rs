//! The companion's setup from the launcher (docs/companions.md): `horizon_companion::setup`, the same
//! code the game's Companion section runs, bound to the game's home (`<home>/companion/`, where the
//! game finds what was set up) and to the game's language (the natural voice speaks it).
use std::path::PathBuf;

use horizon_companion::catalogue::Tier;
use horizon_companion::download::Layout;
use horizon_companion::plan::{PartOption, Plan};
pub use horizon_companion::settings::{BrainChoice, CompanionSettings, PartChoice};
use horizon_companion::setup::Setup;
pub use horizon_companion::setup::{gigabytes, Component, Event, Job, Machine, TestResult};

use crate::backend;

/// Where the companion keeps its runtime, models and settings: the game's home.
pub fn home() -> PathBuf {
    backend::game_home()
}

pub fn layout() -> Layout {
    Layout::new(&home())
}

/// The game's language from its options (`lang:fr_fr`), English until the game has saved one.
pub fn game_language() -> String {
    std::fs::read_to_string(home().join("options.txt"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("lang:").map(|v| v.trim().to_owned()))
        })
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en_us".into())
}

fn setup() -> Setup {
    Setup::new(&home(), &game_language())
}

pub fn load_settings() -> CompanionSettings {
    CompanionSettings::load(&home())
}

/// Saves the settings and tells the game's extras file about No AI.
pub fn save_settings(settings: &CompanionSettings) -> Result<(), String> {
    settings.save(&home()).map_err(|e| e.to_string())?;
    sync_no_ai(settings.no_ai)
}

/// Before each start: the game rewrites its extras file with the keys it knows.
pub fn sync_no_ai(no_ai: bool) -> Result<(), String> {
    horizon_companion::settings::sync_extras(&home(), no_ai).map_err(|e| e.to_string())
}

pub fn detect_machine() -> Machine {
    Machine::detect(&home())
}

/// Every part's options: the brain's and the senses'.
pub fn options() -> Vec<PartOption> {
    setup().options()
}

/// Ids and runtime keys already on disk, for the plan's download size.
pub fn installed_ids() -> Vec<String> {
    setup().installed_ids()
}

pub fn make_plan(machine: &Machine) -> Plan {
    setup().plan(machine)
}

/// What a plan option id downloads.
pub fn model_of(option_id: &str) -> Option<Component> {
    Component::of(option_id)
}

/// Models on disk and their size.
pub fn installed_models() -> Vec<(Component, u64)> {
    let setup = setup();
    let mut seen: Vec<(Component, u64)> = Vec::new();
    for option in setup.options() {
        let Some(c) = Component::of(&option.id) else {
            continue;
        };
        if setup.installed(&option.id) && !seen.iter().any(|(s, _)| s.id() == c.id()) {
            seen.push((c, c.size()));
        }
    }
    seen
}

/// Deletes every model and runtime (No AI's "delete downloaded models"); returns the bytes freed.
pub fn delete_everything() -> Result<u64, String> {
    setup().delete_everything()
}

pub fn delete_model(component: Component) -> Result<(), String> {
    let layout = layout();
    match component {
        Component::Brain(m) => layout.delete_model(m),
        Component::Sense(m) => layout.delete_sense_model(m),
    }
    .map_err(|e| e.to_string())
}

/// "Set up companion" (`Setup::set_up`).
pub fn set_up(settings: CompanionSettings) -> Job {
    setup().set_up(settings)
}

/// Starts the brain the settings pick, warms it and plays one turn (`Setup::test_brain`).
pub fn test_brain(settings: CompanionSettings, machine: Machine) -> Job {
    setup().test_brain(settings, machine)
}

/// "Light", "Standard"…
pub fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Light => "Light",
        Tier::Standard => "Standard",
        Tier::Strong => "Strong",
        Tier::Best => "Best",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_option_ids_name_what_they_download() {
        for option in options() {
            let component = model_of(&option.id).expect(&option.id);
            assert_eq!(option.download, component.size());
        }
        assert!(model_of("nothing@gpu").is_none());
    }
}
