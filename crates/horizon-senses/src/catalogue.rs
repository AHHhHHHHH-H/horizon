//! Everything the senses download, pinned: one audio.cpp release with an archive per OS and backend,
//! and the speech to text, text to speech and classifier models. Sizes and SHA-256 come from GitHub's
//! asset digests and the Hugging Face tree API (the LFS oid is the file's SHA-256). Each entry says
//! what it needs and how fast it was on the machines it was measured on (docs/companions.md, "Voice").
//!
//! The types mirror `horizon-companion`'s catalogue (`ModelFile` is the same shape) so the brain's
//! `plan()` can take these options as more kinds of the same thing.

/// The audio.cpp release the voice runs (`audiocpp_server --version`: 0.9.2, git 0097d365).
pub const AUDIOCPP_VERSION: &str = "v0.9.2";
pub const AUDIOCPP_COMMIT: &str = "0097d365";
pub const AUDIOCPP_LICENSE: &str = "Apache-2.0";
pub const AUDIOCPP_CREDIT: &str = "audio.cpp by 0xShug0 and contributors (ggml-based)";

/// What runs the model inside the sidecar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Apple's GPU API; the macOS builds also have the CPU backend.
    Metal,
    /// Any GPU with a Vulkan driver; the build also has the CPU backend.
    Vulkan,
    /// NVIDIA only; a much larger download (the CUDA runtime comes in a second archive).
    Cuda,
    Cpu,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Metal => "Metal",
            Backend::Vulkan => "Vulkan",
            Backend::Cuda => "CUDA",
            Backend::Cpu => "CPU",
        }
    }

    /// The value of audio.cpp's `backend` setting.
    pub fn audiocpp_name(self) -> &'static str {
        match self {
            Backend::Metal => "metal",
            Backend::Vulkan => "vulkan",
            Backend::Cuda => "cuda",
            Backend::Cpu => "cpu",
        }
    }
}

/// One file of a download: a release asset or a file of a Hugging Face repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// One prebuilt audio.cpp archive (the `bin` package: `audiocpp_server`, the CLI and the model specs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioCppBuild {
    /// `std::env::consts::OS`: `macos`, `linux` or `windows`.
    pub os: &'static str,
    /// `std::env::consts::ARCH`.
    pub arch: &'static str,
    pub backend: Backend,
    pub archive: ModelFile,
    /// A second archive unpacked beside the first: the CUDA runtime of the CUDA builds.
    pub runtime: Option<ModelFile>,
}

impl AudioCppBuild {
    pub fn url(file: &ModelFile) -> String {
        format!(
            "https://github.com/0xShug0/audio.cpp/releases/download/{AUDIOCPP_VERSION}/{}",
            file.path
        )
    }

    pub fn size(&self) -> u64 {
        self.archive.size + self.runtime.map_or(0, |r| r.size)
    }

    /// Its folder under `<data>/companion/voice-runtime/`, e.g. `v0.9.2-macos-aarch64-metal`.
    pub fn key(&self) -> String {
        format!(
            "{AUDIOCPP_VERSION}-{}-{}-{}",
            self.os,
            self.arch,
            self.backend.audiocpp_name()
        )
    }

    pub fn server_exe(&self) -> &'static str {
        if self.os == "windows" {
            "audiocpp_server.exe"
        } else {
            "audiocpp_server"
        }
    }
}

macro_rules! file {
    ($path:literal, $size:literal, $sha:literal) => {
        ModelFile {
            path: $path,
            size: $size,
            sha256: $sha,
        }
    };
}

macro_rules! build {
    ($os:literal, $arch:literal, $backend:ident, $file:literal, $size:literal, $sha:literal) => {
        AudioCppBuild {
            os: $os,
            arch: $arch,
            backend: Backend::$backend,
            archive: file!($file, $size, $sha),
            runtime: None,
        }
    };
}

/// Every archive, the preferred one first for each OS and architecture. Linux and Windows use the
/// `portable` builds: the others are compiled for the release machine's CPU (`GGML_NATIVE`) and can
/// stop with an illegal instruction on an older processor; the portable ones pick the CPU code at run
/// time. The Vulkan builds also hold the CPU backend; the CPU builds are the fallback when Vulkan can't
/// start. CUDA is offered on Windows only (Advanced): 0.9 GB with its runtime, and Vulkan runs the same
/// models on NVIDIA cards. audio.cpp has no Linux or Windows arm64 builds.
pub const AUDIOCPP_BUILDS: &[AudioCppBuild] = &[
    build!(
        "macos",
        "aarch64",
        Metal,
        "audio-v0.9.2-bin-macos-arm64-metal.tar.gz",
        31972675,
        "827239e1d5824f8294cce461661dc7f20f933869bd333dfc60ba3c089f29faab"
    ),
    build!(
        "macos",
        "x86_64",
        Metal,
        "audio-v0.9.2-bin-macos-x64-metal.tar.gz",
        33862576,
        "b199add8ab8d53f81769367b9869a021eae838fa6104c9b05a012ec7899774d8"
    ),
    build!(
        "linux",
        "x86_64",
        Vulkan,
        "audio-v0.9.2-bin-ubuntu-x64-vulkan-portable.tar.gz",
        73616441,
        "37294a0d9d52383475a47df95dc725c83faee0dbfae00088f1509ed4d7f3619d"
    ),
    build!(
        "linux",
        "x86_64",
        Cpu,
        "audio-v0.9.2-bin-ubuntu-x64-cpu-portable.tar.gz",
        59857917,
        "72bfb07f46bb8f4cc0217a604619dbb78cfc388dfa4e639feb412ca7e2410980"
    ),
    build!(
        "windows",
        "x86_64",
        Vulkan,
        "audio-v0.9.2-bin-windows-x64-vulkan-portable.zip",
        62556276,
        "4d11a82c560f7a1dc0934e6fef847d6e0b779cf79854aa7d90d8b28e4ad04d93"
    ),
    build!(
        "windows",
        "x86_64",
        Cpu,
        "audio-v0.9.2-bin-windows-x64-cpu-portable.zip",
        28114705,
        "4bea04239d753d2d0a294580ad4af2a0fa81b0de244a1b9146650c7f9fa4e84d"
    ),
    AudioCppBuild {
        os: "windows",
        arch: "x86_64",
        backend: Backend::Cuda,
        archive: file!(
            "audio-v0.9.2-bin-windows-x64-cuda13.3.zip",
            275142402,
            "ddf7474656e4c47a1bc5b5a53a268709d968d87132367d646baae5a77c34391f"
        ),
        runtime: Some(file!(
            "audio-v0.9.2-cudart-windows-x64-cuda13.3.zip",
            575457454,
            "f3bb616e43f4b8aec4be7f2c2b323aec381a1993bac382401a011c121c126e59"
        )),
    },
];

/// The builds for an OS and architecture, the one to try first first.
pub fn audiocpp_builds(os: &str, arch: &str) -> Vec<&'static AudioCppBuild> {
    AUDIOCPP_BUILDS
        .iter()
        .filter(|b| b.os == os && b.arch == arch)
        .collect()
}

