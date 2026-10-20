//! The Companion section's side in the game (`companion_options`): what it shows (this PC, the
//! plan, the setup's progress, who is here) and what its buttons do. Setting up runs
//! `horizon_companion::setup`, the launcher's code, on its own thread; the settings are the
//! launcher's file, so a change on either side shows on the other.
use super::{Game, companion_ai::home};
use horizon_client::{
    companion as c,
    companion_options::Action,
    text::{self, Text},
};
use horizon_companion::{
    catalogue::Tier,
    plan::{Device, Part, Plan, Runs},
    prompt::{Perception, Role},
    senses::{self, SenseModel},
    settings::{CompanionSettings, sync_extras},
    setup::{self, Component, Event, Job, Machine, Setup},
};
use std::sync::mpsc::{self, Receiver};

#[derive(Default)]
pub(super) struct SetupState {
    /// The PC and the plan, worked out off the game thread.
    machine: Option<Receiver<Checked>>,
    job: Option<Job>,
    /// The machine was checked for this view.
    checked: bool,
    /// The voice model the settings would run, for the voice picker.
    tts: Option<&'static SenseModel>,
}

struct Checked {
    machine: Machine,
    plan: Plan,
    choices: Vec<(Part, Vec<(String, String)>)>,
    disk: u64,
    tts: Option<&'static SenseModel>,
    salty_brain: Option<(String, String)>,
}

fn part_name(part: Part) -> &'static str {
    match part {
        Part::Brain => "Brain",
        Part::Classifier => "Commands",
        Part::TextToSpeech => "Voice",
        Part::SpeechToText => "Listening",
    }
}

