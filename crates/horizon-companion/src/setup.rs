//! Setting the companion up, shared by the launcher's card and the game's Companion section
//! (docs/companions.md, "Fitting the player's PC"): what this PC is, the plan for it, downloading
//! what the plan (or Advanced) picked with progress and cancel, deleting it, and, for the game, which
//! of the picked parts are on disk and how to start them. Everything lives in the game's home
//! (`<home>/companion/`). The long work runs on its own thread and reports over a channel that the
//! window or the game polls ([`Job`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::catalogue::{self, Backend, Model, RuntimeBuild, Tier};
use crate::client::{Brain, ChatRequest, Endpoint, Event as BrainEvent};
use crate::download::{self, Layout, Phase, Progress};
use crate::hardware::{self, Hardware};
use crate::plan::{self, Device, Part, PartOption, Pick, Plan, Runs};
use crate::prompt::{self, Budget, Input};
use crate::runtime::{LaunchConfig, Runtime};
use crate::senses::{self, AudioCppBuild, Engine, SenseModel};
use crate::settings::{BrainChoice, CompanionSettings};

/// What the screens show about the PC, before and after the runtime can list its GPUs.
#[derive(Clone, Debug)]
pub struct Machine {
    pub hardware: Hardware,
    /// The GPUs come from the runtime: until it is downloaded only RAM and cores are known.
    pub gpus_known: bool,
}

impl Machine {
    /// RAM and cores now, and the GPUs if the runtime is already installed (a second, at most).
    pub fn detect(home: &Path) -> Machine {
        let mut hardware = Hardware::detect();
        let server = runtime_build().and_then(|b| Layout::new(home).installed_runtime(b));
        let gpus_known = match server {
            Some(server) => match hardware::list_devices(&server) {
                Ok(out) => {
                    hardware.gpus = hardware::parse_devices(&out);
                    true
                }
                Err(_) => false,
            },
            None => false,
        };
        Machine {
            hardware,
            gpus_known,
        }
    }

    /// "16 GB of memory, 6 cores, NVIDIA GeForce RTX 3060 (11.2 GB free)".
    pub fn summary(&self) -> String {
        let hw = &self.hardware;
        let mut out = format!(
            "{} GB of memory, {} cores",
            hw.nominal_ram_gb(),
            hw.physical_cores
        );
        let gpu = if hw.apple_silicon {
            hw.gpus.iter().find(|g| g.id.starts_with("MTL"))
        } else {
            hw.best_gpu()
        };
        match gpu {
            Some(g) if hw.apple_silicon => out += &format!(", {}", g.name),
            Some(g) => out += &format!(", {} ({:.1} GB free)", g.name, g.free_mb as f64 / 1024.0),
            None if self.gpus_known => out += ", no graphics card the companion can use",
            None => out += "; the graphics card is checked during setup",
        }
        out
    }
}

/// The llama.cpp build for this PC: the GPU one where there is one (it also runs on the CPU). The
/// brain and the classifier share it.
pub fn runtime_build() -> Option<&'static RuntimeBuild> {
    catalogue::runtime_builds_here().into_iter().next()
}

/// The audio.cpp build for this PC (Metal on macOS, Vulkan with its CPU backend elsewhere).
pub fn voice_build() -> Option<&'static AudioCppBuild> {
    senses::audiocpp_builds(std::env::consts::OS, std::env::consts::ARCH)
        .into_iter()
        .next()
}

/// What a plan option id (`qwen3.5-4b-q4_k_m@gpu`, `parakeet-tdt-0.6b-v3-q8_0@cpu`) downloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Brain(&'static Model),
    Sense(&'static SenseModel),
}

impl Component {
    pub fn of(option_id: &str) -> Option<Component> {
        let id = option_id.split('@').next()?;
        catalogue::MODELS
            .iter()
            .find(|m| m.id == id)
            .map(Component::Brain)
            .or_else(|| senses::model(id).map(Component::Sense))
    }

