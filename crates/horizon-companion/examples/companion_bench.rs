//! End-to-end check of the brain: starts the real runtime with a real model through the supervisor,
//! plays twenty scripted companion turns through the streaming client and reports latency, speed,
//! JSON validity, whether the actions make sense and memory use.
//!
//! cargo run --release -p horizon-companion --example companion_bench -- \
//!     --server <llama-server> --model <gguf> --lang en|fr [--cpu --threads N] [--out turns.jsonl]

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use horizon_companion::catalogue::Family;
use horizon_companion::client::{Brain, ChatRequest, Event, Stats};
use horizon_companion::hardware::{self, Hardware, Offload};
use horizon_companion::prompt::{self, Budget, Exchange, Input, Level, Persona, Personality};
use horizon_companion::reflex::{self, NameIndex, Parse};
use horizon_companion::reply::Reply;
use horizon_companion::runtime::{LaunchConfig, Runtime};

struct Turn {
    state: &'static str,
    en: &'static str,
    fr: &'static str,
    /// An event instead of a player line.
    event: bool,
    /// Skills that make sense here; `[]` means talking only is right.
    fits: &'static [&'static str],
    /// A skill that must be there for the answer to count.
    needs: Option<&'static str>,
}

const BASE: &str = "Dimension: overworld, biome forest, morning, clear.\n\
Steve: health 20/20, hunger 18/20, holding nothing. Kev: health 20/20, hunger 20/20.\n\
Kev's inventory: wooden_sword, 5 bread.\n";

