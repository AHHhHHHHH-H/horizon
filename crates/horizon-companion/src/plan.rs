//! One recommended setup for this PC across every part of the companion (docs/companions.md, "Fitting
//! the player's PC"): which option runs each part and where, what it downloads, what it takes from RAM
//! and VRAM while the game runs, and a sentence that says so.
//!
//! The parts' options come from lists: the brain's from its catalogue ([`brain_options`]), the
//! classifier's, speech recognition's and the voice's from the senses' ([`sense_options`]); [`options`]
//! is all of them. `plan` only needs each option's needs.

use crate::catalogue::{self, Backend, Model, ModelKind, Tier};
use crate::hardware::{GAME_VRAM_MB, Gpu, GpuKind, Hardware};
use crate::runtime;
use crate::senses::{self, Engine, SenseKind, SenseModel};

/// RAM the game itself keeps (chunks, textures, the JVM-free client still needs room).
pub const GAME_RAM_MB: u64 = 3072;
/// RAM the system and the other open programs keep.
pub const SYSTEM_RAM_MB: u64 = 2048;
/// A brain is only picked for speed if its first word comes within this; slower brains are picked only
/// when nothing faster fits, and the plan says replies are slow. The reflexes and the instant
/// acknowledgement cover the wait.
pub const BRAIN_FIRST_WORD_MS: u32 = 2000;

/// The parts, in the order they are given room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Brain,
    /// The embedding classifier behind the rules (`reflex::IntentClassifier`).
    Classifier,
    /// The companion's voice.
    TextToSpeech,
    /// Push-to-talk recognition; it comes with the voice.
    SpeechToText,
}

impl Part {
    pub const ALL: [Part; 4] = [
        Part::Brain,
        Part::Classifier,
        Part::TextToSpeech,
        Part::SpeechToText,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Part::Brain => "brain",
            Part::Classifier => "command recognition",
            Part::SpeechToText => "speech recognition",
            Part::TextToSpeech => "voice",
        }
    }
}

/// Where an option runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// On a discrete GPU (or Apple Silicon's), with one of its backends.
    Gpu,
    Cpu,
}

/// One way to run a part, and what it needs there.
#[derive(Clone, Debug, PartialEq)]
pub struct PartOption {
    /// Unique: the catalogue id plus where it runs (`qwen3.5-4b-q4_k_m@gpu`).
    pub id: String,
    pub part: Part,
    /// The plain label ("Standard", "Natural voice").
    pub label: String,
    /// The model's own name, for Advanced.
    pub name: String,
    /// Higher is better, within a part.
    pub quality: u8,
    pub runs: Runs,
    /// GPU backends it works with (only for [`Runs::Gpu`]).
    pub backends: Vec<Backend>,
    /// Model files to download, in bytes.
    pub download: u64,
    /// The runtime it needs (`b11756-macos-aarch64-metal`) and its size: shared parts download it once.
    pub runtime: Option<(String, u64)>,
    /// System RAM while it runs, the model included when it runs on the CPU.
    pub ram_mb: u64,
    /// GPU memory while it runs.
    pub vram_mb: u64,
    /// CPU threads it takes.
    pub threads: usize,
    /// Runs only while the brain waits: speech recognition and the classifier finish before the brain
    /// starts, so they borrow its threads. The voice speaks while the brain still writes, so it needs
    /// threads of its own.
    pub before_brain: bool,
    /// Expected wait before the first word or sound, in ms, on a typical PC of its kind.
    pub latency_ms: u32,
    /// The catalogue's quality line for Advanced ("Q4_K_M, 8K context").
    pub detail: String,
}

/// The brain's options from the catalogue: every tier on a GPU, and Light and Standard on the CPU
/// (bigger models are too slow there to be worth offering).
pub fn brain_options() -> Vec<PartOption> {
    let gpu_runtime = catalogue::runtime_builds_here()
        .into_iter()
        .find(|b| b.backend != Backend::Cpu)
        .or_else(|| catalogue::runtime_builds_here().into_iter().next());
    let cpu_runtime = catalogue::runtime_builds_here()
        .into_iter()
        .find(|b| b.backend == Backend::Cpu)
        .or(gpu_runtime);
    let runtime_of = |b: Option<&catalogue::RuntimeBuild>| b.map(|b| (b.key(), b.size));
    let mut out = Vec::new();
    for tier in Tier::ALL {
        let model = tier.model();
        let weight = brain_weight(tier);
        out.push(PartOption {
            id: format!("{}@gpu", model.id),
            part: Part::Brain,
            label: tier_label(tier).into(),
            name: model.name.into(),
            quality: tier as u8 + 1,
            runs: Runs::Gpu,
            backends: vec![Backend::Metal, Backend::Vulkan],
            download: model.size(),
            runtime: runtime_of(gpu_runtime),
            ram_mb: 400,
            vram_mb: brain_vram_mb(model),
            threads: 1,
            before_brain: false,
            latency_ms: (150.0 * weight) as u32,
            detail: format!(
                "{}, {}K context",
                model.quantisation,
                runtime::CONTEXT / 1024
            ),
        });
        if matches!(tier, Tier::Light | Tier::Standard) {
            out.push(PartOption {
                id: format!("{}@cpu", model.id),
                runs: Runs::Cpu,
                backends: Vec::new(),
                runtime: runtime_of(cpu_runtime),
                // Measured: Light peaked at 2.3 GB on the ThinkPad (a 1.2 GB file).
                ram_mb: (model.size() >> 20) + 1100,
                vram_mb: 0,
                threads: 2,
                latency_ms: (CPU_LIGHT_MS_PER_THREAD as f64 / 2.0 * weight) as u32,
                ..out.last().cloned().expect("pushed above")
            });
        }
    }
    out
}

