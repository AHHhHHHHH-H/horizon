//! The companion's mind in the game (docs/companions.md, "In the game"): what it hears, how a line
//! is routed (rules, then the classifier, then the brain), what it says back in chat and aloud, and
//! the processes behind it.
//!
//! Nothing starts until a world with a companion in it is open and the companion was set up (the
//! launcher's or the Companion section's "Set up companion"): then one worker thread starts the
//! classifier's embedding server and the brain's llama-server and keeps them until the world
//! closes; the voice sidecar starts beside them (game/companion_voice.rs). The game thread only
//! polls channels, once a tick. Without a setup, the rules and the phrase bank still answer
//! commands: they are code, not models. With No AI, nothing here runs and companions stay away.
use super::{Game, companion::companion_index};
use horizon_client::{
    companion::{self as c, CompanionReport},
    lang,
    text::{self, Text},
};
use horizon_companion::senses::{SenseKind, SenseModel};
use horizon_companion::{
    catalogue::Tier,
    client::{Brain, ChatRequest, Endpoint, Event as BrainEvent, Message, Stats},
    reply::Reply,
    guard::{self, Trip},
    moderation::{Gate, GateEvent, GuardClient, Mode},
    partner,
    phrases::{self, Ack, Cooldowns, Range},
    plan::Part,
    prompt::{self, Budget, Exchange, Input, Perception, Persona, Role},
    reflex::{self, Intent, NameIndex, Parse, Route, Stage, Target},
    reply::{Action, Skill as BrainSkill, Tone},
    runtime::{Runtime, State},
    settings::CompanionSettings,
    setup::{BrainPart, ClassifierPart, Machine, Setup, VoicePart},
};
use horizon_senses::{
    classifier::{self, Classifier, EmbedServer, ReflexClassifier},
    voice::{Installed, Speak, VoiceSetup},
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

/// With no sentence from the brain after this, the thinking line covers the wait.
const THINK_AFTER: Duration = Duration::from_millis(1200);
/// A brain expected to take this long before its first word gets the thinking line at once.
const THINK_AT_ONCE_MS: u32 = 1500;
/// Memory notes kept per companion (the prompt keeps the newest that fit its budget).
const NOTES: usize = 24;
/// The owner losing this much health in one tick is worth a word.
const BIG_HIT: f32 = 6.0;
/// Super strong's effects last this long and are renewed before they run out (ticks).
const BOOST_TICKS: i32 = 600;
const BOOST_EVERY: u64 = 200;

/// The mind's state on the game thread (`Game::companion_ai`).
#[derive(Default)]
pub(super) struct AiState {
    /// Read when a world opens and when the Companion section saves.
    settings: Option<CompanionSettings>,
    worker: Option<Worker>,
    brain: Option<Brain>,
    /// The warm-up request, whose answer is ignored.
    warm_up: Option<u64>,
    /// Light's brain runs only some of its actions (`reply::Skill::allowed_for`).
    tier: Option<Tier>,
    /// What the plan expects before the brain's first word, in ms.
    first_word_ms: Option<u32>,
    classifier_ready: bool,
    /// The brain could not start: lines that need it get the "huh" line.
    brain_failed: bool,
    names: Option<NameIndex>,
    minds: Vec<Mind>,
    /// Lines heard from guests (the hub) and push-to-talk, routed on the next tick.
    heard: Vec<(String, String, bool)>,
    /// Lines the classifier is looking at: (id, companion, who, line).
    classifying: Vec<(u64, String, String, String)>,
    /// Reports from the bodies since the last tick.
    reports: Vec<(String, CompanionReport)>,
    /// Lines for the brain while it loads: (companion, input).
    queued: Vec<(String, Input)>,
    next_id: u64,
    cooldowns: Cooldowns,
    last_remark: Option<f64>,
    owner_health: Option<f32>,
    /// The companion everything typed goes to (`@Name` alone switches it on and off).
    pub chat_mode: Option<String>,
    /// For the Companion section and `companion ai`.
    pub status: Vec<String>,
    /// The last turns' timings, newest last.
    pub timings: Vec<Value>,
    /// What the companions said lately (`companion ai`), newest last.
    pub said: Vec<Value>,
    /// The world was created with "Start with a companion", and the role picked there.
    pub spawn_on_arrival: bool,
    pub spawn_role: Option<Role>,
    /// Replies the guard stopped this session (`companion ai`).
    pub guarded: u32,
    /// The safety gate between the brain and the chat (`moderation::Gate`): the guard model's
    /// verdicts, the keyword list in front.
    gate: Gate,
    /// The guard is part of what runs: Salty mode can be on (`setup::Parts::guard`).
    guard_planned: bool,
    /// The guard failed while Salty was on: the companion restarted clean for this world.
    salty_suspended: bool,
    guard_ready: bool,
    /// The text-to-speech model the voice runs, for the voice the settings pick on it.
    /// The voice runs with the voice pack: cloning models speak as the cast.
    cast: bool,
    tts: Option<&'static SenseModel>,
}

/// One companion's side of the conversation.
struct Mind {
    uuid: String,
    name: String,
    system: String,
    history: Vec<Exchange>,
    /// The request in flight, what was sent, and when.
    asking: Option<(u64, Vec<Message>, Instant)>,
    /// Whether a sentence of it was said (and after how long), and whether the thinking line was.
    spoke: bool,
    first_said: Option<Duration>,
    thought: bool,
    /// The line came from someone (not an event): it deserves the thinking line.
    said: bool,
    /// What the body couldn't do last turn, told to the brain with the next state.
    feedback: Vec<String>,
    step: Option<partner::Step>,
    /// The brain turn the gate holds (its request id), from the ask to the last sentence passed.
    turn: Option<u64>,
    /// The sentences of the turn the gate holds, as the brain wrote them.
    held: Vec<(String, Tone)>,
    /// The finished reply, waiting for the gate to pass its last sentence.
    finished: Option<Finished>,
}

/// A complete reply the gate hasn't released yet.
struct Finished {
    reply: Reply,
    raw: String,
    stats: Stats,
    sent: Vec<Message>,
    asked: Instant,
}

/// The worker thread: started with the world, stopped (and its processes with it) when the world
/// closes.
struct Worker {
    to: Sender<ToWorker>,
    from: Receiver<FromWorker>,
}

enum ToWorker {
    Classify {
        id: u64,
        line: String,
        language: String,
    },
}

enum FromWorker {
    Planned {
        tier: Option<Tier>,
        first_word_ms: Option<u32>,
        voice: Option<VoicePart>,
        missing: Vec<String>,
        brain: Option<String>,
        /// Salty mode runs (it needs the guard).
        salty: bool,
        /// The guard is part of the setup and starting.
        guard: bool,
    },
    Classifier(Result<(), String>),
    Brain(Result<Endpoint, String>),
    Guard(Result<Endpoint, String>),
    Routed {
        id: u64,
        route: Route,
    },
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The worker sees its channel close and stops its servers; nothing waits for it.
        log::info!("[companion] stopping the brain and the classifier");
    }
}

impl AiState {
    /// A guest said something in chat (`HostEvent::Said`).
    pub fn guest_said(&mut self, name: String, text: String) {
        if self.settings.as_ref().is_some_and(|s| !s.no_ai) {
            self.heard.push((name, text, false));
        }
    }

    /// A body's report, for callouts and the brain.
    pub fn report(&mut self, name: &str, report: &CompanionReport) {
        if self.settings.is_some() {
            self.reports.push((name.to_owned(), report.clone()));
        }
    }
}

/// The game's home (where `options.txt` is): the companion's files are in `<home>/companion/`.
pub(super) fn home() -> PathBuf {
    let options = Game::options_path();
    options.parent().map(PathBuf::from).unwrap_or_default()
}

