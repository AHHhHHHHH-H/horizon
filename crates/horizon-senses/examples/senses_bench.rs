//! Measures the senses on real models (dev only; nothing here ships):
//!
//! ```text
//! # The classifier: tune on the bank (leave-one-out), then score the held-out set.
//! cargo run --release -p horizon-senses --example senses_bench -- classify \
//!     --llama-server <llama.cpp>/llama-server --gguf <model.gguf> --id multilingual-e5-small-q8_0 \
//!     [--threads 1] [--work <dir>]
//! # Voice: speak a line per language, save WAVs, then transcribe them back.
//! cargo run --release -p horizon-senses --example senses_bench -- voice \
//!     --runtime <audio.cpp dir> --backend cpu --threads 2 --out <dir> \
//!     --model pocket-tts-en-q8_0=<dir> --model parakeet-tdt-0.6b-v3-q8_0=<dir> [--tones]
//!     [--voices <voice pack dir> --cast veteran]
//! ```
//! With `--voices`, cloning models speak as the `--cast` character, from the clip of each tone.
use horizon_companion::{
    catalogue::Backend,
    senses::{self as catalogue, SenseKind},
};
use horizon_senses::{
    classifier::{self, Classifier, EmbedServer, Example, Intent, Tuning},
    dsp,
    voice::{Installed, Speak, SpeechStats, Tone, VoiceServer, VoiceSetup},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let flags = |name: &str| -> Vec<String> {
        args.windows(2)
            .filter(|w| w[0] == name)
            .map(|w| w[1].clone())
            .collect()
    };
    match args.first().map(String::as_str) {
        Some("classify") => classify(
            flag("--llama-server").ok_or("--llama-server")?.into(),
            flag("--gguf").ok_or("--gguf")?.into(),
            &flag("--id").ok_or("--id")?,
            flag("--threads").map_or(Ok(1), |t| t.parse())?,
            flag("--work").map_or_else(std::env::temp_dir, PathBuf::from),
        ),
        Some("voice") => voice(
            flag("--runtime").ok_or("--runtime")?.into(),
            match flag("--backend").as_deref() {
                Some("metal") => Backend::Metal,
                Some("vulkan") => Backend::Vulkan,
                Some("cuda") => Backend::Cuda,
                _ => Backend::Cpu,
            },
            flag("--threads").map_or(Ok(2), |t| t.parse())?,
            flag("--out").ok_or("--out")?.into(),
            &flags("--model"),
            args.iter().any(|a| a == "--tones"),
            flag("--voices").map(PathBuf::from),
            flag("--cast"),
        ),
        _ => Err("usage: senses_bench classify|voice …".into()),
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// What the game does with a line: run a command, ask the brain, or ignore it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Command(Intent),
    Brain,
    Ignore,
}

fn truth(intent: Intent) -> Route {
    if intent.needs_brain() {
        Route::Brain
    } else if intent == Intent::NotAddressed {
        Route::Ignore
    } else {
        Route::Command(intent)
    }
}

fn route(v: &classifier::Verdict) -> Route {
    match (v.command(), v.ignore()) {
        (Some(i), _) => Route::Command(i),
        (None, true) => Route::Ignore,
        _ => Route::Brain,
    }
}

/// Score a routing: right minus the costly mistakes (a wrong command runs, or the player is
/// ignored); a command sent to the brain only costs time.
fn cost(want: Route, got: Route) -> f64 {
    match (want, got) {
        (a, b) if a == b => 1.0,
        (_, Route::Command(_)) => -3.0,
        (Route::Command(_) | Route::Brain, Route::Ignore) => -2.0,
        _ => 0.0,
    }
}

fn classify(llama: PathBuf, gguf: PathBuf, id: &str, threads: usize, work: PathBuf) -> Result<()> {
    let bank = classifier::parse_examples(classifier::BANK)?;
    let test = classifier::parse_examples(include_str!("../data/intents-test.tsv"))?;
    let started = Instant::now();
    let server = EmbedServer::start(&llama, &gguf, id, threads, &work)?;
    println!("server up in {:.2} s", started.elapsed().as_secs_f64());
    let embedder = server.client();
    let started = Instant::now();
    let mut classifier = Classifier::build(embedder, &bank, None)?;
    println!(
        "bank of {} embedded in {:.2} s",
        classifier.len(),
        started.elapsed().as_secs_f64()
    );
    // Embed every line once; the vote is then cheap to repeat for each tuning.
    let prefix = classifier::prefix_for(id);
    let embed = |examples: &[Example]| -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::new();
        for batch in examples.chunks(64) {
            let texts: Vec<String> = batch
                .iter()
                .map(|e| format!("{prefix}{}", e.text))
                .collect();
            let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
            out.extend(
                horizon_senses::classifier::Embed::embed(embedder, &refs)?
                    .into_iter()
                    .map(|v| {
                        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                        v.into_iter().map(|x| x / n).collect::<Vec<f32>>()
                    }),
            );
        }
        Ok(out)
    };
    let bank_vectors = embed(&bank)?;
    // Every bank line's similarity to every other.
    let similarities: Vec<Vec<f32>> = bank_vectors
        .iter()
        .map(|a| {
            bank_vectors
                .iter()
                .map(|b| a.iter().zip(b).map(|(x, y)| x * y).sum())
                .collect()
        })
        .collect();
    // Leave-one-out tuning on the bank.
    let mut best = (f64::MIN, classifier.tuning);
    for k in [3, 5, 7, 9] {
        for temperature in [0.01, 0.02, 0.03, 0.05, 0.08] {
            for min_similarity in [0.0, 0.5, 0.6, 0.7, 0.8, 0.84, 0.86, 0.88, 0.9, 0.92] {
                for min_share in [0.4, 0.5, 0.6, 0.7, 0.8] {
                    classifier.tuning = Tuning {
                        k,
                        temperature,
                        min_similarity,
                        min_share,
                    };
                    let score: f64 = bank
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            let verdict = classifier.decide(&similarities[i], &e.text, Some(i));
                            cost(truth(e.intent), route(&verdict))
                        })
                        .sum();
                    if score > best.0 {
                        best = (score, classifier.tuning);
                    }
                }
            }
        }
    }
    classifier.tuning = best.1;
    println!(
        "tuning (leave-one-out on the bank): {:?}, score {:.0}/{}",
        best.1,
        best.0,
        bank.len()
    );
    // The held-out set, one line at a time as the game sends them.
    let mut latencies = Vec::new();
    let (mut strict, mut routed, mut wrong_command) = (0, 0, 0);
    let (mut tp, mut fp, mut fn_) = (0, 0, 0);
    let mut per_language: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut confusions: BTreeMap<(Intent, Intent), usize> = BTreeMap::new();
    for e in &test {
        let started = Instant::now();
        let v = classifier.classify(embedder, &e.text)?;
        latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        let (want, got) = (truth(e.intent), route(&v));
        strict += usize::from(v.intent() == e.intent);
        routed += usize::from(want == got);
        if want != got && matches!(got, Route::Command(_)) {
            wrong_command += 1;
            println!(
                "  wrong command: {:?} for {} ({})",
                got,
                e.text,
                e.intent.name()
            );
        }
        let lang = per_language.entry(e.language.clone()).or_default();
        lang.0 += usize::from(want == got);
        lang.1 += 1;
        match (e.intent.needs_brain(), v.needs_brain) {
            (true, true) => tp += 1,
            (false, true) => fp += 1,
            (true, false) => fn_ += 1,
            _ => {}
        }
        if v.intent() != e.intent {
            *confusions.entry((e.intent, v.intent())).or_default() += 1;
        }
    }
    latencies.sort_by(f64::total_cmp);
    let n = test.len() as f64;
    println!(
        "\n{id} on {} held-out lines ({threads} thread):",
        test.len()
    );
    println!(
        "  intent accuracy (23-way)    {:.1} %",
        100.0 * strict as f64 / n
    );
    println!(
        "  routing accuracy            {:.1} %",
        100.0 * routed as f64 / n
    );
    println!(
        "  wrong commands run          {wrong_command} ({:.1} %)",
        100.0 * wrong_command as f64 / n
    );
    println!(
        "  needs_brain precision       {:.1} %",
        100.0 * tp as f64 / (tp + fp).max(1) as f64
    );
    println!(
        "  needs_brain recall          {:.1} %",
        100.0 * tp as f64 / (tp + fn_).max(1) as f64
    );
    println!(
        "  latency p50 / p95 / max     {:.1} / {:.1} / {:.1} ms",
        percentile(&latencies, 0.5),
        percentile(&latencies, 0.95),
        latencies[latencies.len() - 1]
    );
    for (lang, (ok, all)) in &per_language {
        println!(
            "  routing {lang}: {:.1} %",
            100.0 * *ok as f64 / *all as f64
        );
    }
    let mut top: Vec<_> = confusions.into_iter().collect();
    top.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for ((want, got), n) in top.iter().take(8) {
        println!("  confused {} → {}: {n}", want.name(), got.name());
    }
    Ok(())
}

