//! "Set up companion" from the command line: the same `Setup::set_up` the launcher and the game's
//! Companion section run, over a game home. Prints each step, then the parts the game would start.
//!
//! cargo run -p horizon-companion --example companion_setup -- <game home> [language] [--voice]
//!   [--brain <option id>] [--stt <option id>] [--tts <option id>] [--classifier <option id>]
//!   [--salty] [--plan-only]

use std::path::PathBuf;

use horizon_companion::settings::{BrainChoice, CompanionSettings, PartChoice};
use horizon_companion::setup::{Event, Machine, Setup};

fn main() {
    let mut args = std::env::args().skip(1);
    let home = PathBuf::from(args.next().expect("game home"));
    let mut language = "en_us".to_owned();
    let mut settings = CompanionSettings::load(&home);
    let mut plan_only = false;
    while let Some(arg) = args.next() {
        let mut value = || args.next().expect("a value");
        match arg.as_str() {
            "--voice" => settings.voice = true,
            // Salty mode (confirmed 18+): Set up also fetches the brain's uncensored variant.
            "--salty" => {
                settings.persona.salty = true;
                settings.persona.adult_confirmed = true;
                settings.persona.kid_friendly = false;
            }
            "--brain" => settings.brain = BrainChoice::Option { id: value() },
            "--stt" => settings.speech_to_text = PartChoice::Option { id: value() },
            "--tts" => settings.text_to_speech = PartChoice::Option { id: value() },
            "--classifier" => settings.classifier = PartChoice::Option { id: value() },
            "--plan-only" => plan_only = true,
            other => language = other.to_owned(),
        }
    }
    let setup = Setup::new(&home, &language);
    let machine = Machine::detect(&home);
    println!("machine: {}", machine.summary());
    let plan = setup.plan(&machine);
    for pick in &plan.picks {
        println!(
            "plan: {:?} {} ({}) on {:?}, first word ~{} ms",
            pick.option.part, pick.option.label, pick.option.id, pick.device, pick.latency_ms
        );
    }
    for missing in &plan.missing {
        println!("missing: {:?}: {}", missing.part, missing.reason);
    }
    println!("still to download: {} MB", plan.download >> 20);
    if plan_only {
        return;
    }
    let job = setup.set_up(settings.clone());
    let mut last = String::new();
    for event in job.rx.iter() {
        match event {
            Event::Step(step, progress) => {
                let line = match progress {
                    Some(f) => format!("{step} {:.0}%", (f * 10.0).floor() * 10.0),
                    None => step,
                };
                if line != last {
                    println!("{line}");
                    last = line;
                }
            }
            Event::Planned(machine, plan) => {
                println!(
                    "planned on {}: {} parts",
                    machine.summary(),
                    plan.picks.len()
                );
            }
            Event::Done(what) => {
                println!("done: {what}");
                break;
            }
            Event::Failed(e) => {
                println!("failed: {e}");
                std::process::exit(1);
            }
            Event::Cancelled => break,
            Event::Tested(_) => {}
        }
    }
    let settings = CompanionSettings::load(&home);
    let parts = setup.parts(&settings, &Machine::detect(&home));
    println!("brain: {}", parts.brain.is_some());
    println!(
        "classifier: {:?}",
        parts.classifier.as_ref().map(|c| c.model.id)
    );
    println!(
        "voice: {:?}",
        parts
            .voice
            .as_ref()
            .map(|v| v.models.iter().map(|(m, _)| m.id).collect::<Vec<_>>())
    );
    for m in parts.missing {
        println!("missing: {m}");
    }
}