/// What a sense model is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SenseKind {
    /// The intent classifier behind the reflex parser; runs in a second `llama-server --embeddings`.
    Classifier,
    /// Push-to-talk; runs in audio.cpp.
    SpeechToText,
    /// The companion's voice; runs in audio.cpp.
    TextToSpeech,
}

/// The program that loads the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// audio.cpp, with the model family its server config names.
    AudioCpp { family: &'static str },
    /// The brain's llama.cpp build, as `llama-server --embeddings --pooling mean`.
    LlamaEmbeddings,
}

/// What a model needs from the PC, beyond the game's own share (`plan()` keeps that free).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// Resident memory while loaded and speaking, measured.
    pub ram_mb: u64,
    /// GPU memory when it runs on the GPU (0: a CPU model).
    pub vram_mb: u64,
    /// The threads it is given.
    pub cores: usize,
    /// The backends it runs on, best first. Only `Cpu`: it never uses the GPU, the game keeps it.
    pub backends: &'static [Backend],
}

/// A built-in voice of a text to speech model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voice {
    /// The id the request names.
    pub id: &'static str,
    /// What it sounds like, for Advanced.
    pub description: &'static str,
}

/// One downloadable sense model: files of a Hugging Face repository at a pinned commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SenseModel {
    /// Its folder under `<data>/companion/models/`.
    pub id: &'static str,
    pub kind: SenseKind,
    /// The plain label of the simple screen.
    pub label: &'static str,
    /// The name Advanced shows.
    pub name: &'static str,
    pub engine: Engine,
    pub quantisation: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    /// The first file is the one the engine loads; the others sit beside it (voice embeddings).
    pub files: &'static [ModelFile],
    pub license: &'static str,
    pub credit: &'static str,
    /// ISO 639-1 codes it understands or speaks.
    pub languages: &'static [&'static str],
    pub needs: Needs,
    /// What was measured, in plain words for Advanced.
    pub latency: &'static str,
    /// Text to speech: the PCM rate it returns. Speech to text: the rate it is sent.
    pub sample_rate: u32,
    pub voices: &'static [Voice],
    /// Text to speech: it follows a natural-language delivery instruction ("shouting, furious").
    pub instruct: bool,
    /// Text to speech: it speaks in the voice of a reference clip (the cast, `crate::cast`), taking
    /// the delivery from the clip.
    pub clone: bool,
    /// audio.cpp `load_options` (Pocket TTS picks its language here).
    pub load_language: Option<&'static str>,
}

impl SenseModel {
    /// A file at its pinned commit: the same bytes forever, without an account.
    pub fn url(&self, file: &ModelFile) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, file.path
        )
    }

    pub fn page(&self) -> String {
        format!("https://huggingface.co/{}", self.repo)
    }

    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    pub fn main_file(&self) -> &'static ModelFile {
        &self.files[0]
    }

    /// The file name of a file inside the model's folder (Hugging Face paths keep their folders:
    /// `embeddings/alba.safetensors` stays beside the GGUF the way audio.cpp looks for it).
    pub fn local_path(&self, file: &ModelFile) -> &'static str {
        let prefix = self
            .main_file()
            .path
            .rsplit_once('/')
            .map_or("", |(dir, _)| dir);
        file.path
            .strip_prefix(prefix)
            .map_or(file.path, |p| p.trim_start_matches('/'))
    }

    pub fn speaks(&self, language: &str) -> bool {
        self.languages.contains(&language)
    }
}

/// audio.cpp's model repository at the commit every audio.cpp model here was checked against.
const AUDIOCPP_GGUF: &str = "audio-cpp/audio.cpp-gguf";
const AUDIOCPP_GGUF_REVISION: &str = "02f5de92a82339ebb722ea6fdadc14b7bec43a7f";

const POCKET_CREDIT: &str =
    "Pocket TTS by Kyutai (CC-BY-4.0); GGUF conversion by the audio.cpp project";
const POCKET_NEEDS: Needs = Needs {
    ram_mb: 400,
    vram_mb: 0,
    cores: 2,
    backends: &[Backend::Cpu],
};
const POCKET_LATENCY: &str = "First sound in 0.04 s and 4× faster than real time on 2 cores of an \
     Apple M4 Pro; 0.12 s and 1.6× on 2 cores of a 2016 laptop (Core i7-7500U)";
