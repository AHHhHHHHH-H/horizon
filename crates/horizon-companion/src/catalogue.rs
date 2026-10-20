//! Everything the companion downloads, pinned: one llama.cpp release with an archive per OS and
//! backend, and the four model tiers. Sizes and SHA-256 come from GitHub's asset digests and the
//! Hugging Face tree API (the LFS oid is the file's SHA-256); the archives and the Light and
//! Standard models were downloaded and checked against them on 2026-10-20.

/// The llama.cpp release the companion runs (`llama-server --version`: build 11756, commit d80619397).
pub const LLAMA_CPP_BUILD: &str = "b11756";
pub const LLAMA_CPP_COMMIT: &str = "d80619397";
pub const LLAMA_CPP_LICENSE: &str = "MIT";
pub const LLAMA_CPP_CREDIT: &str = "llama.cpp by Georgi Gerganov and the ggml-org contributors";

/// What runs the model inside the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Apple's GPU API; the macOS arm64 build also has the CPU backend.
    Metal,
    /// Any GPU with a Vulkan driver (NVIDIA, AMD, Intel); the build also has the CPU backend.
    Vulkan,
    Cpu,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Metal => "Metal",
            Backend::Vulkan => "Vulkan",
            Backend::Cpu => "CPU",
        }
    }
}

/// One prebuilt llama.cpp archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeBuild {
    /// `std::env::consts::OS`: `macos`, `linux` or `windows`.
    pub os: &'static str,
    /// `std::env::consts::ARCH`: `aarch64` or `x86_64`.
    pub arch: &'static str,
    pub backend: Backend,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

impl RuntimeBuild {
    pub fn url(&self) -> String {
        format!(
            "https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_CPP_BUILD}/{}",
            self.file
        )
    }

    /// The folder it is installed in under `<data>/companion/runtime/`, e.g. `b11756-macos-aarch64-metal`.
    pub fn key(&self) -> String {
        format!(
            "{LLAMA_CPP_BUILD}-{}-{}-{}",
            self.os,
            self.arch,
            self.backend.label().to_ascii_lowercase()
        )
    }

    pub fn server_exe(&self) -> &'static str {
        if self.os == "windows" {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    }
}

macro_rules! build {
    ($os:literal, $arch:literal, $backend:ident, $file:literal, $size:literal, $sha:literal) => {
        RuntimeBuild {
            os: $os,
            arch: $arch,
            backend: Backend::$backend,
            file: $file,
            size: $size,
            sha256: $sha,
        }
    };
}

/// Every archive, the preferred one first for each OS and architecture. The Vulkan builds also hold the
/// CPU backend; the CPU builds are the fallback when the Vulkan one can't start (no Vulkan loader).
pub const RUNTIME_BUILDS: &[RuntimeBuild] = &[
    build!(
        "macos",
        "aarch64",
        Metal,
        "llama-b11756-bin-macos-arm64.tar.gz",
        16103382,
        "f86b5eb838b985a17587e794dee9c6b34193aca6dceec018031a4ba7951fb7e6"
    ),
    build!(
        "macos",
        "x86_64",
        Cpu,
        "llama-b11756-bin-macos-x64.tar.gz",
        11809449,
        "6d71968268a01ae975acde18441a70b2c7ba128b7a127f46b5ced4e8c5dd4eca"
    ),
    build!(
        "linux",
        "x86_64",
        Vulkan,
        "llama-b11756-bin-ubuntu-vulkan-x64.tar.gz",
        34877974,
        "7b23a9192e013ee8cf2011c192fd6a6197a41b562ff1b1a8e41f523a933d75cf"
    ),
    build!(
        "linux",
        "x86_64",
        Cpu,
        "llama-b11756-bin-ubuntu-x64.tar.gz",
        22240705,
        "9def57d9e9b0149979a9ceba40896a4e2bf88c94734069a5567d2f487ceab551"
    ),
    build!(
        "linux",
        "aarch64",
        Vulkan,
        "llama-b11756-bin-ubuntu-vulkan-arm64.tar.gz",
        28322778,
        "bbaacf9fbe9e565bb3a2c437a91de49bc02305cebc58bb8898a3cd1d80b86910"
    ),
    build!(
        "linux",
        "aarch64",
        Cpu,
        "llama-b11756-bin-ubuntu-arm64.tar.gz",
        18034586,
        "42bb5ef275743e0183fa53e3c93b2b54796997d49a9d0d540d0a8cc945f1ed43"
    ),
    build!(
        "windows",
        "x86_64",
        Vulkan,
        "llama-b11756-bin-win-vulkan-x64.zip",
        36751610,
        "75ce69fdda503c3d6d3b3401f61369bd072194746ec65fbbc5fd7e749edd2914"
    ),
    build!(
        "windows",
        "x86_64",
        Cpu,
        "llama-b11756-bin-win-cpu-x64.zip",
        23959301,
        "d164195b8f3e92f8fd92bc3a5c0eeaef8707b2f1a979e36360ce8192ebf4fbb3"
    ),
    build!(
        "windows",
        "aarch64",
        Vulkan,
        "llama-b11756-bin-win-vulkan-arm64.zip",
        24993785,
        "3ea056e7f3f0eee0a58c9ea1b4e3ecba259e8433afb902bf9862f45e0eb4fda7"
    ),
    build!(
        "windows",
        "aarch64",
        Cpu,
        "llama-b11756-bin-win-cpu-arm64.zip",
        12609658,
        "ba47a12a87d45a3a717d717e81334e65b725df051c8adf8f68accd824d05a126"
    ),
];

