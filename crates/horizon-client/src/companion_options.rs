//! The Companion section of the options (docs/companions.md, "Settings"): the rows both menu
//! styles show, their values, and what a click does. The game fills [`CompanionView`] (the PC, the
//! plan, the setup's progress, who is in the world) and carries out the [`Action`]s; the settings
//! themselves are the launcher's `<home>/companion/settings.json`, so the two stay in sync.
use horizon_companion::{
    plan::Part,
    prompt::{Perception, Personality, Role},
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
    /// Salty was picked and waits for the 18+ confirmation.
    pub confirm_salty: bool,
    /// The text-to-speech model that would speak, and its voices: (id, label, description).
    pub tts_model: Option<String>,
    pub voices: Vec<(String, String, String)>,
    /// The personality's default voice on that model.
    pub default_voice: Option<String>,
    /// Advanced: the brain tier's uncensored variant for Salty mode: (name, licence), or `None`
    /// while none is chosen.
    pub salty_brain: Option<(String, String)>,
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
    Role,
    Perception,
    Language,
    /// The 18+ confirmation Salty needs.
    SaltyConfirm,
    Chattiness,
    Voice,
    /// Which voice it speaks with, and a sample of it.
    VoiceCharacter,
    VoiceSample,
    TalkKey,
    Spawn,
    Advanced,
    Part(Part),
    /// Salty mode's uncensored brain.
    SaltyBrain,
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
    /// Say a short line with the chosen voice.
    PlaySample,
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
        Row::Role,
        Row::Perception,
        Row::Language,
    ]);
    if view.confirm_salty {
        rows.push(Row::SaltyConfirm);
    }
    rows.extend([Row::Chattiness, Row::Voice]);
    if view.settings.voice {
        if !view.voices.is_empty() {
            rows.extend([Row::VoiceCharacter, Row::VoiceSample]);
        }
        rows.push(Row::TalkKey);
    }
    rows.extend([Row::Spawn, Row::Advanced]);
    if view.advanced {
        rows.push(Row::Header("Advanced"));
        rows.extend([Part::Brain, Part::Classifier].map(Row::Part));
        if view.settings.voice {
            rows.extend([Part::SpeechToText, Part::TextToSpeech].map(Row::Part));
        }
        if view.settings.persona.salty_on() {
            rows.push(Row::SaltyBrain);
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
pub fn row_parts(
    row: Row,
    view: &CompanionView,
    talk_key: &str,
    editing: Option<&str>,
) -> (String, String, Control) {
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
        Row::Role => ("Role".into(), p.role.blurb().into(), Control::Button(p.role.label().into())),
        Row::Perception => (
            "Perception".into(),
            p.perception.blurb().into(),
            Control::Button(p.perception.label().into()),
        ),
        Row::Language => (
            "Language".into(),
            if p.kid_friendly {
                "Gentle humour, no swearing, nothing scary."
            } else if p.salty_on() {
                "Real swearing and attitude. Still refuses anything harmful."
            } else {
                "No swearing. Salty (18+) swears for real; Kid-friendly is gentler."
            }
            .into(),
            Control::Button(language_label(s).into()),
        ),
        Row::SaltyConfirm => (
            "Salty is for adults".into(),
            "Real swearing and attitude. Confirm that you are 18 or older.".into(),
            Control::Button("I'm 18 or older".into()),
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
        Row::VoiceCharacter => {
            let chosen = voice_choice(view);
            let shown = chosen.clone().or_else(|| view.default_voice.clone());
            let (label, description) = view
                .voices
                .iter()
                .find(|(id, ..)| Some(id) == shown.as_ref())
                .map(|(_, l, d)| (l.clone(), d.clone()))
                .unwrap_or_default();
            let label = if chosen.is_none() { format!("{label} (default)") } else { label };
            ("Sounds like".into(), description, Control::Button(label))
        }
        Row::VoiceSample => (
            String::new(),
            "A short line in this voice.".into(),
            Control::Button("Play sample".into()),
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
        Row::SaltyBrain => (
            "Salty brain".into(),
            match &view.salty_brain {
                Some((name, licence)) => format!("Uncensored: {name} ({licence})."),
                None => "No uncensored model is chosen yet: the normal brain runs with the Salty prompt.".into(),
            },
            Control::Switch(s.salty_uncensored),
        ),
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
        Row::Role => {
            s.persona.role = cycle(&Role::ALL, s.persona.role, back);
            Some(Action::Changed)
        }
        Row::Perception => {
            s.persona.perception = cycle(&Perception::ALL, s.persona.perception, back);
            Some(Action::Changed)
        }
        Row::Language => {
            let next = cycle(&[0, 1, 2], language_index(view), back);
            let s = &mut view.settings;
            view.confirm_salty = false;
            match next {
                // Salty waits for the 18+ confirmation the first time.
                1 if !s.persona.adult_confirmed => {
                    view.confirm_salty = true;
                    s.persona.kid_friendly = false;
                    s.persona.salty = false;
                }
                _ => {
                    s.persona.salty = next == 1;
                    s.persona.kid_friendly = next == 2;
                }
            }
            Some(Action::Changed)
        }
        Row::SaltyConfirm => {
            view.confirm_salty = false;
            s.persona.adult_confirmed = true;
            s.persona.salty = true;
            s.persona.kid_friendly = false;
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
        Row::VoiceCharacter => {
            let ids: Vec<Option<String>> = std::iter::once(None)
                .chain(view.voices.iter().map(|(id, ..)| Some(id.clone())))
                .collect();
            let next = cycle(&ids, voice_choice(view), back);
            if let Some(model) = view.tts_model.clone() {
                let s = &mut view.settings;
                match next {
                    Some(id) => s.voices.insert(model, id),
                    None => s.voices.remove(&model),
                };
            }
            Some(Action::Changed)
        }
        Row::VoiceSample => Some(Action::PlaySample),
        Row::SaltyBrain => {
            s.salty_uncensored = !s.salty_uncensored;
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

/// Clean, Salty (also while it waits for the confirmation) or Kid-friendly.
fn language_index(view: &CompanionView) -> usize {
    let p = &view.settings.persona;
    if p.kid_friendly {
        2
    } else if p.salty_on() || view.confirm_salty {
        1
    } else {
        0
    }
}

fn language_label(s: &CompanionSettings) -> &'static str {
    if s.persona.kid_friendly {
        "Kid-friendly"
    } else if s.persona.salty_on() {
        "Salty (18+)"
    } else {
        "Clean"
    }
}

/// The voice the settings name for the speaking model, if they name one it has.
fn voice_choice(view: &CompanionView) -> Option<String> {
    let model = view.tts_model.as_ref()?;
    view.settings
        .voices
        .get(model)
        .filter(|id| view.voices.iter().any(|(v, ..)| v == *id))
        .cloned()
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
        assert_eq!(click(Row::Role, &mut view, false), Some(Action::Changed));
        assert_eq!(view.settings.persona.role, Role::Teammate);
        click(Row::Perception, &mut view, true);
        assert_eq!(view.settings.persona.perception, Perception::AllKnowing);
        // Salty asks for 18+ first, and stays off until it is confirmed.
        click(Row::Language, &mut view, false);
        assert!(view.confirm_salty && !view.settings.persona.salty_on());
        assert!(rows(&view).contains(&Row::SaltyConfirm));
        click(Row::SaltyConfirm, &mut view, false);
        assert!(view.settings.persona.salty_on() && !rows(&view).contains(&Row::SaltyConfirm));
        assert_eq!(
            row_parts(Row::Language, &view, "", None).2,
            Control::Button("Salty (18+)".into())
        );
        // Kid-friendly turns it off; coming back needs no second confirmation.
        click(Row::Language, &mut view, false);
        assert!(view.settings.persona.kid_friendly && !view.settings.persona.salty_on());
        click(Row::Language, &mut view, false);
        click(Row::Language, &mut view, false);
        assert!(view.settings.persona.salty_on() && !view.confirm_salty);
        view.choices = vec![(Part::Classifier, vec![("e5@cpu".into(), "Accurate".into())])];
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(
            view.settings.classifier,
            PartChoice::Option {
                id: "e5@cpu".into()
            }
        );
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(view.settings.classifier, PartChoice::Off);
        click(Row::Part(Part::Classifier), &mut view, false);
        assert_eq!(view.settings.classifier, PartChoice::Auto);
        assert_eq!(click(Row::Spawn, &mut view, false), None);
        view.in_world = true;
        assert_eq!(click(Row::Spawn, &mut view, false), Some(Action::Spawn));
        assert_eq!(name_chars("Kev the 2nd!"), "Kevthe2nd");
    }

    #[test]
    fn voices_default_to_the_personality_and_can_be_picked() {
        let mut view = CompanionView::default();
        view.settings.voice = true;
        assert!(!rows(&view).contains(&Row::VoiceCharacter));
        view.tts_model = Some("tts".into());
        view.voices = vec![
            ("ryan".into(), "Energetic, bright".into(), "Man".into()),
            ("uncle_fu".into(), "Gruff, deep".into(), "Man, deep".into()),
        ];
        view.default_voice = Some("uncle_fu".into());
        assert!(rows(&view).contains(&Row::VoiceSample));
        let label = |v: &CompanionView| row_parts(Row::VoiceCharacter, v, "", None).2;
        assert_eq!(
            label(&view),
            Control::Button("Gruff, deep (default)".into())
        );
        click(Row::VoiceCharacter, &mut view, false);
        assert_eq!(
            view.settings.voices.get("tts").map(String::as_str),
            Some("ryan")
        );
        assert_eq!(label(&view), Control::Button("Energetic, bright".into()));
        click(Row::VoiceCharacter, &mut view, true);
        assert!(view.settings.voices.is_empty());
        assert_eq!(
            click(Row::VoiceSample, &mut view, false),
            Some(Action::PlaySample)
        );
    }
}
