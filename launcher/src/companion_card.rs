//! Settings' AI companion section (docs/companions.md): the No AI switch, then the simple card (what
//! it is, this PC, the plan, "Set up companion" with progress and cancel, "Ready") and Advanced (every
//! brain, other AI programs, a cloud model, your own GGUF, who it is, tuning, a live test, downloaded
//! models). The work is `horizon_launcher::companion`'s, on worker threads polled by `drain`.
use super::*;
use horizon_companion::client::{self, Detected, Provider};
use horizon_companion::plan::{Part, Plan, Runs};
use horizon_companion::prompt::{Perception, Personality, Role};
use horizon_companion::settings::keychain;
use horizon_launcher::companion::{
    self, BrainChoice, CompanionSettings, Event, Job, Machine, PartChoice, TestResult,
};

/// Advanced's text fields.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    CloudUrl,
    CloudModel,
    CloudKey,
    Gguf,
}

pub(crate) struct CompanionUi {
    pub settings: CompanionSettings,
    machine: Option<Machine>,
    plan: Option<Plan>,
    detect: Option<Receiver<(Machine, Plan)>>,
    job: Option<Job>,
    /// What the job is doing, and the bar's previous and current fraction.
    step: Option<(String, Option<(f32, f32)>)>,
    note: Option<String>,
    error: Option<String>,
    advanced: bool,
    tested: Option<TestResult>,
    servers: Option<Vec<Detected>>,
    finding: Option<Receiver<Vec<Detected>>>,
    confirm_delete: bool,
    /// Salty was switched on and waits for the 18+ confirmation.
    confirm_salty: bool,
    fields: [(Field, String, FocusHandle, bool); 4],
}

impl CompanionUi {
    pub fn new(cx: &mut App) -> Self {
        let settings = companion::load_settings();
        let (url, model) = match &settings.brain {
            BrainChoice::Server {
                provider: Provider::Cloud,
                base_url,
                model,
            } => (base_url.clone(), model.clone()),
            _ => (String::new(), String::new()),
        };
        let gguf = match &settings.brain {
            BrainChoice::Gguf { path } => path.display().to_string(),
            _ => String::new(),
        };
        Self {
            settings,
            machine: None,
            plan: None,
            detect: None,
            job: None,
            step: None,
            note: None,
            error: None,
            advanced: false,
            tested: None,
            servers: None,
            finding: None,
            confirm_delete: false,
            confirm_salty: false,
            fields: [
                (Field::CloudUrl, url, cx.focus_handle(), false),
                (Field::CloudModel, model, cx.focus_handle(), false),
                (Field::CloudKey, String::new(), cx.focus_handle(), false),
                (Field::Gguf, gguf, cx.focus_handle(), false),
            ],
        }
    }

    pub fn note_focus(&mut self, window: &Window) {
        for f in &mut self.fields {
            f.3 = f.2.is_focused(window);
        }
    }

    fn value(&self, field: Field) -> &str {
        &self
            .fields
            .iter()
            .find(|f| f.0 == field)
            .expect("every field")
            .1
    }

    fn value_mut(&mut self, field: Field) -> &mut String {
        &mut self
            .fields
            .iter_mut()
            .find(|f| f.0 == field)
            .expect("every field")
            .1
    }

