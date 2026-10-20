//! The voice sidecar: audio.cpp's server with the chosen speech to text and text to speech models, and
//! the two clients the game uses, streaming text to speech and push-to-talk speech to text.
//!
//! Speech arrives as server-sent events of base64 PCM (`stream_format: "sse"`) rather than a raw PCM
//! body, because an error after the first chunk then still arrives as an event.
use crate::cast;
use crate::catalogue::{Backend, Engine, SenseKind, SenseModel};
use crate::sidecar::{Launch, Sidecar};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

/// One installed model and the folder its files are in.
#[derive(Clone, Debug)]
pub struct Installed {
    pub model: &'static SenseModel,
    pub dir: PathBuf,
}

/// What the voice sidecar runs.
#[derive(Clone, Debug)]
pub struct VoiceSetup {
    /// The folder `audiocpp_server` was unpacked in (its `model_specs/` is beside it).
    pub runtime: PathBuf,
    pub exe: &'static str,
    pub backend: Backend,
    /// The thread cap (2 is enough for Pocket TTS and Parakeet; the game keeps the rest).
    pub threads: usize,
    /// The speech to text and text to speech models.
    pub models: Vec<Installed>,
    /// Where the config and the log are written.
    pub work: PathBuf,
    /// The unpacked voice pack (`<data>/companion/voices/<version>/`, [`cast::VOICE_PACK`]): the
    /// cast's reference clips, audio.cpp's `voice_dir`. Cloning models need it to speak as the cast.
    pub voices: Option<PathBuf>,
}

/// Reference voices kept prepared per model: one character's clips (5 emotions × 2 languages), so
/// switching tone never encodes a clip twice.
const PREPARED_VOICES: usize = 10;

/// audio.cpp's server config: 127.0.0.1, every model registered under its catalogue id and loaded on
/// first use, speech in streaming mode, recognition offline (the whole clip is sent on key release).
pub fn server_config(setup: &VoiceSetup) -> Value {
    let models: Vec<Value> = setup
        .models
        .iter()
        .filter_map(|i| {
            let Engine::AudioCpp { family } = i.model.engine else {
                return None;
            };
            let path = i.dir.join(i.model.local_path(i.model.main_file()));
            let mut entry = json!({
                "id": i.model.id,
                "family": family,
                "path": path.to_string_lossy(),
            });
            match i.model.kind {
                SenseKind::TextToSpeech => {
                    entry["task"] = "tts".into();
                    entry["mode"] = "streaming".into();
                    if let Some(voice) = i.model.voices.first() {
                        entry["default_voice_preset"] = json!({ "voice_id": voice.id });
                    }
                    if i.model.clone {
                        let key = match family {
                            "qwen3_tts" => Some("qwen3_tts.voice_prompt_cache_slots"),
                            "pocket_tts" => Some("pocket_tts.voice_state_cache_slots"),
                            _ => None,
                        };
                        if let Some(key) = key {
                            entry["session_options"] = json!({ key: PREPARED_VOICES });
                        }
                    }
                }
                _ => {
                    entry["task"] = "asr".into();
                    entry["mode"] = "offline".into();
                }
            }
            if let Some(language) = i.model.load_language {
                entry["load_options"] = json!({ "language": language });
            }
            Some(entry)
        })
        .collect();
    let mut config = json!({
        "host": "127.0.0.1",
        "port": "{port}",
        "backend": setup.backend.audiocpp_name(),
        "threads": setup.threads.max(1),
        "lazy_load": true,
        "log_request_body": false,
        "models": models,
    });
    if let Some(voices) = &setup.voices {
        config["voice_dir"] = voices.to_string_lossy().into();
    }
    config
}

/// The running voice sidecar.
pub struct VoiceServer {
    sidecar: Sidecar,
    models: Vec<&'static SenseModel>,
    /// The voice pack is installed.
    cast: bool,
}

