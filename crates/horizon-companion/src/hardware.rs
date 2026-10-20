//! What the PC can run: RAM, cores, Apple Silicon and the GPUs llama.cpp sees, then the tier and the
//! runtime's thread count and GPU offload (docs/companions.md, "Models and runtime").

use std::path::Path;
use std::process::Command;

use crate::catalogue::Tier;

/// The graphics memory left to the game itself when the model's share is worked out.
pub const GAME_VRAM_MB: u64 = 1536;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    pub ram_mb: u64,
    pub physical_cores: usize,
    pub logical_cores: usize,
    /// An arm64 Mac, whatever the architecture of this binary (Rosetta included).
    pub apple_silicon: bool,
    /// From `llama-server --list-devices`; empty until the runtime is installed.
    pub gpus: Vec<Gpu>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuKind {
    Discrete,
    /// Shares the game's GPU and the system RAM: the model stays on the CPU.
    Integrated,
    /// llvmpipe, SwiftShader: a CPU pretending.
    Software,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    /// The llama.cpp device name (`MTL0`, `Vulkan1`, `CUDA0`): what `--device` takes.
    pub id: String,
    pub name: String,
    pub total_mb: u64,
    pub free_mb: u64,
    pub kind: GpuKind,
}

impl Hardware {
    /// RAM and cores of this PC; the GPUs come from [`parse_devices`] once the runtime is there.
    pub fn detect() -> Hardware {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        Hardware {
            ram_mb: ram_bytes().map_or(0, |b| b >> 20),
            physical_cores: physical_cores().unwrap_or(logical).max(1),
            logical_cores: logical,
            apple_silicon: apple_silicon(),
            gpus: Vec::new(),
        }
    }

    /// The RAM as sold: firmware and integrated GPUs keep a few hundred MB, so 15.6 GB counts as 16.
    pub fn nominal_ram_gb(&self) -> u64 {
        (self.ram_mb + 512) / 1024
    }

    /// The discrete GPU with the most free memory.
    pub fn best_gpu(&self) -> Option<&Gpu> {
        self.gpus
            .iter()
            .filter(|g| g.kind == GpuKind::Discrete)
            .max_by_key(|g| g.free_mb)
    }

    /// Free VRAM of the best discrete GPU once the game has its share.
    pub fn usable_vram_mb(&self) -> u64 {
        self.best_gpu()
            .map_or(0, |g| g.free_mb.saturating_sub(GAME_VRAM_MB))
    }
}

/// The tier the simple screen sets up: the design's table, highest first.
pub fn pick_tier(hw: &Hardware) -> Tier {
    let vram_gb = hw.usable_vram_mb() as f64 / 1024.0;
    let ram_gb = hw.nominal_ram_gb();
    let mac = hw.apple_silicon;
    if vram_gb >= 11.0 || (mac && ram_gb >= 32) {
        Tier::Best
    } else if vram_gb >= 8.0 || (mac && ram_gb >= 24) {
        Tier::Strong
    } else if vram_gb >= 5.0 || (mac && ram_gb >= 16) || (ram_gb >= 16 && hw.physical_cores >= 6) {
        Tier::Standard
    } else {
        Tier::Light
    }
}

/// `--threads`: physical cores minus one for the game, at least 1; a 2-core PC uses both, since one
/// thread makes every reply twice as slow and the game's main work runs on one core anyway.
pub fn runtime_threads(physical_cores: usize) -> usize {
    match physical_cores {
        0 | 1 => 1,
        2 => 2,
        n => n - 1,
    }
}

/// Where the model's layers go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offload {
    /// Everything on the GPU (`-ngl all`).
    All { device: String },
    /// A discrete GPU without room for all of it: llama.cpp's `--fit` places what fits, keeping the
    /// game's share free.
    Fit { device: String },
    /// CPU only (`-ngl 0 --device none`), so the game keeps its integrated GPU to itself.
    None,
}

