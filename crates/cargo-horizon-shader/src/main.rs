//! `cargo horizon-shader`: write Horizon shader pack stages in Rust
//! (`docs/shader-packs.md`, "Horizon shaders v1").
//!
//! - `build [<pack>]`: compiles the pack's Rust crate to SPIR-V with rust-gpu
//!   (through `cargo gpu`), checks every module and copies it into the pack's
//!   `shaders/`, where its `pack.toml` stages point. Players get the `.spv`
//!   and need no toolchain.
//! - `check [<pack>]`: only the checks, on the `.spv` files already there.
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// Where `cargo gpu` writes the modules, inside the pack folder.
const OUTPUT: &str = "target/horizon-shader";
const INSTALL: &str =
    "cargo install --git https://github.com/Rust-GPU/rust-gpu --tag v0.11.0 cargo-gpu";

fn main() -> ExitCode {
    // Cargo runs `cargo-horizon-shader horizon-shader <args>`.
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "horizon-shader") {
        args.remove(0);
    }
    let pack = |i: usize| PathBuf::from(args.get(i).map_or(".", String::as_str));
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&pack(1)),
        Some("check") => check(&pack(1)),
        _ => {
            Err("usage: cargo horizon-shader build [<pack folder>] | check [<pack folder>]".into())
        }
    };
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build(pack: &Path) -> Result<Vec<String>, String> {
    if !pack.join("Cargo.toml").is_file() || !pack.join("pack.toml").is_file() {
        return Err(format!(
            "{}: a shader pack folder holds pack.toml and the Cargo.toml of its Rust stages",
            pack.display()
        ));
    }
    let has_gpu = Command::new("cargo")
        .args(["gpu", "--version"])
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_gpu {
        return Err(format!(
            "`cargo gpu` (rust-gpu's builder) isn't installed; install it with\n  {INSTALL}"
        ));
    }
    // cargo gpu builds the crate in the folder it runs in (its default
    // `--shader-crate ./`).
    let status = Command::new("cargo")
        .args([
            "gpu",
            "build",
            "--auto-install-rust-toolchain",
            "--output-dir",
            OUTPUT,
        ])
        .current_dir(pack)
        .status()
        .map_err(|e| format!("cargo gpu: {e}"))?;
    if !status.success() {
        return Err("cargo gpu build failed".into());
    }
    let out = pack.join(OUTPUT);
    let shaders = pack.join("shaders");
    fs::create_dir_all(&shaders).map_err(|e| format!("{}: {e}", shaders.display()))?;
    let mut copied = Vec::new();
    for entry in fs::read_dir(&out).map_err(|e| format!("{}: {e}", out.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "spv") {
            module(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let to = shaders.join(path.file_name().unwrap_or_default());
            fs::copy(&path, &to).map_err(|e| format!("{}: {e}", to.display()))?;
            copied.push(format!("Built {}", to.display()));
        }
    }
    if copied.is_empty() {
        return Err(format!("cargo gpu wrote no .spv in {}", out.display()));
    }
    copied.extend(check(pack)?);
    Ok(copied)
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    stage: Vec<Stage>,
}

#[derive(Deserialize)]
struct Stage {
    shader: String,
    entry: Option<String>,
}

/// Every compiled stage `pack.toml` lists: its module parses, validates and
/// converts to WGSL (as the game does), and has the fragment entry point.
fn check(pack: &Path) -> Result<Vec<String>, String> {
    let file = pack.join("pack.toml");
    let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut lines = Vec::new();
    for stage in manifest.stage.iter().filter(|s| s.shader.ends_with(".spv")) {
        let path = pack.join(&stage.shader);
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let module = module(&bytes).map_err(|e| format!("{}: {e}", stage.shader))?;
        let entry = stage.entry.as_deref().unwrap_or("fs_main");
        if !module
            .entry_points
            .iter()
            .any(|e| e.name == entry && e.stage == naga::ShaderStage::Fragment)
        {
            return Err(format!(
                "{}: no fragment entry point `{entry}`",
                stage.shader
            ));
        }
        lines.push(format!("Checked {} ({entry})", stage.shader));
    }
    if lines.is_empty() {
        lines.push("pack.toml lists no compiled (.spv) stage".into());
    }
    Ok(lines)
}

/// A SPIR-V module as the game loads it.
fn module(bytes: &[u8]) -> Result<naga::Module, String> {
    let module =
        naga::front::spv::parse_u8_slice(bytes, &Default::default()).map_err(|e| e.to_string())?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| e.as_inner().to_string())?;
    naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
        .map_err(|e| e.to_string())?;
    Ok(module)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn the_example_pack_checks() {
        let pack =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shaderpacks/daybreak");
        let lines = super::check(&pack).unwrap();
        assert_eq!(lines.len(), 5, "{lines:?}");
    }

    #[test]
    fn a_missing_entry_point_is_an_error() {
        let dir = std::env::temp_dir().join(format!("horizon-shader-check-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("shaders")).unwrap();
        let spv = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/shaderpacks/daybreak/shaders/daybreak.spv");
        std::fs::copy(spv, dir.join("shaders/a.spv")).unwrap();
        std::fs::write(
            dir.join("pack.toml"),
            "[[stage]]\nshader = \"shaders/a.spv\"\nentry = \"nope\"\n",
        )
        .unwrap();
        let error = super::check(&dir).unwrap_err();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(error.contains("no fragment entry point `nope`"), "{error}");
    }
}