    pub fn id(self) -> &'static str {
        match self {
            Component::Brain(m) => m.id,
            Component::Sense(m) => m.id,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Component::Brain(m) => m.name,
            Component::Sense(m) => m.name,
        }
    }

    pub fn license(self) -> &'static str {
        match self {
            Component::Brain(m) => m.license,
            Component::Sense(m) => m.license,
        }
    }

    pub fn size(self) -> u64 {
        match self {
            Component::Brain(m) => m.size(),
            Component::Sense(m) => m.size(),
        }
    }

    /// The Hugging Face repository and the pinned commit it comes from.
    pub fn source(self) -> (&'static str, &'static str) {
        match self {
            Component::Brain(m) => (m.repo, m.revision),
            Component::Sense(m) => (m.repo, m.revision),
        }
    }
}

/// What a setup reports.
#[derive(Clone, Debug)]
pub enum Event {
    /// What is happening now ("Downloading the runtime"), and how far, when known.
    Step(String, Option<f32>),
    /// The plan once the runtime could see the graphics card.
    Planned(Machine, Box<Plan>),
    Done(String),
    Failed(String),
    Cancelled,
    /// The brain test's numbers ([`Setup::test_brain`]).
    Tested(TestResult),
}

/// One turn of the brain the settings pick: load time, first word, memory and speed, for Advanced's
/// live numbers.
#[derive(Clone, Debug)]
pub struct TestResult {
    pub load: Duration,
    pub first_word: Duration,
    pub total: Duration,
    pub ram_mb: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub say: String,
}