/// The offload for a model of `model_bytes`. `need_extra_mb` is the KV cache and compute buffers on
/// top of the weights at the runtime's context size.
pub fn plan_offload(hw: &Hardware, model_bytes: u64, need_extra_mb: u64) -> Offload {
    if hw.apple_silicon
        && let Some(metal) = hw.gpus.iter().find(|g| g.id.starts_with("MTL"))
    {
        return Offload::All {
            device: metal.id.clone(),
        };
    }
    let Some(gpu) = hw.best_gpu() else {
        return Offload::None;
    };
    let need = (model_bytes >> 20) + need_extra_mb;
    let usable = gpu.free_mb.saturating_sub(GAME_VRAM_MB);
    if usable >= need {
        Offload::All {
            device: gpu.id.clone(),
        }
    } else if usable >= need / 2 {
        Offload::Fit {
            device: gpu.id.clone(),
        }
    } else {
        Offload::None
    }
}

/// Runs `llama-server --list-devices` and returns everything it printed (the device list goes to
/// stdout; the Vulkan backend's per-device details, which say whether a GPU is integrated, to stderr).
pub fn list_devices(server: &Path) -> std::io::Result<String> {
    // -v adds the Vulkan backend's per-device line with `uma:` (shared memory) on it.
    let out = Command::new(server)
        .args(["-v", "--list-devices"])
        .output()?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() && !text.contains("Available devices") {
        return Err(std::io::Error::other(format!(
            "llama-server --list-devices failed ({}): {}",
            out.status,
            text.trim()
        )));
    }
    Ok(text)
}

/// The GPUs in `llama-server -v --list-devices` output. Lines look like
/// `  Vulkan0: NVIDIA GeForce RTX 3060 (12288 MiB, 11200 MiB free)`; devices without memory
/// (`BLAS: Accelerate (0 MiB, 0 MiB free)`) are not GPUs. A Vulkan GPU is integrated when the backend's
/// `ggml_vulkan: N = … | uma: 1 |` line says it shares system memory, or, without that line, by its name.
pub fn parse_devices(output: &str) -> Vec<Gpu> {
    let mut uma: Vec<(usize, bool)> = Vec::new();
    for line in output.lines() {
        // `0.00.008.996 D ggml_vulkan: 0 = Intel(R) HD Graphics 620 (KBL GT2) (…) | uma: 1 | …`
        let Some((_, rest)) = line.split_once("ggml_vulkan: ") else {
            continue;
        };
        let Some((index, details)) = rest.split_once(" = ") else {
            continue;
        };
        if let (Ok(index), Some(flag)) = (index.trim().parse(), field(details, "uma")) {
            uma.push((index, flag == "1"));
        }
    }
    let mut gpus = Vec::new();
    let mut listing = false;
    for line in output.lines() {
        if line.trim_start().starts_with("Available devices") {
            listing = true;
            continue;
        }
        if !listing {
            continue;
        }
        let Some(gpu) = parse_device_line(line) else {
            continue;
        };
        if gpu.total_mb == 0 {
            continue;
        }
        let vulkan_index = gpu.id.strip_prefix("Vulkan").and_then(|n| n.parse().ok());
        let kind = match vulkan_index.and_then(|i: usize| uma.iter().find(|(n, _)| *n == i)) {
            _ if is_software(&gpu.name) => GpuKind::Software,
            _ if gpu.id.starts_with("MTL") => GpuKind::Integrated,
            Some((_, true)) => GpuKind::Integrated,
            Some((_, false)) => GpuKind::Discrete,
            None if looks_integrated(&gpu.name) => GpuKind::Integrated,
            None => GpuKind::Discrete,
        };
        gpus.push(Gpu { kind, ..gpu });
    }
    gpus
}