/// Every part's options for a game language (`fr_fr`): the brain's and the senses'.
pub fn options(language: &str) -> Vec<PartOption> {
    options_with(language, false)
}

/// The same, with the cloning voices when the voice pack (the cast's clips) is installed.
pub fn options_with(language: &str, cast: bool) -> Vec<PartOption> {
    let mut out = brain_options();
    out.extend(sense_options(language, cast));
    out
}

/// The senses' options from their catalogue: the classifier (on the brain's llama.cpp), speech
/// recognition and the voice (on audio.cpp). Of the natural voice's packs only the game language's is
/// offered (English when it has none); the expressive voice speaks every language in one model.
pub fn sense_options(language: &str, cast: bool) -> Vec<PartOption> {
    let llama = catalogue::runtime_builds_here()
        .into_iter()
        .next()
        .map(|b| (b.key(), b.size));
    let audio = senses::audiocpp_builds(std::env::consts::OS, std::env::consts::ARCH)
        .into_iter()
        .next()
        .map(|b| (b.key(), b.size()));
    let pocket = senses::pocket_for(language).id;
    senses::MODELS
        .iter()
        .filter(|m| {
            m.kind != SenseKind::TextToSpeech
                || m.instruct
                || m.id == pocket
                // Qwen3-TTS Base has no voice of its own: only with the cast's clips.
                || (cast && m.clone && m.voices.is_empty())
        })
        .flat_map(|m| {
            let runtime = match m.engine {
                Engine::LlamaEmbeddings => llama.clone(),
                Engine::AudioCpp { .. } => audio.clone(),
            };
            let gpu: Vec<Backend> = m
                .needs
                .backends
                .iter()
                .copied()
                .filter(|b| *b != Backend::Cpu)
                .collect();
            let on_cpu = m.needs.backends.contains(&Backend::Cpu);
            let base = sense_option(m, runtime);
            let gpu_option = (!gpu.is_empty()).then(|| PartOption {
                id: format!("{}@gpu", m.id),
                runs: Runs::Gpu,
                backends: gpu,
                vram_mb: m.needs.vram_mb,
                ..base.clone()
            });
            gpu_option.into_iter().chain(on_cpu.then_some(base))
        })
        .collect()
}

fn sense_option(m: &SenseModel, runtime: Option<(String, u64)>) -> PartOption {
    let part = match m.kind {
        SenseKind::Classifier => Part::Classifier,
        SenseKind::SpeechToText => Part::SpeechToText,
        SenseKind::TextToSpeech => Part::TextToSpeech,
    };
    PartOption {
        id: format!("{}@cpu", m.id),
        part,
        // "Natural voice", from "Natural voice — runs on the processor".
        label: m.label.split(" — ").next().unwrap_or(m.label).to_owned(),
        name: m.name.to_owned(),
        quality: m.quality,
        runs: Runs::Cpu,
        backends: Vec::new(),
        download: m.size(),
        runtime,
        ram_mb: m.needs.ram_mb,
        vram_mb: 0,
        threads: m.needs.cores,
        // Recognition and the classifier finish before the brain starts; the voice speaks while it
        // still writes.
        before_brain: part != Part::TextToSpeech,
        latency_ms: m.first_ms,
        detail: format!("{}, {}", m.quantisation, m.latency),
    }
}

/// First-word time of the Light brain on the CPU, times its thread count, on a current desktop CPU:
/// measured 1.3 s on two Apple M4 cores (2600); a recent x86 desktop core is assumed 1.5× slower.
const CPU_LIGHT_MS_PER_THREAD: u32 = 4000;
/// CPUs with four threads or fewer are old or low-power laptops: measured 3.8 s on two threads of the
/// ThinkPad's i7-7500U, 1.9× the desktop estimate.
const SMALL_CPU_SLOWDOWN: f64 = 1.9;