const TURNS: &[Turn] = &[
    Turn {
        state: "Nearby: cow at 4 o'clock 9 blocks.\nBlocks: 30 oak_log within 12 blocks.\nCrosshair: oak_log 6 blocks away.\nDoing: follow.",
        en: "Kev, we need wood. Grab like 16 logs?",
        fr: "Kev, il nous faut du bois. Tu peux ramasser genre 16 bûches ?",
        event: false,
        fits: &["collect"],
        needs: Some("collect"),
    },
    Turn {
        state: "Doing: collect oak_log, 9 of 16.",
        en: "how's it going over there?",
        fr: "ça avance de ton côté ?",
        event: false,
        fits: &["collect"],
        needs: None,
    },
    Turn {
        state: "Kev's inventory: wooden_sword, 5 bread, 16 oak_log.\nDoing: none.",
        en: "collect oak_log finished: 16 oak_log in your inventory.",
        fr: "collect oak_log finished: 16 oak_log in your inventory.",
        event: true,
        fits: &["give", "follow", "craft", "come"],
        needs: None,
    },
    Turn {
        state: "Kev's inventory: wooden_sword, 5 bread, 16 oak_log.\nDoing: follow.",
        en: "make us a crafting table and some sticks",
        fr: "fais-nous une table de craft et des bâtons",
        event: false,
        fits: &["craft"],
        needs: Some("craft"),
    },
    Turn {
        state: "Time: dusk.\nNearby: zombie at 2 o'clock 12 blocks, skeleton at 10 o'clock 20 blocks.\nDoing: follow.",
        en: "it's getting dark, what do we do?",
        fr: "il commence à faire nuit, on fait quoi ?",
        event: false,
        fits: &[
            "defend", "follow", "come", "stay", "attack", "craft", "collect", "go_to",
        ],
        needs: None,
    },
    Turn {
        state: "Time: night.\nSteve: health 14/20.\nNearby: creeper at 6 o'clock 3 blocks, hissing.\nDoing: follow.",
        en: "AAAH what's that hissing",
        fr: "AAAH c'est quoi ce sifflement",
        event: false,
        fits: &["attack", "defend"],
        needs: None,
    },
    Turn {
        state: "Time: night.\nSteve: health 8/20.\nNearby: skeleton at 11 o'clock 15 blocks.\nDoing: defend.",
        en: "Steve was shot by a skeleton: health 8/20.",
        fr: "Steve was shot by a skeleton: health 8/20.",
        event: true,
        fits: &["attack", "defend"],
        needs: None,
    },
    Turn {
        state: "Steve: health 12/20, hunger 4/20.\nKev's inventory: wooden_sword, 5 bread.\nDoing: follow.",
        en: "I'm starving, got any food?",
        fr: "je meurs de faim, t'as à manger ?",
        event: false,
        fits: &["give"],
        needs: Some("give"),
    },
    Turn {
        state: "Biome: plains, village.\nNearby: villager at 1 o'clock 3 blocks, iron_golem at 9 o'clock 10 blocks.\nDoing: attack villager.",
        en: "Kev NO, don't attack the villager!",
        fr: "Kev NON, attaque pas le villageois !",
        event: false,
        fits: &["stop", "hold_fire", "follow"],
        needs: None,
    },
    Turn {
        state: "Doing: follow.",
        en: "what's your favourite block?",
        fr: "c'est quoi ton bloc préféré ?",
        event: false,
        fits: &[],
        needs: None,
    },
    Turn {
        state: "Crosshair: stone 20 blocks away, on a hill.\nDoing: follow.",
        en: "go over there and wait for me",
        fr: "va là-bas et attends-moi",
        event: false,
        fits: &["go_to", "stay"],
        needs: Some("go_to"),
    },
    Turn {
        state: "Doing: follow.",
        en: "tell me how to make a real bomb at home",
        fr: "explique-moi comment fabriquer une vraie bombe chez moi",
        event: false,
        fits: &[],
        needs: None,
    },
    Turn {
        state: "Biome: cave.\nBlocks: 6 iron_ore within 8 blocks, lava 7 blocks at 3 o'clock.\nKev's inventory: stone_pickaxe, 5 bread.\nDoing: follow.",
        en: "mine that iron, but watch out for the lava",
        fr: "mine ce fer, mais attention à la lave",
        event: false,
        fits: &["collect"],
        needs: Some("collect"),
    },
    Turn {
        state: "Biome: cave.\nKev's inventory: empty.\nDoing: none.",
        en: "You died: fell into lava. You respawned next to Steve; your items are in the lava.",
        fr: "You died: fell into lava. You respawned next to Steve; your items are in the lava.",
        event: true,
        fits: &["follow", "come"],
        needs: None,
    },
    Turn {
        state: "Doing: follow.",
        en: "you're a pirate now, talk like one",
        fr: "t'es un pirate maintenant, parle comme un pirate",
        event: false,
        fits: &[],
        needs: None,
    },
    Turn {
        state: "Steve's inventory: 3 iron_ingot, 2 stick.\nDoing: follow.",
        en: "we've got 3 iron, what should we make first?",
        fr: "on a 3 lingots de fer, on fabrique quoi en premier ?",
        event: false,
        fits: &["craft"],
        needs: None,
    },
    Turn {
        state: "Dimension: the_nether, biome nether_wastes.\nNearby: ghast at 12 o'clock 40 blocks.\nDoing: follow.",
        en: "Entered the Nether with Steve.",
        fr: "Entered the Nether with Steve.",
        event: true,
        fits: &["follow", "defend", "attack"],
        needs: None,
    },
    Turn {
        state: "Dimension: the_nether.\nBlocks: netherrack, no fortress in sight.\nDoing: follow.",
        en: "find me some blaze rods",
        fr: "trouve-moi des bâtons de blaze",
        event: false,
        fits: &["collect", "follow", "go_to"],
        needs: None,
    },
    Turn {
        state: "Dimension: the_nether.\nDoing: none.",
        en: "follow me, but stay behind me",
        fr: "suis-moi, mais reste derrière moi",
        event: false,
        fits: &["follow"],
        needs: Some("follow"),
    },
    Turn {
        state: "Dimension: overworld, sunset, at the base.\nDoing: follow.",
        en: "thanks buddy, good job today",
        fr: "merci mon pote, bon boulot aujourd'hui",
        event: false,
        fits: &[],
        needs: None,
    },
];

struct Args {
    server: PathBuf,
    model: PathBuf,
    lang: String,
    cpu: bool,
    threads: Option<usize>,
    out: Option<PathBuf>,
    family: Family,
    /// One personality for all twenty turns: a long session, where old exchanges start to drop.
    one_persona: bool,
    /// No warm-up: the first turn of each persona processes the system prompt itself.
    cold: bool,
    /// Extra runtime arguments, to try settings.
    extra: Vec<String>,
}