fn field<'a>(details: &'a str, key: &str) -> Option<&'a str> {
    details.split('|').find_map(|part| {
        let (k, v) = part.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// `  CUDA0: NVIDIA GeForce RTX 4090 (24563 MiB, 23879 MiB free)`
fn parse_device_line(line: &str) -> Option<Gpu> {
    let (id, rest) = line.trim().split_once(": ")?;
    if id.is_empty() || id.contains(' ') {
        return None;
    }
    let open = rest.rfind(" (")?;
    let (name, memory) = (&rest[..open], rest[open + 2..].strip_suffix(')')?);
    let (total, free) = memory.split_once(", ")?;
    let mib = |s: &str| s.trim().strip_suffix("MiB")?.trim().parse::<u64>().ok();
    Some(Gpu {
        id: id.to_owned(),
        name: name.trim().to_owned(),
        total_mb: mib(total)?,
        free_mb: mib(free.strip_suffix("free")?)?,
        kind: GpuKind::Discrete,
    })
}

fn is_software(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("llvmpipe") || n.contains("swiftshader") || n.contains("lavapipe")
}

/// Names of integrated GPUs, for drivers that don't report memory sharing: Intel's (but not the Arc
/// A and B cards), and AMD's APUs (`Radeon(TM) Graphics`, `Radeon 780M`, Vega).
fn looks_integrated(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("intel") {
        let discrete_arc = n.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
            w.len() >= 4
                && (w.starts_with('a') || w.starts_with('b'))
                && w[1..].bytes().all(|c| c.is_ascii_digit())
        });
        return !discrete_arc;
    }
    if n.contains("amd") || n.contains("radeon") {
        let apu_model = n.split_whitespace().any(|w| {
            w.len() == 4 && w.ends_with('m') && w[..3].bytes().all(|c| c.is_ascii_digit())
        });
        return apu_model
            || n.contains("radeon(tm) graphics")
            || n.contains("radeon graphics")
            || n.contains("vega");
    }
    false
}

#[cfg(any(target_os = "macos", windows))]
fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(target_os = "macos")]
fn ram_bytes() -> Option<u64> {
    output("sysctl", &["-n", "hw.memsize"])?.parse().ok()
}

#[cfg(target_os = "macos")]
fn physical_cores() -> Option<usize> {
    output("sysctl", &["-n", "hw.physicalcpu"])?.parse().ok()
}

#[cfg(target_os = "macos")]
fn apple_silicon() -> bool {
    // 1 on Apple Silicon, also for an x86_64 binary under Rosetta.
    output("sysctl", &["-n", "hw.optional.arm64"]).as_deref() == Some("1")
}

#[cfg(target_os = "linux")]
fn ram_bytes() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo_total(&info)
}

#[cfg(any(target_os = "linux", test))]
fn meminfo_total(info: &str) -> Option<u64> {
    let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb = line["MemTotal:".len()..]
        .trim()
        .trim_end_matches("kB")
        .trim();
    kb.parse::<u64>().ok().map(|kb| kb * 1024)
}

#[cfg(target_os = "linux")]
fn physical_cores() -> Option<usize> {
    cpuinfo_physical_cores(&std::fs::read_to_string("/proc/cpuinfo").ok()?)
}

/// Distinct (`physical id`, `core id`) pairs; ARM kernels print neither, then the logical count is used.
#[cfg(any(target_os = "linux", test))]
fn cpuinfo_physical_cores(info: &str) -> Option<usize> {
    let mut cores = std::collections::HashSet::new();
    let (mut package, mut core) = (None, None);
    for line in info.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if let Some(c) = core.take() {
                cores.insert((package.take().unwrap_or(0), c));
            }
            package = None;
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "physical id" => package = value.trim().parse::<u32>().ok(),
            "core id" => core = value.trim().parse::<u32>().ok(),
            _ => {}
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

#[cfg(target_os = "linux")]
fn apple_silicon() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn ram_bytes() -> Option<u64> {
    output(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        ],
    )?
    .parse()
    .ok()
}

#[cfg(target_os = "windows")]
fn physical_cores() -> Option<usize> {
    output(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_Processor | Measure-Object -Property NumberOfCores -Sum).Sum",
        ],
    )?
    .parse()
    .ok()
}