impl VoiceServer {
    /// Write the config and start the server (models load on first use: see [`VoiceServer::warm_up`]).
    pub fn start(setup: &VoiceSetup) -> io::Result<VoiceServer> {
        std::fs::create_dir_all(&setup.work)?;
        let config = setup.work.join("audiocpp-server.json");
        // The port is a string in the template so the JSON stays valid; it becomes a number here.
        let text = serde_json::to_string_pretty(&server_config(setup))
            .map_err(io::Error::other)?
            .replace("\"{port}\"", "{port}");
        let launch = Launch {
            exe: setup.runtime.join(setup.exe),
            args: vec![
                "--config".into(),
                config.clone().into(),
                "--no-ui".into(),
                "--port".into(),
                "{port}".into(),
            ],
            dir: setup.runtime.clone(),
            log: setup.work.join("audiocpp-server.log"),
            files: vec![(config, text)],
        };
        let sidecar = Sidecar::start(&launch, "/health", Duration::from_secs(20))?;
        Ok(VoiceServer {
            sidecar,
            models: setup.models.iter().map(|i| i.model).collect(),
            cast: setup.voices.is_some(),
        })
    }

    pub fn base_url(&self) -> String {
        self.sidecar.base_url()
    }

    pub fn sidecar(&mut self) -> &mut Sidecar {
        &mut self.sidecar
    }

    /// The text to speech client, if a voice model is installed.
    pub fn speech(&self) -> Option<SpeechClient> {
        let models: Vec<_> = self
            .models
            .iter()
            .copied()
            .filter(|m| m.kind == SenseKind::TextToSpeech)
            .collect();
        (!models.is_empty())
            .then(|| SpeechClient::new(&self.base_url(), models).with_cast(self.cast))
    }

    /// The speech to text client, if a recognition model is installed.
    pub fn recognition(&self) -> Option<RecognitionClient> {
        let model = self
            .models
            .iter()
            .copied()
            .find(|m| m.kind == SenseKind::SpeechToText)?;
        Some(RecognitionClient::new(&self.base_url(), model))
    }

    /// Load every model now (a short request each), so the first real line doesn't wait for it.
    pub fn warm_up(&self) -> io::Result<()> {
        if let Some(speech) = self.speech() {
            for model in &speech.models {
                let language = model.languages[0];
                speech.speak_blocking(
                    &Speak {
                        text: "Ok.".into(),
                        voice: None,
                        tone: Tone::Normal,
                        language: language.into(),
                    },
                    |_| {},
                )?;
            }
        }
        if let Some(recognition) = self.recognition() {
            recognition.transcribe(&vec![0.0; 16000], None)?;
        }
        Ok(())
    }
}

/// How a line is said. The brain's JSON names it; an expressive model gets it as an instruction, and
/// every voice is played with its gain: Qwen3-TTS evens out its loudness (a shout and a whisper came
/// out within 0.2 dB RMS of each other on an M4 Pro), so the instruction changes the delivery and the
/// game makes it loud or quiet.
#[derive(Clone, Debug, PartialEq)]
pub enum Tone {
    Normal,
    Shout,
    Whisper,
    Laugh,
    Sarcastic,
    Scared,
    Calm,
    /// Any other instruction, in plain words ("slow and sleepy").
    Other(String),
}

impl Tone {
    /// The brain's `tone` value.
    pub fn parse(s: &str) -> Tone {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "normal" | "neutral" => Tone::Normal,
            "shout" | "shouting" | "yell" | "angry" => Tone::Shout,
            "whisper" | "whispering" => Tone::Whisper,
            "laugh" | "laughing" | "happy" => Tone::Laugh,
            "sarcastic" | "dry" => Tone::Sarcastic,
            "scared" | "afraid" | "panic" => Tone::Scared,
            "calm" | "soft" => Tone::Calm,
            other => Tone::Other(other.to_owned()),
        }
    }

    /// The delivery instruction for an expressive model.
    pub fn instruction(&self) -> Option<&str> {
        Some(match self {
            Tone::Normal => return None,
            Tone::Shout => "Shouting loudly, urgent and intense.",
            Tone::Whisper => "Whispering very softly and quietly, almost breathless.",
            Tone::Laugh => "Laughing, cheerful and amused.",
            Tone::Sarcastic => "Dry and sarcastic, slightly mocking.",
            Tone::Scared => "Scared and panicked, speaking fast.",
            Tone::Calm => "Calm, slow and reassuring.",
            Tone::Other(s) => s,
        })
    }

    /// The gain the line is played with.
    pub fn playback_gain(&self) -> f32 {
        match self {
            Tone::Shout => 1.6,
            Tone::Whisper => 0.35,
            Tone::Calm => 0.8,
            _ => 1.0,
        }
    }
}