/// The builds for an OS and architecture, the one to try first first.
pub fn runtime_builds(os: &str, arch: &str) -> Vec<&'static RuntimeBuild> {
    RUNTIME_BUILDS
        .iter()
        .filter(|b| b.os == os && b.arch == arch)
        .collect()
}

/// The builds for the computer this runs on, the one to try first first.
pub fn runtime_builds_here() -> Vec<&'static RuntimeBuild> {
    runtime_builds(std::env::consts::OS, std::env::consts::ARCH)
}

/// The chat model tiers of the simple screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Light,
    Standard,
    Strong,
    Best,
}

impl Tier {
    pub const ALL: [Tier; 4] = [Tier::Light, Tier::Standard, Tier::Strong, Tier::Best];

    /// The chat model of this tier.
    pub fn model(self) -> &'static Model {
        MODELS
            .iter()
            .find(|m| m.kind == ModelKind::Chat(self))
            .expect("every tier has a chat model")
    }
}

/// What a model is for. Chat models are the brain; the other kinds belong to the speech and
/// classifier sidecar and are added to [`MODELS`] by its slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Chat(Tier),
    /// The fast intent classifier behind the reflex parser (`reflex::IntentClassifier`).
    Classifier,
    SpeechToText,
    TextToSpeech,
    VoiceActivity,
}

/// How the model's chat template turns thinking off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Qwen3.5: thinks unless `enable_thinking` is false.
    Qwen35,
    /// Gemma 4: thinks only when asked.
    Gemma4,
    /// Not a chat model.
    None,
}

/// One file of a model, as the Hugging Face tree API lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// One downloadable model: one or more files of a Hugging Face repository at a pinned commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// Its folder under `<data>/companion/models/`.
    pub id: &'static str,
    pub kind: ModelKind,
    /// The plain label of the simple screen.
    pub label: &'static str,
    /// The name Advanced shows.
    pub name: &'static str,
    pub family: Family,
    pub quantisation: &'static str,
    pub repo: &'static str,
    pub revision: &'static str,
    /// The first file is the one the runtime loads.
    pub files: &'static [ModelFile],
    pub license: &'static str,
    pub credit: &'static str,
}

impl Model {
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
}

const QWEN_CREDIT: &str = "Qwen3.5 by the Qwen team (Alibaba Cloud); GGUF quantisation by Unsloth";