impl Game<'_> {
    fn companion_setup(&self) -> Setup {
        Setup::new(&home(), &self.settings.language)
    }

    /// While the options are open: the Companion section's view.
    pub(super) fn companion_view_refresh(&mut self) {
        if self.no_ai() {
            self.settings.companion.ready = false;
            return;
        }
        if !self.settings.companion.ready {
            let settings = self.companion_settings().clone();
            let view = &mut self.settings.companion;
            view.ready = true;
            view.settings = settings;
        }
        if !self.companion_setup.checked && self.companion_setup.machine.is_none() {
            self.companion_check_machine();
        }
        if let Some(rx) = &self.companion_setup.machine
            && let Ok(checked) = rx.try_recv()
        {
            self.companion_setup.machine = None;
            self.companion_setup.checked = true;
            let view = &mut self.settings.companion;
            view.machine = checked.machine.summary();
            view.plan = plan_lines(&checked.plan);
            view.download = checked.plan.download;
            view.choices = checked.choices;
            view.disk = checked.disk;
            view.salty_brain = checked.salty_brain;
            self.companion_setup.tts = checked.tts;
        }
        let view = &mut self.settings.companion;
        let tts = self.companion_setup.tts;
        view.tts_model = tts.map(|m| m.id.to_owned());
        view.voices = tts.map_or_else(Vec::new, |m| {
            m.voices
                .iter()
                .map(|v| {
                    (
                        v.id.to_owned(),
                        v.label.to_owned(),
                        v.description.to_owned(),
                    )
                })
                .collect()
        });
        view.default_voice = tts
            .and_then(|m| senses::default_voice(m, view.settings.persona.personality))
            .map(|v| v.id.to_owned());
        self.tick_companion_setup();
        let here_file = self
            .lan
            .as_ref()
            .and_then(|h| h.companions.first())
            .map(|b| b.file.clone());
        let here = here_file.as_ref().map(|f| f.name.clone());
        // The companion in the world shows its own role and perception.
        if let Some(f) = &here_file {
            let p = &mut self.settings.companion.settings.persona;
            p.role = Role::parse(&f.role).unwrap_or(p.role);
            p.perception = Perception::parse(&f.perception).unwrap_or(p.perception);
        }
        let in_world = self.player.is_some()
            && self.remote.is_none()
            && self.integrated.is_none()
            && !self.serving();
        let status = self.companion_ai.status.clone();
        let view = &mut self.settings.companion;
        view.here = here;
        view.in_world = in_world;
        view.status = status;
    }

    fn companion_check_machine(&mut self) {
        let setup = self.companion_setup();
        let settings = self.settings.companion.settings.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("companion check".into())
            .spawn(move || {
                let machine = Machine::detect(&setup.home);
                let plan = setup.plan(&machine);
                let mut choices: Vec<(Part, Vec<(String, String)>)> =
                    Part::ALL.iter().map(|p| (*p, Vec::new())).collect();
                for option in setup.options() {
                    let runs = match option.runs {
                        Runs::Cpu => "processor",
                        Runs::Gpu => "graphics card",
                    };
                    if let Some((_, list)) = choices.iter_mut().find(|(p, _)| *p == option.part) {
                        list.push((option.id.clone(), format!("{} ({runs})", option.label)));
                    }
                }
                let disk = setup.disk_usage();
                let options = setup.options();
                let chosen = settings.selection(&plan, &options);
                let component = |part: Part| {
                    chosen
                        .iter()
                        .find(|o| o.part == part)
                        .and_then(|o| Component::of(&o.id))
                };
                let tts = match component(Part::TextToSpeech) {
                    Some(Component::Sense(m)) => Some(m),
                    _ => None,
                };
                let salty_brain = match component(Part::Brain) {
                    Some(Component::Brain(model)) => Tier::ALL
                        .into_iter()
                        .find(|t| t.model().id == model.id)
                        .and_then(Tier::uncensored)
                        .map(|m| (m.name.to_owned(), m.license.to_owned())),
                    _ => None,
                };
                let _ = tx.send(Checked {
                    machine,
                    plan,
                    choices,
                    disk,
                    tts,
                    salty_brain,
                });
            });
        if spawned.is_ok() {
            self.companion_setup.machine = Some(rx);
        }
    }

    /// Each tick: the setup's progress, also while the options are closed.
    pub(super) fn tick_companion_setup(&mut self) {
        let Some(job) = &self.companion_setup.job else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(e) = job.rx.try_recv() {
            events.push(e);
        }
        for event in events {
            match event {
                Event::Step(step, progress) => {
                    self.settings.companion.progress = Some((step, progress));
                }
                Event::Planned(..) | Event::Tested(_) => {}
                Event::Done(what) => {
                    self.companion_setup.job = None;
                    self.settings.companion.progress = None;
                    self.settings.companion.error = None;
                    // The setup saved the settings with `set_up`.
                    let settings = CompanionSettings::load(&home());
                    self.settings.companion.settings = settings.clone();
                    self.companion_settings_changed(settings);
                    self.companion_setup.checked = false;
                    self.system_message(Text::literal(what).with_color(text::GREEN));
                }
                Event::Failed(e) => {
                    self.companion_setup.job = None;
                    self.settings.companion.progress = None;
                    self.settings.companion.error = Some(e);
                }
                Event::Cancelled => {
                    self.companion_setup.job = None;
                    self.settings.companion.progress = None;
                }
            }
        }
    }

    /// A Companion section button.
    pub(super) fn companion_action(&mut self, action: Action) {
        match action {
            Action::Changed => {
                let settings = self.settings.companion.settings.clone();
                if let Err(e) = settings.save(&home()) {
                    log::warn!("[companion] could not save the settings: {e}");
                }
                let _ = sync_extras(&home(), settings.no_ai);
                self.companion_settings_changed(settings);
                self.companion_setup.checked = false;
            }
            Action::SetUp => {
                let settings = self.settings.companion.settings.clone();
                self.settings.companion.error = None;
                self.settings.companion.progress = Some(("Starting".into(), None));
                self.companion_setup.job = Some(self.companion_setup().set_up(settings));
            }
            Action::Cancel => {
                if let Some(job) = &self.companion_setup.job {
                    job.cancel();
                }
            }
            Action::Spawn => {
                let name = self.settings.companion.settings.persona.name.clone();
                let result = self.companion_spawn(c::valid_name(&name).as_deref());
                self.companion_result(result.map(|n| format!("{n} joined you")));
            }
            Action::Dismiss => {
                let name = self.settings.companion.here.clone();
                let result = self.companion_dismiss(name.as_deref());
                self.companion_result(result.map(|n| format!("{n} left")));
            }
            Action::Delete => {
                self.companion_ai_stop();
                let result = self.companion_setup().delete_everything();
                let mut settings = self.settings.companion.settings.clone();
                settings.set_up = false;
                let _ = settings.save(&home());
                self.settings.companion.settings = settings.clone();
                self.companion_settings_changed(settings);
                self.companion_setup.checked = false;
                self.companion_result(
                    result.map(|freed| format!("Deleted {}", setup::gigabytes(freed))),
                );
            }
            Action::PlaySample => self.companion_voice_sample(),
            Action::BindTalk | Action::EditName => {}
        }
    }

    fn companion_result(&mut self, result: Result<String, String>) {
        match result {
            Ok(m) => self.system_message(Text::literal(m)),
            Err(e) => self.system_message(Text::literal(e).with_color(text::RED)),
        }
    }

    /// The extras' No AI switch may have changed: the companion's settings (and so the launcher)
    /// follow.
    pub(super) fn companion_sync_no_ai(&mut self) {
        let no_ai = self.settings.extras.no_ai;
        let mut settings = self.companion_settings().clone();
        if settings.no_ai == no_ai {
            return;
        }
        settings.no_ai = no_ai;
        if let Err(e) = settings.save(&home()) {
            log::warn!("[companion] could not save the settings: {e}");
        }
        self.settings.companion.settings = settings.clone();
        self.settings.companion.ready = false;
        self.companion_settings_changed(settings);
    }
}

fn plan_lines(plan: &Plan) -> Vec<String> {
    let mut lines: Vec<String> = plan
        .picks
        .iter()
        .map(|p| {
            let device = match &p.device {
                Device::Cpu { threads } => format!("on the processor ({threads} threads)"),
                Device::Gpu { name, .. } => format!("on {name}"),
            };
            let wait = if p.latency_ms >= 1000 {
                format!("{:.1} s", f64::from(p.latency_ms) / 1000.0)
            } else {
                format!("{} ms", p.latency_ms)
            };
            format!(
                "{}: {} {device}, first word in about {wait}",
                part_name(p.option.part),
                p.option.label
            )
        })
        .collect();
    lines.extend(
        plan.missing
            .iter()
            .map(|m| format!("{}: {}", part_name(m.part), m.reason)),
    );
    lines
}