impl Game<'_> {
    /// No AI, from the extras file or the companion's settings.
    pub(super) fn no_ai(&self) -> bool {
        self.settings.extras.no_ai || self.companion_ai.settings.as_ref().is_some_and(|s| s.no_ai)
    }

    /// The companion settings, read once per world.
    pub(super) fn companion_settings(&mut self) -> &CompanionSettings {
        self.companion_ai
            .settings
            .get_or_insert_with(|| CompanionSettings::load(&home()))
    }

    /// The Companion section saved: new settings take effect (the processes restart if a part
    /// changed; persona changes reach the next turn).
    pub(super) fn companion_settings_changed(&mut self, settings: CompanionSettings) {
        let old = self.companion_ai.settings.replace(settings.clone());
        self.settings.extras.no_ai = settings.no_ai;
        let parts_changed = old.as_ref().is_none_or(|o| {
            o.brain != settings.brain
                || o.classifier != settings.classifier
                || o.speech_to_text != settings.speech_to_text
                || o.text_to_speech != settings.text_to_speech
                || o.voice != settings.voice
                || o.set_up != settings.set_up
                || o.tuning != settings.tuning
                || o.no_ai != settings.no_ai
                // Salty may swap the brain for its uncensored variant.
                || (o.persona.salty_on() && o.salty_uncensored) != (settings.persona.salty_on() && settings.salty_uncensored)
        });
        if parts_changed && self.companion_ai.worker.is_some() {
            self.companion_ai_stop();
        }
        self.companion_apply_persona(settings.persona.role, settings.persona.perception);
        if parts_changed {
            // Salty again once the new plan says the guard runs.
            self.companion_ai.guard_planned = false;
            self.companion_ai.salty_suspended = false;
        }
        self.refresh_systems();
        if settings.no_ai {
            self.companions_hide_for_no_ai();
        }
        // A newly picked voice (or personality) gets its clips ready before it speaks.
        self.companion_voice_prepare();
    }

    /// No AI: the companions leave, saved as they are, to come back with the world once it is off.
    fn companions_hide_for_no_ai(&mut self) {
        self.companions.pending.clear();
        self.companions.parked.clear();
        let Some(mut host) = self.lan.take() else {
            return;
        };
        while !host.companions.is_empty() {
            self.companion_leave(&mut host, 0, true);
        }
        let close = host.private;
        self.lan = Some(host);
        if close {
            self.lan_close(false);
        }
    }

    /// The settings' persona, as it runs: Salty only when the guard runs beside it.
    fn persona(&mut self) -> Persona {
        let can = self.salty_can_run();
        let mut persona = self.companion_settings().persona.clone();
        if !can {
            persona.salty = false;
        }
        persona
    }

    fn salty_can_run(&self) -> bool {
        self.companion_ai.guard_planned && !self.companion_ai.salty_suspended
    }

    /// The system prompts follow the persona as it runs; a changed one starts a new conversation.
    fn refresh_systems(&mut self) {
        let owner = self.owner_name();
        let language = self.settings.language.clone();
        let uuids: Vec<String> = self
            .companion_ai
            .minds
            .iter()
            .map(|m| m.uuid.clone())
            .collect();
        for (i, uuid) in uuids.iter().enumerate() {
            let system = prompt::system_prompt(&self.companion_persona(uuid), &owner, &language);
            let mind = &mut self.companion_ai.minds[i];
            if mind.system != system {
                mind.system = system;
                // A new character starts a new conversation (the cached prefix changed anyway).
                mind.history.clear();
            }
        }
        let persona = self.persona();
        self.companion_ai
            .gate
            .set_mode(Mode::of(&persona), persona.salty_on());
    }

    /// A companion's persona: the settings', with its name, and the role and perception its save
    /// file keeps (each companion has its own, picked at world creation or in the Companion screen).
    fn companion_persona(&mut self, uuid: &str) -> Persona {
        let mut persona = self.persona();
        if let Some(b) = self
            .lan
            .as_ref()
            .and_then(|h| h.companions.iter().find(|b| b.file.uuid == uuid))
        {
            persona.name = b.file.name.clone();
            persona.role = Role::parse(&b.file.role).unwrap_or(persona.role);
            persona.perception =
                Perception::parse(&b.file.perception).unwrap_or(persona.perception);
        }
        persona
    }

    /// The role of the companion with this name.
    fn role_of(&mut self, name: &str) -> Role {
        let fallback = self.companion_settings().persona.role;
        self.lan
            .as_ref()
            .and_then(|h| h.companions.iter().find(|b| b.file.name == name))
            .and_then(|b| Role::parse(&b.file.role))
            .unwrap_or(fallback)
    }

    /// The perception of the companion with this name (the Watch HUD shows it).
    pub(super) fn perception_of(&mut self, name: &str) -> Perception {
        let fallback = self.companion_settings().persona.perception;
        self.lan
            .as_ref()
            .and_then(|h| h.companions.iter().find(|b| b.file.name == name))
            .and_then(|b| Perception::parse(&b.file.perception))
            .unwrap_or(fallback)
    }

    /// The Companion screen changed the role or perception: the companions in the world take them.
    pub(super) fn companion_apply_persona(&mut self, role: Role, perception: Perception) {
        if let Some(w) = self.companions.watch.as_mut() {
            w.perception = perception.label().to_owned();
        }
        let world = self.world_root.clone();
        let Some(host) = self.lan.as_mut() else {
            return;
        };
        for b in &mut host.companions {
            if b.file.role != role.id() || b.file.perception != perception.id() {
                b.file.role = role.id().into();
                b.file.perception = perception.id().into();
                if let Err(e) = b.file.save(&world) {
                    log::warn!("[companion] could not save {}: {e}", b.file.name);
                }
            }
        }
    }

    fn owner_name(&self) -> String {
        self.lan.as_ref().map_or_else(
            || self.options.player_name().to_string(),
            |h| h.host_name.clone(),
        )
    }

    /// The world closes: the processes stop, the conversation is forgotten (its notes are in the
    /// companions' files).
    pub(super) fn companion_ai_world_closed(&mut self) {
        self.companion_ai_stop();
        self.companion_ai = AiState::default();
    }

    pub(super) fn companion_ai_stop(&mut self) {
        let ai = &mut self.companion_ai;
        ai.worker = None;
        ai.brain = None;
        ai.warm_up = None;
        ai.classifier_ready = false;
        ai.brain_failed = false;
        ai.tier = None;
        ai.first_word_ms = None;
        ai.guard_ready = false;
        ai.gate = Gate::default();
        ai.classifying.clear();
        ai.status.clear();
        for mind in &mut ai.minds {
            mind.asking = None;
            mind.turn = None;
            mind.held.clear();
            mind.finished = None;
        }
        self.companion_voice_stop();
    }

    /// Once a tick: start or stop the processes, route what was heard, answer, call out.
    pub(super) fn tick_companion_ai(&mut self) {
        let have = self.lan.as_ref().is_some_and(|h| !h.companions.is_empty());
        let coming = !self.companions.pending.is_empty() || !self.companions.parked.is_empty();
        if !have
            && !coming
            && self.companion_ai.worker.is_none()
            && !self.companion_ai.spawn_on_arrival
        {
            // The common case: one check a tick.
            self.companion_ai.heard.clear();
            self.companion_ai.reports.clear();
            return;
        }
        if self.companion_ai.settings.is_none() {
            self.companion_settings();
        }
        if self.no_ai() {
            self.companion_ai.heard.clear();
            self.companion_ai.reports.clear();
            self.companion_ai.spawn_on_arrival = false;
            if have || coming {
                self.companions_hide_for_no_ai();
            }
            if self.companion_ai.worker.is_some() {
                self.companion_ai_stop();
            }
            return;
        }
        if self.companion_ai.spawn_on_arrival
            && self.player.is_some()
            && self.companions_ready_to_spawn()
        {
            self.companion_ai.spawn_on_arrival = false;
            let name = self.persona().name;
            match self.companion_spawn(c::valid_name(&name).as_deref()) {
                Ok(name) => {
                    // The role picked at world creation goes into its save file.
                    if let Some(role) = self.companion_ai.spawn_role.take() {
                        let world = self.world_root.clone();
                        if let Some(b) = self
                            .lan
                            .as_mut()
                            .and_then(|h| h.companions.iter_mut().find(|b| b.file.name == name))
                        {
                            b.file.role = role.id().into();
                            if let Err(e) = b.file.save(&world) {
                                log::warn!("[companion] could not save {name}: {e}");
                            }
                        }
                    }
                    self.system_message(Text::literal(format!("{name} joined you")));
                }
                Err(e) => log::warn!("[companion] could not start with a companion: {e}"),
            }
        }
        if have && self.companion_ai.worker.is_none() && self.companion_settings().set_up {
            self.companion_ai_start();
        }
        self.sync_minds();
        self.poll_worker();
        self.poll_brain();
        self.poll_gate();
        for (who, text, owner) in std::mem::take(&mut self.companion_ai.heard) {
            self.companion_hear(&who, &text, owner);
        }
        for text in self.companion_heard() {
            let who = self.owner_name();
            self.companion_hear_voice(&who, &text);
        }
        for (name, report) in std::mem::take(&mut self.companion_ai.reports) {
            self.companion_ai_report(&name, report);
        }
        self.owner_events();
        self.partner_progress();
        self.think_fillers();
        self.place_voices();
        self.boost_super_strong();
    }

    fn companions_ready_to_spawn(&self) -> bool {
        let Some(p) = self.player.as_ref() else {
            return false;
        };
        let (x, z) = (p.position.x.floor() as i32, p.position.z.floor() as i32);
        self.world.loaded(x, z)
    }

    // ---- the processes --------------------------------------------------------------------

    fn companion_ai_start(&mut self) {
        let mut settings = self.companion_settings().clone();
        if self.companion_ai.salty_suspended {
            settings.persona.salty = false;
        }
        let language = self.settings.language.clone();
        let names = self.name_index().clone();
        let classifier_names = item_names();
        let (to, worker_rx) = mpsc::channel();
        let (worker_tx, from) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("companion mind".into())
            .spawn(move || {
                worker(
                    settings,
                    language,
                    names,
                    classifier_names,
                    worker_rx,
                    worker_tx,
                )
            });
        match spawned {
            Ok(_) => {
                log::info!("[companion] starting the brain and the classifier");
                self.companion_ai.status = vec!["Starting…".into()];
                self.companion_ai.worker = Some(Worker { to, from });
            }
            Err(e) => log::warn!("[companion] no worker thread: {e}"),
        }
    }

    fn name_index(&mut self) -> &NameIndex {
        self.companion_ai.names.get_or_insert_with(|| {
            let mut index = NameIndex::new();
            for (id, name) in item_names() {
                index.add_item(&id, &name);
            }
            if let Some(language) = lang::current() {
                for (key, name) in language.entries() {
                    if let Some(id) = key
                        .strip_prefix("entity.minecraft.")
                        .filter(|id| !id.contains('.'))
                    {
                        index.add_mob(id, name);
                        index.add_mob(id, &id.replace('_', " "));
                    }
                }
            }
            index
        })
    }

    fn poll_worker(&mut self) {
        let Some(worker) = self.companion_ai.worker.as_ref() else {
            return;
        };
        let mut got = Vec::new();
        while let Ok(message) = worker.from.try_recv() {
            got.push(message);
        }
        for message in got {
            match message {
                FromWorker::Planned {
                    tier,
                    first_word_ms,
                    voice,
                    missing,
                    brain,
                    salty,
                    guard,
                } => {
                    let ai = &mut self.companion_ai;
                    ai.tier = tier;
                    ai.first_word_ms = first_word_ms;
                    ai.guard_planned = guard;
                    ai.status = Vec::new();
                    if !salty && ai.settings.as_ref().is_some_and(|s| s.persona.salty_on()) {
                        ai.status
                            .push("Salty mode needs the safety guard: talking clean".into());
                    }
                    if guard {
                        ai.status.push("Safety guard: loading".into());
                    }
                    if let Some(label) = brain {
                        ai.status.push(format!("Brain: {label}, loading"));
                    }
                    for m in &missing {
                        log::warn!("[companion] {m}");
                        ai.status.push(format!("Missing: {m}"));
                    }
                    ai.tts = voice.as_ref().and_then(|v| {
                        v.models
                            .iter()
                            .map(|(m, _)| *m)
                            .find(|m| m.kind == SenseKind::TextToSpeech)
                    });
                    if let Some(voice) = voice {
                        let voices = horizon_companion::senses::voice_pack_dir(&home());
                        self.companion_ai.cast = voices.is_some();
                        let setup = VoiceSetup {
                            runtime: voice.runtime,
                            exe: voice.exe,
                            backend: voice.backend,
                            threads: voice.threads,
                            models: voice
                                .models
                                .into_iter()
                                .map(|(model, dir)| Installed { model, dir })
                                .collect(),
                            work: home().join("companion").join("logs"),
                            voices,
                        };
                        self.companion_voice_start(setup, false);
                    }
                    self.refresh_systems();
                }
                FromWorker::Guard(result) => match result {
                    Ok(endpoint) => {
                        log::info!("[companion] safety guard ready");
                        let ai = &mut self.companion_ai;
                        ai.guard_ready = true;
                        ai.gate.set_judge(Box::new(GuardClient::new(endpoint)));
                        ai.status.retain(|s| !s.starts_with("Safety guard"));
                        ai.status.push("Safety guard ready".into());
                    }
                    Err(e) => {
                        log::warn!("[companion] the safety guard did not start: {e}");
                        let ai = &mut self.companion_ai;
                        ai.gate.drop_judge();
                        ai.status.retain(|s| !s.starts_with("Safety guard"));
                        ai.status.push(format!(
                            "Safety guard failed: {}",
                            e.lines().next().unwrap_or("")
                        ));
                        let salty = ai.guard_planned
                            && ai.settings.as_ref().is_some_and(|s| s.persona.salty_on());
                        ai.guard_planned = false;
                        if salty {
                            // Salty never runs without it: start again, clean, for this world.
                            self.companion_ai.salty_suspended = true;
                            self.companion_ai_stop();
                            self.refresh_systems();
                            self.system_message(
                                Text::literal(
                                    "Salty mode is off for now: the safety guard didn't start.",
                                )
                                .with_color(text::GRAY),
                            );
                            return;
                        }
                    }
                },
                FromWorker::Classifier(result) => match result {
                    Ok(()) => {
                        self.companion_ai.classifier_ready = true;
                        self.companion_ai.status.push("Classifier ready".into());
                    }
                    Err(e) => {
                        log::warn!("[companion] the classifier did not start: {e}");
                        self.companion_ai
                            .status
                            .push(format!("Classifier failed: {e}"));
                    }
                },
                FromWorker::Brain(result) => match result {
                    Ok(endpoint) => {
                        let mut brain = Brain::new(endpoint);
                        // The system prompt is processed now, so the first turn starts as fast as the others.
                        if let Some(first) = self.companion_ai.minds.first().map(|m| m.uuid.clone())
                        {
                            let messages =
                                self.turn_messages(&first, &Input::Event("Loading.".into()));
                            if let Some(system) = messages.into_iter().next() {
                                self.companion_ai.warm_up =
                                    Some(brain.ask(ChatRequest::warm_up(system)));
                            }
                        }
                        log::info!("[companion] brain ready");
                        let ai = &mut self.companion_ai;
                        ai.status.retain(|s| !s.starts_with("Brain"));
                        ai.status.push("Brain ready".into());
                        ai.brain = Some(brain);
                        for (uuid, input) in std::mem::take(&mut ai.queued) {
                            self.companion_ask(&uuid, input);
                        }
                    }
                    Err(e) => {
                        log::warn!("[companion] the brain did not start: {e}");
                        let ai = &mut self.companion_ai;
                        ai.brain_failed = true;
                        ai.status.retain(|s| !s.starts_with("Brain"));
                        ai.status
                            .push(format!("Brain failed: {}", e.lines().next().unwrap_or("")));
                        let queued = std::mem::take(&mut ai.queued);
                        for (uuid, _) in queued {
                            self.companion_ack(&uuid, Ack::Huh, "");
                        }
                    }
                },
                FromWorker::Routed { id, route } => {
                    let Some(i) = self.companion_ai.classifying.iter().position(|w| w.0 == id)
                    else {
                        continue;
                    };
                    let (_, uuid, who, line) = self.companion_ai.classifying.remove(i);
                    match route {
                        Route::Reflex { intent, by } => self.companion_reflex(&uuid, intent, by),
                        Route::Brain => self.companion_ask(&uuid, Input::Said { who, text: line }),
                    }
                }
            }
        }
    }

    /// A mind for each companion in the hub, its prompt from the settings' persona.
    fn sync_minds(&mut self) {
        let bodies: Vec<(String, String)> = self
            .lan
            .as_ref()
            .map(|h| {
                h.companions
                    .iter()
                    .map(|b| (b.file.uuid.clone(), b.file.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        if bodies.len() == self.companion_ai.minds.len()
            && bodies
                .iter()
                .zip(&self.companion_ai.minds)
                .all(|(b, m)| b.0 == m.uuid)
        {
            return;
        }
        let owner = self.owner_name();
        let language = self.settings.language.clone();
        let systems: Vec<String> = bodies
            .iter()
            .map(|(uuid, _)| {
                prompt::system_prompt(&self.companion_persona(uuid), &owner, &language)
            })
            .collect();
        let mut old = std::mem::take(&mut self.companion_ai.minds);
        for ((uuid, name), system) in bodies.into_iter().zip(systems) {
            let mind = match old.iter().position(|m| m.uuid == uuid) {
                Some(i) => old.remove(i),
                None => Mind {
                    system,
                    uuid,
                    name,
                    history: Vec::new(),
                    asking: None,
                    spoke: false,
                    first_said: None,
                    thought: false,
                    said: false,
                    feedback: Vec::new(),
                    step: None,
                    turn: None,
                    held: Vec::new(),
                    finished: None,
                },
            };
            self.companion_ai.minds.push(mind);
        }
    }

    // ---- hearing ----------------------------------------------------------------------------

    /// The owner typed a line (not a command): true when a companion took it, so it isn't also
    /// sent to the brain twice. The line still shows in chat as the owner's.
    pub(super) fn companion_chat_line(&mut self, line: &str) {
        if self.lan.as_ref().is_none_or(|h| h.companions.is_empty()) || self.no_ai() {
            return;
        }
        let who = self.owner_name();
        self.companion_hear(&who, line, true);
    }

    /// Push-to-talk: addressed to the companion unless it names another.
    fn companion_hear_voice(&mut self, who: &str, line: &str) {
        let Some(host) = self.lan.as_ref() else {
            return;
        };
        let named = host.companions.iter().find_map(|b| {
            reflex::addressed(line, &b.file.name, false)
                .map(|cmd| (b.file.uuid.clone(), cmd.to_owned()))
        });
        let (uuid, cmd) = match named {
            Some(n) => n,
            None => match self
                .chat_mode_target()
                .or_else(|| host.companions.first().map(|b| b.file.uuid.clone()))
            {
                Some(uuid) => (uuid, line.to_owned()),
                None => return,
            },
        };
        self.companion_route(&uuid, who, &cmd);
    }

    fn chat_mode_target(&self) -> Option<String> {
        let name = self.companion_ai.chat_mode.as_ref()?;
        let host = self.lan.as_ref()?;
        companion_index(host, Some(name)).map(|i| host.companions[i].file.uuid.clone())
    }

    /// A chat line from the owner (`owner`) or a guest: the companion it is addressed to, if any,
    /// takes it.
    fn companion_hear(&mut self, who: &str, line: &str, owner: bool) {
        let Some(host) = self.lan.as_ref() else {
            return;
        };
        let trimmed = line.trim();
        // `@Name` alone: talk to that companion until `@Name` (or `@`) again.
        if owner && let Some(rest) = trimmed.strip_prefix('@') {
            let rest = rest.trim();
            if rest.is_empty() || !rest.contains(' ') {
                let hit =
                    companion_index(host, Some(rest)).map(|i| host.companions[i].file.name.clone());
                let message = match (&self.companion_ai.chat_mode, hit) {
                    (Some(_), _) if rest.is_empty() => {
                        self.companion_ai.chat_mode = None;
                        "Back to the chat".to_owned()
                    }
                    (Some(on), Some(name)) if on.eq_ignore_ascii_case(&name) => {
                        self.companion_ai.chat_mode = None;
                        "Back to the chat".to_owned()
                    }
                    (_, Some(name)) => {
                        let message = format!(
                            "Talking to {name}: what you type goes to {name}. Type @ to stop."
                        );
                        self.companion_ai.chat_mode = Some(name);
                        message
                    }
                    (_, None) => return,
                };
                self.system_message(Text::literal(message).with_color(text::GRAY));
                return;
            }
        }
        let mode = if owner {
            self.companion_ai.chat_mode.clone()
        } else {
            None
        };
        let target = host.companions.iter().find_map(|b| {
            let in_mode = mode
                .as_ref()
                .is_some_and(|m| m.eq_ignore_ascii_case(&b.file.name));
            reflex::addressed(trimmed, &b.file.name, in_mode)
                .map(|cmd| (b.file.uuid.clone(), cmd.to_owned()))
        });
        if let Some((uuid, cmd)) = target {
            self.companion_route(&uuid, who, &cmd);
        }
    }

    /// Rules first, here and now; then the classifier on the worker; then the brain.
    fn companion_route(&mut self, uuid: &str, who: &str, line: &str) {
        let language = self.settings.language.clone();
        let started = Instant::now();
        // A request the guard's words catch never reaches the brain (the guard model judges the
        // rest when it goes to the brain, `companion_ask`).
        if let Some(trip) = guard::check_request(line) {
            log::info!(
                "[companion] heard {who}: {line} (stopped by the guard: {})",
                trip.name()
            );
            self.companion_ai.guarded += 1;
            self.companion_refuse(uuid, trip);
            return;
        }
        let parse = reflex::parse(line, &language, self.name_index());
        log::info!(
            "[companion] heard {who}: {line} ({:?} in {:?})",
            parse,
            started.elapsed()
        );
        match parse {
            Parse::Intent(intent) => self.companion_reflex(uuid, intent, Stage::Rules),
            Parse::Unclear if self.companion_ai.classifier_ready => {
                let id = self.companion_ai.next_id;
                self.companion_ai.next_id += 1;
                let sent = self.companion_ai.worker.as_ref().is_some_and(|w| {
                    w.to.send(ToWorker::Classify {
                        id,
                        line: line.to_owned(),
                        language: language.clone(),
                    })
                    .is_ok()
                });
                if sent {
                    self.companion_ai.classifying.push((
                        id,
                        uuid.to_owned(),
                        who.to_owned(),
                        line.to_owned(),
                    ));
                } else {
                    self.companion_ask(
                        uuid,
                        Input::Said {
                            who: who.to_owned(),
                            text: line.to_owned(),
                        },
                    );
                }
            }
            Parse::Unclear => self.companion_ask(
                uuid,
                Input::Said {
                    who: who.to_owned(),
                    text: line.to_owned(),
                },
            ),
        }
    }

    // ---- reflexes ---------------------------------------------------------------------------

    /// A recognised command: run it now and say so from the phrase bank.
    fn companion_reflex(&mut self, uuid: &str, intent: Intent, by: Stage) {
        log::info!("[companion] reflex {intent:?} by {by:?}");
        let what = self.intent_what(&intent);
        let ack = Ack::of(&intent);
        let words = self.intent_words(&intent);
        match self.companion_run(uuid, &words) {
            Ok(()) => self.companion_ack(uuid, ack, &what),
            Err(Refused::NotYet(skill)) => {
                log::info!("[companion] {skill}: not yet");
                self.companion_ack(uuid, Ack::NotYet, &what);
            }
            Err(Refused::Bad(e)) => {
                log::info!("[companion] {words:?}: {e}");
                self.companion_ack(uuid, Ack::Huh, &what);
            }
        }
    }

    /// "10 oak logs" in the game's words, for `{what}`.
    fn intent_what(&self, intent: &Intent) -> String {
        let (item, count) = match intent {
            Intent::Collect { item, count }
            | Intent::Give { item, count }
            | Intent::Craft { item, count } => (item.as_str(), *count),
            _ => return String::new(),
        };
        let name = item_display(item);
        match count {
            Some(n) if n > 1 => format!("{n} {name}"),
            _ => name,
        }
    }

    /// A command as the body's words (`companion::parse_command`): the skills the body doesn't
    /// have yet fail there, and work as soon as it learns them.
    fn intent_words(&self, intent: &Intent) -> Vec<String> {
        let w = |s: &[&str]| s.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match intent {
            Intent::Follow => w(&["follow"]),
            Intent::Stay => w(&["stay"]),
            Intent::Come => w(&["come"]),
            Intent::GoTo {
                target: Target::Crosshair { .. },
            } => w(&["goto", "there"]),
            Intent::Stop => w(&["stop"]),
            Intent::Attack { target: None } => w(&["attack", "nearest"]),
            Intent::Attack { target: Some(kind) } => match self.nearest_mob(kind) {
                Some(id) => vec!["attack".into(), id.to_string()],
                None => w(&["attack", "nearest"]),
            },
            Intent::Defend => w(&["defend"]),
            Intent::HoldFire(true) => w(&["hold"]),
            Intent::HoldFire(false) => w(&["fire"]),
            Intent::Jump => w(&["jump"]),
            Intent::Crouch(on) => w(&["crouch", if *on { "on" } else { "off" }]),
            Intent::Eat => w(&["eat"]),
            Intent::Sleep => w(&["sleep"]),
            Intent::Collect { item, count } => counted("collect", item, *count),
            Intent::Give { item, count } => counted("give", item, *count),
            Intent::Craft { item, count } => counted("make", item, *count),
        }
    }

    /// The brain's action as the body's words.
    fn action_words(&self, a: &Action) -> Vec<String> {
        let item = a.item.clone().unwrap_or_default();
        let target = a
            .target
            .as_deref()
            .map(|t| t.strip_prefix("minecraft:").unwrap_or(t));
        let intent = match a.skill {
            BrainSkill::Follow => Intent::Follow,
            BrainSkill::Stay => Intent::Stay,
            BrainSkill::Come => Intent::Come,
            BrainSkill::GoTo => match target {
                None | Some("crosshair") | Some("there") => Intent::GoTo {
                    target: Target::Crosshair { kind: None },
                },
                Some("owner") | Some("me") => Intent::Come,
                Some(block) => return vec!["goto".into(), block.to_owned()],
            },
            BrainSkill::Stop => Intent::Stop,
            BrainSkill::Attack => Intent::Attack {
                target: target.filter(|t| *t != "nearest").map(str::to_owned),
            },
            BrainSkill::Defend => Intent::Defend,
            BrainSkill::HoldFire => Intent::HoldFire(true),
            BrainSkill::Fire => Intent::HoldFire(false),
            BrainSkill::Jump => Intent::Jump,
            BrainSkill::Crouch => Intent::Crouch(true),
            BrainSkill::StandUp => Intent::Crouch(false),
            BrainSkill::Collect => Intent::Collect {
                item,
                count: a.count,
            },
            BrainSkill::Give => Intent::Give {
                item,
                count: a.count,
            },
            BrainSkill::Eat => Intent::Eat,
            BrainSkill::Sleep => Intent::Sleep,
            BrainSkill::Craft => Intent::Craft {
                item,
                count: a.count,
            },
            // No spoken intent for smelting yet: the brain's word goes straight to the body.
            BrainSkill::Smelt => return counted("smelt", &item, a.count),
        };
        self.intent_words(&intent)
    }

    /// The nearest living mob of a kind around the owner.
    fn nearest_mob(&self, kind: &str) -> Option<u32> {
        let p = self.player.as_ref()?;
        let at = [p.position.x, p.position.y, p.position.z];
        let kind = kind.strip_prefix("minecraft:").unwrap_or(kind);
        self.mobs
            .iter()
            .filter(|m| m.alive() && c::short_id(&m.kind.id) == kind)
            .map(|m| {
                (
                    m.id,
                    c::distance(at, [m.position.x, m.position.y, m.position.z]),
                )
            })
            .filter(|(_, d)| *d <= 24.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    fn companion_run(&mut self, uuid: &str, words: &[String]) -> Result<(), Refused> {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let command = c::parse_command(&words).map_err(|e| {
            if e.starts_with("unknown companion command") {
                Refused::NotYet(words.first().copied().unwrap_or("").to_owned())
            } else {
                Refused::Bad(e)
            }
        })?;
        let name = self
            .mind_name(uuid)
            .ok_or(Refused::Bad("no such companion".into()))?;
        self.companion_command(Some(&name), command)
            .map(|_| ())
            .map_err(Refused::Bad)
    }

    fn mind_name(&self, uuid: &str) -> Option<String> {
        self.lan
            .as_ref()?
            .companions
            .iter()
            .find(|b| b.file.uuid == uuid)
            .map(|b| b.file.name.clone())
    }

    /// A phrase-bank line, said at once.
    fn companion_ack(&mut self, uuid: &str, ack: Ack, what: &str) {
        let persona = self.persona();
        let language = self.settings.language.clone();
        let line = if persona.salty_on() {
            phrases::acknowledge_salty(ack, persona.personality, &language, what, self.ticks)
        } else {
            phrases::acknowledge(ack, persona.personality, &language, what, self.ticks)
        };
        let tone = if ack == Ack::Attack {
            Tone::Excited
        } else {
            Tone::Normal
        };
        if let Some(name) = self.mind_name(uuid) {
            self.companion_speak(&name, &line, tone);
        }
    }

    /// A line from a companion: in the owner's chat, the guests' (tagged as a companion's), and
    /// aloud when voice is on.
    pub(super) fn companion_speak(&mut self, name: &str, line: &str, tone: Tone) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        log::info!("[companion] {name} says ({}): {line}", tone.name());
        let said = &mut self.companion_ai.said;
        if said.len() >= 64 {
            said.remove(0);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        said.push(json!({ "at": at, "name": name, "line": line, "tone": tone.name() }));
        let chat = Text::translate("chat.type.text", &[c::name_text(name), Text::literal(line)]);
        let focused = self.screen == super::Screen::Chat;
        self.ui
            .chat
            .add(&self.gui.atlas, chat, None, self.ticks, focused);
        if let Some(host) = &self.lan {
            host.hub.companion_chat(name, line);
        }
        let language = self
            .settings
            .language
            .split('_')
            .next()
            .unwrap_or("en")
            .to_owned();
        let voice = self.companion_voice_id();
        self.companion_say(
            name,
            Speak {
                text: line.to_owned(),
                voice,
                tone: horizon_senses::voice::Tone::parse(tone.name()),
                language,
            },
        );
    }

    /// The voice the settings pick on the running voice model (else the personality's default).
    pub(super) fn companion_voice_id(&mut self) -> Option<String> {
        let model = self.companion_ai.tts?;
        let cast = self.companion_ai.cast;
        super::companion_voice::chosen_voice_id(self.companion_settings(), model, cast)
    }

    /// The Companion screen's "Play sample": a short line in the chosen voice. It plays like any
    /// line (and lands in the WAV tap when one is set); without the voice running, the screen says so.
    pub(super) fn companion_voice_sample(&mut self) {
        let personality = self.companion_settings().persona.personality;
        let name = self.companion_settings().persona.name.clone();
        let language = self.settings.language.clone();
        let line = phrases::acknowledge(Ack::Follow, personality, &language, "", self.ticks);
        let voice = self.companion_voice_id();
        if self.companion_ai.tts.is_none()
            || !self
                .companion_voice_status()
                .get("text_to_speech")
                .is_some_and(|v| v == true)
        {
            self.system_message(Text::literal(
                "The voice starts with your companion: open a world and spawn it to hear a sample.",
            ));
            return;
        }
        let spoken = self.mind_name_any().unwrap_or(name);
        self.companion_say(
            &spoken,
            Speak {
                text: line,
                voice,
                tone: horizon_senses::voice::Tone::Normal,
                language: language.split('_').next().unwrap_or("en").to_owned(),
            },
        );
    }

    fn mind_name_any(&self) -> Option<String> {
        self.companion_ai.minds.first().map(|m| m.name.clone())
    }

    // ---- the brain ----------------------------------------------------------------------

    /// The messages of a turn: the system prompt, the memory, the state and the history.
    fn turn_messages(&mut self, uuid: &str, input: &Input) -> Vec<Message> {
        let Some(mi) = self.companion_ai.minds.iter().position(|m| m.uuid == uuid) else {
            return Vec::new();
        };
        let name = self.companion_ai.minds[mi].name.clone();
        let mut state = self
            .companion_state_summary(Some(&name))
            .unwrap_or_default();
        if self.role_of(&name).follows_the_plan() {
            let step = self.partner_step(&name);
            state = format!("{}\n{state}", partner::state_line(step));
        }
        let mind = &mut self.companion_ai.minds[mi];
        for f in mind.feedback.drain(..) {
            state = format!("Note: {f}\n{state}");
        }
        let memory = self
            .lan
            .as_ref()
            .and_then(|h| h.companions.iter().find(|b| b.file.uuid == uuid))
            .map(|b| b.file.notes.join("\n"))
            .unwrap_or_default();
        let style = prompt::style_reminder(&self.persona());
        let mind = &self.companion_ai.minds[mi];
        prompt::messages_styled(
            &mind.system,
            &memory,
            &state,
            &mind.history,
            input,
            &self.settings.language,
            style,
            &Budget::default(),
        )
    }

    /// The Partner's step, from what the owner and the companion carry.
    fn partner_step(&self, name: &str) -> partner::Step {
        let mut items: Vec<(String, u32)> = Vec::new();
        if let Some(host) = &self.lan
            && let Some(bi) = companion_index(host, Some(name))
            && let Some(g) = host.hub.guest(host.companions[bi].conn)
        {
            for s in g
                .inventory
                .iter()
                .chain(g.armor.iter())
                .chain([&g.offhand])
                .flatten()
            {
                items.push((s.item.to_string(), s.count));
            }
        }
        for s in self
            .inventory
            .iter()
            .chain(self.armor.iter())
            .chain([&self.offhand])
            .flatten()
        {
            items.push((s.item.to_string(), s.count));
        }
        partner::step(&items, self.dim.id())
    }

    /// Send a turn to the brain (queued while it loads).
    fn companion_ask(&mut self, uuid: &str, input: Input) {
        let said = matches!(input, Input::Said { .. });
        if self.companion_ai.brain.is_none() {
            let loading = self.companion_ai.worker.is_some() && !self.companion_ai.brain_failed;
            if loading {
                if said {
                    self.companion_ack(uuid, Ack::Think, "");
                }
                self.companion_ai.queued.push((uuid.to_owned(), input));
            } else if said {
                self.companion_ack(uuid, Ack::Huh, "");
            }
            return;
        }
        let heard = match &input {
            Input::Said { text, .. } => Some(text.clone()),
            Input::Event(_) => None,
        };
        // The keyword list first: what it knows never reaches the brain.
        if let Some(trip) = heard.as_deref().and_then(guard::check_request) {
            self.companion_ai.guarded += 1;
            self.companion_refuse(uuid, trip);
            return;
        }
        let messages = self.turn_messages(uuid, &input);
        let first_word_ms = self.companion_ai.first_word_ms;
        let temperature = self.brain_temperature();
        let ai = &mut self.companion_ai;
        let Some(brain) = ai.brain.as_mut() else {
            return;
        };
        let Some(mind) = ai.minds.iter_mut().find(|m| m.uuid == uuid) else {
            return;
        };
        mind.asking = None;
        if let Some(old) = mind.turn.take() {
            // Something new was said: the old answer is no longer wanted, even if it was only
            // waiting for the guard.
            brain.cancel_up_to(old);
            ai.gate.cancel(old);
        }
        mind.finished = None;
        mind.held.clear();
        let mut request = ChatRequest::turn(messages.clone());
        request.temperature = temperature;
        let id = brain.ask(request);
        // The brain starts now; the guard judges the line while it reads its prompt, and nothing it
        // writes is said before the line passed (`moderation::Gate`).
        if let Err(trip) = ai.gate.open(id, heard.as_deref(), Instant::now()) {
            brain.cancel_up_to(id);
            let uuid = uuid.to_owned();
            self.companion_ai.guarded += 1;
            self.companion_refuse(&uuid, trip);
            return;
        }
        mind.asking = Some((id, messages, Instant::now()));
        mind.turn = Some(id);
        mind.spoke = false;
        mind.first_said = None;
        mind.thought = false;
        mind.said = said;
        log::info!("[companion] asked the brain ({}): {}", id, input.heard());
        if said && first_word_ms.is_some_and(|ms| ms >= THINK_AT_ONCE_MS) {
            mind.thought = true;
            let uuid = uuid.to_owned();
            self.companion_ack(&uuid, Ack::Think, "");
        }
    }

    /// A brain slower than expected still gets the thinking line before the silence feels long.
    fn think_fillers(&mut self) {
        let late: Vec<String> = self
            .companion_ai
            .minds
            .iter_mut()
            .filter(|m| m.said && !m.spoke && !m.thought)
            .filter(|m| {
                m.asking
                    .as_ref()
                    .is_some_and(|(_, _, at)| at.elapsed() >= THINK_AFTER)
            })
            .map(|m| {
                m.thought = true;
                m.uuid.clone()
            })
            .collect();
        for uuid in late {
            self.companion_ack(&uuid, Ack::Think, "");
        }
    }

    fn poll_brain(&mut self) {
        let mut events = Vec::new();
        if let Some(brain) = self.companion_ai.brain.as_ref() {
            while let Some(event) = brain.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            if Some(event.id()) == self.companion_ai.warm_up {
                if !matches!(event, BrainEvent::Sentence { .. }) {
                    self.companion_ai.warm_up = None;
                }
                continue;
            }
            let Some(mi) = self
                .companion_ai
                .minds
                .iter()
                .position(|m| m.asking.as_ref().is_some_and(|(id, ..)| *id == event.id()))
            else {
                continue;
            };
            match event {
                BrainEvent::Sentence { id, text, tone } => {
                    let ai = &mut self.companion_ai;
                    let mind = &mut ai.minds[mi];
                    mind.held.push((text.clone(), tone));
                    ai.gate.sentence(id, &text, Instant::now());
                }
                BrainEvent::Done {
                    id,
                    reply,
                    raw,
                    stats,
                } => {
                    let ai = &mut self.companion_ai;
                    let mind = &mut ai.minds[mi];
                    let Some((_, sent, asked)) = mind.asking.take() else {
                        continue;
                    };
                    if !ai.gate.holds(id) {
                        // Stopped already: the refusal was said and the rest dropped.
                        continue;
                    }
                    let say = reply.say.clone();
                    mind.finished = Some(Finished {
                        reply,
                        raw,
                        stats,
                        sent,
                        asked,
                    });
                    ai.gate.done(id, &say, Instant::now());
                }
                BrainEvent::Failed { id, error } => {
                    let ai = &mut self.companion_ai;
                    ai.gate.cancel(id);
                    let mind = &mut ai.minds[mi];
                    let said = mind.said;
                    let uuid = mind.uuid.clone();
                    mind.asking = None;
                    mind.turn = None;
                    mind.held.clear();
                    log::warn!("[companion] the brain failed: {error}");
                    if said {
                        self.companion_ack(&uuid, Ack::Huh, "");
                    }
                }
            }
        }
    }

    /// What the gate lets through, in order: sentences to say, a stopped turn's refusal, a passed
    /// reply's actions and memory.
    fn poll_gate(&mut self) {
        let events = self.companion_ai.gate.poll(Instant::now());
        for event in events {
            let turn = match &event {
                GateEvent::Speak { turn, .. }
                | GateEvent::Blocked { turn, .. }
                | GateEvent::Finished { turn } => *turn,
            };
            let Some(mi) = self
                .companion_ai
                .minds
                .iter()
                .position(|m| m.turn == Some(turn))
            else {
                continue;
            };
            let mind = &mut self.companion_ai.minds[mi];
            let (uuid, name) = (mind.uuid.clone(), mind.name.clone());
            match event {
                GateEvent::Speak { index, .. } => {
                    let Some((text, tone)) = mind.held.get(index).cloned() else {
                        continue;
                    };
                    mind.spoke = true;
                    if mind.first_said.is_none() {
                        mind.first_said = mind
                            .asking
                            .as_ref()
                            .map(|(_, _, at)| at.elapsed())
                            .or_else(|| mind.finished.as_ref().map(|f| f.asked.elapsed()));
                    }
                    self.companion_speak(&name, &text, tone);
                }
                GateEvent::Blocked { trip, heard, .. } => {
                    log::info!(
                        "[companion] the guard stopped {} ({})",
                        if heard { "a line heard" } else { "a reply" },
                        trip.name()
                    );
                    if let Some((id, ..)) = mind.asking.take()
                        && let Some(brain) = self.companion_ai.brain.as_ref()
                    {
                        brain.cancel_up_to(id);
                    }
                    let mind = &mut self.companion_ai.minds[mi];
                    mind.turn = None;
                    // The exchange is kept as refused (a line heard that stopped is not kept at all).
                    if let Some(f) = mind.finished.take()
                        && !heard
                    {
                        mind.history.push(Exchange::new(
                            &f.sent,
                            r#"{"tone":"normal","say":"(refused: that stays out of the game)","actions":[]}"#,
                        ));
                    }
                    mind.held.clear();
                    mind.spoke = true;
                    self.companion_ai.guarded += 1;
                    self.companion_refuse(&uuid, trip);
                }
                GateEvent::Finished { .. } => {
                    mind.turn = None;
                    let Some(f) = mind.finished.take() else {
                        continue;
                    };
                    mind.held.clear();
                    mind.history.push(Exchange::new(&f.sent, f.raw));
                    let first_said = mind.first_said.unwrap_or_else(|| f.asked.elapsed());
                    self.record_timing(&name, f.asked, first_said, &f.stats);
                    self.run_actions(&uuid, &f.reply.actions, &f.reply.rejected);
                    if let Some(note) = f
                        .reply
                        .remember
                        .filter(|n| !n.trim().is_empty() && guard::check(n).is_none())
                    {
                        self.companion_remember(&uuid, note.trim());
                    }
                }
            }
        }
    }

    /// The brain's temperature: Advanced's, else the mode's (Salty runs hotter: more attitude).
    fn brain_temperature(&mut self) -> f32 {
        if let Some(t) = self.companion_settings().tuning.temperature {
            return t;
        }
        prompt::temperature(&self.persona())
    }

    fn companion_refuse(&mut self, uuid: &str, trip: Trip) {
        log::info!("[companion] refused ({})", trip.name());
        let ack = if trip == Trip::SelfHarm {
            Ack::Care
        } else {
            Ack::Refuse
        };
        self.companion_ack(uuid, ack, "");
    }

    fn record_timing(&mut self, name: &str, asked: Instant, first_said: Duration, stats: &Stats) {
        let ms = |d: Option<Duration>| d.map(|d| d.as_millis() as u64);
        let v = json!({
            "name": name,
            "first_token_ms": ms(stats.first_token),
            "first_sentence_ms": ms(stats.first_sentence),
            "total_ms": stats.total.as_millis() as u64,
            "first_said_ms": first_said.as_millis() as u64,
            "since_heard_ms": asked.elapsed().as_millis() as u64,
            "prompt_tokens": stats.prompt_tokens,
            "cached_tokens": stats.cached_tokens,
            "completion_tokens": stats.completion_tokens,
            "tokens_per_second": stats.tokens_per_second,
        });
        log::info!("[companion] turn: {v}");
        let timings = &mut self.companion_ai.timings;
        if timings.len() >= 32 {
            timings.remove(0);
        }
        timings.push(v);
    }

    /// The brain's actions: Light runs only those its tier is trusted with; a skill the body
    /// doesn't have yet is said and told to the brain next turn.
    fn run_actions(&mut self, uuid: &str, actions: &[Action], rejected: &[String]) {
        let allowed = BrainSkill::allowed_for(self.companion_ai.tier);
        let mut feedback: Vec<String> = rejected
            .iter()
            .map(|r| format!("there is no skill called {r}"))
            .collect();
        let mut not_yet = false;
        for a in actions {
            if allowed.is_some_and(|list| !list.contains(&a.skill)) {
                log::info!(
                    "[companion] {:?} left to the reflexes on this brain",
                    a.skill
                );
                continue;
            }
            let words = self.action_words(a);
            match self.companion_run(uuid, &words) {
                Ok(()) => log::info!("[companion] brain action {}", words.join(" ")),
                Err(Refused::NotYet(skill)) => {
                    not_yet = true;
                    feedback.push(format!(
                        "you can't {skill} yet: that skill isn't available, say so if asked again"
                    ));
                }
                Err(Refused::Bad(e)) => feedback.push(format!("{} failed: {e}", a.skill.name())),
            }
        }
        if not_yet {
            self.companion_ack(uuid, Ack::NotYet, "");
        }
        if let Some(mind) = self.companion_ai.minds.iter_mut().find(|m| m.uuid == uuid) {
            mind.feedback.extend(feedback);
        }
    }

    /// A memory note into the companion's file (saved with the world); "forget" is the player's.
    fn companion_remember(&mut self, uuid: &str, note: &str) {
        let Some(host) = self.lan.as_mut() else {
            return;
        };
        let Some(b) = host.companions.iter_mut().find(|b| b.file.uuid == uuid) else {
            return;
        };
        let note: String = note.chars().take(160).collect();
        if b.file.notes.iter().any(|n| n.eq_ignore_ascii_case(&note)) {
            return;
        }
        log::info!("[companion] {} remembers: {note}", b.file.name);
        b.file.notes.push(note);
        if b.file.notes.len() > NOTES {
            b.file.notes.remove(0);
        }
    }

    // ---- events -------------------------------------------------------------------------

    fn now_seconds(&self) -> f64 {
        self.ticks as f64 / 20.0
    }

    /// A remark the brain may make on its own, if the chattiness allows one now.
    fn remark(&mut self, name: &str, what: String, important: bool) {
        let chattiness = self.companion_settings().chattiness;
        let Some(cooldown) = chattiness.remark_cooldown() else {
            return;
        };
        let now = self.now_seconds();
        if !important
            && self
                .companion_ai
                .last_remark
                .is_some_and(|at| now - at < cooldown)
        {
            return;
        }
        let Some(uuid) = self
            .companion_ai
            .minds
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.uuid.clone())
        else {
            return;
        };
        if self.companion_ai.brain.is_none() {
            return;
        }
        // An answer in progress is not interrupted for a remark.
        if self
            .companion_ai
            .minds
            .iter()
            .any(|m| m.uuid == uuid && m.asking.is_some())
        {
            return;
        }
        self.companion_ai.last_remark = Some(now);
        self.companion_ask(&uuid, Input::Event(what));
    }

    fn companion_ai_report(&mut self, name: &str, report: CompanionReport) {
        match report {
            CompanionReport::SawHostile {
                kind,
                bearing,
                distance,
            } => {
                let range = Range::of(f64::from(distance));
                if range == Range::Far {
                    return;
                }
                let cooldown = self.companion_settings().chattiness.callout_cooldown();
                let now = self.now_seconds();
                if !self.companion_ai.cooldowns.ready(&kind, now, cooldown) {
                    return;
                }
                let personality = self.companion_settings().persona.personality;
                let what = lang::get_or(
                    &format!("entity.minecraft.{}", c::short_id(&kind)),
                    c::short_id(&kind),
                );
                let (line, tone) =
                    phrases::callout(&what, bearing, range, personality, &self.settings.language);
                self.companion_speak(name, &line, tone);
            }
            CompanionReport::Died { cause } => self.remark(
                name,
                format!("You died ({cause}). You respawn in 10 seconds."),
                true,
            ),
            CompanionReport::ChangedDimension { dimension } => {
                self.remark(
                    name,
                    format!(
                        "You came along with the owner to {}.",
                        c::short_id(&dimension)
                    ),
                    true,
                );
            }
            CompanionReport::StayedBehind { dimension } => {
                self.remark(
                    name,
                    format!(
                        "The owner left; you stayed behind in {}.",
                        c::short_id(&dimension)
                    ),
                    false,
                );
            }
            CompanionReport::SkillFailed { skill, reason } => {
                self.remark(
                    name,
                    format!("Your {} skill failed: {reason}.", skill.name()),
                    true,
                );
            }
            CompanionReport::SkillFinished(skill)
                if !matches!(
                    skill.name(),
                    "idle" | "follow" | "stay" | "come" | "go to" | "attack" | "defend" | "retreat"
                ) =>
            {
                self.remark(name, format!("You finished: {}.", skill.name()), true);
            }
            _ => {}
        }
    }

    /// The owner's own moments: a big hit, a death, an advancement.
    fn owner_events(&mut self) {
        let Some(health) = self.player.as_ref().map(|p| p.health.health()) else {
            return;
        };
        let before = self.companion_ai.owner_health.replace(health);
        let Some(before) = before else {
            return;
        };
        let Some(name) = self.companion_ai.minds.first().map(|m| m.name.clone()) else {
            return;
        };
        let owner = self.owner_name();
        if before > 0.0 && health <= 0.0 {
            self.remark(&name, format!("{owner} died."), true);
        } else if before - health >= BIG_HIT {
            self.remark(
                &name,
                format!(
                    "{owner} just lost {:.0} health and has {health:.0} of 20 left.",
                    before - health
                ),
                false,
            );
        }
    }

    /// Teammate, Speedrunner and Coach: when the run reaches a new step, the brain hears of it (and
    /// says the new plan).
    fn partner_progress(&mut self) {
        if !self.ticks.is_multiple_of(BOOST_EVERY) {
            return;
        }
        let Some(name) = self.companion_ai.minds.first().map(|m| m.name.clone()) else {
            return;
        };
        if !self.role_of(&name).follows_the_plan() {
            return;
        }
        let step = self.partner_step(&name);
        let before = self.companion_ai.minds[0].step.replace(step);
        if before.is_some_and(|b| b != step) {
            self.remark(
                &name,
                format!("New step of the plan: {}.", step.goal()),
                true,
            );
        }
    }

    /// The owner made an advancement (game/advancements.rs).
    pub(super) fn companion_owner_advancement(&mut self, title: &str) {
        if self.companion_ai.minds.is_empty() || self.no_ai() {
            return;
        }
        let name = self.companion_ai.minds[0].name.clone();
        let owner = self.owner_name();
        self.remark(
            &name,
            format!("{owner} made the advancement \"{title}\"."),
            false,
        );
    }

    /// Each body's voice follows it.
    fn place_voices(&mut self) {
        let Some(host) = self.lan.as_ref() else {
            return;
        };
        let here = self.dim;
        let places: Vec<(String, [f64; 3], bool)> = host
            .companions
            .iter()
            .filter_map(|b| {
                host.hub
                    .guest(b.conn)
                    .map(|g| (b.file.name.clone(), g.position, b.dim != here))
            })
            .collect();
        for (name, at, elsewhere) in places {
            self.companion_voice_place(&name, at, elsewhere);
        }
    }

    /// Bodyguard: Strength II, Resistance II and Regeneration I, with their particles, kept up.
    fn boost_super_strong(&mut self) {
        if !self.ticks.is_multiple_of(BOOST_EVERY) {
            return;
        }
        let fallback = self.companion_settings().persona.role;
        let Some(mut host) = self.lan.take() else {
            return;
        };
        let conns: Vec<u32> = host
            .companions
            .iter()
            .filter(|b| Role::parse(&b.file.role).unwrap_or(fallback) == Role::Bodyguard)
            .map(|b| b.conn)
            .collect();
        for conn in conns {
            self.as_guest(&mut host, conn, |game| {
                for (id, amplifier) in [("strength", 1), ("resistance", 1), ("regeneration", 0)] {
                    game.add_player_effect(horizon_client::effects::EffectInstance::new(
                        id,
                        BOOST_TICKS,
                        amplifier,
                    ));
                }
            });
        }
        self.lan = Some(host);
    }

    // ---- dev and tests --------------------------------------------------------------------

    /// `companion ai`, `companion hear <text>`, `companion hear-wav <file>`, `companion forget [name]`.
    pub(super) fn companion_ai_words(&mut self, words: &[&str]) -> Option<Result<Value, String>> {
        let first = words.first().copied()?;
        let ours = matches!(
            first,
            "ai" | "hear" | "hear-voice" | "hear-wav" | "forget" | "notes" | "persona" | "sample"
        ) || first == "spawn" && self.no_ai();
        ours.then(|| self.companion_ai_run(words))
    }

    fn companion_ai_run(&mut self, words: &[&str]) -> Result<Value, String> {
        match words[0] {
            "spawn" => Err("No AI is on: companions are hidden".into()),
            "persona" => {
                let uuids: Vec<String> = self
                    .companion_ai
                    .minds
                    .iter()
                    .map(|m| m.uuid.clone())
                    .collect();
                let voice = self.companion_voice_id();
                let minds: Vec<Value> = uuids
                    .iter()
                    .map(|uuid| {
                        let p = self.companion_persona(uuid);
                        json!({
                            "name": p.name, "personality": p.personality.label(), "role": p.role.id(),
                            "perception": p.perception.id(), "salty": p.salty_on(), "kid_friendly": p.kid_friendly,
                        })
                    })
                    .collect();
                Ok(
                    json!({ "ok": true, "minds": minds, "voice": voice, "tts": self.companion_ai.tts.map(|m| m.id) }),
                )
            }
            "sample" => {
                self.companion_voice_sample();
                Ok(json!({ "ok": true, "voice": self.companion_voice_id() }))
            }
            "ai" => {
                let ai = &self.companion_ai;
                Ok(json!({
                    "ok": true,
                    "running": ai.worker.is_some(),
                    "brain": ai.brain.is_some(),
                    "classifier": ai.classifier_ready,
                    "tier": ai.tier.map(|t| format!("{t:?}")),
                    "first_word_ms": ai.first_word_ms,
                    "status": ai.status,
                    "asking": ai.minds.iter().any(|m| m.asking.is_some()),
                    "queued": ai.queued.len(),
                    "timings": ai.timings,
                    "said": ai.said,
                    "guarded": ai.guarded,
                    "guard": {
                        "ready": ai.guard_ready,
                        "planned": ai.guard_planned,
                        "salty_suspended": ai.salty_suspended,
                        "mode": format!("{:?}", ai.gate.mode()),
                        "required": ai.gate.required(),
                        "stopped": ai.gate.stopped,
                        "failures": ai.gate.failures,
                        "last_failure": ai.gate.last_failure,
                        "check_ms": ai.gate.timings().iter().map(|(side, d)| json!({
                            "side": format!("{side:?}"), "ms": d.as_millis() as u64,
                        })).collect::<Vec<_>>(),
                    },
                    "chat_mode": ai.chat_mode,
                    "no_ai": self.no_ai(),
                    "voice": self.companion_voice_status(),
                    "message": ai.status.join(", "),
                }))
            }
            "hear" => {
                let text = words[1..].join(" ");
                let who = self.owner_name();
                self.companion_hear(&who, &text, true);
                Ok(json!({ "ok": true, "message": format!("heard: {text}") }))
            }
            "hear-voice" => {
                let text = words[1..].join(" ");
                let who = self.owner_name();
                self.companion_hear_voice(&who, &text);
                Ok(json!({ "ok": true }))
            }
            "hear-wav" => {
                let path = words.get(1).ok_or("hear-wav <file.wav>")?;
                self.companion_voice_hear_wav(std::path::Path::new(path))
                    .map(|()| json!({ "ok": true }))
            }
            "forget" => {
                let host = self.lan.as_mut().ok_or("no companion here")?;
                let bi = companion_index(host, words.get(1).copied())
                    .ok_or("no companion by that name")?;
                host.companions[bi].file.notes.clear();
                let uuid = host.companions[bi].file.uuid.clone();
                if let Some(m) = self.companion_ai.minds.iter_mut().find(|m| m.uuid == uuid) {
                    m.history.clear();
                }
                Ok(json!({ "ok": true, "message": "forgotten" }))
            }
            "notes" => {
                let host = self.lan.as_ref().ok_or("no companion here")?;
                let bi = companion_index(host, words.get(1).copied())
                    .ok_or("no companion by that name")?;
                Ok(json!({ "ok": true, "notes": host.companions[bi].file.notes }))
            }
            other => Err(format!("unknown companion command: {other}")),
        }
    }
}

/// Why a command didn't run.
enum Refused {
    /// The body has no such skill yet.
    NotYet(String),
    Bad(String),
}

/// `collect 10 oak_log`: the count first, as the body's words read it.
fn counted(verb: &str, item: &str, count: Option<u32>) -> Vec<String> {
    vec![
        verb.to_owned(),
        count.unwrap_or(1).to_string(),
        item.strip_prefix("minecraft:").unwrap_or(item).to_owned(),
    ]
}

/// An item or tag in the game's language: `oak_log` → "Oak Log", `#logs` → "logs".
fn item_display(item: &str) -> String {
    if let Some(tag) = item.strip_prefix('#') {
        return tag_display(tag, &lang::code());
    }
    let id = item.strip_prefix("minecraft:").unwrap_or(item);
    let fallback = id.replace('_', " ");
    let name = lang::get_or(&format!("item.minecraft.{id}"), "");
    if !name.is_empty() {
        return name;
    }
    lang::get_or(&format!("block.minecraft.{id}"), &fallback)
}

/// The generic materials the reflexes name by tag (`reflex::material`), in the game's language:
/// the language files have no names for tags.
fn tag_display(tag: &str, language: &str) -> String {
    let names: &[&str; 5] = match tag {
        "logs" => &["logs", "bûches", "troncos", "Stämme", "troncos"],
        "planks" => &["planks", "planches", "tablones", "Bretter", "tábuas"],
        "wool" => &["wool", "laine", "lana", "Wolle", "lã"],
        _ => return tag.replace('_', " "),
    };
    let i = match language.get(..2).unwrap_or("en") {
        "fr" => 1,
        "es" => 2,
        "de" => 3,
        "pt" => 4,
        _ => 0,
    };
    names[i].to_owned()
}

/// Every item and block name of the game's language, and the English one from its id.
fn item_names() -> Vec<(String, String)> {
    let mut names = Vec::new();
    if let Some(language) = lang::current() {
        for (key, name) in language.entries() {
            let id = key
                .strip_prefix("item.minecraft.")
                .or_else(|| key.strip_prefix("block.minecraft."))
                .filter(|id| !id.contains('.'));
            if let Some(id) = id {
                names.push((id.to_owned(), name.to_owned()));
                names.push((id.to_owned(), id.replace('_', " ")));
            }
        }
    }
    names
}

/// The worker: plans the parts, starts the brain and the classifier, then answers the
/// classifier's questions until the game hangs up (which stops both servers).
fn worker(
    settings: CompanionSettings,
    language: String,
    names: NameIndex,
    item_names: Vec<(String, String)>,
    rx: Receiver<ToWorker>,
    tx: Sender<FromWorker>,
) {
    let home = home();
    let setup = Setup::new(&home, &language);
    let machine = Machine::detect(&home);
    let parts = setup.parts(&settings, &machine);
    let (tier, label) = match &parts.brain {
        Some(BrainPart::Local { tier, label, .. }) => (*tier, Some(label.clone())),
        Some(BrainPart::Server(e)) => (None, Some(e.model.clone())),
        None => (None, None),
    };
    let first_word_ms = parts.plan.pick(Part::Brain).map(|p| p.latency_ms);
    let _ = tx.send(FromWorker::Planned {
        tier,
        first_word_ms,
        voice: parts.voice.clone(),
        missing: parts.missing.clone(),
        brain: label,
        salty: parts.salty,
        guard: parts.guard.is_some(),
    });
    // The safety guard loads beside the brain (on the processor, a few hundred MB).
    let mut guard_runtime = None;
    let mut guard_sent = true;
    if let Some(part) = &parts.guard {
        match Runtime::start(&part.config) {
            Ok(r) => {
                guard_runtime = Some(r);
                guard_sent = false;
            }
            Err(e) => {
                let _ = tx.send(FromWorker::Guard(Err(e.to_string())));
            }
        }
    }
    // The brain loads while the classifier starts.
    let mut runtime = None;
    match parts.brain {
        Some(BrainPart::Local { config, .. }) => match Runtime::start(&config) {
            Ok(r) => runtime = Some(r),
            Err(e) => {
                let _ = tx.send(FromWorker::Brain(Err(e.to_string())));
            }
        },
        Some(BrainPart::Server(endpoint)) => {
            let _ = tx.send(FromWorker::Brain(Ok(endpoint)));
        }
        None => {
            let _ = tx.send(FromWorker::Brain(Err("no brain is set up".into())));
        }
    }
    let mut embed: Option<EmbedServer> = None;
    let pipeline =
        parts
            .classifier
            .and_then(|part| match start_classifier(&home, &part, item_names) {
                Ok((server, pipeline)) => {
                    embed = Some(server);
                    let _ = tx.send(FromWorker::Classifier(Ok(())));
                    Some(pipeline)
                }
                Err(e) => {
                    let _ = tx.send(FromWorker::Classifier(Err(e)));
                    None
                }
            });
    let rules = reflex::Pipeline::default();
    let mut brain_sent = runtime.is_none();
    loop {
        if !guard_sent && let Some(r) = guard_runtime.as_mut() {
            match r.poll().clone() {
                State::Loading => {}
                State::Ready { load_time } => {
                    log::info!("[companion] safety guard loaded in {load_time:?}");
                    guard_sent = true;
                    let _ = tx.send(FromWorker::Guard(Ok(r.endpoint())));
                }
                State::Failed(e) => {
                    guard_sent = true;
                    let _ = tx.send(FromWorker::Guard(Err(e)));
                }
            }
        }
        if !brain_sent && let Some(r) = runtime.as_mut() {
            match r.poll().clone() {
                State::Loading => {}
                State::Ready { load_time } => {
                    log::info!("[companion] brain loaded in {load_time:?}");
                    brain_sent = true;
                    let _ = tx.send(FromWorker::Brain(Ok(r.endpoint())));
                }
                State::Failed(e) => {
                    brain_sent = true;
                    let _ = tx.send(FromWorker::Brain(Err(e)));
                }
            }
        }
        let loading = !brain_sent || !guard_sent;
        match rx.recv_timeout(Duration::from_millis(if loading { 50 } else { 500 })) {
            Ok(ToWorker::Classify { id, line, language }) => {
                let started = Instant::now();
                let route = match &pipeline {
                    Some(p) => p.route(&line, &language, &names),
                    None => rules.route(&line, &language, &names),
                };
                log::info!(
                    "[companion] classified {line:?}: {route:?} in {:?}",
                    started.elapsed()
                );
                if tx.send(FromWorker::Routed { id, route }).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    // Dropping them stops the processes.
    drop(runtime);
    drop(guard_runtime);
    drop(embed);
    log::info!("[companion] brain and classifier stopped");
}

fn start_classifier(
    home: &std::path::Path,
    part: &ClassifierPart,
    item_names: Vec<(String, String)>,
) -> Result<(EmbedServer, reflex::Pipeline<ReflexClassifier>), String> {
    let model = part.dir.join(part.model.local_path(part.model.main_file()));
    let work = home.join("companion").join("logs");
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let server = EmbedServer::start(&part.server, &model, part.model.id, part.threads, &work)
        .map_err(|e| e.to_string())?;
    let examples = classifier::parse_examples(classifier::BANK)?;
    let cache = classifier::cache_dir(&home.join("companion"));
    let started = Instant::now();
    let mut built =
        Classifier::build(server.client(), &examples, Some(&cache)).map_err(|e| e.to_string())?;
    built.add_items(item_names);
    log::info!(
        "[companion] classifier ready ({} examples in {:?})",
        built.len(),
        started.elapsed()
    );
    let client = server.client().clone();
    Ok((
        server,
        ReflexClassifier {
            classifier: built,
            client,
        }
        .pipeline(),
    ))
}

#[cfg(test)]
mod tests {
    use super::tag_display;

    #[test]
    fn tagged_materials_are_named_in_the_game_language() {
        assert_eq!(tag_display("logs", "fr_fr"), "bûches");
        assert_eq!(tag_display("logs", "en_us"), "logs");
        assert_eq!(tag_display("planks", "de_de"), "Bretter");
        assert_eq!(tag_display("small_flowers", "fr_fr"), "small flowers");
    }
}