/// Compute relative to Light (measured on Metal: Standard made 59 tokens/s to Light's 123).
fn brain_weight(tier: Tier) -> f64 {
    match tier {
        Tier::Light => 1.0,
        Tier::Standard => 2.1,
        Tier::Strong => 4.3,
        Tier::Best => 5.5,
    }
}

fn tier_label(tier: Tier) -> &'static str {
    match tier {
        Tier::Light => "Light",
        Tier::Standard => "Standard",
        Tier::Strong => "Strong",
        Tier::Best => "Best",
    }
}

/// The catalogue's measured working set.
fn brain_vram_mb(model: &Model) -> u64 {
    model.memory_mb
}

/// Where a picked option runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Device {
    Gpu { id: String, name: String },
    Cpu { threads: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub option: PartOption,
    pub device: Device,
    /// Expected first word or sound on this PC, in ms.
    pub latency_ms: u32,
}

/// A part the PC can't run, and why, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub part: Part,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub picks: Vec<Pick>,
    pub missing: Vec<Missing>,
    /// Everything still to download, runtimes included.
    pub download: u64,
    /// What the companion takes while the game runs.
    pub ram_mb: u64,
    pub vram_mb: u64,
    /// One sentence for the simple screen.
    pub summary: String,
}

impl Plan {
    pub fn pick(&self, part: Part) -> Option<&Pick> {
        self.picks.iter().find(|p| p.option.part == part)
    }

    /// No part runs locally: the companion still follows commands with its reflexes and phrases.
    pub fn chat_only(&self) -> bool {
        self.pick(Part::Brain).is_none()
    }
}

/// What the plan may use once the game and the system have their share.
#[derive(Clone, Debug)]
struct Room {
    ram_mb: u64,
    vram_mb: u64,
    gpu: Option<Gpu>,
    /// Apple Silicon: GPU memory is the same RAM.
    unified: bool,
    /// The best brain quality allowed, on a GPU the game draws on too.
    brain_cap: u8,
    threads: usize,
    physical: usize,
    /// Four threads or fewer.
    small_cpu: bool,
    /// Threads taken by the brain and the voice.
    taken_threads: usize,
    disk: Option<u64>,
}

impl Room {
    fn of(hw: &Hardware, disk: Option<u64>) -> Room {
        let gpu = if hw.apple_silicon {
            // Before the runtime lists it, Metal's working set is about two thirds of the RAM.
            hw.gpus
                .iter()
                .find(|g| g.id.starts_with("MTL"))
                .cloned()
                .or_else(|| {
                    Some(Gpu {
                        id: "MTL0".into(),
                        name: "Apple Silicon".into(),
                        total_mb: hw.ram_mb * 2 / 3,
                        free_mb: hw.ram_mb * 2 / 3,
                        kind: GpuKind::Integrated,
                    })
                })
        } else {
            hw.best_gpu().cloned()
        };
        let logical = hw.logical_cores.max(hw.physical_cores).max(1);
        // The game keeps one core on small CPUs and two on the rest (its main and render threads,
        // plus chunk work).
        let game_threads = if logical <= 4 { 2 } else { 4 };
        Room {
            ram_mb: hw.ram_mb.saturating_sub(GAME_RAM_MB + SYSTEM_RAM_MB),
            vram_mb: gpu
                .as_ref()
                .map_or(0, |g| g.free_mb.saturating_sub(GAME_VRAM_MB)),
            unified: hw.apple_silicon && gpu.is_some(),
            // On a Mac the game renders on the GPU the model runs on, so a bigger model costs frames,
            // not just memory: the design's Mac classes cap the tier (16 GB Standard, 24 GB Strong,
            // 32 GB Best).
            brain_cap: if hw.apple_silicon {
                match hw.nominal_ram_gb() {
                    32.. => Tier::Best as u8 + 1,
                    24.. => Tier::Strong as u8 + 1,
                    16.. => Tier::Standard as u8 + 1,
                    _ => Tier::Light as u8 + 1,
                }
            } else {
                u8::MAX
            },
            gpu,
            threads: logical.saturating_sub(game_threads).max(1),
            physical: hw.physical_cores.max(1),
            small_cpu: logical <= 4,
            taken_threads: 0,
            disk,
        }
    }