/// One line to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Speak {
    pub text: String,
    /// A character of the cast (`cast::Character::id`), said by a cloning model from the clip that
    /// matches the tone, or a built-in voice of the model (`Voice::id`). `None`: the first of the
    /// cast on a cloning model, else the model's first voice.
    pub voice: Option<String>,
    pub tone: Tone,
    /// ISO 639-1 (`fr`).
    pub language: String,
}

/// What a speech stream sends.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeechEvent {
    /// Mono samples at the stream's rate, as they are generated.
    Audio(Vec<f32>),
    Done(SpeechStats),
    Failed(String),
}

/// How a line went.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpeechStats {
    /// From the request to the first samples.
    pub first_audio: Option<Duration>,
    pub total: Duration,
    pub samples: usize,
    pub sample_rate: u32,
    /// The gain to play it with (see [`Tone::playback_gain`]).
    pub gain: f32,
}

impl SpeechStats {
    pub fn audio_seconds(&self) -> f64 {
        self.samples as f64 / f64::from(self.sample_rate.max(1))
    }

    /// Real-time factor: generation time over audio time (below 1 is faster than real time).
    pub fn rtf(&self) -> f64 {
        self.total.as_secs_f64() / self.audio_seconds().max(1e-9)
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(2))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// Streaming text to speech.
#[derive(Clone)]
pub struct SpeechClient {
    agent: ureq::Agent,
    base: String,
    models: Vec<&'static SenseModel>,
    /// The voice pack is in the server's `voice_dir`.
    cast: bool,
}

impl SpeechClient {
    pub fn new(base: &str, models: Vec<&'static SenseModel>) -> SpeechClient {
        SpeechClient {
            agent: agent(),
            base: base.trim_end_matches('/').to_owned(),
            models,
            cast: false,
        }
    }

    /// The server has the voice pack: cloning models speak as the cast.
    pub fn with_cast(mut self, cast: bool) -> SpeechClient {
        self.cast = cast;
        self
    }

    /// The model that says a line in this language: an expressive (graphics card) one first, else
    /// the natural pack of that language, else the first one. A model that can only clone needs the
    /// voice pack.
    pub fn model_for(&self, language: &str) -> &'static SenseModel {
        let usable = |m: &&&SenseModel| m.speaks(language) && (self.cast || !m.voices.is_empty());
        self.models
            .iter()
            .filter(usable)
            .find(|m| m.needs.vram_mb > 0)
            .or_else(|| self.models.iter().find(usable))
            .unwrap_or(&self.models[0])
    }

    /// The request body for a line.
    pub fn request(&self, speak: &Speak) -> (Value, &'static SenseModel, f32) {
        let model = self.model_for(&speak.language);
        let mut body = json!({
            "model": model.id,
            "input": speak.text,
            "response_format": "pcm",
            "stream_format": "sse",
        });
        let built_in = speak
            .voice
            .as_deref()
            .filter(|voice| model.voices.iter().any(|v| v.id == *voice));
        if let Some(voice) = built_in {
            body["voice"] = voice.into();
        } else if model.clone && self.cast {
            // The cast: the clip of the line's emotion carries the delivery.
            let character = speak
                .voice
                .as_deref()
                .and_then(cast::character)
                .unwrap_or(&cast::CAST[0]);
            let clip = character.reference(&speak.language, cast::Emotion::for_tone(&speak.tone));
            if clip.language == speak.language || !is_qwen3(model) {
                body["voice"] = clip.name.into();
            } else {
                // No clips in this language: Qwen3-TTS Base copies the clip's accent and words into
                // the line (32% word errors for French said from the English clips, 1% from the
                // French ones), so it takes only the speaker embedding of the neutral clip (0%
                // errors, the same voice, the delivery flat: the tone gain still applies).
                body["voice"] = character
                    .reference("en", cast::Emotion::Neutral)
                    .name
                    .into();
                body["options"] = json!({ "x_vector_only_mode": true });
            }
        }
        let gain = speak.tone.playback_gain();
        if model.languages.len() > 1 {
            body["language"] = speak.language.as_str().into();
        }
        if model.instruct
            && let Some(instruction) = speak.tone.instruction()
        {
            body["options"] = json!({ "instruct": instruction });
        }
        (body, model, gain)
    }

