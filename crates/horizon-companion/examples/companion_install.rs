//! Installs the runtime for this PC and chat models into a data folder, with the crate's own
//! downloader, and prints what the hardware detection sees.
//!
//! cargo run -p horizon-companion --example companion_install -- <data dir> [light|standard|strong|best]...

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use horizon_companion::catalogue::{self, Tier};
use horizon_companion::download::{Layout, Progress};
use horizon_companion::hardware::{self, Hardware};

fn tier(name: &str) -> Tier {
    match name {
        "light" => Tier::Light,
        "standard" => Tier::Standard,
        "strong" => Tier::Strong,
        "best" => Tier::Best,
        other => panic!("unknown tier {other}"),
    }
}

fn reporter(name: String) -> impl FnMut(Progress) {
    let mut last = 0.0;
    let started = Instant::now();
    move |p: Progress| {
        let f = p.fraction();
        if f - last >= 0.1 || f >= 1.0 && last < 1.0 {
            last = f;
            let secs = started.elapsed().as_secs_f64().max(0.001);
            eprintln!(
                "{name}: {:?} {:>3.0}% ({} / {} MB, {:.1} MB/s)",
                p.phase,
                f * 100.0,
                p.done >> 20,
                p.total >> 20,
                (p.done >> 20) as f64 / secs
            );
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let data = PathBuf::from(args.next().expect("data dir"));
    let layout = Layout::new(&data);
    let cancel = AtomicBool::new(false);
    let build = catalogue::runtime_builds_here()[0];
    let started = Instant::now();
    let server = match layout.installed_runtime(build) {
        Some(server) => server,
        None => layout
            .install_runtime(build, &mut reporter(build.file.into()), &cancel)
            .unwrap_or_else(|e| panic!("{e}")),
    };
    println!(
        "runtime {} at {} ({:.1} s)",
        build.key(),
        server.display(),
        started.elapsed().as_secs_f64()
    );
    let mut hw = Hardware::detect();
    let devices = hardware::list_devices(&server).expect("list devices");
    hw.gpus = hardware::parse_devices(&devices);
    println!("hardware: {hw:?}");
    println!("pick_tier: {:?}", hardware::pick_tier(&hw));
    for name in args {
        let model = tier(&name).model();
        let started = Instant::now();
        let path = match layout.installed_model(model) {
            Some(path) => path,
            None => layout
                .install_model(model, &mut reporter(model.id.into()), &cancel)
                .unwrap_or_else(|e| panic!("{e}")),
        };
        println!(
            "model {} at {} ({:.1} s)",
            model.id,
            path.display(),
            started.elapsed().as_secs_f64()
        );
    }
}