    /// Why `option` doesn't fit, or None if it does.
    fn refuse(&self, option: &PartOption, download: u64) -> Option<String> {
        let gpu_mb = if option.runs == Runs::Gpu {
            match &self.gpu {
                Some(gpu) if option.backends.contains(&gpu.backend()) => option.vram_mb,
                Some(_) => return Some("it doesn't support this graphics card".into()),
                None => return Some("it needs a graphics card of its own".into()),
            }
        } else {
            0
        };
        if option.part == Part::Brain && option.quality > self.brain_cap {
            return Some("the game needs this Mac's graphics for itself".into());
        }
        let ram = self.ram_charge(option);
        if gpu_mb > self.vram_mb {
            return Some(format!(
                "it needs {} of graphics memory and {} is free",
                gb(gpu_mb),
                gb(self.vram_mb)
            ));
        }
        if ram > self.ram_mb {
            return Some(format!(
                "it needs {} of RAM and {} is free",
                gb(ram),
                gb(self.ram_mb)
            ));
        }
        let needed = if option.before_brain {
            option.threads
        } else {
            self.taken_threads + self.threads_for(option)
        };
        if needed > self.threads {
            return Some("the processor has no core to spare".into());
        }
        if let Some(disk) = self.disk
            && download > disk
        {
            return Some(format!(
                "it needs {} of disk space and {} is free",
                gb(download >> 20),
                gb(disk >> 20)
            ));
        }
        None
    }

    /// RAM an option takes. With unified memory its GPU working set is RAM, and the process's own
    /// share is inside it (measured: Light peaked at 1.9 GB on Metal for 1.9 GB planned).
    fn ram_charge(&self, option: &PartOption) -> u64 {
        if self.unified && option.runs == Runs::Gpu {
            option.vram_mb.max(option.ram_mb)
        } else {
            option.ram_mb
        }
    }

    /// The threads an option keeps for itself here. A brain on the CPU takes physical cores − 1 of what
    /// is left, keeping two for a voice when there are six or more.
    fn threads_for(&self, option: &PartOption) -> usize {
        match (option.runs, option.part) {
            (Runs::Gpu, _) => 0,
            (Runs::Cpu, Part::Brain) => {
                let left = self.threads.saturating_sub(self.taken_threads);
                let spare = if left >= 6 { 2 } else { 0 };
                (self.physical.saturating_sub(1))
                    .min(left - spare)
                    .max(option.threads)
            }
            (Runs::Cpu, _) => option.threads,
        }
    }

    fn take(&mut self, option: &PartOption, download: u64) {
        let gpu_mb = if option.runs == Runs::Gpu {
            option.vram_mb
        } else {
            0
        };
        self.vram_mb -= gpu_mb;
        self.ram_mb -= self.ram_charge(option);
        if !option.before_brain {
            self.taken_threads += self.threads_for(option);
        }
        if let Some(disk) = &mut self.disk {
            *disk -= download;
        }
    }
}

/// The recommended setup. `options` lists every part's options (the brain's from
/// [`brain_options`], the others from their sidecar); `installed` names the option ids and runtime
/// keys already on disk; `disk` is the free space where models go, when known.
pub fn plan(hw: &Hardware, options: &[PartOption], installed: &[&str], disk: Option<u64>) -> Plan {
    let mut room = Room::of(hw, disk);
    let mut runtimes: Vec<String> = Vec::new();
    let mut picks = Vec::new();
    let mut missing = Vec::new();
    for part in Part::ALL {
        let mut candidates: Vec<&PartOption> = options.iter().filter(|o| o.part == part).collect();
        if candidates.is_empty() {
            continue;
        }
        candidates.sort_by_key(|o| std::cmp::Reverse(o.quality));
        let mut fits = Vec::new();
        let mut refusal = None;
        for option in candidates {
            let download = download_of(option, installed, &runtimes);
            match room.refuse(option, download) {
                None => fits.push((option, download)),
                // The last refusal is the smallest option's: what even that one lacks.
                Some(reason) => refusal = Some(reason),
            }
        }
        let latency = |o: &PartOption| latency_on(o, &room);
        if part == Part::Brain && fits.iter().any(|(o, _)| o.runs == Runs::Gpu) {
            // A graphics card that holds any brain gets it: on the processor even a better brain
            // would take the cores the game runs on, and answer several times slower.
            fits.retain(|(o, _)| o.runs == Runs::Gpu);
        }
        let choice = if part == Part::Brain {
            // The best brain fast enough, else the fastest that fits.
            fits.iter()
                .find(|(o, _)| latency(o) <= BRAIN_FIRST_WORD_MS)
                .or_else(|| fits.iter().min_by_key(|(o, _)| latency(o)))
                .copied()
        } else {
            fits.first().copied()
        };
        match choice {
            Some((option, download)) => {
                let latency_ms = latency(option);
                let device = match option.runs {
                    Runs::Gpu => {
                        let gpu = room.gpu.as_ref().expect("refuse checked the GPU");
                        Device::Gpu {
                            id: gpu.id.clone(),
                            name: gpu.name.clone(),
                        }
                    }
                    Runs::Cpu => Device::Cpu {
                        threads: room.threads_for(option),
                    },
                };
                room.take(option, download);
                if let Some((key, _)) = &option.runtime {
                    runtimes.push(key.clone());
                }
                picks.push(Pick {
                    option: option.clone(),
                    device,
                    latency_ms,
                });
            }
            None => missing.push(Missing {
                part,
                reason: refusal.unwrap_or_default(),
            }),
        }
    }
    // Speech recognition is half of the voice feature: without a voice to answer with, it goes too.
    if missing.iter().any(|m| m.part == Part::TextToSpeech)
        && let Some(i) = picks
            .iter()
            .position(|p| p.option.part == Part::SpeechToText)
    {
        picks.remove(i);
        missing.push(Missing {
            part: Part::SpeechToText,
            reason: "it comes with the voice, which this PC can't run next to the game".into(),
        });
    }
    let download = picks
        .iter()
        .map(|p| download_counted(p, installed))
        .sum::<u64>()
        + runtime_downloads(&picks, installed);
    let ram_mb = picks.iter().map(|p| room.ram_charge(&p.option)).sum();
    let vram_mb = picks
        .iter()
        .filter(|p| p.option.runs == Runs::Gpu)
        .map(|p| p.option.vram_mb)
        .sum();
    let mut plan = Plan {
        picks,
        missing,
        download,
        ram_mb,
        vram_mb,
        summary: String::new(),
    };
    plan.summary = summary(&plan);
    plan
}

