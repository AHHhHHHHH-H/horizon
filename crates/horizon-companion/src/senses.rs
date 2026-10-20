//! Everything the senses download, pinned: one audio.cpp release with an archive per OS and backend,
//! and the speech to text, text to speech and classifier models. Sizes and SHA-256 come from GitHub's
//! asset digests and the Hugging Face tree API (the LFS oid is the file's SHA-256). Each entry says
//! what it needs and how fast it was on the machines it was measured on (docs/companions.md, "Voice").
//!
//! They share the brain's [`Backend`] and [`ModelFile`], and `plan::sense_options` turns them into
//! the plan's options, so one plan covers the brain, the classifier, speech recognition and the
//! voice. The sidecars that run them are in the `horizon-senses` crate.

use crate::catalogue::{Backend, ModelFile};

/// The audio.cpp release the voice runs (`audiocpp_server --version`: 0.9.2, git 0097d365).
pub const AUDIOCPP_VERSION: &str = "v0.9.2";
pub const AUDIOCPP_COMMIT: &str = "0097d365";
pub const AUDIOCPP_LICENSE: &str = "Apache-2.0";
pub const AUDIOCPP_CREDIT: &str = "audio.cpp by 0xShug0 and contributors (ggml-based)";

/// audio.cpp's name for a backend (its server config's `backend`).
pub fn audiocpp_backend(backend: Backend) -> &'static str {
    match backend {
        Backend::Metal => "metal",
        Backend::Vulkan => "vulkan",
        Backend::Cuda => "cuda",
        Backend::Cpu => "cpu",
    }
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
            audiocpp_backend(self.backend)
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
    /// The wait before its first word or sound on a typical PC of its kind, in ms, from the same
    /// measurements: what `plan()` compares.
    pub first_ms: u32,
    /// Higher is better among the models of its kind: the plan takes the best that fits.
    pub quality: u8,
    /// Text to speech: the PCM rate it returns. Speech to text: the rate it is sent.
    pub sample_rate: u32,
    pub voices: &'static [Voice],
    /// Text to speech: it follows a natural-language delivery instruction ("shouting, furious").
    pub instruct: bool,
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
const POCKET_VOICES: &[Voice] = &[
    Voice {
        id: "alba",
        description: "Woman, warm and clear",
    },
    Voice {
        id: "cosette",
        description: "Woman, light and young",
    },
    Voice {
        id: "marius",
        description: "Man, friendly",
    },
    Voice {
        id: "javert",
        description: "Man, low and stern",
    },
];

macro_rules! pocket {
    ($id:literal, $lang:literal, $name:literal, $load:literal, $files:expr) => {
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
            first_ms: 120,
            quality: 1,
            sample_rate: 24000,
            voices: POCKET_VOICES,
            instruct: false,
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
        first_ms: 500,
        quality: 2,
        sample_rate: 16000,
        voices: &[],
        instruct: false,
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
        first_ms: 200,
        quality: 1,
        sample_rate: 16000,
        voices: &[],
        instruct: false,
        load_language: None,
    },
    // Text to speech: Pocket TTS, one pack per language.
    pocket!(
        "pocket-tts-en-q8_0",
        "en",
        "Kyutai Pocket TTS (English)",
        "english",
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
                "PocketTTS-GGUF/english/embeddings/cosette.safetensors",
                6194424,
                "c4fdc15f5a3a20c44dd0064a37e87d15d25562936e8dbad7e07b9832015a545d"
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
        ]
    ),
    pocket!(
        "pocket-tts-fr-q8_0",
        "fr",
        "Kyutai Pocket TTS (French)",
        "french",
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
                "PocketTTS-GGUF/french/embeddings/cosette.safetensors",
                6244152,
                "e3c3d60c8c494592f03b2e9b1e963a90b6953229b78d6f6889898066129ebc89"
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
        ]
    ),
    pocket!(
        "pocket-tts-de-q8_0",
        "de",
        "Kyutai Pocket TTS (German)",
        "german",
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
                "PocketTTS-GGUF/german/embeddings/cosette.safetensors",
                6194424,
                "e9b82e86a807783bf1f17fd5ad9fdedee1af2e111ef3c7789d10a9a76c89a32a"
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
        ]
    ),
    pocket!(
        "pocket-tts-it-q8_0",
        "it",
        "Kyutai Pocket TTS (Italian)",
        "italian",
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
                "PocketTTS-GGUF/italian/embeddings/cosette.safetensors",
                6194424,
                "b8e2734587ea6a67850076ff2c867186ad4cd7a189e4894ecb609c8b79529002"
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
        ]
    ),
    pocket!(
        "pocket-tts-pt-q8_0",
        "pt",
        "Kyutai Pocket TTS (Portuguese)",
        "portuguese",
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
                "PocketTTS-GGUF/portuguese/embeddings/cosette.safetensors",
                6194424,
                "8aafd3e6b527dbb80123376b988ef2f21a8bee46b66e8b841ef529a298ba2acb"
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
        ]
    ),
    pocket!(
        "pocket-tts-es-q8_0",
        "es",
        "Kyutai Pocket TTS (Spanish)",
        "spanish",
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
                "PocketTTS-GGUF/spanish/embeddings/cosette.safetensors",
                6194424,
                "c4a00e5e1c1c953b15fb8100208948a4e38331e1b9230e2f6a0722260a301f41"
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
        first_ms: 90,
        quality: 2,
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
        first_ms: 16,
        quality: 2,
        sample_rate: 0,
        voices: &[],
        instruct: false,
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
        first_ms: 16,
        quality: 1,
        sample_rate: 0,
        voices: &[],
        instruct: false,
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
                assert!(!m.voices.is_empty(), "{}", m.id);
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
