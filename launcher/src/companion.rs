//! The companion's setup from the launcher (docs/companions.md): `horizon_companion::setup`, the same
//! code the game's Companion section runs, bound to the game's home (`<home>/companion/`, where the
//! game finds what was set up) and to the game's language (the natural voice speaks it).
use std::path::{Path, PathBuf};

use horizon_companion::cast;
use horizon_companion::catalogue::Tier;
use horizon_companion::download::Layout;
use horizon_companion::plan::{PartOption, Plan};
use horizon_companion::prompt::Personality;
use horizon_companion::senses::{self, SenseModel};
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

/// The voices a text-to-speech model offers, as the game's Companion section lists them (the cast
/// once its voice pack is here, then the model's own): (id, label, description, sample clip).
pub fn voices(model: &SenseModel) -> Vec<(String, String, String, Option<PathBuf>)> {
    let pack = senses::voice_pack_dir(&home());
    let language = game_language();
    let language = language.get(..2).unwrap_or("en");
    cast::voice_choices(model, pack.is_some())
        .into_iter()
        .map(|(id, label, description)| {
            let sample = pack
                .as_deref()
                .and_then(|p| cast::sample_clip(p, &id, language));
            (id, label, description, sample)
        })
        .collect()
}

/// The voice a personality starts with on the model, as the game picks it.
pub fn default_voice(model: &SenseModel, personality: Personality) -> Option<String> {
    let pack = senses::voice_pack_dir(&home()).is_some();
    cast::default_voice_id(model, personality, pack)
}

/// Play a voice's sample clip with the system's own player, in the background (no window).
pub fn play_sample(clip: &Path) -> Result<(), String> {
    let path = clip.to_string_lossy().into_owned();
    let tries: Vec<(&str, Vec<String>)> = if cfg!(target_os = "macos") {
        vec![("afplay", vec![path])]
    } else if cfg!(windows) {
        let quoted = path.replace('\'', "''");
        vec![(
            "powershell",
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                format!("(New-Object Media.SoundPlayer '{quoted}').PlaySync()"),
            ],
        )]
    } else {
        vec![
            ("paplay", vec![path.clone()]),
            ("pw-play", vec![path.clone()]),
            ("aplay", vec!["-q".into(), path]),
        ]
    };
    for (program, args) in tries {
        let started = std::process::Command::new(program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut child) = started {
            // Reaped once it has played.
            std::thread::spawn(move || child.wait());
            return Ok(());
        }
    }
    Err("no sound player found".into())
}

/// The settings, with No AI also on when the game's Extras switch says so.
pub fn load_settings() -> CompanionSettings {
    let mut settings = CompanionSettings::load(&home());
    settings.no_ai |= horizon_companion::settings::extras_no_ai(&home());
    settings
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
