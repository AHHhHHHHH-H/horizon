//! The companion's setup from the launcher (docs/companions.md): what this PC is, the plan for it,
//! downloading what the plan picked with progress and cancel, testing the brain, and deleting models.
//! The work itself is `horizon-companion`'s; this runs it on worker threads and reports over a channel
//! the window polls. Everything lives in the game's home (`<home>/companion/`), where the game finds it.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use horizon_companion::catalogue::{self, Model, Tier};
use horizon_companion::client::{Brain, ChatRequest, Event as BrainEvent};
use horizon_companion::download::{self, Layout, Phase, Progress};
use horizon_companion::hardware::{self, Hardware};
use horizon_companion::plan::{self, Part, Plan};
use horizon_companion::prompt::{self, Budget, Input};
use horizon_companion::runtime::{LaunchConfig, Runtime};
pub use horizon_companion::settings::{BrainChoice, CompanionSettings, PartChoice};

use crate::backend;

/// Where the companion keeps its runtime, models and settings: the game's home.
pub fn home() -> PathBuf {
    backend::game_home()
}

pub fn layout() -> Layout {
    Layout::new(&home())
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

/// What the window shows about the PC, before and after the runtime can list its GPUs.
#[derive(Clone, Debug)]
pub struct Machine {
    pub hardware: Hardware,
    /// The GPUs come from the runtime: until it is downloaded only RAM and cores are known.
    pub gpus_known: bool,
}

impl Machine {
    /// RAM and cores now, and the GPUs if the runtime is already installed (a second, at most).
    pub fn detect() -> Machine {
        let mut hardware = Hardware::detect();
        let server = runtime_build().and_then(|b| layout().installed_runtime(b));
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

/// The runtime build for this PC: the GPU one where there is one (it also runs on the CPU).
pub fn runtime_build() -> Option<&'static catalogue::RuntimeBuild> {
    catalogue::runtime_builds_here().into_iter().next()
}

/// The options the launcher can set up today: the brain's. The senses sidecar adds its own at
/// integration.
pub fn options() -> Vec<plan::PartOption> {
    plan::brain_options()
}

/// Ids and runtime keys already on disk, for the plan's download size.
pub fn installed_ids() -> Vec<String> {
    let layout = layout();
    let mut ids = Vec::new();
    if let Some(build) = runtime_build() {
        if layout.installed_runtime(build).is_some() {
            ids.push(build.key());
        }
    }
    for option in options() {
        if model_of(&option.id).is_some_and(|m| layout.installed_model(m).is_some()) {
            ids.push(option.id);
        }
    }
    ids
}

pub fn make_plan(machine: &Machine) -> Plan {
    let installed = installed_ids();
    let installed: Vec<&str> = installed.iter().map(String::as_str).collect();
    let disk = download::free_space(&home()).or_else(|| download::free_space(&backend::data_dir()));
    plan::plan(&machine.hardware, &options(), &installed, disk)
}

/// The catalogue model behind a plan option id (`qwen3.5-4b-q4_k_m@gpu`).
pub fn model_of(option_id: &str) -> Option<&'static Model> {
    let id = option_id.split('@').next()?;
    catalogue::MODELS.iter().find(|m| m.id == id)
}

/// Models on disk and their size.
pub fn installed_models() -> Vec<(&'static Model, u64)> {
    let layout = layout();
    catalogue::MODELS
        .iter()
        .filter(|m| layout.installed_model(m).is_some())
        .map(|m| (m, m.size()))
        .collect()
}

/// Deletes every model and runtime (No AI's "delete downloaded models").
pub fn delete_everything() -> Result<u64, String> {
    let layout = layout();
    let before = layout.disk_usage();
    for model in catalogue::MODELS {
        layout.delete_model(model).map_err(|e| e.to_string())?;
    }
    for build in catalogue::RUNTIME_BUILDS {
        layout.delete_runtime(build).map_err(|e| e.to_string())?;
    }
    Ok(before)
}

pub fn delete_model(model: &Model) -> Result<(), String> {
    layout().delete_model(model).map_err(|e| e.to_string())
}

/// What a setup or a test reports.
#[derive(Clone, Debug)]
pub enum Event {
    /// What is happening now ("Downloading the runtime"), and how far, when known.
    Step(String, Option<f32>),
    /// The plan once the runtime could see the graphics card.
    Planned(Machine, Box<Plan>),
    Done(String),
    Failed(String),
    Cancelled,
    Tested(TestResult),
}

#[derive(Clone, Debug)]
pub struct TestResult {
    pub load: Duration,
    pub first_word: Duration,
    pub total: Duration,
    pub ram_mb: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub say: String,
}

/// A setup or a test running on its own thread.
pub struct Job {
    pub rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn spawn(
    work: impl FnOnce(&Sender<Event>, &AtomicBool) -> Result<String, String> + Send + 'static,
) -> Job {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let event = match work(&tx, &flag) {
            Ok(done) => Event::Done(done),
            Err(_) if flag.load(Ordering::Relaxed) => Event::Cancelled,
            Err(e) => Event::Failed(e),
        };
        let _ = tx.send(event);
    });
    Job { rx, cancel }
}