fn download_of(option: &PartOption, installed: &[&str], runtimes: &[String]) -> u64 {
    let model = if installed.contains(&option.id.as_str()) {
        0
    } else {
        option.download
    };
    let runtime = match &option.runtime {
        Some((key, size)) if !installed.contains(&key.as_str()) && !runtimes.contains(key) => *size,
        _ => 0,
    };
    model + runtime
}

fn download_counted(pick: &Pick, installed: &[&str]) -> u64 {
    if installed.contains(&pick.option.id.as_str()) {
        0
    } else {
        pick.option.download
    }
}

fn runtime_downloads(picks: &[Pick], installed: &[&str]) -> u64 {
    let mut seen: Vec<&str> = Vec::new();
    let mut total = 0;
    for (key, size) in picks.iter().filter_map(|p| p.option.runtime.as_ref()) {
        if !installed.contains(&key.as_str()) && !seen.contains(&key.as_str()) {
            seen.push(key);
            total += size;
        }
    }
    total
}

fn latency_on(option: &PartOption, room: &Room) -> u32 {
    if option.part == Part::Brain && option.runs == Runs::Cpu {
        let threads = room.threads_for(option).max(1) as f64;
        let slowdown = if room.small_cpu {
            SMALL_CPU_SLOWDOWN
        } else {
            1.0
        };
        (option.latency_ms as f64 * option.threads as f64 / threads * slowdown) as u32
    } else {
        option.latency_ms
    }
}

fn gb(mb: u64) -> String {
    if mb < 1024 {
        format!("{mb} MB")
    } else {
        format!("{:.1} GB", mb as f64 / 1024.0)
    }
}

/// "RTX 3060" from "NVIDIA GeForce RTX 3060", "Radeon RX 6600" from "AMD Radeon RX 6600 (RADV NAVI23)".
fn short_gpu_name(name: &str) -> String {
    let mut short = name.replace("(TM)", "").replace("(R)", "");
    // The Vulkan driver's own suffix.
    if let Some(i) = short.find(" (") {
        short.truncate(i);
    }
    for prefix in ["NVIDIA ", "GeForce ", "AMD ", "Intel "] {
        if let Some(rest) = short.trim_start().strip_prefix(prefix) {
            short = rest.to_owned();
        }
    }
    short.trim().to_owned()
}

fn on(pick: &Pick) -> String {
    match &pick.device {
        Device::Gpu { name, .. } => format!("on your {}", short_gpu_name(name)),
        Device::Cpu { .. } => "on the processor".into(),
    }
}

fn summary(plan: &Plan) -> String {
    let Some(brain) = plan.pick(Part::Brain) else {
        return "This PC can't run a brain next to the game: your companion follows simple commands, \
                or connect Ollama, LM Studio or a cloud model in Advanced."
            .into();
    };
    let mut out = format!("{} brain {}", brain.option.label, on(brain));
    if brain.latency_ms > BRAIN_FIRST_WORD_MS {
        out += " (slow replies)";
    }
    match plan.pick(Part::TextToSpeech) {
        Some(voice) if on(voice) == on(brain) => {
            out = format!(
                "{} brain and {} {}",
                brain.option.label,
                lower(&voice.option.label),
                on(brain)
            );
        }
        Some(voice) => out += &format!(", {} {}", lower(&voice.option.label), on(voice)),
        None if plan.missing.iter().any(|m| m.part == Part::TextToSpeech) => {
            out += ", chat only (no voice on this PC)"
        }
        None => {}
    }
    if plan.download > 0 {
        out += &format!(": {} to download", gb(plan.download.div_ceil(1 << 20)));
    }
    out
}