// The built-in voices of each pack. Left out: cosette (Expresso) and jean (EARS), both
// CC-BY-NC; juergen and rafael, whose recordings have no stated source.
const POCKET_VOICES_EN: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman, medium pitch (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman, higher (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man, medium pitch (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman, lower (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man, deep (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman, lower (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman, higher (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman, medium pitch (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman, medium pitch (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man, higher (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man, medium pitch (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman, higher (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman, higher (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman, medium pitch (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man, medium pitch (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man, medium pitch (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man, deep (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man, medium pitch (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman, lower (VCTK p229, CC-BY-4.0)",
    },
];
const POCKET_VOICES_FR: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman, medium pitch (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman, higher (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man, medium pitch (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman, lower (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man, deep (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "daan",
        description: "Man, medium pitch (CML-TTS, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman, lower (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman, higher (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman, medium pitch (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman, medium pitch (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man, medium pitch (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man, higher (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman, higher (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman, higher (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman, medium pitch (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man, deep (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man, medium pitch (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man, deep (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man, medium pitch (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman, medium pitch (VCTK p229, CC-BY-4.0)",
    },
];
const POCKET_VOICES_DE: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman (VCTK p229, CC-BY-4.0)",
    },
];
const POCKET_VOICES_IT: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman (VCTK p229, CC-BY-4.0)",
    },
];
const POCKET_VOICES_PT: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman (VCTK p229, CC-BY-4.0)",
    },
];
const POCKET_VOICES_ES: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear (Alba MacKenna, CC-BY-4.0)",
    },
    Voice {
        id: "marius",
        description: "Man, friendly (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern (Kyutai voice donation, CC0)",
    },
    Voice {
        id: "anna",
        description: "Woman (VCTK p228, CC-BY-4.0)",
    },
    Voice {
        id: "azelma",
        description: "Woman (VCTK p303, CC-BY-4.0)",
    },
    Voice {
        id: "bill_boerst",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "caro_davy",
        description: "Woman (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "charles",
        description: "Man (VCTK p254, CC-BY-4.0)",
    },
    Voice {
        id: "eponine",
        description: "Woman (VCTK p262, CC-BY-4.0)",
    },
    Voice {
        id: "estelle",
        description: "Woman (Kyutai recording, CC0)",
    },
    Voice {
        id: "eve",
        description: "Woman (VCTK p361, CC-BY-4.0)",
    },
    Voice {
        id: "fantine",
        description: "Woman (VCTK p244, CC-BY-4.0)",
    },
    Voice {
        id: "george",
        description: "Man (VCTK p315, CC-BY-4.0)",
    },
    Voice {
        id: "giovanni",
        description: "Man (Common Voice, CC0)",
    },
    Voice {
        id: "jane",
        description: "Woman (VCTK p339, CC-BY-4.0)",
    },
    Voice {
        id: "lola",
        description: "Woman (Common Voice, CC0)",
    },
    Voice {
        id: "mary",
        description: "Woman (VCTK p333, CC-BY-4.0)",
    },
    Voice {
        id: "michael",
        description: "Man (VCTK p360, CC-BY-4.0)",
    },
    Voice {
        id: "paul",
        description: "Man (VCTK p259, CC-BY-4.0)",
    },
    Voice {
        id: "peter_yearsley",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "stuart_bell",
        description: "Man (LibriVox (Voice-Zero), public domain)",
    },
    Voice {
        id: "vera",
        description: "Woman (VCTK p229, CC-BY-4.0)",
    },
];

const QWEN3_TTS_CREDIT: &str = "Qwen3-TTS by the Qwen team (Alibaba Cloud, Apache-2.0); GGUF conversion by the audio.cpp project";
const QWEN3_TTS_LANGUAGES: &[&str] = &["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"];
// Measured on the Mac mini (M4 Pro GPU, Metal; unified memory, so the peak footprint stands for
// the GPU memory), streaming 120 short lines of the cast while another model shared the GPU.
const BASE_17_VRAM_MB: u64 = 4300;
const BASE_17_LATENCY: &str = "First sound in 0.3 s once a character's clips are prepared (0.9 s for a \
     clip's first line), on an Apple M4 Pro GPU shared with another model";
const BASE_06_VRAM_MB: u64 = 3300;
const BASE_06_LATENCY: &str = "First sound in 0.24 s once a character's clips are prepared (0.8 s for \
     a clip's first line), on an Apple M4 Pro GPU shared with another model";

macro_rules! pocket {
    ($id:literal, $lang:literal, $name:literal, $load:literal, $voices:expr, $files:expr) => {
        SenseModel {
            id: $id,
            kind: SenseKind::TextToSpeech,
            label: "Natural voice — runs on the processor",
            name: $name,
            engine: Engine::AudioCpp {
                family: "pocket_tts",
            },
            quantisation: "Q8_0",
            repo: AUDIOCPP_GGUF,
            revision: AUDIOCPP_GGUF_REVISION,
            files: $files,
            license: "CC-BY-4.0",
            credit: POCKET_CREDIT,
            languages: &[$lang],
            needs: POCKET_NEEDS,
            latency: POCKET_LATENCY,
            sample_rate: 24000,
            voices: $voices,
            instruct: false,
            clone: true,
            load_language: Some($load),
        }
    };
}

pub const MODELS: &[SenseModel] = &[
    // Speech to text.
    SenseModel {
        id: "parakeet-tdt-0.6b-v3-q8_0",
        kind: SenseKind::SpeechToText,
        label: "Speech recognition — 25 European languages",
        name: "NVIDIA Parakeet TDT 0.6B v3",
        engine: Engine::AudioCpp {
            family: "parakeet_tdt",
        },
        quantisation: "Q8_0",
        repo: AUDIOCPP_GGUF,
        revision: AUDIOCPP_GGUF_REVISION,
        files: &[file!(
            "Parakeet-TDT-0.6B-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf",
            915735760,
            "6fa498f10f8cb606f041aa9512733cfee842a932359b00b0c45b9b723c4f4f4b"
        )],
        license: "CC-BY-4.0",
        credit: "Parakeet TDT 0.6B v3 by NVIDIA (CC-BY-4.0); GGUF conversion by the audio.cpp project",
        languages: &[
            "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt",
            "lv", "mt", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
        ],
        needs: Needs {
            ram_mb: 3300,
            vram_mb: 0,
            cores: 2,
            backends: &[Backend::Cpu],
        },
        latency: "A 3.5 s sentence in 0.3 s on 2 cores of an Apple M4 Pro (0.04 s on its GPU), \
                  0.7 s on 2 cores of a 2016 laptop (Core i7-7500U)",
        sample_rate: 16000,
        voices: &[],
        instruct: false,
        clone: false,
        load_language: None,
    },
    SenseModel {
        id: "canary-180m-flash-q8_0",
        kind: SenseKind::SpeechToText,
        label: "Light speech recognition — for older PCs, makes more mistakes",
        name: "NVIDIA Canary 180M Flash",
        engine: Engine::AudioCpp {
            family: "canary_asr",
        },
        quantisation: "Q8_0",
        repo: AUDIOCPP_GGUF,
        revision: AUDIOCPP_GGUF_REVISION,
        files: &[file!(
            "Canary-180M-Flash-GGUF/canary-180m-flash-q8_0.gguf",
            249496928,
            "fa751a543aa0db02ce6f0487ed836f95255f09a34db021cd4c280ed301089517"
        )],
        license: "CC-BY-4.0",
        credit: "Canary 180M Flash by NVIDIA (CC-BY-4.0); GGUF conversion by the audio.cpp project",
        languages: &["en", "de", "es", "fr"],
        needs: Needs {
            ram_mb: 820,
            vram_mb: 0,
            cores: 2,
            backends: &[Backend::Cpu],
        },
        latency: "A 3.5 s sentence in 0.14 s on 2 cores of an Apple M4 Pro",
        sample_rate: 16000,
        voices: &[],
        instruct: false,
        clone: false,
        load_language: None,
    },
    // Text to speech: Pocket TTS, one pack per language.
    pocket!(
        "pocket-tts-en-q8_0",
        "en",
        "Kyutai Pocket TTS (English)",
        "english",
        POCKET_VOICES_EN,
        &[
            file!(
                "PocketTTS-GGUF/english/pocket-tts-english-q8_0.gguf",
                127867968,
                "3f71cf6a51f93c2f2e554e98970a3f49fb9a98d0c64bed5a815e65be3d2ff588"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/alba.safetensors",
                6194424,
                "69c32db63ca56843d994f81f343f62e0bf2d73f7e4c9bc73e44bb1110b1d8845"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/marius.safetensors",
                6194424,
                "04f84efcb77a0547ba582c058db496f7ff4920891d49d37b9950d128422582a8"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/javert.safetensors",
                6194424,
                "0ae88e03ca4e76a0e16cbf321a807428febda9d9e9bc0358c02e7f9c9e2c263b"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/anna.safetensors",
                7816440,
                "5ea82f78db006c9fd34e32ddd5aae82674b5b32646097977436458d00af80dfa"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/azelma.safetensors",
                7963896,
                "9f3e69f29075f991fd47774566865ef0e0e637cb5a35992c9919761b5b84b1de"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/bill_boerst.safetensors",
                6735096,
                "75610127d44e0b05b442154f80f89f993df235aecc6cad7070f11000d006c188"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/caro_davy.safetensors",
                5260536,
                "a5961b63a2e7a5cfd7edc383aa9042fb70fd14a9dee6310cdc633881a7f2449a"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/charles.safetensors",
                6194424,
                "299edc20182eeccfbf94e308626f259da4fbf339daa8d5905218f2b1774639b8"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/eponine.safetensors",
                6931704,
                "bda3b76a384ff355fe0350736387765946304ae8ca16e59f60ea3296a1c99cc6"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/estelle.safetensors",
                8258808,
                "ccebef7f51762c7fc08870f5ecf268e8713551ae2c9f7984ddaec0c1e1c77153"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/eve.safetensors",
                6538488,
                "ea9c2faf862a6c9d2cb61910fdf02842ae56940382cc8c1000fdb1b43269692b"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/fantine.safetensors",
                6538488,
                "51a8a4355d7f912d4959e4b1918314fda85ad47eba0a33a1d78a4a505d3465f5"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/george.safetensors",
                6243576,
                "0c1c6c57c55a98d81254b33728150c7776f40647fe95258d1a6c1a02780b5d02"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/giovanni.safetensors",
                4621552,
                "a5ee718157ec1c6fd9c1e66a7733c7e6337d474a74a7da756455d27717885c59"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/jane.safetensors",
                7374072,
                "37386227ca8ec5bf1b8e516c13d132ce5ff5437a304fe90129a1c62f41d9a008"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/lola.safetensors",
                5948664,
                "34972e86b07b17272a8061460054a08119399cace9454979052b9e2d96664e8c"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/mary.safetensors",
                6194424,
                "a8f2adf260cab966fe0a113d6b549d6efdeaa79de544ae0ff34b5b6a41445a59"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/michael.safetensors",
                7275768,
                "8937f724ac4719b9aa51ea0ba1f18f9de0af7a663ad6263558266c1a53c9722d"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/paul.safetensors",
                6980856,
                "ed7a019168f94dfe77009f1b0de59387abc6fbb0db954d38ce722ecb77da61aa"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/peter_yearsley.safetensors",
                3736816,
                "dd977a6e15591e347c9a23fa7cc09e35a65b462917f5eeb162baff6dc9e3f685"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/stuart_bell.safetensors",
                5260536,
                "5a49da7ca5df05d02587ec4a0981c0d318e045f68e24423c4203ce474d9b33dc"
            ),
            file!(
                "PocketTTS-GGUF/english/embeddings/vera.safetensors",
                6735096,
                "4bf50ddd957b5d218b264fdcf18efbbc7384d12da3eca98ca19b9e8dd6976acc"
            ),
        ]
    ),
    pocket!(
        "pocket-tts-fr-q8_0",
        "fr",
        "Kyutai Pocket TTS (French)",
        "french",
        POCKET_VOICES_FR,
        &[
            file!(
                "PocketTTS-GGUF/french/pocket-tts-french-q8_0.gguf",
                128117408,
                "ae7e6d536c20245d216a7ec9404124cf67f913380bf41a6380c55d6ca9dbaef5"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/alba.safetensors",
                6244152,
                "4c80b1438668290542068bbc9515abd2c267f4cc86764c6633fcde395d1b2681"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/marius.safetensors",
                6244152,
                "f2b741148f2bc01f617e0aedfc5767f477b150b36f98490fa49f8ed0a39556fd"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/javert.safetensors",
                6145848,
                "de53da3263a749289bfed884faab4aba87541133828af506412f88becd3adbc1"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/anna.safetensors",
                7620408,
                "7a7fae31e4b0e16102cb2514f96189cdb57c8403f5f7aba2b37aa942e2d03488"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/azelma.safetensors",
                7178040,
                "d5f6e1176878323101cecf7595c3a8fd998048a0a5eefb864dbfcc0e453065f8"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/bill_boerst.safetensors",
                6539064,
                "acf6b6911e12f9e46b35971cdefb5d2a63b9308834a06477b2cc22ec4003ee61"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/caro_davy.safetensors",
                5064504,
                "40db2cb5e47bb98f151d7a3591ae269fd437682c08e4762a231009ac516bb66d"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/charles.safetensors",
                6195000,
                "956942f32745023f512ca99a45514648c8bbf4a4edc5bb5fdcb7c1a5dd5fbf97"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/daan.safetensors",
                3884848,
                "2b0282b9597ed5938b8c3f454198e7d491330b07d1514db6db903de71c937c5d"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/eponine.safetensors",
                6932280,
                "8e216220a7e8a69f7aa01ec85db2e63a445f9af6b907c43640a53a1889e6125b"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/estelle.safetensors",
                7571256,
                "d9efd55657f74ba9483caec23f9fd1d155e121b058feef86af3760d54ea8aa15"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/eve.safetensors",
                6145848,
                "b642a43b786fb60571656814cc66f9c24f8a457ea9f727903ea97d647d5ceedb"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/fantine.safetensors",
                6440760,
                "2ebba7baef8586851c589226e58414b21386a20dbac4c45746cd4317e51ff57b"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/george.safetensors",
                5900088,
                "b7513caf686418ec303dfd87dc6ddbcc020df9bf5964cb4afcd0b72b602afa6f"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/giovanni.safetensors",
                4130608,
                "b43d37b9ac7c48cb47d7d4bb5b320635b31d4e9d2ff036453414f634305c7896"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/jane.safetensors",
                7325496,
                "1482685d8a1a22a7294df650f61293bab353445710e26a60ae057cbf4ad85221"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/lola.safetensors",
                5457720,
                "751ff3e4cf0a917cf520e8ece16960d588b350b2dcb7463ce5c2ea1d4ee1666a"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/mary.safetensors",
                5949240,
                "8d5d4f03bce7660da0ea81d96f8a0ea0913685b21fbbbd7bba714242822368dc"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/michael.safetensors",
                7128888,
                "7dc7c44c71e514fcec8203a310676c410b3a68280dd52f6d60c424d51a8b638a"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/paul.safetensors",
                6833976,
                "85d2f623d5de93dbc33b56f7cbdb90bfaf100c1a7a0958848326223bc7cda5ed"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/peter_yearsley.safetensors",
                3639088,
                "ecd54a21b65e9d94964756421b0b2d2742be3075409aeb52806da2ea15b90880"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/stuart_bell.safetensors",
                5064504,
                "7d5442a128569a5e81e20e1e43866a7773b989438ee2e0f7e2f0a7904672944d"
            ),
            file!(
                "PocketTTS-GGUF/french/embeddings/vera.safetensors",
                6686520,
                "dca0db2aa7d5e63dab39a1826f7a4833b6852057eebb1ce21f4cd0945dce9870"
            ),
        ]
    ),
    pocket!(
        "pocket-tts-de-q8_0",
        "de",
        "Kyutai Pocket TTS (German)",
        "german",
        POCKET_VOICES_DE,
        &[
            file!(
                "PocketTTS-GGUF/german/pocket-tts-german-q8_0.gguf",
                127868448,
                "d99a966c42026dbe119863ad8a8c96ddd3c8d2e800dfe2ab75a0caa579293268"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/alba.safetensors",
                6194424,
                "b5a1933388634f63f0f709d3139f64d3c78281160d4a3bf6ea3bd1abc3ba94b6"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/marius.safetensors",
                6194424,
                "7f74e9727ee0ad2698113e07b75e24519c7a180f9ba3b033bd54bc1ef18c5d0f"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/javert.safetensors",
                6194424,
                "71eed75d77eb0c3f902ef0900401530c06983aa100ea4cdbec036ad607778a16"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/anna.safetensors",
                7816440,
                "6aa8e46421e4a8a602976918ed407fde55e7603b313acbe2e4432905b8bee7bc"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/azelma.safetensors",
                7963896,
                "e8fab25e856720159754ecda80a8791d6da09f70cdda32be2eda887e0ff1dcf6"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/bill_boerst.safetensors",
                6735096,
                "b95ed4aab6539920466ed8b8ed1e4f3f21899079345d6f415f48b44faf0e18d3"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/caro_davy.safetensors",
                5260536,
                "caada25a417c0cbdf48a78ae4b00480e1dcd9d7487f41dcd569376e71b9cf245"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/charles.safetensors",
                6194424,
                "c8bbc4a0e46cd0f16df2aaed7dfe811a0ca2d93799837ca4200851db906530b4"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/eponine.safetensors",
                6931704,
                "111350eaf68881da45a08239541b445481756365ac03308234a863f070334f33"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/estelle.safetensors",
                8258808,
                "61285c542f94b345dae0f810a540d4baaca2b0ceb9cd741cd3bd1af993ec74f4"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/eve.safetensors",
                6538488,
                "223f60a58a28236b27ca361e01f182fb919e72f0e3dceffb3bf18f006a0d0097"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/fantine.safetensors",
                6538488,
                "f0c2c0d22f5835c8cf8698cc7cb6ab93778229163eb1c55c86fe7e4ec512501a"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/george.safetensors",
                6243576,
                "bb0ecc9d5524fe64102f44c190be74d2dbee33bcf51f44dbb16e9b6b046496e1"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/giovanni.safetensors",
                4621552,
                "35127fade02a96edca0f0ff6ec9f129e935d39bc774d0245e6e3ea2da2276db9"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/jane.safetensors",
                7374072,
                "4572c0d06ce058ecb4f54d5059dcb872b3084d0cef95958faaf73bd4e1ccaf28"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/lola.safetensors",
                5948664,
                "4fdba576772c7b1c90e8e37ec0b9bd45d8b37d1d01b2060caea100e482b60394"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/mary.safetensors",
                6194424,
                "476f100e38206c5dff2cb22d582857241b7b718376f65d0ac69b9b0ea9dc1ebb"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/michael.safetensors",
                7275768,
                "6a491e111f277fab05bc05696691d621fd0847ae3b3be9271beb64453970d0f2"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/paul.safetensors",
                6980856,
                "3f74b464af87e9f8ee59895d806d6a2a05b5e7c345491d03177a92bc66a0eede"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/peter_yearsley.safetensors",
                3736816,
                "1cc888975274ecb60968e5c6ffd653f9bc3e9fe5b2527f71df52b1a7b3efba09"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/stuart_bell.safetensors",
                5260536,
                "08d8f183ef1d300acad1b6018d102e862b90d4df64bc79c77e619b99f7a172ce"
            ),
            file!(
                "PocketTTS-GGUF/german/embeddings/vera.safetensors",
                6735096,
                "53727005ebf05cac548aa372e26f93ca89b883210b904b58449c247a4f5444d1"
            ),
        ]
    ),
    pocket!(
        "pocket-tts-it-q8_0",
        "it",
        "Kyutai Pocket TTS (Italian)",
        "italian",
        POCKET_VOICES_IT,
        &[
            file!(
                "PocketTTS-GGUF/italian/pocket-tts-italian-q8_0.gguf",
                127868704,
                "aa25eb6c0d73fa90a26c07ed5453f44c321ff483b33e96e5c3693b6f76ef9149"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/alba.safetensors",
                6194424,
                "7d2bd2e5df41e5ede229857bcc2b56d41856725465260c330ba847e8a945442c"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/marius.safetensors",
                6194424,
                "ed70813d5f48d0b37ab671051fc2226588aec0922bf589f8a2f99fb4db6f199f"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/javert.safetensors",
                6194424,
                "a4f0b3ca1da9b276b0304f67f83342147afd28e823f7dd9775b62cc1c588bba5"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/anna.safetensors",
                7816440,
                "2d1bb064780b6b3c4a77a336c51335fde6dd07b506ccf329d2d0872ad6327c5f"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/azelma.safetensors",
                7963896,
                "bbd40c3dd7d74434e5d274ab87a1e00ea1c06216a3242eec783a7b6c882cf3ba"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/bill_boerst.safetensors",
                6735096,
                "fb5fc864bf42564a1362df9e37cc169b2570b40a0c7fc11a024b9009a89f5bc2"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/caro_davy.safetensors",
                5260536,
                "f13fb23a7c105ddeb5908483d6261ded35a2d8b053b97a77af6baf7a6ba4b78a"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/charles.safetensors",
                6194424,
                "1e4b6dafd17e0bb4489590f821f7083b598bd0dbe6d93bcabc8b72bb383881d5"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/eponine.safetensors",
                6931704,
                "31259726243a19758b6ba2d6dfa80bae3db70eedd96a183f5ce05d496eff4cdb"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/estelle.safetensors",
                8258808,
                "7c768f5234601c670917be9a8db2fa005f88d0dc97080fb415ea19c2154ddada"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/eve.safetensors",
                6538488,
                "6bd49ded51178b6a388bfad57360d4f3b795f89fa7155923efbd968405f01c9f"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/fantine.safetensors",
                6538488,
                "88a60059c7bed630cb77de4787b473da747e20140dfb93a4ade353c176bae7f1"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/george.safetensors",
                6243576,
                "c54fd34b252255f61f007fda923d8cb33b3f960b4806335ace0a74c2227a878d"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/giovanni.safetensors",
                4621552,
                "e2b47d9d63ad18cfb2e98591b714a0464456a739b1b4316b6427fcf9d5ea671f"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/jane.safetensors",
                7374072,
                "56bcf13e73c2143ab431e59be4d4b67ef0cb086ce63626f290a5edcbd1c3fa7f"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/lola.safetensors",
                5948664,
                "6c881871e4799a4927f97d1e2c75005f8bd4aebbcc772ab3b33cdb61fa376d49"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/mary.safetensors",
                6194424,
                "8cb071cafd9f2490094715ab3dc7c84a5f10b9c0539ab30526a1cce0416696b4"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/michael.safetensors",
                7275768,
                "3a52c4eb205affd8da5b8d8d2e384328b258f6652f2ad170311bb28f5cdab714"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/paul.safetensors",
                6980856,
                "c0a1913b9bf224d61d15562c3f1ff778e6ecfb77432c998ce785b3c04a19d9a0"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/peter_yearsley.safetensors",
                3736816,
                "74cabecedcaa39bafc75d4b8ccf9435c0bd6da0eeb5417d68bc68fb91c406a81"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/stuart_bell.safetensors",
                5260536,
                "c1438f5e99714fb10ccb1eae17bc00191b076ad753fb85d140575c396e5c1e2e"
            ),
            file!(
                "PocketTTS-GGUF/italian/embeddings/vera.safetensors",
                6735096,
                "e5e506fda84961dba96295915a9957c4e0c3db00a49f946cd9ec3f7a546a4583"
            ),
        ]
    ),
    pocket!(
        "pocket-tts-pt-q8_0",
        "pt",
        "Kyutai Pocket TTS (Portuguese)",
        "portuguese",
        POCKET_VOICES_PT,
        &[
            file!(
                "PocketTTS-GGUF/portuguese/pocket-tts-portuguese-q8_0.gguf",
                127869632,
                "e6e76fde1770586b11fab5a7d4fa07006d911098718991e242ca6006bd0513cd"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/alba.safetensors",
                6194424,
                "855b277ffa0e1f1600acacdb72f4e8609af7db7bbb5236dd904d1472c27541d2"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/marius.safetensors",
                6194424,
                "dda7f1265e424907c0260b8b640b4aaae6a2e926e7fa40ceda3ac270f316308f"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/javert.safetensors",
                6194424,
                "d4f340f52817471a7460530eddb77e8a6b4c06fea5bf285fb80dae46f1e73254"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/anna.safetensors",
                7816440,
                "70ef78480c1d90f502826c10b0512effa34262453e22a76d5992d5fa1e8182bb"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/azelma.safetensors",
                7963896,
                "9abb3ad69fbea053e44faa7c93d62d660b3552157ee02bcefc9157101ef269c2"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/bill_boerst.safetensors",
                6735096,
                "e5ed6c383f0ae3c92d86202d5f20b2cdd424ad3221873f4322728bdf61890cc6"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/caro_davy.safetensors",
                5260536,
                "85ff67e73e0f1c58e573eb10786da410e1fb79fdc71a10989e7bc11690cb8c04"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/charles.safetensors",
                6194424,
                "e643bddfba4c1c302f0589c09a18ac553af9fa2c7e82109e12a1a89225dd06bb"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/eponine.safetensors",
                6931704,
                "268bf106463dc70867a965a724af55c7d25c926d5408a6d962cb601c7bdac957"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/estelle.safetensors",
                8258808,
                "8d6489742a5d4026f23f115f3430cafc4fe931fe9faa2df69e6c3cca5ba8e3c4"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/eve.safetensors",
                6538488,
                "c3d3526bdf0d4f82a3e6cc8bca62148a20c69bbe6596fb72920478688f378488"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/fantine.safetensors",
                6538488,
                "826b7131c94bb43ef9123780189e1f1ffac737c2bfec1c897e1244437b481d06"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/george.safetensors",
                6243576,
                "f727f2820a343dc8cb0f867478e8feae9ecc8da41d6a84e6e051f1c192d20bb4"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/giovanni.safetensors",
                4621552,
                "099e94d27a8eb01df59e9284de8ca9fd5dc9c1cd48929b9818bed28f7958bb56"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/jane.safetensors",
                7374072,
                "de8f3516290a8f554785366cd2ce5dac1ad6abf694b6d37a41553b092eeb508e"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/lola.safetensors",
                5948664,
                "50a3a48ec0b67918e4775894a719ef9d4ce642bd355c275ad84decf14471fc55"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/mary.safetensors",
                6194424,
                "7c7a02fa20d4581e36b54fef748d189873e2ce398ccc57376bf76a8466a6d9cb"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/michael.safetensors",
                7275768,
                "d9348313f6e9d7badd420cffac11a59dc5767eeeccc7a679f8cfaaffecce7f2c"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/paul.safetensors",
                6980856,
                "ce48b67d732704f38985b38f5d5dea54175fa5bca795379cad0cffd55e534668"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/peter_yearsley.safetensors",
                3736816,
                "fad9560971b95008f09a58b9faf689c6e35dc6e1594d8a049c9ce9d65392a6cf"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/stuart_bell.safetensors",
                5260536,
                "4e069c47b4a88d163847763efd654666de6c745f692d56d04a52a6b2e662bfa5"
            ),
            file!(
                "PocketTTS-GGUF/portuguese/embeddings/vera.safetensors",
                6735096,
                "af141d1203871caef36d734d28b3aaee1acd2db00cf690fedcbc48d8268887de"
            ),
        ]
    ),
    pocket!(
        "pocket-tts-es-q8_0",
        "es",
        "Kyutai Pocket TTS (Spanish)",
        "spanish",
        POCKET_VOICES_ES,
        &[
            file!(
                "PocketTTS-GGUF/spanish/pocket-tts-spanish-q8_0.gguf",
                127869504,
                "d2645064c191d0b9b8daf6d24d2b8a310d89f694c9b3523c1258bc9455adcaff"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/alba.safetensors",
                6194424,
                "694be334ed6dda508b125e22a6ae480036d1c99923cccd92a643a348867f3235"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/marius.safetensors",
                6194424,
                "56490a863e39b33fd1ab9c9e1fdaa87127f7b73cf73f259dc308c40015abb9f9"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/javert.safetensors",
                6194424,
                "8a1d914ade79e8e470de3bd0725ad8bce62081976e352929d9d958a8773eba56"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/anna.safetensors",
                7816440,
                "fe964cdad79e8c2c0d2ca30bc9dc8479c6f38b45b77512975a6e272109367337"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/azelma.safetensors",
                7963896,
                "7fe8e3e2bf5258c73c765f5553478b95386e760ccecbb46fe24c2632b5fd7458"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/bill_boerst.safetensors",
                6735096,
                "5b95f21e267bd88357fbb4fe3cba173d9e4ba39ade8f386bb004a293ab5b470f"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/caro_davy.safetensors",
                5260536,
                "d07fb793439eca0e2d650e5c8b49f21b829351ca76e9525f9ab39c136ceaf079"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/charles.safetensors",
                6194424,
                "d620942926b0a30dcf002212221c89bb4f3b8b9b9bd2cc7d034c5e1c792a675f"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/eponine.safetensors",
                6931704,
                "06e688d7b179125afc56009a0f9d263768daa2c9ac3a0c7d07d70e411c01d5a7"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/estelle.safetensors",
                8258808,
                "2342bd79d85e80da2f39a9cb4b4a5c3db4dec79cbbecfdf01c9af7e9578ea187"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/eve.safetensors",
                6538488,
                "3fdb6a2ddd06f339b015ec90aea2ea004c260fe463c9526e020249b307096a41"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/fantine.safetensors",
                6538488,
                "7941dd5a2666afc6db9dadfcd27ee0255df89b50227cbd9628a3c0f280ff6f91"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/george.safetensors",
                6243576,
                "da140d63300c46c31b49ec26402a0b4efed5d773da8310cf4ccca115853cdc87"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/giovanni.safetensors",
                4621552,
                "aed540f9c8b2336f9ff6670fcab8d077f7bc7b076790da773a6038d7bb041cb4"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/jane.safetensors",
                7374072,
                "1bf9bfe3172412c44d0c7227e49eeb40b060f6833e2f6f365f0bbcde757afd09"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/lola.safetensors",
                5948664,
                "5ec51484cea179be24582c3d3326398c5175a6d4ed7e1f2eb3491f8933e5a243"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/mary.safetensors",
                6194424,
                "9a2ccf97785a059592b3e56c6277ec1091b997e38452c69dd3f0e10ba1096603"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/michael.safetensors",
                7275768,
                "ca3dfdaff6a2bf70ef9b2ab69bc66dce6d16fb06d4d7473be92610d3e43e9e15"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/paul.safetensors",
                6980856,
                "fcd677f0459352f7cdf62d7a1424a663bd49fc8921fb2b8e346ef9b45c92dd04"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/peter_yearsley.safetensors",
                3736816,
                "088277ee6dbb17f00ac3519dd41c0c7dd5b977b4b946f173f6122f513d1921a7"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/stuart_bell.safetensors",
                5260536,
                "3182401767b42c6c6cf42c08047c939a903c9fb8895633e0b71bca65c6b476b0"
            ),
            file!(
                "PocketTTS-GGUF/spanish/embeddings/vera.safetensors",
                6735096,
                "65787d133a1a55bc26ce50bb9cc0a992e8343b3ab006e27c2e755239512cd727"
            ),
        ]
    ),
    // Text to speech: Qwen3-TTS, every language in one model, delivery by instruction.
    SenseModel {
        id: "qwen3-tts-12hz-1.7b-customvoice-q8_0",
        kind: SenseKind::TextToSpeech,
        label: "Expressive voice — needs a strong graphics card",
        name: "Qwen3-TTS 12Hz 1.7B CustomVoice",
        engine: Engine::AudioCpp {
            family: "qwen3_tts",
        },
        quantisation: "Q8_0",
        repo: AUDIOCPP_GGUF,
        revision: AUDIOCPP_GGUF_REVISION,
        files: &[file!(
            "Qwen3-TTS-12Hz-1.7B-CustomVoice-GGUF/qwen3-tts-12hz-1.7b-customvoice-q8_0.gguf",
            2817055200,
            "c8ac3f94b6e73f137c0e7ecc9270bc8faec7a476ff799b87e48844d1d0987ac3"
        )],
        license: "Apache-2.0",
        credit: "Qwen3-TTS by the Qwen team (Alibaba Cloud, Apache-2.0); GGUF conversion by the audio.cpp project",
        languages: &["zh", "en", "ja", "ko", "de", "fr", "ru", "pt", "es", "it"],
        needs: Needs {
            ram_mb: 1000,
            vram_mb: 5000,
            cores: 2,
            backends: &[Backend::Metal, Backend::Cuda, Backend::Vulkan],
        },
        latency: "First sound in 0.06–0.09 s, about 2.2× faster than real time on an Apple M4 Pro GPU",
        sample_rate: 24000,
        voices: &[
            Voice {
                id: "ryan",
                description: "Man, energetic (English)",
            },
            Voice {
                id: "aiden",
                description: "Man, sunny (English)",
            },
            Voice {
                id: "vivian",
                description: "Woman, bright (Chinese)",
            },
            Voice {
                id: "serena",
                description: "Woman, gentle (Chinese)",
            },
            Voice {
                id: "dylan",
                description: "Man, youthful (Beijing dialect)",
            },
            Voice {
                id: "eric",
                description: "Man, lively (Sichuan dialect)",
            },
            Voice {
                id: "uncle_fu",
                description: "Man, deep and mellow (Chinese)",
            },
            Voice {
                id: "ono_anna",
                description: "Woman, playful (Japanese)",
            },
            Voice {
                id: "sohee",
                description: "Woman, warm (Korean)",
            },
        ],
        instruct: true,
        clone: false,
        load_language: None,
    },
    // Text to speech: Qwen3-TTS Base, which speaks as the cast from the clip of each tone.
    SenseModel {
        id: "qwen3-tts-12hz-1.7b-base-q8_0",
        kind: SenseKind::TextToSpeech,
        label: "Expressive voices — needs a strong graphics card",
        name: "Qwen3-TTS 12Hz 1.7B Base",
        engine: Engine::AudioCpp {
            family: "qwen3_tts",
        },
        quantisation: "Q8_0",
        repo: AUDIOCPP_GGUF,
        revision: AUDIOCPP_GGUF_REVISION,
        files: &[file!(
            "Qwen3-TTS-12Hz-1.7B-Base-GGUF/qwen3-tts-12hz-1.7b-base-q8_0_v2.gguf",
            2695186240,
            "92641a8022d521efd79a5db45e952589b3f2b781680f9abfcf26f075d37b86c6"
        )],
        license: "Apache-2.0",
        credit: QWEN3_TTS_CREDIT,
        languages: QWEN3_TTS_LANGUAGES,
        needs: Needs {
            ram_mb: 1000,
            vram_mb: BASE_17_VRAM_MB,
            cores: 2,
            backends: &[Backend::Metal, Backend::Cuda, Backend::Vulkan],
        },
        latency: BASE_17_LATENCY,
        sample_rate: 24000,
        voices: &[],
        instruct: false,
        clone: true,
        load_language: None,
    },
    SenseModel {
        id: "qwen3-tts-12hz-0.6b-base-q8_0",
        kind: SenseKind::TextToSpeech,
        label: "Expressive voices — for mid-range graphics cards",
        name: "Qwen3-TTS 12Hz 0.6B Base",
        engine: Engine::AudioCpp {
            family: "qwen3_tts",
        },
        quantisation: "Q8_0",
        repo: AUDIOCPP_GGUF,
        revision: AUDIOCPP_GGUF_REVISION,
        files: &[file!(
            "Qwen3-TTS-12Hz-0.6B-Base-GGUF/qwen3-tts-12hz-0.6b-base-q8_0.gguf",
            1991217728,
            "c7d0e6c9485a25ae1f6a4c0796f0023230a0051c44cbd2bd05d73770f2cce637"
        )],
        license: "Apache-2.0",
        credit: QWEN3_TTS_CREDIT,
        languages: QWEN3_TTS_LANGUAGES,
        needs: Needs {
            ram_mb: 1000,
            vram_mb: BASE_06_VRAM_MB,
            cores: 2,
            backends: &[Backend::Metal, Backend::Cuda, Backend::Vulkan],
        },
        latency: BASE_06_LATENCY,
        sample_rate: 24000,
        voices: &[],
        instruct: false,
        clone: true,
        load_language: None,
    },
    // The classifier.
    SenseModel {
        id: "multilingual-e5-small-q8_0",
        kind: SenseKind::Classifier,
        label: "Instant commands — runs on the processor",
        name: "multilingual-e5-small",
        engine: Engine::LlamaEmbeddings,
        quantisation: "Q8_0",
        repo: "TwinSunsLLC/multilingual-e5-small-gguf",
        revision: "b6cac9615d4ecce28d7f22539b7322d695fc2886",
        files: &[file!(
            "multilingual-e5-small-q8_0.gguf",
            132439008,
            "e011debc1208e31bf7b6aebee2d9fc8bd2ca11694a77ed66ac9d0c9d0a877c93"
        )],
        license: "MIT",
        credit: "multilingual-e5-small by Microsoft (intfloat, MIT); GGUF by TwinSuns LLC",
        languages: &["en", "fr", "es", "de", "pt", "it", "nl", "pl", "ru"],
        needs: Needs {
            ram_mb: 280,
            vram_mb: 0,
            cores: 1,
            backends: &[Backend::Cpu],
        },
        latency: "3 ms a line on 1 core of an Apple M4 Pro, 16 ms on 1 core of a 2016 laptop \
                  (Core i7-7500U)",
        sample_rate: 0,
        voices: &[],
        instruct: false,
        clone: false,
        load_language: None,
    },
    SenseModel {
        id: "paraphrase-multilingual-minilm-l12-v2-q8_0",
        kind: SenseKind::Classifier,
        label: "Instant commands (alternative) — runs on the processor",
        name: "paraphrase-multilingual-MiniLM-L12-v2",
        engine: Engine::LlamaEmbeddings,
        quantisation: "Q8_0",
        repo: "mykor/paraphrase-multilingual-MiniLM-L12-v2.gguf",
        revision: "a964c09982b6e28b13646d5f9860b8a04f4182c4",
        files: &[file!(
            "paraphrase-multilingual-MiniLM-L12-118M-v2-Q8_0.gguf",
            132438304,
            "1bcc1fd3f65d1269f8e733218d77ac5a479796fade0e87257606c2c4f2854662"
        )],
        license: "Apache-2.0",
        credit: "paraphrase-multilingual-MiniLM-L12-v2 by the sentence-transformers team (Apache-2.0); GGUF by mykor",
        languages: &["en", "fr", "es", "de", "pt", "it", "nl", "pl", "ru"],
        needs: Needs {
            ram_mb: 280,
            vram_mb: 0,
            cores: 1,
            backends: &[Backend::Cpu],
        },
        latency: "2.6 ms a line on 1 core of an Apple M4 Pro",
        sample_rate: 0,
        voices: &[],
        instruct: false,
        clone: false,
        load_language: None,
    },
];

/// The models of one kind, the default first.
pub fn models(kind: SenseKind) -> impl Iterator<Item = &'static SenseModel> {
    MODELS.iter().filter(move |m| m.kind == kind)
}

pub fn model(id: &str) -> Option<&'static SenseModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// The natural voice for a game language (`fr_fr`, `pt_br`, `en_us`…): its pack, else English.
pub fn pocket_for(game_language: &str) -> &'static SenseModel {
    let code = game_language.get(..2).unwrap_or("en");
    models(SenseKind::TextToSpeech)
        .find(|m| !m.instruct && m.speaks(code))
        .or_else(|| models(SenseKind::TextToSpeech).find(|m| !m.instruct && m.speaks("en")))
        .expect("an English Pocket TTS pack")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn well_formed(f: &ModelFile) {
        assert_eq!(f.sha256.len(), 64, "{}", f.path);
        assert!(
            f.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
            "{}",
            f.path
        );
        assert!(f.size > 1_000_000, "{}", f.path);
    }

    #[test]
    fn every_pin_is_well_formed() {
        for b in AUDIOCPP_BUILDS {
            well_formed(&b.archive);
            assert!(
                b.archive.path.contains(AUDIOCPP_VERSION),
                "{}",
                b.archive.path
            );
            assert!(AudioCppBuild::url(&b.archive).ends_with(b.archive.path));
            b.runtime.iter().for_each(well_formed);
        }
        for m in MODELS {
            assert_eq!(m.revision.len(), 40, "{}", m.id);
            assert!(!m.files.is_empty(), "{}", m.id);
            for f in m.files {
                well_formed(f);
                assert!(m.url(f).ends_with(f.path));
            }
            assert_eq!(
                MODELS.iter().filter(|o| o.id == m.id).count(),
                1,
                "{}",
                m.id
            );
            assert!(!m.languages.is_empty(), "{}", m.id);
            assert!(!m.needs.backends.is_empty(), "{}", m.id);
            if m.kind == SenseKind::TextToSpeech {
                assert!(!m.voices.is_empty() || m.clone, "{}", m.id);
                assert_eq!(m.sample_rate, 24000, "{}", m.id);
            }
        }
    }

    #[test]
    fn every_desktop_platform_has_a_build_with_a_fallback() {
        assert_eq!(
            audiocpp_builds("macos", "aarch64")[0].backend,
            Backend::Metal
        );
        assert_eq!(
            audiocpp_builds("macos", "x86_64")[0].backend,
            Backend::Metal
        );
        for os in ["linux", "windows"] {
            let builds = audiocpp_builds(os, "x86_64");
            assert_eq!(builds[0].backend, Backend::Vulkan, "{os}");
            assert_eq!(builds[1].backend, Backend::Cpu, "{os}");
            assert!(
                builds[..2]
                    .iter()
                    .all(|b| b.archive.path.contains("portable"))
            );
        }
        let cuda = audiocpp_builds("windows", "x86_64")[2];
        assert!(cuda.size() > cuda.archive.size);
        assert_eq!(
            audiocpp_builds("macos", "aarch64")[0].key(),
            "v0.9.2-macos-aarch64-metal"
        );
        assert_eq!(
            audiocpp_builds("windows", "x86_64")[0].server_exe(),
            "audiocpp_server.exe"
        );
    }

    #[test]
    fn the_five_game_languages_have_a_voice_and_speech_recognition() {
        let stt = models(SenseKind::SpeechToText).next().unwrap();
        for (game, code) in [
            ("en_us", "en"),
            ("fr_fr", "fr"),
            ("es_es", "es"),
            ("de_de", "de"),
            ("pt_br", "pt"),
        ] {
            assert!(pocket_for(game).speaks(code), "{game}");
            assert!(stt.speaks(code), "{game}");
            assert!(
                model("qwen3-tts-12hz-1.7b-customvoice-q8_0")
                    .unwrap()
                    .speaks(code)
            );
        }
        assert!(pocket_for("ja_jp").speaks("en"));
    }

    #[test]
    fn natural_voices_are_many_and_free_to_ship_and_cloning_models_have_the_cast() {
        for m in models(SenseKind::TextToSpeech).filter(|m| !m.instruct && m.needs.vram_mb == 0) {
            assert!(m.voices.len() >= 22, "{}", m.id);
            for v in m.voices {
                // Non-commercial or unknown sources.
                assert!(
                    !["cosette", "jean", "juergen", "rafael"].contains(&v.id),
                    "{}: {}",
                    m.id,
                    v.id
                );
                let file = format!("embeddings/{}.safetensors", v.id);
                assert!(
                    m.files.iter().any(|f| m.local_path(f) == file),
                    "{}: {file}",
                    m.id
                );
            }
            assert!(m.clone);
        }
        for id in [
            "qwen3-tts-12hz-1.7b-base-q8_0",
            "qwen3-tts-12hz-0.6b-base-q8_0",
        ] {
            let m = model(id).unwrap();
            assert!(m.clone && !m.instruct && m.voices.is_empty(), "{id}");
            assert!(m.speaks("fr") && m.speaks("es"));
        }
        assert!(!model("qwen3-tts-12hz-1.7b-customvoice-q8_0").unwrap().clone);
    }

    #[test]
    fn voice_embeddings_sit_beside_the_model() {
        let fr = pocket_for("fr_fr");
        assert_eq!(fr.local_path(fr.main_file()), "pocket-tts-french-q8_0.gguf");
        assert_eq!(fr.local_path(&fr.files[1]), "embeddings/alba.safetensors");
        let e5 = model("multilingual-e5-small-q8_0").unwrap();
        assert_eq!(
            e5.local_path(e5.main_file()),
            "multilingual-e5-small-q8_0.gguf"
        );
    }
}