fn reporter<'a>(tx: &'a Sender<Event>, what: &'a str) -> impl FnMut(Progress) + 'a {
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

/// "Set up companion": the runtime first (small), then the PC is checked again with it, then what
/// the settings and the plan pick. `brain` is the option id Advanced chose, if any.
pub fn set_up(settings: CompanionSettings) -> Job {
    spawn(move |tx, cancel| {
        let layout = layout();
        let build = runtime_build().ok_or("No runtime for this system")?;
        if layout.installed_runtime(build).is_none() {
            layout
                .install_runtime(build, &mut reporter(tx, "the runtime"), cancel)
                .map_err(|e| e.to_string())?;
        }
        let machine = Machine::detect();
        let plan = make_plan(&machine);
        let _ = tx.send(Event::Planned(machine, Box::new(plan.clone())));
        let options = options();
        let chosen = settings.selection(&plan, &options);
        let mut set_up = Vec::new();
        for option in chosen {
            let Some(model) = model_of(&option.id) else {
                continue;
            };
            if layout.installed_model(model).is_none() {
                let what = format!("the {} brain", option.label);
                layout
                    .install_model(model, &mut reporter(tx, &what), cancel)
                    .map_err(|e| e.to_string())?;
            }
            set_up.push(option.label.clone());
        }
        if set_up.is_empty()
            && !matches!(
                settings.brain,
                BrainChoice::Gguf { .. } | BrainChoice::Server { .. }
            )
        {
            return Err(plan.summary);
        }
        let mut saved = settings.clone();
        saved.set_up = true;
        save_settings(&saved)?;
        Ok("Ready".into())
    })
}

/// Starts the brain the plan or Advanced picked, warms it and plays one turn: load time, first
/// word, memory and speed, for Advanced's live numbers. Stops it after.
pub fn test_brain(settings: CompanionSettings, machine: Machine) -> Job {
    spawn(move |tx, cancel| {
        let layout = layout();
        let build = runtime_build().ok_or("No runtime for this system")?;
        let server = layout
            .installed_runtime(build)
            .ok_or("Set up the companion first")?;
        let plan = make_plan(&machine);
        let options = options();
        let option = settings
            .selection(&plan, &options)
            .into_iter()
            .find(|o| o.part == Part::Brain)
            .ok_or("No brain is set up")?
            .clone();
        let model = model_of(&option.id).ok_or("Not a catalogue brain")?;
        let path = layout
            .installed_model(model)
            .ok_or("This brain isn't downloaded yet")?;
        let pick = plan
            .pick(Part::Brain)
            .filter(|p| p.option.id == option.id)
            .cloned()
            .unwrap_or(plan::Pick {
                device: match option.runs {
                    plan::Runs::Cpu => plan::Device::Cpu { threads: 2 },
                    plan::Runs::Gpu => {
                        let gpu = machine
                            .hardware
                            .best_gpu()
                            .or(machine.hardware.gpus.first())
                            .ok_or("No graphics card for this option")?;
                        plan::Device::Gpu {
                            id: gpu.id.clone(),
                            name: gpu.name.clone(),
                        }
                    }
                },
                latency_ms: option.latency_ms,
                option: option.clone(),
            });
        let log = home()
            .join("companion")
            .join("logs")
            .join("launcher-test.log");
        let mut config = LaunchConfig::for_pick(&server, &path, model.family, &pick, &log);
        if let Some(threads) = settings.tuning.threads {
            config.threads = threads;
        }
        let _ = tx.send(Event::Step(
            format!("Starting the {} brain", option.label),
            None,
        ));
        let mut runtime = Runtime::start(&config).map_err(|e| e.to_string())?;
        let load = runtime
            .wait_ready(Duration::from_secs(300))
            .map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let _ = tx.send(Event::Step("Reading the instructions".into(), None));
        let mut brain = Brain::new(runtime.endpoint());
        let language = "en_us";
        let system = prompt::system_prompt(&settings.persona, "you", language);
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
            language,
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
                    Some(BrainEvent::Failed { id: got, error }) if got == id => return Err(error),
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
            ram_mb: runtime.memory_mb(),
            tokens_per_second: stats.tokens_per_second,
            say: reply.say,
        };
        runtime.stop();
        let _ = tx.send(Event::Tested(result));
        Ok("Tested".into())
    })
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

pub fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_option_ids_name_their_catalogue_model() {
        for option in options() {
            let model = model_of(&option.id).expect(&option.id);
            assert_eq!(option.download, model.size());
        }
        assert!(model_of("nothing@gpu").is_none());
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
        assert!(known
            .summary()
            .ends_with("no graphics card the companion can use"));
    }
}