#[cfg(target_os = "windows")]
fn apple_silicon() -> bool {
    false
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn ram_bytes() -> Option<u64> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn physical_cores() -> Option<usize> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn apple_silicon() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // tests/fixtures/devices: `llama-server -v --list-devices` of b11756, install paths replaced.
    // Captured: metal-m4-pro (the Mac mini), vulkan-intel-hd620 (the ThinkPad, Mesa 26.1.8 ANV),
    // vulkan-no-driver (the Vulkan build without a Vulkan loader: it runs on the CPU) and cpu-only-linux
    // (the ThinkPad, CPU build). Written by hand in the same format, for want of the GPUs:
    // vulkan-nvidia-rtx3060, vulkan-amd-rx7800xt-and-igpu and cuda-gtx1650.
    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/devices/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    fn pc(ram_gb: u64, cores: usize, gpus: &str) -> Hardware {
        Hardware {
            ram_mb: ram_gb * 1024,
            physical_cores: cores,
            logical_cores: cores * 2,
            apple_silicon: false,
            gpus: parse_devices(&fixture(gpus)),
        }
    }

    #[test]
    fn metal_on_an_m4_pro() {
        // Captured on the Mac mini (M4 Pro, 64 GB) with b11756.
        let gpus = parse_devices(&fixture("metal-m4-pro.txt"));
        assert_eq!(gpus.len(), 1, "BLAS has no memory and is not a GPU");
        assert_eq!(gpus[0].id, "MTL0");
        assert_eq!(gpus[0].name, "Apple M4 Pro");
        assert_eq!((gpus[0].total_mb, gpus[0].free_mb), (53084, 53083));
        let mac = Hardware {
            ram_mb: 65536,
            physical_cores: 14,
            logical_cores: 14,
            apple_silicon: true,
            gpus,
        };
        assert_eq!(pick_tier(&mac), Tier::Best);
        assert_eq!(
            plan_offload(&mac, Tier::Best.model().size(), 1024),
            Offload::All {
                device: "MTL0".into()
            }
        );
    }

    #[test]
    fn apple_silicon_tiers_follow_unified_memory() {
        let mac = |gb: u64| Hardware {
            ram_mb: gb * 1024,
            physical_cores: 8,
            logical_cores: 8,
            apple_silicon: true,
            gpus: Vec::new(),
        };
        assert_eq!(pick_tier(&mac(8)), Tier::Light);
        assert_eq!(pick_tier(&mac(16)), Tier::Standard);
        assert_eq!(pick_tier(&mac(24)), Tier::Strong);
        assert_eq!(pick_tier(&mac(36)), Tier::Best);
    }

    #[test]
    fn nvidia_discrete_on_vulkan() {
        let hw = pc(32, 8, "vulkan-nvidia-rtx3060.txt");
        assert_eq!(hw.gpus.len(), 1);
        assert_eq!(hw.gpus[0].kind, GpuKind::Discrete);
        assert_eq!(hw.gpus[0].free_mb, 11520);
        // 11.25 GB free minus the game's 1.5 GB.
        assert_eq!(pick_tier(&hw), Tier::Strong);
        assert_eq!(
            plan_offload(&hw, Tier::Strong.model().size(), 1024),
            Offload::All {
                device: "Vulkan0".into()
            }
        );
    }

    #[test]
    fn amd_discrete_and_its_apu_side_by_side() {
        let hw = pc(32, 8, "vulkan-amd-rx7800xt-and-igpu.txt");
        let kinds: Vec<_> = hw.gpus.iter().map(|g| (g.id.as_str(), g.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("Vulkan0", GpuKind::Discrete),
                ("Vulkan1", GpuKind::Integrated)
            ]
        );
        assert_eq!(
            hw.best_gpu().unwrap().name,
            "AMD Radeon RX 7800 XT (RADV NAVI32)"
        );
        assert_eq!(pick_tier(&hw), Tier::Best);
    }

    #[test]
    fn intel_igpu_keeps_the_model_on_the_cpu() {
        // The ThinkPad's profile: i7-7500U (2 cores, 4 threads), HD 620, 32 GB.
        let hw = pc(32, 2, "vulkan-intel-hd620.txt");
        assert_eq!(hw.gpus[0].kind, GpuKind::Integrated);
        assert_eq!(hw.usable_vram_mb(), 0);
        assert_eq!(
            pick_tier(&hw),
            Tier::Light,
            "2 cores is below the CPU path's 6"
        );
        assert_eq!(
            plan_offload(&hw, Tier::Light.model().size(), 512),
            Offload::None
        );
        assert_eq!(runtime_threads(hw.physical_cores), 2);
    }

    #[test]
    fn cpu_only_pcs() {
        for fixture in ["cpu-only-linux.txt", "vulkan-no-driver.txt"] {
            let hw = pc(16, 6, fixture);
            assert!(hw.gpus.is_empty(), "{fixture}");
            assert_eq!(pick_tier(&hw), Tier::Standard, "16 GB and 6 cores");
            assert_eq!(pick_tier(&pc(16, 4, fixture)), Tier::Light);
            assert_eq!(pick_tier(&pc(8, 8, fixture)), Tier::Light);
        }
    }

    #[test]
    fn small_discrete_gpus_fit_what_they_can() {
        let hw = pc(16, 4, "cuda-gtx1650.txt");
        assert_eq!(hw.gpus[0].id, "CUDA0");
        assert_eq!(pick_tier(&hw), Tier::Light);
        let standard = Tier::Standard.model().size();
        assert_eq!(
            plan_offload(&hw, Tier::Light.model().size(), 512),
            Offload::All {
                device: "CUDA0".into()
            }
        );
        assert_eq!(
            plan_offload(&hw, standard, 512),
            Offload::Fit {
                device: "CUDA0".into()
            }
        );
    }

    #[test]
    fn software_renderers_are_not_gpus() {
        let gpus = parse_devices(
            "Available devices:\n  Vulkan0: llvmpipe (LLVM 19.1.7, 256 bits) (32000 MiB, 32000 MiB free)\n",
        );
        assert_eq!(gpus[0].kind, GpuKind::Software);
        let hw = Hardware {
            gpus,
            ..pc(8, 4, "cpu-only-linux.txt")
        };
        assert_eq!(plan_offload(&hw, 1 << 30, 0), Offload::None);
    }

    #[test]
    fn names_alone_tell_integrated_from_discrete() {
        assert!(looks_integrated("Intel(R) UHD Graphics 630 (CFL GT2)"));
        assert!(looks_integrated("Intel(R) Arc(TM) 140V GPU (16GB)"));
        assert!(!looks_integrated("Intel(R) Arc(TM) A770 Graphics"));
        assert!(!looks_integrated("Intel(R) Arc(TM) B580 Graphics"));
        assert!(looks_integrated("AMD Radeon(TM) Graphics"));
        assert!(looks_integrated("AMD Radeon 780M (RADV PHOENIX)"));
        assert!(!looks_integrated("AMD Radeon RX 6600 XT"));
        assert!(!looks_integrated("NVIDIA GeForce RTX 4070"));
    }

    #[test]
    fn threads_leave_a_core_for_the_game() {
        assert_eq!(runtime_threads(0), 1);
        assert_eq!(runtime_threads(1), 1);
        assert_eq!(runtime_threads(2), 2);
        assert_eq!(runtime_threads(4), 3);
        assert_eq!(runtime_threads(14), 13);
    }

    #[test]
    fn linux_proc_files() {
        assert_eq!(
            meminfo_total("MemTotal:       32788148 kB\nMemFree: 1 kB\n"),
            Some(32788148 * 1024)
        );
        // Two cores with two threads each, as /proc/cpuinfo prints the i7-7500U.
        let mut info = String::new();
        for (cpu, core) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
            info += &format!("processor\t: {cpu}\nphysical id\t: 0\ncore id\t\t: {core}\n\n");
        }
        assert_eq!(cpuinfo_physical_cores(&info), Some(2));
        assert_eq!(
            cpuinfo_physical_cores("processor\t: 0\nBogoMIPS\t: 48.00\n\n"),
            None
        );
    }

    #[test]
    fn detect_finds_this_pc() {
        let hw = Hardware::detect();
        assert!(hw.ram_mb > 1024);
        assert!(hw.physical_cores >= 1 && hw.physical_cores <= hw.logical_cores);
        assert_eq!(
            hw.apple_silicon,
            cfg!(all(target_os = "macos", target_arch = "aarch64"))
        );
    }
}