    /// Get a character's clips in `language` ready: a cloning model encodes a clip the first time a
    /// line is said from it (0.6–0.7 s on an M4 Pro GPU, which delays that line's first sound) and
    /// keeps it prepared afterwards. Call it on a worker thread when the companion's voice is chosen
    /// or the game's language changes. Nothing to do on a model that doesn't clone.
    pub fn prepare(&self, character: &str, language: &str) -> io::Result<()> {
        let model = self.model_for(language);
        if !(model.clone && self.cast) {
            return Ok(());
        }
        for emotion in cast::Emotion::ALL {
            self.speak_blocking(
                &Speak {
                    text: "Ok.".into(),
                    voice: Some(character.into()),
                    tone: emotion.tone(),
                    language: language.into(),
                },
                |_| {},
            )?;
        }
        Ok(())
    }

    /// Say a line on a worker thread; the samples arrive on the channel as they are generated.
    pub fn speak(&self, speak: Speak) -> mpsc::Receiver<SpeechEvent> {
        let (send, receive) = mpsc::channel();
        let client = self.clone();
        let spawned = std::thread::Builder::new()
            .name("companion voice".into())
            .spawn(move || {
                let audio = send.clone();
                let result = client.speak_blocking(&speak, |samples| {
                    let _ = audio.send(SpeechEvent::Audio(samples.to_vec()));
                });
                let _ = send.send(match result {
                    Ok(stats) => SpeechEvent::Done(stats),
                    Err(e) => SpeechEvent::Failed(e.to_string()),
                });
            });
        if let Err(e) = spawned {
            let (send, receive) = mpsc::channel();
            let _ = send.send(SpeechEvent::Failed(e.to_string()));
            return receive;
        }
        receive
    }

    /// Say a line on this thread, handing each chunk to `on_audio` as it arrives.
    pub fn speak_blocking(
        &self,
        speak: &Speak,
        mut on_audio: impl FnMut(&[f32]),
    ) -> io::Result<SpeechStats> {
        let (body, model, gain) = self.request(speak);
        let start = Instant::now();
        let response = self
            .agent
            .post(&format!("{}/v1/audio/speech", self.base))
            .set("Accept", "text/event-stream")
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(http_error)?;
        let mut stats = SpeechStats {
            sample_rate: model.sample_rate,
            gain,
            ..SpeechStats::default()
        };
        let mut done = false;
        for event in sse_events(BufReader::new(response.into_reader())) {
            let event = event?;
            match event.get("type").and_then(Value::as_str) {
                Some("speech.audio.delta") => {
                    let Some(audio) = event.get("audio").and_then(Value::as_str) else {
                        continue;
                    };
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(audio)
                        .map_err(io::Error::other)?;
                    let samples = crate::dsp::pcm16_to_f32(&bytes);
                    if samples.is_empty() {
                        continue;
                    }
                    stats.first_audio.get_or_insert_with(|| start.elapsed());
                    stats.samples += samples.len();
                    on_audio(&samples);
                }
                Some("speech.audio.done") => done = true,
                _ => {
                    if let Some(error) = event.get("error") {
                        return Err(io::Error::other(format!("voice: {error}")));
                    }
                }
            }
        }
        if !done {
            return Err(io::Error::other(
                "voice: the stream ended before its last event",
            ));
        }
        stats.total = start.elapsed();
        Ok(stats)
    }
}

fn is_qwen3(model: &SenseModel) -> bool {
    matches!(
        model.engine,
        Engine::AudioCpp {
            family: "qwen3_tts"
        }
    )
}

/// The JSON of each `data:` line of a server-sent event stream, up to `[DONE]`.
fn sse_events(reader: impl BufRead) -> impl Iterator<Item = io::Result<Value>> {
    reader
        .lines()
        .filter_map(|line| match line {
            Err(e) => Some(Some(Err(e))),
            Ok(line) => {
                let data = line.strip_prefix("data:")?.trim();
                if data == "[DONE]" {
                    return Some(None);
                }
                Some(Some(serde_json::from_str(data).map_err(io::Error::other)))
            }
        })
        .map_while(|event| event)
}

fn http_error(e: ureq::Error) -> io::Error {
    match e {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            io::Error::other(format!("HTTP {code}: {}", body.trim()))
        }
        ureq::Error::Transport(t) => io::Error::other(t.to_string()),
    }
}

/// What the player said.
#[derive(Clone, Debug, PartialEq)]
pub struct Transcript {
    pub text: String,
    /// ISO 639-1, when the model reports it: Canary repeats the language it was asked for,
    /// Parakeet reports none (the brain reads the language from the text).
    pub language: Option<String>,
    /// From sending the clip to the text.
    pub took: Duration,
}