/// A setup running on its own thread.
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    /// Runs `work` on its own thread; its result arrives as the last event.
    pub fn spawn(
        work: impl FnOnce(&Sender<Event>, &AtomicBool) -> Result<String, String> + Send + 'static,
    ) -> Job {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("companion setup".into())
            .spawn(move || {
                let event = match work(&tx, &flag) {
                    Ok(done) => Event::Done(done),
                    Err(_) if flag.load(Ordering::Relaxed) => Event::Cancelled,
                    Err(e) => Event::Failed(e),
                };
                let _ = tx.send(event);
            });
        if let Err(e) = spawned {
            let (tx, rx) = channel();
            let _ = tx.send(Event::Failed(e.to_string()));
            return Job { rx, cancel };
        }
        Job { rx, cancel }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Download progress as setup steps, a step every half percent.
pub fn reporter<'a>(tx: &'a Sender<Event>, what: &'a str) -> impl FnMut(Progress) + 'a {
    let mut last = -1.0;
    move |p: Progress| {
        let f = p.fraction();
        if f - last >= 0.005 || f >= 1.0 {
            last = f;
            let step = match p.phase {
                Phase::Unpacking => format!("Unpacking {what}"),
                _ => format!(
                    "Downloading {what}: {} of {} MB",
                    p.done / 1_000_000,
                    p.total / 1_000_000
                ),
            };
            let _ = tx.send(Event::Step(step, Some(f)));
        }
    }
}

/// The brain to start: the bundled runtime with a model, or a server the player named.
#[derive(Clone, Debug)]
pub enum BrainPart {
    Local {
        config: LaunchConfig,
        /// The catalogue tier, when it is one (Light's actions are restricted: `reply::Skill`).
        tier: Option<Tier>,
        label: String,
    },
    Server(Endpoint),
}

/// The classifier to start: the brain's llama-server with `--embeddings`.
#[derive(Clone, Debug)]
pub struct ClassifierPart {
    pub server: PathBuf,
    pub model: &'static SenseModel,
    /// The folder of its files.
    pub dir: PathBuf,
    pub threads: usize,
}

/// audio.cpp with the voice and speech recognition models.
#[derive(Clone, Debug)]
pub struct VoicePart {
    /// The folder `audiocpp_server` is in.
    pub runtime: PathBuf,
    pub exe: &'static str,
    pub backend: Backend,
    pub threads: usize,
    pub models: Vec<(&'static SenseModel, PathBuf)>,
}

/// What the game starts when a world with a companion loads: each part the settings chose, if it is
/// on disk. `missing` says in plain words what was chosen but isn't (the game then runs without it).
#[derive(Clone, Debug)]
pub struct Parts {
    pub plan: Plan,
    pub brain: Option<BrainPart>,
    pub classifier: Option<ClassifierPart>,
    pub voice: Option<VoicePart>,
    pub missing: Vec<String>,
}

/// The companion's files in a game home, for a game language.
#[derive(Clone, Debug)]
pub struct Setup {
    pub home: PathBuf,
    /// The game's language (`fr_fr`): the natural voice's pack follows it.
    pub language: String,
}

impl Setup {
    pub fn new(home: &Path, language: &str) -> Setup {
        Setup {
            home: home.to_owned(),
            language: language.to_owned(),
        }
    }

    pub fn layout(&self) -> Layout {
        Layout::new(&self.home)
    }

    /// Every part's options (`plan::options`).
    pub fn options(&self) -> Vec<PartOption> {
        plan::options(&self.language)
    }

    /// Whether an option's model is on disk.
    pub fn installed(&self, option_id: &str) -> bool {
        let layout = self.layout();
        match Component::of(option_id) {
            Some(Component::Brain(m)) => layout.installed_model(m).is_some(),
            Some(Component::Sense(m)) => layout.installed_sense_model(m).is_some(),
            None => false,
        }
    }

    /// Option ids and runtime keys already on disk, for the plan's download size.
    pub fn installed_ids(&self) -> Vec<String> {
        let layout = self.layout();
        let mut ids = Vec::new();
        if let Some(build) = runtime_build()
            && layout.installed_runtime(build).is_some()
        {
            ids.push(build.key());
        }
        if let Some(build) = voice_build()
            && layout.installed_voice_runtime(build).is_some()
        {
            ids.push(build.key());
        }
        for option in self.options() {
            if self.installed(&option.id) {
                ids.push(option.id);
            }
        }
        ids
    }

    pub fn plan(&self, machine: &Machine) -> Plan {
        let installed = self.installed_ids();
        let installed: Vec<&str> = installed.iter().map(String::as_str).collect();
        let disk = download::free_space(&self.home);
        plan::plan(&machine.hardware, &self.options(), &installed, disk)
    }

    /// Bytes on disk under `companion/`.
    pub fn disk_usage(&self) -> u64 {
        self.layout().disk_usage()
    }

    /// Deletes every model and runtime (No AI's "delete downloaded models"); returns the bytes freed.
    pub fn delete_everything(&self) -> Result<u64, String> {
        let layout = self.layout();
        let before = layout.disk_usage();
        let err = |e: std::io::Error| e.to_string();
        for model in catalogue::MODELS {
            layout.delete_model(model).map_err(err)?;
        }
        for model in senses::MODELS {
            layout.delete_sense_model(model).map_err(err)?;
        }
        for build in catalogue::RUNTIME_BUILDS {
            layout.delete_runtime(build).map_err(err)?;
        }
        for build in senses::AUDIOCPP_BUILDS {
            layout.delete_voice_runtime(build).map_err(err)?;
        }
        Ok(before.saturating_sub(layout.disk_usage()))
    }

    /// "Set up companion": the runtime first (small), then the PC is checked again with it, then what
    /// the settings and the plan pick, audio.cpp included when a voice part is picked. Saves the
    /// settings with `set_up` when done.
    pub fn set_up(&self, settings: CompanionSettings) -> Job {
        let setup = self.clone();
        Job::spawn(move |tx, cancel| {
            let layout = setup.layout();
            let build = runtime_build().ok_or("No runtime for this system")?;
            if layout.installed_runtime(build).is_none() {
                layout
                    .install_runtime(build, &mut reporter(tx, "the runtime"), cancel)
                    .map_err(|e| e.to_string())?;
            }
            let machine = Machine::detect(&setup.home);
            let plan = setup.plan(&machine);
            let _ = tx.send(Event::Planned(machine, Box::new(plan.clone())));
            let options = setup.options();
            let chosen = settings.selection(&plan, &options);
            let mut set_up = Vec::new();
            for option in &chosen {
                match Component::of(&option.id) {
                    Some(Component::Brain(model)) if layout.installed_model(model).is_none() => {
                        let what = format!("the {} brain", option.label);
                        layout
                            .install_model(model, &mut reporter(tx, &what), cancel)
                            .map_err(|e| e.to_string())?;
                    }
                    Some(Component::Sense(model)) => {
                        if matches!(model.engine, Engine::AudioCpp { .. }) {
                            let voice = voice_build().ok_or("No voice runtime for this system")?;
                            if layout.installed_voice_runtime(voice).is_none() {
                                layout
                                    .install_voice_runtime(
                                        voice,
                                        &mut reporter(tx, "the voice runtime"),
                                        cancel,
                                    )
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        if layout.installed_sense_model(model).is_none() {
                            let what = format!("the {}", option.label.to_lowercase());
                            layout
                                .install_sense_model(model, &mut reporter(tx, &what), cancel)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    _ => {}
                }
                set_up.push(option.label.clone());
            }
            let own_brain = matches!(
                settings.brain,
                BrainChoice::Gguf { .. } | BrainChoice::Server { .. }
            );
            if set_up.is_empty() && !own_brain {
                return Err(plan.summary);
            }
            let mut saved = settings.clone();
            saved.set_up = true;
            saved.save(&setup.home).map_err(|e| e.to_string())?;
            crate::settings::sync_extras(&setup.home, saved.no_ai).map_err(|e| e.to_string())?;
            Ok("Ready".into())
        })
    }

    /// What to start: each part the settings chose and the plan placed, if it is on disk.
    pub fn parts(&self, settings: &CompanionSettings, machine: &Machine) -> Parts {
        let layout = self.layout();
        let plan = self.plan(machine);
        let options = self.options();
        let chosen = settings.selection(&plan, &options);
        let mut missing = Vec::new();
        let llama = runtime_build().and_then(|b| layout.installed_runtime(b));
        let pick_of = |option: &PartOption| -> Pick {
            plan.picks
                .iter()
                .find(|p| p.option.id == option.id)
                .cloned()
                .unwrap_or_else(|| Pick {
                    device: match option.runs {
                        Runs::Cpu => Device::Cpu {
                            threads: option.threads,
                        },
                        Runs::Gpu => {
                            let gpu = machine
                                .hardware
                                .best_gpu()
                                .or(machine.hardware.gpus.first());
                            match gpu {
                                Some(g) => Device::Gpu {
                                    id: g.id.clone(),
                                    name: g.name.clone(),
                                },
                                None => Device::Cpu {
                                    threads: option.threads,
                                },
                            }
                        }
                    },
                    latency_ms: option.latency_ms,
                    option: option.clone(),
                })
        };
        let log = self.home.join("companion").join("logs").join("brain.log");
        let brain = match &settings.brain {
            BrainChoice::Server { .. } => settings.brain.endpoint().map(BrainPart::Server),
            BrainChoice::Gguf { path } => match &llama {
                Some(server) if path.exists() => {
                    let pick = plan.pick(Part::Brain).cloned();
                    let mut config = match &pick {
                        Some(pick) => LaunchConfig::for_pick(
                            server,
                            path,
                            catalogue::Family::Qwen35,
                            pick,
                            &log,
                        ),
                        None => LaunchConfig::for_hardware(
                            server,
                            path,
                            catalogue::Family::Qwen35,
                            &machine.hardware,
                            &log,
                        ),
                    };
                    tune(&mut config, settings);
                    Some(BrainPart::Local {
                        config,
                        tier: None,
                        label: "your own model".into(),
                    })
                }
                _ => {
                    missing.push(format!("the brain file {} is not there", path.display()));
                    None
                }
            },
            BrainChoice::Auto | BrainChoice::Option { .. } => {
                let option = chosen.iter().find(|o| o.part == Part::Brain);
                match (option, &llama) {
                    (Some(option), Some(server)) => match Component::of(&option.id) {
                        Some(Component::Brain(model)) => match layout.installed_model(model) {
                            Some(path) => {
                                let mut config = LaunchConfig::for_pick(
                                    server,
                                    &path,
                                    model.family,
                                    &pick_of(option),
                                    &log,
                                );
                                tune(&mut config, settings);
                                let tier = catalogue::Tier::ALL
                                    .into_iter()
                                    .find(|t| t.model().id == model.id);
                                Some(BrainPart::Local {
                                    config,
                                    tier,
                                    label: option.label.clone(),
                                })
                            }
                            None => {
                                missing
                                    .push(format!("the {} brain isn't downloaded", option.label));
                                None
                            }
                        },
                        _ => None,
                    },
                    (Some(_), None) => {
                        missing.push("the runtime isn't downloaded".into());
                        None
                    }
                    (None, _) => None,
                }
            }
        };
        let classifier = chosen
            .iter()
            .find(|o| o.part == Part::Classifier)
            .and_then(|option| {
                let Some(Component::Sense(model)) = Component::of(&option.id) else {
                    return None;
                };
                match (layout.installed_sense_model(model), &llama) {
                    (Some(dir), Some(server)) => Some(ClassifierPart {
                        server: server.clone(),
                        model,
                        dir,
                        threads: option.threads.max(1),
                    }),
                    _ => {
                        missing.push(format!("the {} isn't downloaded", option.label));
                        None
                    }
                }
            });
        let voice_options: Vec<&&PartOption> = chosen
            .iter()
            .filter(|o| matches!(o.part, Part::TextToSpeech | Part::SpeechToText))
            .collect();
        let voice = if voice_options.is_empty() {
            None
        } else {
            let runtime = voice_build().and_then(|b| Some((b, layout.installed_voice_runtime(b)?)));
            let mut models = Vec::new();
            let (mut threads, mut gpu) = (1, false);
            for option in &voice_options {
                let Some(Component::Sense(model)) = Component::of(&option.id) else {
                    continue;
                };
                match layout.installed_sense_model(model) {
                    Some(dir) => {
                        models.push((model, dir));
                        threads = threads.max(option.threads);
                        gpu |= option.runs == Runs::Gpu;
                    }
                    None => missing.push(format!("the {} isn't downloaded", option.label)),
                }
            }
            match runtime {
                Some((build, dir)) if !models.is_empty() => Some(VoicePart {
                    runtime: dir,
                    exe: build.server_exe(),
                    backend: if gpu { build.backend } else { Backend::Cpu },
                    threads,
                    models,
                }),
                Some(_) => None,
                None => {
                    missing.push("the voice runtime isn't downloaded".into());
                    None
                }
            }
        };
        Parts {
            plan,
            brain,
            classifier,
            voice,
            missing,
        }
    }
}

impl Setup {
    /// Starts the brain the settings pick, warms it and plays one turn, then stops it.
    pub fn test_brain(&self, settings: CompanionSettings, machine: Machine) -> Job {
        let setup = self.clone();
        Job::spawn(move |tx, cancel| {
            let parts = setup.parts(&settings, &machine);
            let mut runtime = None;
            let mut load = Duration::ZERO;
            let endpoint = match parts.brain {
                Some(BrainPart::Server(endpoint)) => endpoint,
                Some(BrainPart::Local {
                    mut config, label, ..
                }) => {
                    config.log = setup.home.join("companion").join("logs").join("test.log");
                    let _ = tx.send(Event::Step(format!("Starting the {label} brain"), None));
                    let started =
                        runtime.insert(Runtime::start(&config).map_err(|e| e.to_string())?);
                    load = started
                        .wait_ready(Duration::from_secs(300))
                        .map_err(|e| e.to_string())?;
                    started.endpoint()
                }
                None => {
                    return Err(parts
                        .missing
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "No brain is set up".into()));
                }
            };
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let _ = tx.send(Event::Step("Reading the instructions".into(), None));
            let mut brain = Brain::new(endpoint);
            let language = setup.language.clone();
            let system = prompt::system_prompt(&settings.persona, "you", &language);
            let input = Input::Said {
                who: "You".into(),
                text: "hey, can you grab some wood for us?".into(),
            };
            let sent = prompt::messages(
                &system,
                "",
                "Dimension: overworld, forest, morning.\nBlocks: 30 oak_log within 12 blocks.",
                &[],
                &input,
                &language,
                &Budget::default(),
            );
            let wait = |brain: &mut Brain, id: u64| -> Result<_, String> {
                let until = Instant::now() + Duration::from_secs(120);
                loop {
                    if cancel.load(Ordering::Relaxed) || Instant::now() > until {
                        return Err("cancelled".into());
                    }
                    match brain.try_recv() {
                        Some(BrainEvent::Done {
                            id: got,
                            reply,
                            stats,
                            ..
                        }) if got == id => return Ok((reply, stats)),
                        Some(BrainEvent::Failed { id: got, error }) if got == id => {
                            return Err(error);
                        }
                        _ => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            };
            let id = brain.ask(ChatRequest::warm_up(sent[0].clone()));
            wait(&mut brain, id)?;
            let _ = tx.send(Event::Step("Answering".into(), None));
            let id = brain.ask(ChatRequest::turn(sent));
            let (reply, stats) = wait(&mut brain, id)?;
            let result = TestResult {
                load,
                first_word: stats.first_token.unwrap_or(stats.total),
                total: stats.total,
                ram_mb: runtime.as_ref().and_then(Runtime::memory_mb),
                tokens_per_second: stats.tokens_per_second,
                say: reply.say,
            };
            if let Some(mut runtime) = runtime {
                runtime.stop();
            }
            let _ = tx.send(Event::Tested(result));
            Ok("Tested".into())
        })
    }
}

/// Advanced's runtime knobs on top of the plan's.
fn tune(config: &mut LaunchConfig, settings: &CompanionSettings) {
    if let Some(threads) = settings.tuning.threads {
        config.threads = threads.max(1);
    }
    if let Some(context) = settings.tuning.context {
        config.context = context.clamp(2048, 32768);
    }
    if settings.tuning.gpu_layers == Some(0) {
        config.offload = hardware::Offload::None;
    }
}

/// "1.2 GB".
pub fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_names_what_it_downloads() {
        for option in plan::options("fr_fr") {
            let component = Component::of(&option.id).expect(&option.id);
            assert_eq!(option.download, component.size(), "{}", option.id);
        }
        assert!(Component::of("nothing@gpu").is_none());
        assert!(matches!(
            Component::of("parakeet-tdt-0.6b-v3-q8_0@cpu"),
            Some(Component::Sense(_))
        ));
    }

    #[test]
    fn the_machine_line_says_what_is_not_known_yet() {
        let machine = Machine {
            hardware: Hardware {
                ram_mb: 16 * 1024 - 300,
                physical_cores: 6,
                logical_cores: 12,
                apple_silicon: false,
                gpus: Vec::new(),
            },
            gpus_known: false,
        };
        assert_eq!(
            machine.summary(),
            "16 GB of memory, 6 cores; the graphics card is checked during setup"
        );
        let known = Machine {
            gpus_known: true,
            ..machine
        };
        assert!(
            known
                .summary()
                .ends_with("no graphics card the companion can use")
        );
    }

    #[test]
    fn nothing_is_started_or_found_in_an_empty_home() {
        let home = std::env::temp_dir().join(format!("horizon-setup-test-{}", std::process::id()));
        let setup = Setup::new(&home, "en_us");
        let machine = Machine {
            hardware: Hardware {
                ram_mb: 16 * 1024 - 300,
                physical_cores: 6,
                logical_cores: 12,
                apple_silicon: false,
                gpus: Vec::new(),
            },
            gpus_known: true,
        };
        let parts = setup.parts(&CompanionSettings::default(), &machine);
        assert!(parts.brain.is_none() && parts.classifier.is_none() && parts.voice.is_none());
        assert!(!parts.missing.is_empty());
        assert!(setup.installed_ids().is_empty());
        assert!(!home.exists(), "looking must not create the home");
    }
}