    fn detect(&mut self) {
        if self.detect.is_some() || self.settings.no_ai {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let machine = companion::detect_machine();
            let plan = companion::make_plan(&machine);
            let _ = tx.send((machine, plan));
        });
        self.detect = Some(rx);
    }

    /// Reads what the worker threads sent; true when something changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.detect {
            if let Ok((machine, plan)) = rx.try_recv() {
                self.machine = Some(machine);
                self.plan = Some(plan);
                changed = true;
            }
        }
        if let Some(rx) = &self.finding {
            if let Ok(found) = rx.try_recv() {
                self.servers = Some(found);
                self.finding = None;
                changed = true;
            }
        }
        let mut finished = false;
        if let Some(job) = &self.job {
            while let Ok(event) = job.rx.try_recv() {
                changed = true;
                match event {
                    Event::Step(what, fraction) => {
                        let from = self.step.as_ref().and_then(|s| s.1).map_or(0., |b| b.1);
                        let bar = fraction.map(|f| (if f < from { 0. } else { from }, f));
                        self.step = Some((what, bar));
                    }
                    Event::Planned(machine, plan) => {
                        self.machine = Some(machine);
                        self.plan = Some(*plan);
                    }
                    Event::Tested(result) => self.tested = Some(result),
                    Event::Done(what) => {
                        if what == "Ready" {
                            self.settings.set_up = true;
                        }
                        finished = true;
                    }
                    Event::Failed(e) => {
                        self.error = Some(e);
                        finished = true;
                    }
                    Event::Cancelled => {
                        self.note =
                            Some("Cancelled. What was downloaded is kept for next time.".into());
                        finished = true;
                    }
                }
            }
        }
        if finished {
            self.job = None;
            self.step = None;
            // What is on disk changed, and the download size with it.
            self.detect = None;
            self.detect();
        }
        changed
    }

    fn save(&mut self) {
        if let Err(e) = companion::save_settings(&self.settings) {
            self.error = Some(format!("Could not save the companion's settings: {e}"));
        }
    }

    fn set_brain(&mut self, choice: BrainChoice) {
        self.settings.brain = choice;
        self.tested = None;
        self.save();
    }

    fn start(&mut self) {
        self.error = None;
        self.note = None;
        self.step = Some(("Starting".into(), None));
        self.job = Some(companion::set_up(self.settings.clone()));
    }

    fn test(&mut self) {
        let Some(machine) = self.machine.clone() else {
            return;
        };
        self.error = None;
        self.tested = None;
        self.step = Some(("Starting the brain".into(), None));
        self.job = Some(companion::test_brain(self.settings.clone(), machine));
    }

    /// Whether what the settings pick is on disk.
    fn ready(&self) -> bool {
        if !self.settings.set_up {
            return false;
        }
        match &self.settings.brain {
            BrainChoice::Gguf { .. } | BrainChoice::Server { .. } => true,
            _ => self.plan.as_ref().is_some_and(|plan| {
                let options = companion::options();
                let installed = companion::installed_ids();
                self.settings
                    .selection(plan, &options)
                    .iter()
                    .all(|o| installed.contains(&o.id))
            }),
        }
    }
}