/// Push-to-talk speech to text: the whole clip, sent when the key is released.
#[derive(Clone)]
pub struct RecognitionClient {
    agent: ureq::Agent,
    base: String,
    model: &'static SenseModel,
}

impl RecognitionClient {
    pub fn new(base: &str, model: &'static SenseModel) -> RecognitionClient {
        RecognitionClient {
            agent: agent(),
            base: base.trim_end_matches('/').to_owned(),
            model,
        }
    }

    pub fn model(&self) -> &'static SenseModel {
        self.model
    }

    /// Transcribe 16 kHz mono samples. `language` (ISO 639-1, the game's language) is what the
    /// player most likely speaks: Canary needs it or it translates to English, Parakeet finds the
    /// language itself and ignores it.
    pub fn transcribe(&self, samples: &[f32], language: Option<&str>) -> io::Result<Transcript> {
        let wav = crate::dsp::wav_pcm16(samples, 16000);
        let boundary = "horizon-senses-7f3a9c1e5b";
        let mut body = Vec::with_capacity(wav.len() + 512);
        let field = |body: &mut Vec<u8>, name: &str, value: &str| {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        };
        field(&mut body, "model", self.model.id);
        if let Some(language) = language.filter(|l| self.model.speaks(l)) {
            field(&mut body, "language", language);
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&wav);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let start = Instant::now();
        let response: Value = self
            .agent
            .post(&format!("{}/v1/audio/transcriptions/details", self.base))
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send_bytes(&body)
            .map(|r| serde_json::from_reader(r.into_reader()))
            .map_err(http_error)?
            .map_err(io::Error::other)?;
        Ok(parse_transcript(&response, start.elapsed()))
    }
}

fn parse_transcript(response: &Value, took: Duration) -> Transcript {
    Transcript {
        text: response
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned(),
        language: response
            .get("language")
            .and_then(Value::as_str)
            .filter(|l| !l.is_empty())
            .map(|l| l.get(..2).unwrap_or(l).to_ascii_lowercase()),
        took,
    }
}