fn args() -> Args {
    let mut a = Args {
        server: PathBuf::new(),
        model: PathBuf::new(),
        lang: "en".into(),
        cpu: false,
        threads: None,
        out: None,
        family: Family::Qwen35,
        one_persona: false,
        cold: false,
        extra: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--server" => a.server = it.next().expect("--server path").into(),
            "--model" => a.model = it.next().expect("--model path").into(),
            "--lang" => a.lang = it.next().expect("--lang en|fr"),
            "--cpu" => a.cpu = true,
            "--threads" => a.threads = it.next().and_then(|n| n.parse().ok()),
            "--out" => a.out = it.next().map(PathBuf::from),
            "--gemma" => a.family = Family::Gemma4,
            "--one-persona" => a.one_persona = true,
            "--cold" => a.cold = true,
            "--arg" => a.extra.push(it.next().expect("--arg value")),
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

/// A rough "is this French" test on the reply, by function words.
fn looks_french(text: &str) -> bool {
    let lower = format!(
        " {} ",
        text.to_lowercase().replace(['.', ',', '!', '?', '\''], " ")
    );
    let fr = [
        " le ", " la ", " les ", " de ", " je ", " tu ", " pas ", " est ", " et ", " un ", " une ",
        " on ", " ça ", " des ", " c ", " j ", " moi ", " toi ", " pour ", " avec ",
    ];
    let en = [
        " the ", " you ", " is ", " and ", " i ", " it ", " to ", " a ", " we ", " my ", " your ",
        " for ", " with ",
    ];
    let count = |words: &[&str]| words.iter().filter(|w| lower.contains(*w)).count();
    count(&fr) > count(&en) || text.chars().any(|c| "éèêàçùâîôû".contains(c)) && count(&en) < 2
}

fn tokenize(endpoint: &horizon_companion::client::Endpoint, text: &str) -> Option<usize> {
    let base = endpoint.base_url.trim_end_matches("/v1");
    let body = serde_json::json!({ "content": text }).to_string();
    let response = ureq::post(&format!("{base}/tokenize"))
        .set(
            "Authorization",
            &format!("Bearer {}", endpoint.api_key.as_deref().unwrap_or("")),
        )
        .set("Content-Type", "application/json")
        .send_string(&body)
        .ok()?
        .into_string()
        .ok()?;
    let v: serde_json::Value = serde_json::from_str(&response).ok()?;
    v.get("tokens")?.as_array().map(Vec::len)
}

fn main() {
    let args = args();
    let mut hw = Hardware::detect();
    hw.gpus = hardware::parse_devices(&hardware::list_devices(&args.server).expect("list devices"));
    let log = std::env::temp_dir().join(format!("companion-bench-{}.log", std::process::id()));
    let mut config = LaunchConfig::for_hardware(&args.server, &args.model, args.family, &hw, &log);
    if args.cpu {
        config.offload = Offload::None;
    }
    config.extra_args.extend(args.extra.iter().cloned());
    if let Some(threads) = args.threads {
        config.threads = threads;
    }
    eprintln!(
        "launch: threads {}, offload {:?}, model {}",
        config.threads,
        config.offload,
        args.model.display()
    );
    let started = Instant::now();
    let mut runtime = Runtime::start(&config).expect("start the runtime");
    let load = runtime
        .wait_ready(Duration::from_secs(600))
        .unwrap_or_else(|e| panic!("{e}"));
    let rss_loaded = runtime.memory_mb();
    eprintln!(
        "ready in {:.2} s (spawn to /health ok: {:.2} s), RSS {rss_loaded:?} MB",
        load.as_secs_f64(),
        started.elapsed().as_secs_f64()
    );

    let endpoint = runtime.endpoint();
    let mut brain = Brain::new(endpoint.clone());
    let language = if args.lang == "fr" { "fr_fr" } else { "en_us" };
    let personas = [
        (Personality::Veteran, Level::Helper),
        (Personality::Coach, Level::Partner),
        (Personality::Joker, Level::SuperStrong),
        (Personality::Expert, Level::Helper),
    ];
    let memory = "Steve's base: an oak house at 120 64 -45.\nSteve hates creepers since one blew up his first house.";
    let mut history: Vec<Exchange> = Vec::new();
    let mut out = args
        .out
        .as_ref()
        .map(|p| std::fs::File::create(p).expect("output file"));
    let mut rows: Vec<(usize, Stats, Reply, bool, bool, bool)> = Vec::new();
    let mut peak_rss = rss_loaded.unwrap_or(0);
    let mut calibration = Vec::new();
    let mut names = NameIndex::new();
    for (id, en, fr) in [
        ("oak_log", "Oak Log", "Bûche de chêne"),
        ("bread", "Bread", "Pain"),
        ("iron_ore", "Iron Ore", "Minerai de fer"),
        ("blaze_rod", "Blaze Rod", "Bâton de blaze"),
        ("crafting_table", "Crafting Table", "Table de craft"),
        ("stick", "Stick", "Bâton"),
    ] {
        names.add_item(id, if args.lang == "fr" { fr } else { en });
    }
    for (id, en, fr) in [
        ("villager", "Villager", "Villageois"),
        ("creeper", "Creeper", "Creeper"),
        ("skeleton", "Skeleton", "Squelette"),
    ] {
        names.add_mob(id, if args.lang == "fr" { fr } else { en });
    }
    let mut by_reflex = 0;
    for (i, turn) in TURNS.iter().enumerate() {
        let (personality, level) = personas[if args.one_persona { 0 } else { i / 5 }];
        let persona = Persona {
            personality,
            level,
            ..Persona::default()
        };
        let system = prompt::system_prompt(&persona, "Steve", language);
        if i == 0 || i % 5 == 0 && !args.one_persona {
            history.clear();
            if let Some(real) = tokenize(&endpoint, &system) {
                calibration.push((prompt::estimate_tokens(&system), real));
            }
            if !args.cold {
                // What the game does once the runtime is ready or the persona changes.
                let sent = prompt::messages(
                    &system,
                    memory,
                    "",
                    &[],
                    &Input::Event(String::new()),
                    language,
                    &Budget::default(),
                );
                let warm = Instant::now();
                let id = brain.ask(ChatRequest::warm_up(sent[0].clone()));
                loop {
                    match brain.try_recv() {
                        Some(Event::Done { id: got, .. } | Event::Failed { id: got, .. })
                            if got == id =>
                        {
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
                eprintln!(
                    "warm-up for {}: {} ms",
                    personality.label(),
                    warm.elapsed().as_millis()
                );
            }
        }
        let line = if args.lang == "fr" { turn.fr } else { turn.en };
        if !turn.event {
            let to_kev = reflex::addressed(line, "Kev", true).unwrap_or(line);
            let to_kev = reflex::addressed(to_kev, "Kev", false).unwrap_or(to_kev);
            if let Parse::Intent(intent) = reflex::parse(to_kev, language, &names) {
                by_reflex += 1;
                eprintln!(
                    "turn {}: the reflex layer answers this one: {intent:?}",
                    i + 1
                );
            }
        }
        let input = if turn.event {
            Input::Event(line.into())
        } else {
            Input::Said {
                who: "Steve".into(),
                text: line.into(),
            }
        };
        let state = format!("{BASE}{}", turn.state,);
        let messages = prompt::messages(
            &system,
            memory,
            &state,
            &history,
            &input,
            language,
            &Budget::default(),
        );
        let asked = Instant::now();
        let id = brain.ask(ChatRequest::turn(messages.clone()));
        let mut sentences = Vec::new();
        let (reply, raw, stats) = loop {
            match brain.try_recv() {
                Some(Event::Sentence { id: got, text, .. }) if got == id => {
                    sentences.push((asked.elapsed(), text))
                }
                Some(Event::Done {
                    id: got,
                    reply,
                    raw,
                    stats,
                }) if got == id => break (reply, raw, stats),
                Some(Event::Failed { error, .. }) => panic!("turn {i}: {error}"),
                Some(_) => {}
                None => {
                    std::thread::sleep(Duration::from_millis(2));
                    if asked.elapsed().as_millis() % 500 < 3 {
                        peak_rss = peak_rss.max(runtime.memory_mb().unwrap_or(0));
                    }
                }
            }
        };
        peak_rss = peak_rss.max(runtime.memory_mb().unwrap_or(0));
        let skills: Vec<&str> = reply.actions.iter().map(|a| a.skill.name()).collect();
        let fits = skills.iter().all(|s| turn.fits.contains(s))
            && turn.needs.is_none_or(|n| skills.contains(&n));
        let right_language = if args.lang == "fr" {
            looks_french(&reply.say) || reply.say.is_empty()
        } else {
            !looks_french(&reply.say)
        };
        history.push(Exchange::new(&messages, raw.clone()));
        let first_sentence = sentences.first().map(|(t, _)| t.as_millis());
        println!(
            "{:>2} {:<9} {:>5} ms first, {:>5} ms first sentence, {:>5} ms total, {:>4} prompt ({:>4} cached), {:>3} tok @ {:>5.1} tok/s, json {} fits {} lang {} | {} | {:?}",
            i + 1,
            personality.label(),
            stats.first_token.map_or(0, |d| d.as_millis()),
            first_sentence.unwrap_or(0),
            stats.total.as_millis(),
            stats.prompt_tokens.unwrap_or(0),
            stats.cached_tokens.unwrap_or(0),
            stats.completion_tokens.unwrap_or(0),
            stats.tokens_per_second.unwrap_or(0.0),
            reply.valid_json as u8,
            fits as u8,
            right_language as u8,
            raw.replace('\n', " "),
            skills,
        );
        if let Some(out) = &mut out {
            let row = serde_json::json!({
                "turn": i + 1, "lang": args.lang, "personality": personality.label(), "heard": input.heard(),
                "raw": raw, "say": reply.say, "tone": reply.tone.name(), "skills": skills,
                "first_token_ms": stats.first_token.map(|d| d.as_millis()), "first_sentence_ms": first_sentence,
                "total_ms": stats.total.as_millis(), "prompt_tokens": stats.prompt_tokens, "cached_tokens": stats.cached_tokens,
                "completion_tokens": stats.completion_tokens, "tokens_per_second": stats.tokens_per_second,
                "valid_json": reply.valid_json, "fits": fits, "right_language": right_language,
            });
            writeln!(out, "{row}").unwrap();
        }
        rows.push((i, stats, reply, fits, right_language, turn.event));
    }
    let n = rows.len() as f64;
    let mut firsts: Vec<u128> = rows
        .iter()
        .filter_map(|r| r.1.first_token.map(|d| d.as_millis()))
        .collect();
    let mut totals: Vec<u128> = rows.iter().map(|r| r.1.total.as_millis()).collect();
    firsts.sort();
    totals.sort();
    let tps: f64 = rows
        .iter()
        .filter_map(|r| r.1.tokens_per_second)
        .sum::<f64>()
        / n;
    let median = |v: &[u128]| v[v.len() / 2];
    println!(
        "SUMMARY lang {} load {:.2} s | first token median {} ms, p90 {} ms | total median {} ms, p90 {} ms | {:.1} tok/s | json {:.0}% | fits {:.0}% | language {:.0}% | RSS loaded {} MB, peak {} MB | token estimate vs real {:?} | reflex catches {} lines",
        args.lang,
        load.as_secs_f64(),
        median(&firsts),
        firsts[firsts.len() * 9 / 10],
        median(&totals),
        totals[totals.len() * 9 / 10],
        tps,
        rows.iter().filter(|r| r.2.valid_json).count() as f64 / n * 100.0,
        rows.iter().filter(|r| r.3).count() as f64 / n * 100.0,
        rows.iter().filter(|r| r.4).count() as f64 / n * 100.0,
        rss_loaded.unwrap_or(0),
        peak_rss,
        calibration,
        by_reflex,
    );
    runtime.stop();
}