fn mb_text(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

impl Launcher {
    /// A one-line text field for Advanced, like the game arguments' field. The key shows as dots.
    fn companion_field(
        &self,
        field: Field,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = self.theme;
        let (_, value, focus, focused) = self
            .companion
            .fields
            .iter()
            .find(|f| f.0 == field)
            .expect("every field");
        let focused = *focused;
        let shown: String = if field == Field::CloudKey {
            "•".repeat(value.chars().count().min(32))
        } else {
            let chars: Vec<char> = value.chars().collect();
            chars[chars.len().saturating_sub(40)..].iter().collect()
        };
        let id = match field {
            Field::CloudUrl => "companion-cloud-url",
            Field::CloudModel => "companion-cloud-model",
            Field::CloudKey => "companion-cloud-key",
            Field::Gguf => "companion-gguf",
        };
        let empty = value.is_empty();
        div()
            .id(id)
            .track_focus(focus)
            .flex()
            .items_center()
            .w(px(280.))
            .h(px(30.))
            .px(px(12.))
            .overflow_hidden()
            .rounded(px(6.))
            .border_1()
            .border_color(if focused { t.accent } else { t.rule })
            .bg(t.surface)
            .font_family(theme::MONO)
            .text_size(px(12.))
            .cursor_text()
            .child(if empty && !focused {
                div().text_color(t.muted).child(placeholder)
            } else {
                div().text_color(t.ink).child(shown)
            })
            .when(focused, |d| d.child(div().w(px(1.)).h(px(14.)).bg(t.ink)))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if let Some(f) = this.companion.fields.iter().find(|f| f.0 == field) {
                    window.focus(&f.2);
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                let paste = k.key == "v" && (k.modifiers.control || k.modifiers.platform);
                match k.key.as_str() {
                    "backspace" => {
                        this.companion.value_mut(field).pop();
                    }
                    _ if paste => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                            this.companion.value_mut(field).push_str(text.trim());
                        }
                    }
                    _ if !k.modifiers.control && !k.modifiers.platform => {
                        if let Some(ch) = &k.key_char {
                            this.companion.value_mut(field).push_str(ch);
                        }
                    }
                    _ => return,
                }
                this.companion.value_mut(field).truncate(2048);
                cx.stop_propagation();
                cx.notify();
            }))
    }

    /// Settings' AI companion section.
    pub(crate) fn companion_section(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let no_ai = self.companion.settings.no_ai;
        if !no_ai && self.companion.machine.is_none() {
            self.companion.detect();
        }
        let no_ai_switch =
            switch("no-ai", no_ai, &t).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let c = &mut this.companion;
                c.settings.no_ai = !c.settings.no_ai;
                c.confirm_delete = false;
                c.note = None;
                if let Some(job) = &c.job {
                    job.cancel();
                }
                c.save();
                if !c.settings.no_ai {
                    c.detect();
                }
                cx.notify();
            }));
        let mut list = div()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.rule)
            .child(index_row(
                1,
                "No AI",
                Some(
                    div()
                        .child("Hides the companion and every AI feature, here and in the game. Nothing AI downloads or runs.")
                        .into_any_element(),
                ),
                Some(no_ai_switch.into_any_element()),
                &t,
            ));
        if no_ai {
            let on_disk: u64 = companion::installed_models().iter().map(|m| m.1).sum();
            if on_disk > 0 {
                let confirm = self.companion.confirm_delete;
                let delete = button(
                    "companion-delete-all",
                    if confirm { "Delete them?" } else { "Delete" },
                    Kind::Secondary,
                    &t,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    let c = &mut this.companion;
                    if c.confirm_delete {
                        c.note = Some(match companion::delete_everything() {
                            Ok(freed) => {
                                format!("Deleted: {} freed.", companion::gigabytes(freed))
                            }
                            Err(e) => format!("Could not delete everything: {e}"),
                        });
                        c.settings.set_up = false;
                        c.save();
                    }
                    c.confirm_delete = !c.confirm_delete;
                    cx.notify();
                }));
                list = list.child(index_row(
                    2,
                    "Downloaded models",
                    Some(
                        div()
                            .child(format!(
                                "{} of companion models are still on this computer.",
                                companion::gigabytes(on_disk)
                            ))
                            .into_any_element(),
                    ),
                    Some(delete.into_any_element()),
                    &t,
                ));
            }
            return div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(text(theme::HEADING).text_color(t.ink).child("AI companion"))
                .child(list)
                .children(
                    self.companion
                        .note
                        .clone()
                        .map(|n| text(theme::UI).text_color(t.ink).child(n)),
                );
        }

        let c = &self.companion;
        let machine = c
            .machine
            .as_ref()
            .map_or("Checking this PC…".to_string(), |m| m.summary());
        let plan = c
            .plan
            .as_ref()
            .map_or("Working out what this PC can run…".to_string(), |p| {
                p.summary.clone()
            });
        let busy = c.job.is_some();
        let ready = c.ready();
        let status: AnyElement = match (&c.step, busy, ready) {
            (Some((what, bar)), true, _) => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .w(px(260.))
                        .child(text(theme::CAPTION).text_color(t.muted).child(what.clone()))
                        .child(progress_bar(*bar, &t)),
                )
                .child(
                    button("companion-cancel", "Cancel", Kind::Ghost, &t).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            if let Some(job) = &this.companion.job {
                                job.cancel();
                            }
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
            (_, _, true) => div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(6.)).rounded_full().bg(t.success))
                .child(text(theme::UI).text_color(t.ink).child("Ready"))
                .into_any_element(),
            _ => {
                let can = c.plan.as_ref().is_some_and(|p| !p.chat_only())
                    || matches!(
                        c.settings.brain,
                        BrainChoice::Gguf { .. } | BrainChoice::Server { .. }
                    );
                let b = button("companion-set-up", "Set up companion", Kind::Primary, &t);
                if can {
                    b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.companion.start();
                        cx.notify();
                    }))
                    .into_any_element()
                } else {
                    disabled(b).into_any_element()
                }
            }
        };
        let advanced_label = if c.advanced {
            "Hide Advanced"
        } else {
            "Advanced"
        };
        list = list
            .child(index_row(
                2,
                "Companion",
                Some(
                    div()
                        .child("An AI teammate that plays with you: it follows, fights, gathers, crafts and talks. It runs on this computer, offline, and nothing you say leaves it.")
                        .into_any_element(),
                ),
                Some(status),
                &t,
            ))
            .child(index_row(
                3,
                "This PC",
                Some(div().child(machine).into_any_element()),
                None,
                &t,
            ))
            .child(index_row(
                4,
                "Setup",
                Some(div().child(plan).into_any_element()),
                Some(
                    button("companion-advanced", advanced_label, Kind::Ghost, &t)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.companion.advanced = !this.companion.advanced;
                            cx.notify();
                        }))
                        .into_any_element(),
                ),
                &t,
            ));
        let voice = self.companion.settings.voice;
        list = list.child(index_row(
            5,
            "Voice",
            Some(
                div()
                    .child("Hold V in the game to talk to it, and hear it answer. The microphone opens only while the key is held. Set up again after turning it on to fetch the voice.")
                    .into_any_element(),
            ),
            Some(
                switch("companion-voice", voice, &t)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        let c = &mut this.companion;
                        c.settings.voice = !c.settings.voice;
                        c.save();
                        c.detect = None;
                        c.detect();
                        cx.notify();
                    }))
                    .into_any_element(),
            ),
            &t,
        ));
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(text(theme::HEADING).text_color(t.ink).child("AI companion"))
            .child(list)
            .children(c.error.clone().map(|e| notice(e, &t)))
            .children(
                c.note
                    .clone()
                    .map(|n| text(theme::UI).text_color(t.ink).child(n)),
            );
        if self.companion.advanced {
            section = section.child(self.companion_advanced(cx));
        }
        section
    }

    fn companion_advanced(&mut self, cx: &mut Context<Self>) -> Div {
        let t = self.theme;
        let options = companion::options();
        let installed = companion::installed_ids();
        let c = &self.companion;
        let busy = c.job.is_some();
        let plan_pick = c
            .plan
            .as_ref()
            .and_then(|p| p.pick(Part::Brain))
            .map(|p| p.option.id.clone());
        let mut rows = div().flex().flex_col().border_t_1().border_color(t.rule);
        let mut n = 0;
        let mut next = || {
            n += 1;
            n
        };
        // The brain: automatic, then every catalogue option.
        let auto = c.settings.brain == BrainChoice::Auto;
        rows = rows.child(index_row(
            next(),
            "Brain: automatic",
            Some(
                div()
                    .child(match &plan_pick {
                        Some(id) => format!("The plan's choice for this PC: {id}"),
                        None => "This PC can't run a brain next to the game.".into(),
                    })
                    .into_any_element(),
            ),
            Some(if auto {
                text(theme::CAPTION)
                    .text_color(t.muted)
                    .child("In use")
                    .into_any_element()
            } else {
                button("companion-brain-auto", "Use", Kind::Secondary, &t)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.companion.set_brain(BrainChoice::Auto);
                        cx.notify();
                    }))
                    .into_any_element()
            }),
            &t,
        ));
        for option in options.iter().filter(|o| o.part == Part::Brain) {
            let Some(companion::Component::Brain(model)) = companion::model_of(&option.id) else {
                continue;
            };
            let chosen = c.settings.brain
                == BrainChoice::Option {
                    id: option.id.clone(),
                };
            let on_disk = installed.contains(&option.id);
            let gpu = option.runs == Runs::Gpu;
            let detail = format!(
                "{} · {} · {} · {}K context · {} download · needs {} {}{}",
                model.name,
                model.quantisation,
                model.license,
                horizon_companion::runtime::CONTEXT / 1024,
                companion::gigabytes(model.size()),
                mb_text(if gpu { option.vram_mb } else { option.ram_mb }),
                if gpu { "of graphics memory" } else { "of RAM" },
                if on_disk { " · downloaded" } else { "" },
            );
            let id = option.id.clone();
            let right = if chosen {
                text(theme::CAPTION)
                    .text_color(t.muted)
                    .child("In use")
                    .into_any_element()
            } else {
                button(
                    SharedString::from(format!("companion-use-{}", option.id)),
                    "Use",
                    Kind::Secondary,
                    &t,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.companion
                        .set_brain(BrainChoice::Option { id: id.clone() });
                    // Already set up: fetch this one now if it isn't on disk.
                    if this.companion.settings.set_up
                        && !this.companion.ready()
                        && this.companion.job.is_none()
                    {
                        this.companion.start();
                    }
                    cx.notify();
                }))
                .into_any_element()
            };
            rows = rows.child(index_row(
                next(),
                format!(
                    "{} brain{}",
                    option.label,
                    if gpu { "" } else { ", on the processor" }
                ),
                Some(div().child(detail).into_any_element()),
                Some(right),
                &t,
            ));
        }

        // The senses: command recognition, then speech recognition and the voice (with Voice on).
        for (part, title) in [
            (Part::Classifier, "Command recognition"),
            (Part::SpeechToText, "Speech recognition"),
            (Part::TextToSpeech, "Voice"),
        ] {
            if part != Part::Classifier && !c.settings.voice {
                continue;
            }
            let choice = c.settings.choice(part);
            let picked = c.plan.as_ref().and_then(|p| p.pick(part)).map_or_else(
                || "nothing on this PC".to_owned(),
                |p| p.option.name.clone(),
            );
            let choices = std::iter::once((
                PartChoice::Auto,
                format!("{title}: automatic"),
                format!("The plan's choice: {picked}"),
            ))
            .chain(options.iter().filter(|o| o.part == part).map(|o| {
                let on_disk = installed.contains(&o.id);
                (
                    PartChoice::Option { id: o.id.clone() },
                    format!(
                        "{}{}",
                        o.label,
                        if o.runs == Runs::Gpu {
                            ", on the graphics card"
                        } else {
                            ""
                        }
                    ),
                    format!(
                        "{} · {} · {} download · needs {}{}",
                        o.name,
                        o.detail,
                        companion::gigabytes(o.download),
                        mb_text(if o.runs == Runs::Gpu {
                            o.vram_mb
                        } else {
                            o.ram_mb
                        }),
                        if on_disk { " · downloaded" } else { "" },
                    ),
                )
            }))
            .chain(std::iter::once((
                PartChoice::Off,
                format!("{title}: off"),
                "Not used.".to_owned(),
            )));
            for (value, label, detail) in choices {
                let right = if value == choice {
                    text(theme::CAPTION)
                        .text_color(t.muted)
                        .child("In use")
                        .into_any_element()
                } else {
                    let id = SharedString::from(format!("companion-part-{label}"));
                    button(id, "Use", Kind::Secondary, &t)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let s = &mut this.companion.settings;
                            match part {
                                Part::Classifier => s.classifier = value.clone(),
                                Part::SpeechToText => s.speech_to_text = value.clone(),
                                _ => s.text_to_speech = value.clone(),
                            }
                            this.companion.save();
                            cx.notify();
                        }))
                        .into_any_element()
                };
                rows = rows.child(index_row(
                    next(),
                    label,
                    Some(div().child(detail).into_any_element()),
                    Some(right),
                    &t,
                ));
            }
        }

        // The voice it speaks with, on the voice model that runs (each personality has its own).
        if c.settings.voice {
            let tts = c.plan.as_ref().and_then(|plan| {
                c.settings
                    .selection(plan, &options)
                    .into_iter()
                    .find(|o| o.part == Part::TextToSpeech)
                    .and_then(|o| match companion::model_of(&o.id) {
                        Some(companion::Component::Sense(m)) => Some(m),
                        _ => None,
                    })
            });
            if let Some(model) = tts {
                let default =
                    horizon_companion::senses::default_voice(model, c.settings.persona.personality);
                let chosen = c.settings.voices.get(model.id).cloned();
                let auto_detail = format!(
                    "The personality's: {}. Play a sample in the game's Companion settings.",
                    default.map_or("the model's first", |v| v.label)
                );
                let picks =
                    std::iter::once((None, "Voice: the personality's".to_owned(), auto_detail))
                        .chain(model.voices.iter().map(|v| {
                            (
                                Some(v.id.to_owned()),
                                format!("Voice: {}", v.label),
                                v.description.to_owned(),
                            )
                        }));
                for (value, label, detail) in picks {
                    let right = if value == chosen {
                        text(theme::CAPTION)
                            .text_color(t.muted)
                            .child("In use")
                            .into_any_element()
                    } else {
                        let id = SharedString::from(format!("companion-voice-{label}"));
                        let model_id = model.id;
                        button(id, "Use", Kind::Secondary, &t)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                let voices = &mut this.companion.settings.voices;
                                match &value {
                                    Some(v) => voices.insert(model_id.to_owned(), v.clone()),
                                    None => voices.remove(model_id),
                                };
                                this.companion.save();
                                cx.notify();
                            }))
                            .into_any_element()
                    };
                    rows = rows.child(index_row(
                        next(),
                        label,
                        Some(div().child(detail).into_any_element()),
                        Some(right),
                        &t,
                    ));
                }
            }
        }

        // Another AI program on this PC.
        let found: AnyElement = match (&c.servers, &c.finding) {
            (_, Some(_)) => text(theme::CAPTION)
                .text_color(t.muted)
                .child("Looking…")
                .into_any_element(),
            (Some(list), None) if list.is_empty() => text(theme::CAPTION)
                .text_color(t.muted)
                .child("Neither is running.")
                .into_any_element(),
            (Some(list), None) => {
                let mut buttons = div().flex().flex_wrap().gap(px(8.));
                for server in list {
                    let name = match server.provider {
                        Provider::Ollama => "Ollama",
                        _ => "LM Studio",
                    };
                    for model in server.models.iter().take(4) {
                        let choice = BrainChoice::Server {
                            provider: server.provider,
                            base_url: server.base_url.clone(),
                            model: model.clone(),
                        };
                        let chosen = c.settings.brain == choice;
                        buttons = buttons.child(
                            button(
                                SharedString::from(format!("companion-server-{name}-{model}")),
                                format!("{name}: {model}{}", if chosen { " (in use)" } else { "" }),
                                Kind::Secondary,
                                &t,
                            )
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    this.companion.set_brain(choice.clone());
                                    cx.notify();
                                },
                            )),
                        );
                    }
                }
                buttons.into_any_element()
            }
            (None, None) => button(
                "companion-find-servers",
                "Look for them",
                Kind::Secondary,
                &t,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.companion.finding = Some(client::detect_in_background());
                cx.notify();
            }))
            .into_any_element(),
        };
        rows = rows.child(index_row(
            next(),
            "Another AI program",
            Some(
                div()
                    .child("Ollama or LM Studio, if one runs on this PC.")
                    .into_any_element(),
            ),
            Some(found),
            &t,
        ));

        // A cloud model: address, model and key; the key goes to the keychain, never the file.
        let cloud_fields = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.companion_field(Field::CloudUrl, "https://api.example.com/v1", cx))
            .child(self.companion_field(Field::CloudModel, "Model name", cx))
            .child(self.companion_field(Field::CloudKey, "API key (kept in the keychain)", cx))
            .child(
                button(
                    "companion-cloud-use",
                    "Use this cloud model",
                    Kind::Secondary,
                    &t,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    let c = &mut this.companion;
                    let url = c.value(Field::CloudUrl).trim().to_owned();
                    let model = c.value(Field::CloudModel).trim().to_owned();
                    if !url.starts_with("http") || model.is_empty() {
                        c.error = Some(
                            "A cloud model needs its address (https://…/v1) and a model name."
                                .into(),
                        );
                        cx.notify();
                        return;
                    }
                    let choice = BrainChoice::Server {
                        provider: Provider::Cloud,
                        base_url: url,
                        model,
                    };
                    c.error = None;
                    let key = std::mem::take(c.value_mut(Field::CloudKey));
                    if let (false, Some(account)) = (key.is_empty(), choice.key_account()) {
                        if let Err(e) = keychain::set(&account, &key) {
                            c.error =
                                Some(format!("The key could not be saved in the keychain: {e}"));
                        }
                    }
                    c.set_brain(choice);
                    cx.notify();
                })),
            );
        rows = rows.child(index_row(
            next(),
            "Cloud model",
            Some(
                div()
                    .child("Any OpenAI-compatible service, with your own key. Conversations then leave this PC.")
                    .into_any_element(),
            ),
            Some(cloud_fields.into_any_element()),
            &t,
        ));
        let gguf = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.companion_field(Field::Gguf, "/path/to/model.gguf", cx))
            .child(
                button("companion-gguf-use", "Use this file", Kind::Secondary, &t).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        let path =
                            std::path::PathBuf::from(this.companion.value(Field::Gguf).trim());
                        if path.extension().and_then(|e| e.to_str()) != Some("gguf")
                            || !path.is_file()
                        {
                            this.companion.error =
                                Some("That is not a .gguf file on this computer.".into());
                        } else {
                            this.companion.error = None;
                            this.companion.set_brain(BrainChoice::Gguf { path });
                        }
                        cx.notify();
                    }),
                ),
            );
        rows = rows.child(index_row(
            next(),
            "Your own model",
            Some(
                div()
                    .child("A GGUF file, run by the companion's runtime.")
                    .into_any_element(),
            ),
            Some(gguf.into_any_element()),
            &t,
        ));

        // Who it is.
        let c = &self.companion;
        let this = cx.entity().downgrade();
        let presets: Vec<(Personality, &'static str)> = Personality::ALL
            .iter()
            .filter(|p| **p != Personality::Custom)
            .map(|p| (*p, p.label()))
            .collect();
        let personality = segments(
            "companion-personality",
            &presets,
            c.settings.persona.personality,
            &t,
            move |p, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.persona.personality = p;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Personality",
            Some(
                div()
                    .child(c.settings.persona.personality.blurb())
                    .into_any_element(),
            ),
            Some(personality.into_any_element()),
            &t,
        ));
        let this = cx.entity().downgrade();
        let roles: Vec<(Role, &'static str)> = Role::ALL.iter().map(|r| (*r, r.label())).collect();
        let role = segments(
            "companion-role",
            &roles,
            c.settings.persona.role,
            &t,
            move |r, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.persona.role = r;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Role",
            Some(
                div()
                    .child(c.settings.persona.role.blurb())
                    .into_any_element(),
            ),
            Some(role.into_any_element()),
            &t,
        ));
        let this = cx.entity().downgrade();
        let perceptions: Vec<(Perception, &'static str)> =
            Perception::ALL.iter().map(|p| (*p, p.label())).collect();
        let perception = segments(
            "companion-perception",
            &perceptions,
            c.settings.persona.perception,
            &t,
            move |p, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.persona.perception = p;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Perception",
            Some(
                div()
                    .child(c.settings.persona.perception.blurb())
                    .into_any_element(),
            ),
            Some(perception.into_any_element()),
            &t,
        ));
        // Salty: real swearing, for adults only (confirmed once), never with Kid-friendly.
        let kid = c.settings.persona.kid_friendly;
        let salty = switch("companion-salty", c.settings.persona.salty_on(), &t).on_click(
            cx.listener(|this, _: &ClickEvent, _, cx| {
                let c = &mut this.companion;
                let p = &mut c.settings.persona;
                if p.salty_on() {
                    p.salty = false;
                } else if p.adult_confirmed {
                    p.salty = true;
                    p.kid_friendly = false;
                } else {
                    c.confirm_salty = true;
                }
                c.save();
                cx.notify();
            }),
        );
        rows = rows.child(index_row(
            next(),
            "Salty (18+)",
            Some(
                div()
                    .child(if kid {
                        "Real swearing and attitude. Not with Kid-friendly on."
                    } else {
                        "Real swearing and attitude, and an uncensored brain where one is chosen. Anything harmful is still refused. Off: clean."
                    })
                    .into_any_element(),
            ),
            Some(salty.into_any_element()),
            &t,
        ));
        if c.settings.persona.salty_on() {
            let brain = c.plan.as_ref().and_then(|plan| {
                c.settings
                    .selection(plan, &options)
                    .into_iter()
                    .find(|o| o.part == Part::Brain)
                    .and_then(|o| match companion::model_of(&o.id) {
                        Some(companion::Component::Brain(m)) => Some(m),
                        _ => None,
                    })
            });
            let uncensored = brain.and_then(|m| {
                horizon_companion::catalogue::Tier::ALL
                    .into_iter()
                    .find(|t| t.model().id == m.id)
                    .and_then(horizon_companion::catalogue::Tier::uncensored)
            });
            let detail = match uncensored {
                Some(m) => format!(
                    "{} · {} · {} download, fetched by Set up · {}",
                    m.name,
                    m.license,
                    companion::gigabytes(m.size()),
                    m.credit
                ),
                None => {
                    "This brain has no uncensored variant: it runs with the Salty prompt.".into()
                }
            };
            let on = switch(
                "companion-salty-uncensored",
                c.settings.salty_uncensored,
                &t,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                let c = &mut this.companion;
                c.settings.salty_uncensored = !c.settings.salty_uncensored;
                c.save();
                cx.notify();
            }));
            rows = rows.child(index_row(
                next(),
                "Salty brain: uncensored",
                Some(div().child(detail).into_any_element()),
                Some(on.into_any_element()),
                &t,
            ));
        }
        if c.confirm_salty {
            let confirm = div()
                .flex()
                .gap(px(8.))
                .child(
                    button(
                        "companion-salty-adult",
                        "I'm 18 or older",
                        Kind::Primary,
                        &t,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        let c = &mut this.companion;
                        c.confirm_salty = false;
                        let p = &mut c.settings.persona;
                        p.adult_confirmed = true;
                        p.salty = true;
                        p.kid_friendly = false;
                        c.save();
                        cx.notify();
                    })),
                )
                .child(
                    button("companion-salty-cancel", "Cancel", Kind::Secondary, &t).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.companion.confirm_salty = false;
                            cx.notify();
                        }),
                    ),
                );
            rows = rows.child(index_row(
                next(),
                "Salty is for adults",
                Some(
                    div()
                        .child("Confirm that you are 18 or older to turn it on.")
                        .into_any_element(),
                ),
                Some(confirm.into_any_element()),
                &t,
            ));
        }
        let kids = switch("companion-kids", c.settings.persona.kid_friendly, &t).on_click(
            cx.listener(|this, _: &ClickEvent, _, cx| {
                let c = &mut this.companion;
                let p = &mut c.settings.persona;
                p.kid_friendly = !p.kid_friendly;
                if p.kid_friendly {
                    p.salty = false;
                    c.confirm_salty = false;
                }
                c.save();
                cx.notify();
            }),
        );
        rows = rows.child(index_row(
            next(),
            "Kid-friendly",
            Some(
                div()
                    .child("Gentle humour, nothing scary, never swearing. Turns Salty off.")
                    .into_any_element(),
            ),
            Some(kids.into_any_element()),
            &t,
        ));

        // Tuning.
        let this = cx.entity().downgrade();
        let temperature = segments(
            "companion-temperature",
            &[
                (None, "Auto"),
                (Some(0.3), "Steady"),
                (Some(0.7), "Lively"),
                (Some(1.0), "Wild"),
            ],
            c.settings.tuning.temperature,
            &t,
            move |v, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.tuning.temperature = v;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Temperature",
            Some(
                div()
                    .child("How much its answers vary. Auto is 0.7.")
                    .into_any_element(),
            ),
            Some(temperature.into_any_element()),
            &t,
        ));
        let this = cx.entity().downgrade();
        let threads = segments(
            "companion-threads",
            &[
                (None, "Auto"),
                (Some(2), "2"),
                (Some(4), "4"),
                (Some(8), "8"),
            ],
            c.settings.tuning.threads,
            &t,
            move |v, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.tuning.threads = v;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Processor threads",
            Some(
                div()
                    .child("Auto leaves the game its share.")
                    .into_any_element(),
            ),
            Some(threads.into_any_element()),
            &t,
        ));
        let this = cx.entity().downgrade();
        let gpu = segments(
            "companion-gpu-layers",
            &[(None, "Auto"), (Some(0), "Processor only")],
            c.settings.tuning.gpu_layers,
            &t,
            move |v, _, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.companion.settings.tuning.gpu_layers = v;
                    this.companion.save();
                    cx.notify();
                });
            },
        );
        rows = rows.child(index_row(
            next(),
            "Graphics card",
            Some(
                div()
                    .child("Auto puts the whole brain on the graphics card when it fits.")
                    .into_any_element(),
            ),
            Some(gpu.into_any_element()),
            &t,
        ));

        // A live test: load time, first word, memory.
        let measured = c.tested.as_ref().map_or_else(
            || {
                "Starts the brain, asks it one thing and stops it: how long it takes and how much memory it uses."
                    .to_string()
            },
            |r| {
                format!(
                    "Loaded in {:.1} s · first word {:.2} s · whole reply {:.2} s · {} · {}  “{}”",
                    r.load.as_secs_f64(),
                    r.first_word.as_secs_f64(),
                    r.total.as_secs_f64(),
                    r.ram_mb
                        .map_or("memory unknown".into(), |m| format!("{} in use", mb_text(m))),
                    r.tokens_per_second
                        .map_or(String::new(), |s| format!("{s:.0} tokens/s")),
                    r.say
                )
            },
        );
        let test = button("companion-test", "Test", Kind::Secondary, &t);
        let test = if busy || !c.ready() {
            disabled(test)
        } else {
            test.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.companion.test();
                cx.notify();
            }))
        };
        rows = rows.child(index_row(
            next(),
            "Test the brain",
            Some(div().child(measured).into_any_element()),
            Some(test.into_any_element()),
            &t,
        ));

        // What is on disk.
        let models = companion::installed_models();
        if models.is_empty() {
            rows = rows.child(index_row(
                next(),
                "Downloaded",
                Some(div().child("Nothing yet.").into_any_element()),
                None,
                &t,
            ));
        }
        for (model, size) in models {
            let (repo, revision) = model.source();
            rows = rows.child(index_row(
                next(),
                model.name(),
                Some(
                    div()
                        .font_family(theme::MONO)
                        .child(format!(
                            "{}  ·  {}  ·  {} at {}",
                            companion::gigabytes(size),
                            model.license(),
                            repo,
                            &revision[..8]
                        ))
                        .into_any_element(),
                ),
                Some(
                    button(
                        SharedString::from(format!("companion-delete-{}", model.id())),
                        "Delete",
                        Kind::Ghost,
                        &t,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Err(e) = companion::delete_model(model) {
                            this.companion.error = Some(e);
                        }
                        this.companion.detect = None;
                        this.companion.detect();
                        cx.notify();
                    }))
                    .into_any_element(),
                ),
                &t,
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(caps("Advanced").text_color(t.muted))
            .child(rows)
            .child(text(theme::CAPTION).text_color(t.muted).child(format!(
                "Runtime: llama.cpp {} ({}), {}. Models from Hugging Face at pinned versions, checked with SHA-256.",
                horizon_companion::catalogue::LLAMA_CPP_BUILD,
                horizon_companion::catalogue::LLAMA_CPP_LICENSE,
                horizon_companion::catalogue::LLAMA_CPP_CREDIT
            )))
    }
}