/// One line per language, about three seconds, and how a player might say it.
const LINES: &[(&str, &str)] = &[
    (
        "en",
        "Creeper behind you! Run to the house, I will cover you.",
    ),
    (
        "fr",
        "Attention, un creeper derrière toi ! Cours vers la maison, je te couvre.",
    ),
    (
        "es",
        "¡Cuidado, un creeper detrás de ti! Corre a la casa, yo te cubro.",
    ),
    (
        "de",
        "Achtung, ein Creeper hinter dir! Lauf zum Haus, ich gebe dir Deckung.",
    ),
    (
        "pt",
        "Cuidado, tem um creeper atrás de você! Corre pra casa, eu te cubro.",
    ),
];

/// Peak resident memory of a process and its children, sampled every 100 ms.
struct RssWatch {
    stop: Arc<AtomicBool>,
    peak_kb: Arc<AtomicU64>,
}

impl RssWatch {
    fn start(root: u32) -> RssWatch {
        let stop = Arc::new(AtomicBool::new(false));
        let peak_kb = Arc::new(AtomicU64::new(0));
        let (s, p) = (stop.clone(), peak_kb.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                if let Ok(out) = Command::new("ps")
                    .args(["-A", "-o", "pid=,ppid=,rss="])
                    .output()
                {
                    let rows: Vec<(u32, u32, u64)> = String::from_utf8_lossy(&out.stdout)
                        .lines()
                        .filter_map(|l| {
                            let mut c = l.split_whitespace().map(|x| x.parse::<u64>().ok());
                            Some((c.next()?? as u32, c.next()?? as u32, c.next()??))
                        })
                        .collect();
                    let mut family = vec![root];
                    let mut i = 0;
                    while i < family.len() {
                        let parent = family[i];
                        family.extend(rows.iter().filter(|r| r.1 == parent).map(|r| r.0));
                        i += 1;
                    }
                    let total: u64 = rows
                        .iter()
                        .filter(|r| family.contains(&r.0))
                        .map(|r| r.2)
                        .sum();
                    p.fetch_max(total, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        RssWatch { stop, peak_kb }
    }

    fn peak_mb(&self) -> f64 {
        self.peak_kb.load(Ordering::Relaxed) as f64 / 1024.0
    }
}

impl Drop for RssWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Word error rate on folded words.
fn wer(reference: &str, heard: &str) -> f64 {
    let r: Vec<String> = classifier::fold(reference)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let h: Vec<String> = classifier::fold(heard)
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let mut row: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(rw != hw)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[h.len()] as f64 / r.len().max(1) as f64
}

fn voice(
    runtime: PathBuf,
    backend: Backend,
    threads: usize,
    out: PathBuf,
    models: &[String],
    tones: bool,
    voices: Option<PathBuf>,
    cast: Option<String>,
) -> Result<()> {
    std::fs::create_dir_all(&out)?;
    let installed: Vec<Installed> = models
        .iter()
        .map(|m| {
            let (id, dir) = m.split_once('=').ok_or("--model id=dir")?;
            let model = catalogue::model(id).ok_or_else(|| format!("unknown model {id}"))?;
            Ok(Installed {
                model,
                dir: dir.into(),
            })
        })
        .collect::<Result<_>>()?;
    let setup = VoiceSetup {
        runtime,
        exe: "audiocpp_server",
        backend,
        threads,
        models: installed.clone(),
        work: out.clone(),
        voices,
    };
    let started = Instant::now();
    let mut server = VoiceServer::start(&setup)?;
    println!(
        "audio.cpp up in {:.2} s ({} backend, {threads} threads)",
        started.elapsed().as_secs_f64(),
        backend.label()
    );
    let watch = RssWatch::start(server.sidecar().pid());
    let started = Instant::now();
    server.warm_up()?;
    println!("models loaded in {:.2} s", started.elapsed().as_secs_f64());
    let mut spoken: Vec<(String, &str, PathBuf)> = Vec::new();
    if let Some(speech) = server.speech() {
        println!("\n| model | lang | first audio | audio | took | RTF |");
        println!("|---|---|---|---|---|---|");
        let say = |text: &str,
                   language: &str,
                   tone: Tone,
                   name: &str|
         -> Result<(SpeechStats, Vec<f32>)> {
            let mut pcm = Vec::new();
            let stats = speech.speak_blocking(
                &Speak {
                    text: text.into(),
                    voice: cast.clone(),
                    tone,
                    language: language.into(),
                },
                |chunk| pcm.extend_from_slice(chunk),
            )?;
            std::fs::write(out.join(name), dsp::wav_pcm16(&pcm, stats.sample_rate))?;
            Ok((stats, pcm))
        };
        for model in installed
            .iter()
            .filter(|i| i.model.kind == SenseKind::TextToSpeech)
        {
            for (language, text) in LINES.iter().filter(|(l, _)| model.model.speaks(l)) {
                let name = format!("{}-{language}.wav", model.model.id);
                let (stats, _) = say(text, language, Tone::Normal, &name)?;
                println!(
                    "| {} | {language} | {:.0} ms | {:.2} s | {:.2} s | {:.2} |",
                    model.model.id,
                    stats.first_audio.unwrap_or_default().as_secs_f64() * 1000.0,
                    stats.audio_seconds(),
                    stats.total.as_secs_f64(),
                    stats.rtf()
                );
                spoken.push((text.to_string(), language, out.join(name)));
            }
            if tones && (model.model.instruct || model.model.clone) {
                println!("\n| tone | first audio | audio | RTF | peak | RMS (dBFS) |");
                println!("|---|---|---|---|---|---|");
                let text = "Get away from me, there is a creeper right behind you.";
                for (tone, label) in [
                    (Tone::Normal, "normal"),
                    (Tone::Shout, "shout"),
                    (Tone::Whisper, "whisper"),
                    (Tone::Laugh, "laugh"),
                    (Tone::Sarcastic, "sarcastic"),
                ] {
                    let name = format!("{}-tone-{label}.wav", model.model.id);
                    let (stats, pcm) = say(text, "en", tone, &name)?;
                    let (peak, rms) = dsp::peak_rms(&pcm);
                    println!(
                        "| {label} | {:.0} ms | {:.2} s | {:.2} | {peak:.3} | {:.1} |",
                        stats.first_audio.unwrap_or_default().as_secs_f64() * 1000.0,
                        stats.audio_seconds(),
                        stats.rtf(),
                        20.0 * rms.max(1e-9).log10()
                    );
                }
            }
        }
    }
    if let Some(recognition) = server.recognition() {
        println!("\n| clip | audio | took | language | WER | heard |");
        println!("|---|---|---|---|---|---|");
        for (text, language, path) in &spoken {
            let (pcm, rate) = dsp::read_wav_pcm16(&std::fs::read(path)?).ok_or("wav")?;
            let mut clip = Vec::new();
            dsp::To16k::new(rate, 1).push(&pcm, &mut clip);
            let t = recognition.transcribe(&clip, Some(language))?;
            println!(
                "| {} | {:.2} s | {:.0} ms | {} (want {language}) | {:.0} % | {} |",
                path.file_name().unwrap_or_default().to_string_lossy(),
                clip.len() as f64 / 16000.0,
                t.took.as_secs_f64() * 1000.0,
                t.language.as_deref().unwrap_or("?"),
                100.0 * wer(text, &t.text),
                t.text
            );
        }
    }
    println!(
        "\npeak resident memory of the server: {:.0} MB",
        watch.peak_mb()
    );
    Ok(())
}
