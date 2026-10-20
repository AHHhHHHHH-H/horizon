//! The Companion section of the options (docs/companions.md, "Settings"): the rows both menu
//! styles show, their values, and what a click does. The game fills [`CompanionView`] (the PC, the
//! plan, the setup's progress, who is in the world) and carries out the [`Action`]s; the settings
//! themselves are the launcher's `<home>/companion/settings.json`, so the two stay in sync.
use horizon_companion::{
    plan::Part,
    prompt::{Level, Personality},
    settings::{BrainChoice, Chattiness, CompanionSettings, PartChoice},
};

/// What the section shows, filled by the game while the options are open.
#[derive(Clone, Debug, Default)]
pub struct CompanionView {
    /// Filled: the section exists (not in the first-run menu, not under No AI).
    pub ready: bool,
    pub settings: CompanionSettings,
    /// A world is open where companions can play, and the companion in it, if any.
    pub in_world: bool,
    pub here: Option<String>,
    /// "16 GB of memory, 8 cores, Apple M4"; empty while it is being checked.
    pub machine: String,
    /// One line per part of the plan.
    pub plan: Vec<String>,
    /// Still to download, in bytes.
    pub download: u64,
    /// Setting up: the step and its progress.
    pub progress: Option<(String, Option<f32>)>,
    /// The last setup's error.
    pub error: Option<String>,
    /// What runs now ("Brain ready", "Missing: ...").
    pub status: Vec<String>,
    pub advanced: bool,
    /// Advanced's choices per part: (option id, label).
    pub choices: Vec<(Part, Vec<(String, String)>)>,
    /// Models and runtimes on disk, in bytes.
    pub disk: u64,
}

/// One row of the section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Header(&'static str),
    Pc,
    Plan(usize),
    SetUp,
    Status(usize),
    Name,
    Personality,
    Level,
    Language,
    Chattiness,
    Voice,
    TalkKey,
    Spawn,
    Advanced,
    Part(Part),
    Delete,
}

/// What the game does after a click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// The settings changed: save them (and restart the parts that changed).
    Changed,
    SetUp,
    Cancel,
    Spawn,
    Dismiss,
    /// Delete the downloaded models and runtimes.
    Delete,
    /// Bind the push-to-talk key (the next key pressed).
    BindTalk,
    /// The name box took the focus.
    EditName,
}

pub fn rows(view: &CompanionView) -> Vec<Row> {
    let mut rows = vec![Row::Header("Your PC"), Row::Pc];
    rows.extend((0..view.plan.len()).map(Row::Plan));
    rows.push(Row::SetUp);
    rows.extend((0..view.status.len()).map(Row::Status));
    rows.extend([
        Row::Header("Companion"),
        Row::Name,
        Row::Personality,
        Row::Level,
        Row::Language,
        Row::Chattiness,
        Row::Voice,
    ]);
    if view.settings.voice {
        rows.push(Row::TalkKey);
    }
    rows.extend([Row::Spawn, Row::Advanced]);
    if view.advanced {
        rows.push(Row::Header("Advanced"));
        rows.extend([Part::Brain, Part::Classifier].map(Row::Part));
        if view.settings.voice {
            rows.extend([Part::SpeechToText, Part::TextToSpeech].map(Row::Part));
        }
        rows.push(Row::Delete);
    }
    rows
}

/// How a row's control looks.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    None,
    Switch(bool),
    Button(String),
    /// A button that can't be pressed now.
    Inactive(String),
    TextBox(String, bool),
}