pub const MODELS: &[Model] = &[
    Model {
        id: "qwen3.5-2b-q4_k_m",
        kind: ModelKind::Chat(Tier::Light),
        label: "Light — for older PCs",
        name: "Qwen3.5 2B Instruct",
        family: Family::Qwen35,
        quantisation: "Q4_K_M",
        repo: "unsloth/Qwen3.5-2B-GGUF",
        revision: "f6d5376be1edb4d416d56da11e5397a961aca8ae",
        files: &[ModelFile {
            path: "Qwen3.5-2B-Q4_K_M.gguf",
            size: 1280835840,
            sha256: "aaf42c8b7c3cab2bf3d69c355048d4a0ee9973d48f16c731c0520ee914699223",
        }],
        license: "Apache-2.0",
        credit: QWEN_CREDIT,
    },
    Model {
        id: "qwen3.5-4b-q4_k_m",
        kind: ModelKind::Chat(Tier::Standard),
        label: "Standard — most PCs",
        name: "Qwen3.5 4B Instruct",
        family: Family::Qwen35,
        quantisation: "Q4_K_M",
        repo: "unsloth/Qwen3.5-4B-GGUF",
        revision: "e87f176479d0855a907a41277aca2f8ee7a09523",
        files: &[ModelFile {
            path: "Qwen3.5-4B-Q4_K_M.gguf",
            size: 2740937888,
            sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        }],
        license: "Apache-2.0",
        credit: QWEN_CREDIT,
    },
    Model {
        id: "qwen3.5-9b-q4_k_m",
        kind: ModelKind::Chat(Tier::Strong),
        label: "Strong — good graphics card",
        name: "Qwen3.5 9B Instruct",
        family: Family::Qwen35,
        quantisation: "Q4_K_M",
        repo: "unsloth/Qwen3.5-9B-GGUF",
        revision: "3885219b6810b007914f3a7950a8d1b469d598a5",
        files: &[ModelFile {
            path: "Qwen3.5-9B-Q4_K_M.gguf",
            size: 5680522464,
            sha256: "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8",
        }],
        license: "Apache-2.0",
        credit: QWEN_CREDIT,
    },
    Model {
        id: "gemma-4-12b-it-qat-q4_0",
        kind: ModelKind::Chat(Tier::Best),
        label: "Best — needs a strong graphics card",
        name: "Gemma 4 12B Instruct",
        family: Family::Gemma4,
        quantisation: "Q4_0 (QAT)",
        repo: "google/gemma-4-12B-it-qat-q4_0-gguf",
        revision: "29d097773436b69ff9feafd636ab4cf873786537",
        files: &[ModelFile {
            path: "gemma-4-12b-it-qat-q4_0.gguf",
            size: 6975879296,
            sha256: "93567e57a8fe10b23569b9d9ec38cd005deedf71e29477c421a4b83f418a538b",
        }],
        license: "Apache-2.0",
        credit: "Gemma 4 by Google DeepMind; quantisation-aware Q4_0 GGUF by Google",
    },
];

/// The models of one kind (for chat, use [`Tier::model`]).
pub fn models(kind: ModelKind) -> impl Iterator<Item = &'static Model> {
    MODELS.iter().filter(move |m| m.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pin_is_well_formed() {
        for b in RUNTIME_BUILDS {
            assert_eq!(b.sha256.len(), 64, "{}", b.file);
            assert!(b.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
            assert!(b.file.contains(LLAMA_CPP_BUILD), "{}", b.file);
            assert!(b.size > 1_000_000);
        }
        for m in MODELS {
            assert_eq!(m.revision.len(), 40, "{}", m.id);
            assert!(!m.files.is_empty(), "{}", m.id);
            for f in m.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.path);
                assert!(f.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
                assert!(m.url(f).ends_with(f.path));
            }
            assert_eq!(
                MODELS.iter().filter(|o| o.id == m.id).count(),
                1,
                "{}",
                m.id
            );
        }
        for tier in Tier::ALL {
            assert_eq!(tier.model().kind, ModelKind::Chat(tier));
            assert_eq!(tier.model().license, "Apache-2.0");
        }
    }

    #[test]
    fn every_desktop_platform_has_a_build_with_a_fallback() {
        assert_eq!(
            runtime_builds("macos", "aarch64")[0].backend,
            Backend::Metal
        );
        assert_eq!(runtime_builds("macos", "x86_64")[0].backend, Backend::Cpu);
        for os in ["linux", "windows"] {
            for arch in ["x86_64", "aarch64"] {
                let builds = runtime_builds(os, arch);
                assert_eq!(builds.len(), 2, "{os} {arch}");
                assert_eq!(builds[0].backend, Backend::Vulkan);
                assert_eq!(builds[1].backend, Backend::Cpu);
            }
        }
        assert_eq!(
            runtime_builds("windows", "x86_64")[0].server_exe(),
            "llama-server.exe"
        );
        assert_eq!(
            runtime_builds("macos", "aarch64")[0].key(),
            "b11756-macos-aarch64-metal"
        );
    }
}