/// The folder a model is installed in under `<data>/companion/models/`.
pub fn model_dir(models: &Path, model: &SenseModel) -> PathBuf {
    models.join(model.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::{model, pocket_for};

    fn setup() -> VoiceSetup {
        let models = Path::new("/data/companion/models");
        let installed = |m: &'static SenseModel| Installed {
            model: m,
            dir: model_dir(models, m),
        };
        VoiceSetup {
            runtime: "/data/companion/voice-runtime/v0.9.2-macos-aarch64-metal".into(),
            exe: "audiocpp_server",
            backend: Backend::Cpu,
            threads: 2,
            models: vec![
                installed(model("parakeet-tdt-0.6b-v3-q8_0").unwrap()),
                installed(pocket_for("fr_fr")),
                installed(model("qwen3-tts-12hz-1.7b-customvoice-q8_0").unwrap()),
            ],
            work: "/tmp/w".into(),
            voices: None,
        }
    }

    #[test]
    fn the_config_registers_each_model_with_its_task_and_mode() {
        let config = server_config(&setup());
        assert_eq!(config["host"], "127.0.0.1");
        assert_eq!(config["backend"], "cpu");
        assert_eq!(config["threads"], 2);
        let models = config["models"].as_array().unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0]["task"], "asr");
        assert_eq!(models[0]["mode"], "offline");
        assert_eq!(models[0]["family"], "parakeet_tdt");
        assert_eq!(models[1]["task"], "tts");
        assert_eq!(models[1]["mode"], "streaming");
        assert_eq!(models[1]["load_options"]["language"], "french");
        assert_eq!(models[1]["default_voice_preset"]["voice_id"], "alba");
        assert_eq!(
            models[1]["path"],
            "/data/companion/models/pocket-tts-fr-q8_0/pocket-tts-french-q8_0.gguf"
        );
        assert!(models[2].get("load_options").is_none());
    }

    #[test]
    fn expressive_models_get_the_tone_as_an_instruction_and_every_voice_as_gain() {
        let m = setup();
        let client = SpeechClient::new(
            "http://127.0.0.1:1/",
            m.models.iter().skip(1).map(|i| i.model).collect(),
        );
        let line = |language: &str, tone: Tone| Speak {
            text: "Creeper!".into(),
            voice: Some("ryan".into()),
            tone,
            language: language.into(),
        };
        let (body, model, gain) = client.request(&line("fr", Tone::Shout));
        assert!(model.instruct);
        assert_eq!(
            body["options"]["instruct"],
            "Shouting loudly, urgent and intense."
        );
        assert_eq!(body["language"], "fr");
        assert_eq!(body["voice"], "ryan");
        assert!(gain > 1.0);
        let natural = SpeechClient::new("http://x", vec![pocket_for("fr_fr")]);
        let (body, model, gain) = natural.request(&line("fr", Tone::Whisper));
        assert!(!model.instruct);
        assert!(body.get("options").is_none() && body.get("language").is_none());
        // A voice the model doesn't have is left to its default.
        assert!(body.get("voice").is_none());
        assert!(gain < 0.5);
        assert_eq!(natural.model_for("de").id, "pocket-tts-fr-q8_0");
        assert_eq!(Tone::parse("Whispering"), Tone::Whisper);
        assert_eq!(Tone::parse("sleepy"), Tone::Other("sleepy".into()));
    }

    #[test]
    fn the_voice_pack_is_the_voice_library_and_cloning_models_keep_a_character_prepared() {
        let mut m = setup();
        m.voices = Some("/data/companion/voices/v1".into());
        m.models.push(Installed {
            model: model("qwen3-tts-12hz-1.7b-base-q8_0").unwrap(),
            dir: "/m/b".into(),
        });
        let config = server_config(&m);
        assert_eq!(config["voice_dir"], "/data/companion/voices/v1");
        let models = config["models"].as_array().unwrap();
        assert_eq!(
            models[1]["session_options"]["pocket_tts.voice_state_cache_slots"],
            10
        );
        assert!(models[2].get("session_options").is_none(), "CustomVoice");
        assert_eq!(
            models[3]["session_options"]["qwen3_tts.voice_prompt_cache_slots"],
            10
        );
        assert!(server_config(&setup()).get("voice_dir").is_none());
    }

    #[test]
    fn cloning_models_say_each_line_from_the_clip_of_its_tone() {
        let base = model("qwen3-tts-12hz-1.7b-base-q8_0").unwrap();
        let pocket = pocket_for("fr_fr");
        let client = SpeechClient::new("http://x", vec![base, pocket]).with_cast(true);
        let line = |voice: Option<&str>, language: &str, tone: Tone| Speak {
            text: "Creeper !".into(),
            voice: voice.map(Into::into),
            tone,
            language: language.into(),
        };
        let (body, m, gain) = client.request(&line(Some("knight"), "fr", Tone::Shout));
        assert_eq!(m.id, base.id, "the graphics card model first");
        assert_eq!(body["voice"], "hz-knight-fr-shout");
        assert_eq!(body["language"], "fr");
        assert!(body.get("options").is_none(), "Base takes no instruction");
        assert!(gain > 1.0);
        // Spanish has no clips of its own: the voice of the English neutral clip, without its
        // accent.
        let (body, _, _) = client.request(&line(Some("knight"), "es", Tone::Whisper));
        assert_eq!(body["voice"], "hz-knight-en-neutral");
        assert_eq!(body["options"]["x_vector_only_mode"], true);
        // No voice, or one the cast doesn't have: the first character.
        let (body, _, _) = client.request(&line(None, "en", Tone::Laugh));
        assert_eq!(body["voice"], "hz-veteran-en-laugh");
        // The natural voice clones the cast too, and keeps its built-in voices.
        let natural = SpeechClient::new("http://x", vec![pocket]).with_cast(true);
        let (body, m, _) = natural.request(&line(Some("nana"), "fr", Tone::Sarcastic));
        assert_eq!(m.id, pocket.id);
        assert_eq!(body["voice"], "hz-nana-fr-sarcastic");
        assert!(body.get("language").is_none(), "one language per pack");
        let (body, _, _) = natural.request(&line(Some("alba"), "fr", Tone::Shout));
        assert_eq!(body["voice"], "alba");
        // Without the voice pack: no cloning, and Base (which can only clone) is never picked.
        let bare = SpeechClient::new("http://x", vec![base, pocket]);
        let (body, m, _) = bare.request(&line(Some("knight"), "fr", Tone::Shout));
        assert_eq!(m.id, pocket.id);
        assert!(body.get("voice").is_none());
    }

    #[test]
    fn server_sent_events_parse_until_done() {
        let stream = "data: {\"type\":\"speech.audio.delta\",\"audio\":\"AAA=\"}\n\n: comment\ndata: {\"type\":\"speech.audio.done\"}\n\ndata: [DONE]\n\ndata: {\"never\":1}\n";
        let events: Vec<Value> = sse_events(stream.as_bytes()).map(Result::unwrap).collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1]["type"], "speech.audio.done");
    }

    #[test]
    fn transcripts_trim_and_shorten_the_language() {
        let t = parse_transcript(
            &json!({"text": " suis-moi ", "language": "fr-FR"}),
            Duration::ZERO,
        );
        assert_eq!(t.text, "suis-moi");
        assert_eq!(t.language.as_deref(), Some("fr"));
        let t = parse_transcript(&json!({"text": "hi"}), Duration::ZERO);
        assert_eq!(t.language, None);
    }

    /// A stand-in server: answers one request with `reply` and hands back what it received.
    fn one_shot(reply: String) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_millis(300)))
                .unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 65536];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            stream.write_all(reply.as_bytes()).unwrap();
            got
        });
        (base, handle)
    }

    #[test]
    fn speech_streams_chunks_then_reports_its_timing() {
        let chunk = base64::engine::general_purpose::STANDARD.encode([0u8, 0x40, 0, 0xc0]);
        let body = format!(
            "data: {{\"type\":\"speech.audio.delta\",\"audio\":\"{chunk}\"}}\n\ndata: {{\"type\":\"speech.audio.delta\",\"audio\":\"{chunk}\"}}\n\ndata: {{\"type\":\"speech.audio.done\"}}\n\ndata: [DONE]\n\n"
        );
        let (base, server) = one_shot(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
        ));
        let client = SpeechClient::new(&base, vec![pocket_for("en_us")]);
        let events: Vec<SpeechEvent> = client
            .speak(Speak {
                text: "Hello".into(),
                voice: None,
                tone: Tone::Normal,
                language: "en".into(),
            })
            .iter()
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], SpeechEvent::Audio(vec![0.5, -0.5]));
        let SpeechEvent::Done(stats) = &events[2] else {
            panic!("{events:?}");
        };
        assert_eq!(stats.samples, 4);
        assert_eq!(stats.sample_rate, 24000);
        assert!(stats.first_audio.is_some());
        let request = String::from_utf8_lossy(&server.join().unwrap()).into_owned();
        assert!(request.starts_with("POST /v1/audio/speech"));
        assert!(
            request.contains("\"model\":\"pocket-tts-en-q8_0\""),
            "{request}"
        );
    }

    #[test]
    fn a_failed_stream_says_why() {
        let (base, _server) = one_shot(
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\ndata: {\"type\":\"error\",\"error\":{\"message\":\"model missing\"}}\n\n".into(),
        );
        let client = SpeechClient::new(&base, vec![pocket_for("en_us")]);
        let last = client
            .speak(Speak {
                text: "Hello".into(),
                voice: None,
                tone: Tone::Normal,
                language: "en".into(),
            })
            .iter()
            .last()
            .unwrap();
        let SpeechEvent::Failed(why) = last else {
            panic!("{last:?}");
        };
        assert!(why.contains("model missing"), "{why}");
    }

    #[test]
    fn recognition_uploads_a_16_khz_wav() {
        let reply = r#"{"text":"follow me","language":"en","timing":{}}"#;
        let (base, server) = one_shot(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        ));
        let client = RecognitionClient::new(&base, model("parakeet-tdt-0.6b-v3-q8_0").unwrap());
        let t = client.transcribe(&vec![0.25; 1600], Some("fr")).unwrap();
        assert_eq!(t.text, "follow me");
        assert_eq!(t.language.as_deref(), Some("en"));
        let request = server.join().unwrap();
        let text = String::from_utf8_lossy(&request);
        assert!(text.starts_with("POST /v1/audio/transcriptions/details"));
        assert!(text.contains("name=\"model\"\r\n\r\nparakeet-tdt-0.6b-v3-q8_0"));
        assert!(text.contains("name=\"language\"\r\n\r\nfr"));
        let riff = request.windows(4).position(|w| w == b"RIFF").unwrap();
        // 16 000 Hz in the WAV header.
        assert_eq!(&request[riff + 24..riff + 28], &16000u32.to_le_bytes());
    }
}