fn lower(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Whether a catalogue model belongs to a part (sidecar models are listed with their kinds).
pub fn part_of(kind: ModelKind) -> Option<Part> {
    match kind {
        ModelKind::Chat(_) => Some(Part::Brain),
        ModelKind::Classifier => Some(Part::Classifier),
        ModelKind::SpeechToText => Some(Part::SpeechToText),
        ModelKind::TextToSpeech => Some(Part::TextToSpeech),
        ModelKind::VoiceActivity => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<PartOption> {
        options("en_us")
    }

    fn gpu(id: &str, name: &str, total_gb: u64, free_mb: u64, kind: GpuKind) -> Gpu {
        Gpu {
            id: id.into(),
            name: name.into(),
            total_mb: total_gb * 1024,
            free_mb,
            kind,
        }
    }

    fn pc(ram_gb: u64, physical: usize, logical: usize, gpus: Vec<Gpu>) -> Hardware {
        Hardware {
            // What the OS reports: a little under the nominal size.
            ram_mb: ram_gb * 1024 - 300,
            physical_cores: physical,
            logical_cores: logical,
            apple_silicon: gpus.iter().any(|g| g.id.starts_with("MTL")),
            gpus,
        }
    }

    fn brain(plan: &Plan) -> (&str, &Device) {
        let pick = plan.pick(Part::Brain).expect("a brain");
        (pick.option.label.as_str(), &pick.device)
    }

    fn voice(plan: &Plan) -> Option<(&str, Runs)> {
        plan.pick(Part::TextToSpeech)
            .map(|p| (p.option.label.as_str(), p.option.runs))
    }

    fn stays_within(plan: &Plan, hw: &Hardware) {
        assert!(
            plan.ram_mb + GAME_RAM_MB + SYSTEM_RAM_MB <= hw.ram_mb,
            "{} MB of RAM on a {} MB PC",
            plan.ram_mb,
            hw.ram_mb
        );
        if let Some(gpu) = hw.best_gpu() {
            assert!(plan.vram_mb + GAME_VRAM_MB <= gpu.free_mb);
        }
    }

    fn hd620() -> Gpu {
        gpu(
            "Vulkan0",
            "Intel(R) HD Graphics 620 (KBL GT2)",
            16,
            15_000,
            GpuKind::Integrated,
        )
    }

    #[test]
    fn thinkpad_hd620_with_8_gb_gets_light_on_the_cpu_and_chat_only() {
        let hw = pc(8, 2, 4, vec![hd620()]);
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan), ("Light", &Device::Cpu { threads: 2 }));
        assert!(plan.pick(Part::Classifier).is_some(), "{plan:#?}");
        assert!(voice(&plan).is_none(), "{plan:#?}");
        assert!(plan.summary.contains("chat only"), "{}", plan.summary);
        assert!(plan.summary.contains("slow replies"), "{}", plan.summary);
        stays_within(&plan, &hw);
    }

    #[test]
    fn thinkpad_hd620_with_32_gb_still_runs_light_since_two_cores_are_the_limit() {
        let hw = pc(32, 2, 4, vec![hd620()]);
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan), ("Light", &Device::Cpu { threads: 2 }));
        // RAM is plenty, but the voice would need threads the brain and the game use.
        assert!(voice(&plan).is_none());
        assert!(plan.pick(Part::SpeechToText).is_none());
        let why = &plan
            .missing
            .iter()
            .find(|m| m.part == Part::TextToSpeech)
            .unwrap()
            .reason;
        assert!(why.contains("core"), "{why}");
        // The classifier runs before the brain, on its threads.
        assert!(plan.pick(Part::Classifier).is_some());
        stays_within(&plan, &hw);
    }

    #[test]
    fn gtx_1650_runs_light_on_its_4_gb() {
        let hw = pc(
            16,
            4,
            8,
            vec![gpu(
                "Vulkan0",
                "NVIDIA GeForce GTX 1650",
                4,
                3_700,
                GpuKind::Discrete,
            )],
        );
        let plan = plan(&hw, &all(), &[], None);
        let (label, device) = brain(&plan);
        assert_eq!(label, "Light");
        assert!(matches!(device, Device::Gpu { .. }));
        assert_eq!(voice(&plan), Some(("Natural voice", Runs::Cpu)));
        assert!(
            plan.summary
                .starts_with("Light brain on your GTX 1650, natural voice on the processor"),
            "{}",
            plan.summary
        );
        stays_within(&plan, &hw);
    }

    #[test]
    fn rtx_3060_runs_the_best_brain_and_the_voice_on_the_processor() {
        let hw = pc(
            16,
            6,
            12,
            vec![gpu(
                "Vulkan0",
                "NVIDIA GeForce RTX 3060",
                12,
                11_500,
                GpuKind::Discrete,
            )],
        );
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan).0, "Best");
        // The expressive voice would not fit next to it: the natural one runs on the CPU.
        assert_eq!(voice(&plan), Some(("Natural voice", Runs::Cpu)));
        assert_eq!(
            plan.pick(Part::SpeechToText).unwrap().option.name,
            "NVIDIA Parakeet TDT 0.6B v3"
        );
        assert!(
            plan.summary
                .starts_with("Best brain on your RTX 3060, natural voice on the processor: "),
            "{}",
            plan.summary
        );
        assert!(plan.summary.ends_with("GB to download"), "{}", plan.summary);
        stays_within(&plan, &hw);
    }

    #[test]
    fn parakeet_needs_its_3_gb_or_the_light_recognition_is_used() {
        // 8 GB and a 4 GB card: the brain fits on the card, Parakeet's 3.3 GB doesn't fit in what
        // the game and the system leave, Canary's 0.8 GB does.
        let hw = pc(
            8,
            4,
            8,
            vec![gpu(
                "Vulkan0",
                "NVIDIA GeForce GTX 1650",
                4,
                3_700,
                GpuKind::Discrete,
            )],
        );
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(voice(&plan), Some(("Natural voice", Runs::Cpu)));
        assert_eq!(
            plan.pick(Part::SpeechToText).unwrap().option.name,
            "NVIDIA Canary 180M Flash"
        );
        stays_within(&plan, &hw);
        // With 16 GB, Parakeet.
        let hw = Hardware {
            ram_mb: 16 * 1024 - 300,
            ..hw
        };
        let plan = super::plan(&hw, &all(), &[], None);
        assert_eq!(
            plan.pick(Part::SpeechToText).unwrap().option.name,
            "NVIDIA Parakeet TDT 0.6B v3"
        );
    }

    #[test]
    fn the_natural_voice_speaks_the_game_language() {
        let ids: Vec<String> = options("fr_fr").into_iter().map(|o| o.id).collect();
        assert!(
            ids.iter().any(|i| i.starts_with("pocket-tts-fr-")),
            "{ids:?}"
        );
        assert!(
            !ids.iter().any(|i| i.starts_with("pocket-tts-en")),
            "{ids:?}"
        );
        // Expressive voice on a GPU only, the classifier on the brain's llama.cpp build.
        assert!(
            ids.iter()
                .any(|i| i == "qwen3-tts-12hz-1.7b-customvoice-q8_0@gpu")
        );
        assert!(
            !ids.iter()
                .any(|i| i == "qwen3-tts-12hz-1.7b-customvoice-q8_0@cpu")
        );
        let classifier = options("en_us")
            .into_iter()
            .find(|o| o.part == Part::Classifier)
            .unwrap();
        let brain = brain_options().into_iter().next().unwrap();
        assert_eq!(classifier.runtime, brain.runtime);
    }

    #[test]
    fn rtx_4090_runs_everything_on_the_gpu() {
        let hw = pc(
            32,
            16,
            32,
            vec![gpu(
                "Vulkan0",
                "NVIDIA GeForce RTX 4090",
                24,
                23_000,
                GpuKind::Discrete,
            )],
        );
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan).0, "Best");
        assert_eq!(voice(&plan), Some(("Expressive voice", Runs::Gpu)));
        assert_eq!(
            plan.summary.split(':').next().unwrap(),
            "Best brain and expressive voice on your RTX 4090"
        );
        stays_within(&plan, &hw);
    }

    #[test]
    fn m1_with_8_gb_shares_its_memory_and_gets_light() {
        let hw = pc(
            8,
            8,
            8,
            vec![gpu("MTL0", "Apple M1", 5, 5_461, GpuKind::Integrated)],
        );
        let plan = plan(&hw, &all(), &[], None);
        let (label, device) = brain(&plan);
        assert_eq!(label, "Light");
        assert!(matches!(device, Device::Gpu { id, .. } if id == "MTL0"));
        // Unified memory: the GPU's share comes out of the same RAM.
        stays_within(&plan, &hw);
        assert!(plan.ram_mb >= plan.vram_mb);
    }

    #[test]
    fn m4_pro_with_64_gb_runs_the_best_of_everything() {
        let hw = pc(
            64,
            14,
            14,
            vec![gpu("MTL0", "Apple M4 Pro", 51, 53_083, GpuKind::Integrated)],
        );
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan).0, "Best");
        assert_eq!(voice(&plan), Some(("Expressive voice", Runs::Gpu)));
        assert!(plan.missing.is_empty());
        assert!(
            plan.summary
                .starts_with("Best brain and expressive voice on your Apple M4 Pro"),
            "{}",
            plan.summary
        );
    }

    #[test]
    fn rx_6600_over_vulkan_runs_standard() {
        let hw = pc(
            16,
            6,
            12,
            vec![gpu(
                "Vulkan0",
                "AMD Radeon RX 6600 (RADV NAVI23)",
                8,
                7_900,
                GpuKind::Discrete,
            )],
        );
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan).0, "Standard");
        assert_eq!(voice(&plan), Some(("Natural voice", Runs::Cpu)));
        assert!(
            plan.summary
                .starts_with("Standard brain on your Radeon RX 6600, natural voice"),
            "{}",
            plan.summary
        );
        stays_within(&plan, &hw);
    }

    #[test]
    fn a_6_core_pc_without_a_graphics_card_runs_standard_on_the_cpu() {
        let hw = pc(16, 6, 12, Vec::new());
        let plan = plan(&hw, &all(), &[], None);
        assert_eq!(brain(&plan).0, "Standard");
        assert!(
            plan.summary
                .starts_with("Standard brain and natural voice on the processor: "),
            "{}",
            plan.summary
        );
        let Device::Cpu { threads } = brain(&plan).1 else {
            panic!("on the CPU")
        };
        assert!(*threads >= 4, "{threads}");
        assert!(plan.pick(Part::Brain).unwrap().latency_ms <= BRAIN_FIRST_WORD_MS);
        assert_eq!(voice(&plan), Some(("Natural voice", Runs::Cpu)));
        stays_within(&plan, &hw);
    }

    #[test]
    fn a_pc_too_weak_for_any_brain_is_told_so() {
        let hw = pc(4, 2, 2, Vec::new());
        let plan = plan(&hw, &all(), &[], None);
        assert!(plan.chat_only());
        assert!(
            plan.missing
                .iter()
                .any(|m| m.part == Part::Brain && m.reason.contains("RAM"))
        );
        assert!(plan.summary.contains("simple commands"), "{}", plan.summary);
    }

    #[test]
    fn installed_parts_and_shared_runtimes_are_downloaded_once() {
        let hw = pc(16, 6, 12, Vec::new());
        let options = all();
        let fresh = plan(&hw, &options, &[], None);
        let brain_id = fresh.pick(Part::Brain).unwrap().option.id.clone();
        let runtime_key = fresh
            .pick(Part::Brain)
            .unwrap()
            .option
            .runtime
            .clone()
            .unwrap();
        let again = plan(
            &hw,
            &options,
            &[brain_id.as_str(), runtime_key.0.as_str()],
            None,
        );
        assert_eq!(
            fresh.download - again.download,
            fresh.pick(Part::Brain).unwrap().option.download + runtime_key.1
        );
        // The classifier shares the brain's llama.cpp build: it is not counted twice.
        let runtimes: u64 = runtime_downloads(&fresh.picks, &[]);
        assert!(runtimes < 2 * runtime_key.1 + (40 << 20) + 1);
    }

    #[test]
    fn a_full_disk_drops_what_doesnt_fit() {
        let hw = pc(16, 6, 12, Vec::new());
        let plan = plan(&hw, &all(), &[], Some(3 << 30));
        assert!(plan.download <= 3 << 30);
        assert_eq!(brain(&plan).0, "Standard");
        assert!(plan.missing.iter().any(|m| m.reason.contains("disk space")));
    }

    #[test]
    fn cloning_voices_are_offered_only_with_the_voice_pack() {
        let ids = |cast| -> Vec<String> {
            options_with("en_us", cast)
                .into_iter()
                .filter(|o| o.part == Part::TextToSpeech)
                .map(|o| o.id)
                .collect()
        };
        let base = |ids: &[String]| {
            ids.iter()
                .any(|id| id.starts_with("qwen3-tts-12hz-1.7b-base"))
        };
        assert!(!base(&ids(false)));
        assert!(base(&ids(true)));
        assert_eq!(
            ids(false),
            options("en_us")
                .into_iter()
                .filter(|o| o.part == Part::TextToSpeech)
                .map(|o| o.id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn without_sidecar_options_the_plan_is_the_brain_alone() {
        let hw = pc(16, 6, 12, Vec::new());
        let plan = plan(&hw, &brain_options(), &[], None);
        assert_eq!(plan.picks.len(), 1);
        assert!(plan.missing.is_empty());
        assert!(!plan.summary.contains("voice"), "{}", plan.summary);
    }
}