/// The caption, the description and the control of a row.
/// `editing`: the name being typed while the name box has the focus.
pub fn row_parts(row: Row, view: &CompanionView, talk_key: &str, editing: Option<&str>) -> (String, String, Control) {
    let s = &view.settings;
    let p = &s.persona;
    match row {
        Row::Header(h) => (h.into(), String::new(), Control::None),
        Row::Pc => (
            "This PC".into(),
            if view.machine.is_empty() { "Checking…".into() } else { view.machine.clone() },
            Control::None,
        ),
        Row::Plan(i) => (String::new(), view.plan.get(i).cloned().unwrap_or_default(), Control::None),
        Row::SetUp => {
            let (text, control) = match (&view.progress, &view.error) {
                (Some((step, Some(f))), _) => (format!("{step} ({:.0} %)", f * 100.0), Control::Button("Cancel".into())),
                (Some((step, None)), _) => (step.clone(), Control::Button("Cancel".into())),
                (None, Some(e)) => (format!("Setup failed: {e}"), Control::Button("Try again".into())),
                (None, None) if s.set_up && view.download == 0 => (
                    "Set up: everything is on this PC.".into(),
                    Control::Button("Check again".into()),
                ),
                (None, None) => (
                    if view.download > 0 {
                        format!("Downloads {} and runs on this PC, offline.", gigabytes(view.download))
                    } else {
                        "Runs on this PC, offline.".into()
                    },
                    Control::Button("Set up companion".into()),
                ),
            };
            ("Set up".into(), text, control)
        }
        Row::Status(i) => (String::new(), view.status.get(i).cloned().unwrap_or_default(), Control::None),
        Row::Name => {
            let value = editing.map_or_else(|| p.name.clone(), str::to_owned);
            ("Name".into(), "3 to 16 letters, digits or _.".into(), Control::TextBox(value, editing.is_some()))
        }
        Row::Personality => (
            "Personality".into(),
            if p.personality == Personality::Custom {
                "Your own description, from the launcher.".into()
            } else {
                p.personality.blurb().into()
            },
            Control::Button(p.personality.label().into()),
        ),
        Row::Level => (
            "Level".into(),
            match p.level {
                Level::Helper => "Does what you ask and defends you.",
                Level::SuperStrong => "Boosted: Strength II, Resistance II, Regeneration.",
                Level::Partner => "Plans the run to the dragon with you.",
            }
            .into(),
            Control::Button(p.level.label().into()),
        ),
        Row::Language => (
            "Language".into(),
            "Clean is the default. Salty allows mild swearing.".into(),
            Control::Button(language_label(s).into()),
        ),
        Row::Chattiness => (
            "Chattiness".into(),
            match s.chattiness {
                Chattiness::Quiet => "Only warns of danger.",
                Chattiness::Normal => "Warns of danger and remarks now and then.",
                Chattiness::Chatty => "Talks a lot.",
            }
            .into(),
            Control::Button(s.chattiness.label().into()),
        ),
        Row::Voice => (
            "Voice".into(),
            "Hear it speak, and talk to it with push-to-talk. The microphone opens only while the key is held.".into(),
            Control::Switch(s.voice),
        ),
        Row::TalkKey => ("Push-to-talk key".into(), String::new(), Control::Button(talk_key.into())),
        Row::Spawn => {
            let control = match (&view.here, view.in_world) {
                (Some(_), _) => Control::Button("Dismiss".into()),
                (None, true) => Control::Button("Spawn".into()),
                (None, false) => Control::Inactive("Spawn".into()),
            };
            let text = match &view.here {
                Some(name) => format!("{name} is in this world."),
                None if view.in_world => "Brings your companion into this world.".into(),
                None => "Open a world to bring your companion in.".into(),
            };
            ("Companion".into(), text, control)
        }
        Row::Advanced => (
            "Show advanced".into(),
            "Pick each model yourself.".into(),
            Control::Switch(view.advanced),
        ),
        Row::Part(part) => {
            let caption = match part {
                Part::Brain => "Brain",
                Part::Classifier => "Command recognition",
                Part::SpeechToText => "Speech recognition",
                Part::TextToSpeech => "Voice model",
            };
            (caption.into(), String::new(), Control::Button(part_label(view, part)))
        }
        Row::Delete => (
            "Downloaded models".into(),
            format!("{} on disk.", gigabytes(view.disk)),
            if view.disk > 0 { Control::Button("Delete".into()) } else { Control::Inactive("Delete".into()) },
        ),
    }
}

/// A click on a row (`back`: the right button, cycling backwards).
pub fn click(row: Row, view: &mut CompanionView, back: bool) -> Option<Action> {
    let s = &mut view.settings;
    match row {
        Row::SetUp if view.progress.is_some() => Some(Action::Cancel),
        Row::SetUp => Some(Action::SetUp),
        Row::Name => Some(Action::EditName),
        Row::Personality => {
            s.persona.personality = cycle(&Personality::ALL, s.persona.personality, back);
            Some(Action::Changed)
        }
        Row::Level => {
            s.persona.level = cycle(&Level::ALL, s.persona.level, back);
            Some(Action::Changed)
        }
        Row::Language => {
            let next = cycle(&[0, 1, 2], language_index(s), back);
            s.persona.salty = next == 1;
            s.persona.kid_friendly = next == 2;
            Some(Action::Changed)
        }
        Row::Chattiness => {
            s.chattiness = cycle(&Chattiness::ALL, s.chattiness, back);
            Some(Action::Changed)
        }
        Row::Voice => {
            s.voice = !s.voice;
            Some(Action::Changed)
        }
        Row::TalkKey => Some(Action::BindTalk),
        Row::Spawn if view.here.is_some() => Some(Action::Dismiss),
        Row::Spawn if view.in_world => Some(Action::Spawn),
        Row::Advanced => {
            view.advanced = !view.advanced;
            None
        }
        Row::Part(part) => {
            let ids: Vec<Option<String>> = std::iter::once(None)
                .chain(choices(view, part).iter().map(|(id, _)| Some(id.clone())))
                .chain((part != Part::Brain).then(|| Some("off".to_owned())))
                .collect();
            let now = current_id(&view.settings, part);
            let next = cycle(&ids, now, back);
            let s = &mut view.settings;
            match (part, next) {
                (Part::Brain, None) => s.brain = BrainChoice::Auto,
                (Part::Brain, Some(id)) => s.brain = BrainChoice::Option { id },
                (_, choice) => {
                    let choice = match choice.as_deref() {
                        None => PartChoice::Auto,
                        Some("off") => PartChoice::Off,
                        Some(id) => PartChoice::Option { id: id.to_owned() },
                    };
                    match part {
                        Part::Classifier => s.classifier = choice,
                        Part::SpeechToText => s.speech_to_text = choice,
                        _ => s.text_to_speech = choice,
                    }
                }
            }
            Some(Action::Changed)
        }
        Row::Delete if view.disk > 0 => Some(Action::Delete),
        _ => None,
    }
}

fn choices(view: &CompanionView, part: Part) -> &[(String, String)] {
    view.choices
        .iter()
        .find(|(p, _)| *p == part)
        .map_or(&[], |(_, c)| c.as_slice())
}

fn current_id(s: &CompanionSettings, part: Part) -> Option<String> {
    let choice = match part {
        Part::Brain => match &s.brain {
            BrainChoice::Option { id } => return Some(id.clone()),
            _ => return None,
        },
        Part::Classifier => &s.classifier,
        Part::SpeechToText => &s.speech_to_text,
        Part::TextToSpeech => &s.text_to_speech,
    };
    match choice {
        PartChoice::Auto => None,
        PartChoice::Off => Some("off".into()),
        PartChoice::Option { id } => Some(id.clone()),
    }
}

fn part_label(view: &CompanionView, part: Part) -> String {
    let s = &view.settings;
    if part == Part::Brain {
        match &s.brain {
            BrainChoice::Gguf { .. } => return "Your own model".into(),
            BrainChoice::Server { model, .. } => return model.clone(),
            _ => {}
        }
    }
    match current_id(s, part) {
        None => "Automatic".into(),
        Some(id) if id == "off" => "Off".into(),
        Some(id) => choices(view, part)
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(id.clone(), |(_, label)| label.clone()),
    }
}

fn language_index(s: &CompanionSettings) -> usize {
    if s.persona.kid_friendly {
        2
    } else if s.persona.salty {
        1
    } else {
        0
    }
}

fn language_label(s: &CompanionSettings) -> &'static str {
    ["Clean", "Salty", "Kid-friendly"][language_index(s)]
}

fn cycle<T: Clone + PartialEq>(all: &[T], now: T, back: bool) -> T {
    let i = all.iter().position(|v| *v == now).unwrap_or(0);
    let n = all.len();
    all[if back { (i + n - 1) % n } else { (i + 1) % n }].clone()
}

/// "1.4 GB", "350 MB".
pub fn gigabytes(bytes: u64) -> String {
    horizon_companion::setup::gigabytes(bytes)
}

/// A name the companion can have: letters, digits and `_`, 16 at most.
pub fn name_chars(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_voice_and_advanced() {
        let mut view = CompanionView::default();
        assert!(!rows(&view).contains(&Row::TalkKey));
        view.settings.voice = true;
        view.advanced = true;
        let r = rows(&view);
        assert!(r.contains(&Row::TalkKey) && r.contains(&Row::Part(Part::TextToSpeech)));
    }

    #[test]
    fn clicks_cycle_and_report() {
        let mut view = CompanionView::default();
        assert_eq!(click(Row::Level, &mut view, false), Some(Action::Changed));
        assert_eq!(view.settings.persona.level, Level::SuperStrong);
        click(Row::Language, &mut view, false);
        assert!(view.settings.persona.salty);
        click(Row::Language, &mut view, false);
        assert!(view.settings.persona.kid_friendly && !view.settings.persona.salty);
        view.choices = vec![(Part::Classifier, vec![("e5@cpu".into(), "Accurate".into())])];
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(view.settings.classifier, PartChoice::Option { id: "e5@cpu".into() });
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(view.settings.classifier, PartChoice::Off);
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(view.settings.classifier, PartChoice::Auto);
        assert_eq!(click(Row::Spawn, &mut view, false), None);
        view.in_world = true;
        assert_eq!(click(Row::Spawn, &mut view, false), Some(Action::Spawn));
        assert_eq!(name_chars("Kev the 2nd!"), "Kevthe2nd");
    }
}
