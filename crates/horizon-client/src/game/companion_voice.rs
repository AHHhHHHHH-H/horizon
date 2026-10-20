//! The companion's voice in the game (docs/companions.md, "Voice"): the push-to-talk key, the mic
//! icon, what push-to-talk heard, and the lines companions say. All of it is `None` until voice is
//! turned on (`companion_voice_start`): without it the game opens no microphone, starts no sidecar
//! and spends one `Option` check a tick.
//!
//! `HORIZON_COMPANION_VOICE=<file.json>` turns it on for testing without the setup screens: the file
//! names the audio.cpp folder and the models (`{"runtime": "…", "backend": "cpu", "threads": 2,
//! "models": {"pocket-tts-en-q8_0": "<dir>", "parakeet-tdt-0.6b-v3-q8_0": "<dir>"}, "echo": true}`),
//! and `echo` has a companion repeat what it heard. `"voices": "<voice pack dir>"` lets cloning
//! models speak as the cast.
use super::{Game, Screen};
use horizon_client::{
    companion_voice::{Heard, PushToTalk, Speakers},
    hud::{HudBatch, rgba},
    keybinds,
    text::Text,
};
use horizon_companion::{catalogue::Backend, senses};
use horizon_senses::voice::{Installed, Speak, Tone, VoiceServer, VoiceSetup};
use std::{io, sync::mpsc, time::Instant};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub(super) struct VoiceState {
    on: Option<Box<VoiceOn>>,
    /// The sidecar starting on a worker thread (it loads the models before it answers).
    starting: Option<mpsc::Receiver<io::Result<VoiceServer>>>,
    echo: bool,
    /// `HORIZON_COMPANION_VOICE` was read.
    checked_env: bool,
}

struct VoiceOn {
    /// The audio.cpp sidecar; it stops with the game (`Sidecar` kills it when dropped, and its
    /// lifeline when the game dies).
    _server: VoiceServer,
    ptt: Option<PushToTalk>,
    speakers: Option<Speakers>,
    /// What push-to-talk heard, for the companion.
    heard: Vec<String>,
}

/// The speaker of `echo`.
const ECHO: &str = "Echo";

impl Game<'_> {
    /// Turn voice on: start the sidecar with these models on a worker thread; push-to-talk and the
    /// companions' voices work once it answers.
    pub(super) fn companion_voice_start(&mut self, setup: VoiceSetup, echo: bool) {
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("companion voice start".into())
            .spawn(move || {
                let started = VoiceServer::start(&setup).and_then(|server| {
                    server.warm_up()?;
                    Ok(server)
                });
                let _ = send.send(started);
            });
        if let Err(e) = spawned {
            log::warn!("Horizon: companion voice: {e}");
            return;
        }
        self.companion_voice.starting = Some(receive);
        self.companion_voice.echo = echo;
    }

    /// Voice off: the sidecar stops (dropping it kills it; one still starting stops once it has).
    pub(super) fn companion_voice_stop(&mut self) {
        if self.companion_voice.on.is_some() || self.companion_voice.starting.is_some() {
            log::info!("Horizon: companion voice stopped");
            keybinds::show_talk(false);
        }
        self.companion_voice.on = None;
        self.companion_voice.starting = None;
    }

    /// For `companion ai`: whether voice runs, and its models.
    pub(super) fn companion_voice_status(&self) -> serde_json::Value {
        let v = &self.companion_voice;
        match v.on.as_ref() {
            Some(on) => serde_json::json!({
                "on": true,
                "speech_to_text": on.ptt.as_ref().map(|p| p.model().id),
                "text_to_speech": on.speakers.is_some(),
            }),
            None => serde_json::json!({ "on": false, "starting": v.starting.is_some() }),
        }
    }

    /// Tests and demos: a WAV file goes through push-to-talk as if spoken into the microphone.
    pub(super) fn companion_voice_hear_wav(
        &mut self,
        path: &std::path::Path,
    ) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (samples, rate) =
            horizon_senses::dsp::read_wav_pcm16(&bytes).ok_or("not a 16-bit PCM WAV file")?;
        let mut clip = Vec::new();
        horizon_senses::dsp::To16k::new(rate, 1).push(&samples, &mut clip);
        let ptt = self
            .companion_voice
            .on
            .as_mut()
            .and_then(|on| on.ptt.as_mut())
            .ok_or("speech to text is not running")?;
        ptt.transcribe(clip);
        Ok(())
    }

    /// The game's language as ISO 639-1 (`fr_fr` → `fr`).
    fn voice_language(&self) -> String {
        self.settings
            .language
            .split('_')
            .next()
            .unwrap_or("en")
            .to_owned()
    }

    /// A key press or release: push-to-talk while playing with no text open. The key still goes
    /// on to the key binds (F3 + V stays).
    pub(super) fn companion_voice_key(&mut self, key: KeyCode, pressed: bool, repeat: bool) {
        let Some(ptt) = self
            .companion_voice
            .on
            .as_mut()
            .and_then(|on| on.ptt.as_mut())
        else {
            return;
        };
        if repeat
            || keybinds::key_name(key).is_none_or(|n| self.settings.keys.get(keybinds::TALK) != n)
        {
            return;
        }
        let playing = self.screen == Screen::Playing
            && self.title.is_none()
            && self.sign_edit.is_none()
            && self.book.is_none();
        if pressed && playing {
            if let Err(e) = ptt.press() {
                self.message = Some((format!("Microphone: {e}"), Instant::now()));
            }
        } else if !pressed && let Some(heard) = ptt.release() {
            self.voice_heard(heard);
        }
    }

    /// Once per tick: start voice when it is ready, let go of push-to-talk behind a screen, route
    /// what it heard, and play the companions' lines.
    pub(super) fn tick_companion_voice(&mut self) {
        if !self.companion_voice.checked_env {
            self.companion_voice.checked_env = true;
            if let Some(path) = std::env::var_os("HORIZON_COMPANION_VOICE") {
                match dev_setup(std::path::Path::new(&path)) {
                    Ok((setup, echo)) => self.companion_voice_start(setup, echo),
                    Err(e) => log::warn!("Horizon: HORIZON_COMPANION_VOICE: {e}"),
                }
            }
        }
        if let Some(starting) = &self.companion_voice.starting
            && let Ok(started) = starting.try_recv()
        {
            self.companion_voice.starting = None;
            match started {
                Ok(server) => {
                    log::info!(
                        "Horizon: companion voice ready on {} (speech to text: {}, voice: {})",
                        server.base_url(),
                        server.recognition().map_or("none", |r| r.model().id),
                        server.speech().map_or("none", |s| s.model_for("en").id),
                    );
                    let language = self.voice_language();
                    let ptt = server
                        .recognition()
                        .map(|r| PushToTalk::new(r, Some(language)));
                    keybinds::show_talk(ptt.is_some());
                    self.companion_voice.on = Some(Box::new(VoiceOn {
                        ptt,
                        speakers: server.speech().map(Speakers::new),
                        _server: server,
                        heard: Vec::new(),
                    }));
                }
                Err(e) => log::warn!("Horizon: companion voice did not start: {e}"),
            }
        }
        let Some(on) = self.companion_voice.on.as_mut() else {
            return;
        };
        let focused = self.focused || self.control.is_some();
        let playing = self.screen == Screen::Playing && self.title.is_none() && focused;
        let mut heard = None;
        if let Some(ptt) = on.ptt.as_mut() {
            if !playing && ptt.listening() {
                heard = ptt.release();
            }
            heard = heard.or_else(|| ptt.poll());
        }
        let failed = on
            .speakers
            .as_mut()
            .map(|s| s.tick(&mut self.audio))
            .unwrap_or_default();
        for why in failed {
            log::warn!("Horizon: companion voice: {why}");
        }
        if let Some(heard) = heard {
            self.voice_heard(heard);
        }
        if self.companion_voice.echo {
            let at = self.audio.listener;
            for text in self.companion_heard() {
                let speak = Speak {
                    text,
                    voice: None,
                    tone: Tone::Normal,
                    language: self.voice_language(),
                };
                self.companion_say(ECHO, speak);
            }
            self.companion_voice_place(ECHO, at, false);
        }
    }

    /// Push-to-talk's result: the words go to the chat as the player's line (so a mistake shows)
    /// and to the companion; nothing heard says so.
    fn voice_heard(&mut self, heard: Heard) {
        match heard {
            Heard::Said { text, .. } => {
                let line = Text::translate(
                    "chat.type.text",
                    &[
                        Text::literal(self.options.player_name()),
                        Text::literal(&text),
                    ],
                );
                self.system_message(line);
                if let Some(on) = self.companion_voice.on.as_mut() {
                    // Kept for the companion; the oldest go if nothing takes them.
                    if on.heard.len() >= 16 {
                        on.heard.remove(0);
                    }
                    on.heard.push(text);
                }
            }
            Heard::Nothing => {
                self.message = Some(("Didn't catch that".into(), Instant::now()));
            }
            Heard::Failed(e) => {
                log::warn!("Horizon: speech to text: {e}");
                self.message = Some((
                    "Couldn't hear you (speech to text failed)".into(),
                    Instant::now(),
                ));
            }
        }
    }

    /// What push-to-talk heard since the last call, oldest first: the companion's lines to answer.
    pub(super) fn companion_heard(&mut self) -> Vec<String> {
        self.companion_voice
            .on
            .as_mut()
            .map(|on| std::mem::take(&mut on.heard))
            .unwrap_or_default()
    }

    /// Have `speaker` say a line (a sentence of its reply), in its tone and the game's language.
    pub(super) fn companion_say(&mut self, speaker: &str, speak: Speak) {
        if let Some(s) = self
            .companion_voice
            .on
            .as_mut()
            .and_then(|on| on.speakers.as_mut())
        {
            s.say(speaker, speak);
        }
    }

    /// Where `speaker` is, each tick while it talks; `elsewhere` when it is in another dimension.
    pub(super) fn companion_voice_place(&mut self, speaker: &str, at: [f64; 3], elsewhere: bool) {
        if let Some(s) = self
            .companion_voice
            .on
            .as_mut()
            .and_then(|on| on.speakers.as_mut())
        {
            s.place(speaker, at, elsewhere);
        }
    }

    /// The mic icon at the bottom left while push-to-talk listens (with the level beside it), grey
    /// while what it heard is being transcribed.
    pub(super) fn draw_companion_mic(&self, batch: &mut HudBatch) {
        let Some(ptt) = self
            .companion_voice
            .on
            .as_ref()
            .and_then(|on| on.ptt.as_ref())
        else {
            return;
        };
        let (listening, busy) = (ptt.listening(), ptt.busy());
        if !listening && !busy {
            return;
        }
        let atlas = &self.gui.atlas;
        let (x, y) = (4.0, batch.height - 20.0);
        let color = if listening {
            rgba(255, 85, 85, 255)
        } else {
            rgba(170, 170, 170, 255)
        };
        batch.fill(atlas, x, y, 16.0, 16.0, rgba(0, 0, 0, 128));
        // The capsule, its cradle and the stand.
        batch.fill(atlas, x + 6.0, y + 2.0, 4.0, 7.0, color);
        batch.fill(atlas, x + 4.0, y + 7.0, 1.0, 3.0, color);
        batch.fill(atlas, x + 11.0, y + 7.0, 1.0, 3.0, color);
        batch.fill(atlas, x + 4.0, y + 10.0, 8.0, 1.0, color);
        batch.fill(atlas, x + 7.5, y + 11.0, 1.0, 2.0, color);
        batch.fill(atlas, x + 5.0, y + 13.0, 6.0, 1.0, color);
        if listening {
            let level = (ptt.level() * 2.0).clamp(0.0, 1.0) * 16.0;
            batch.fill(
                atlas,
                x + 18.0,
                y + 16.0 - level,
                2.0,
                level,
                rgba(85, 255, 85, 255),
            );
        }
    }
}

/// The `HORIZON_COMPANION_VOICE` file.
fn dev_setup(path: &std::path::Path) -> Result<(VoiceSetup, bool), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let runtime = v["runtime"].as_str().ok_or("no runtime folder")?;
    let backend = match v["backend"].as_str().unwrap_or("cpu") {
        "metal" => Backend::Metal,
        "vulkan" => Backend::Vulkan,
        "cuda" => Backend::Cuda,
        _ => Backend::Cpu,
    };
    let models = v["models"]
        .as_object()
        .ok_or("no models")?
        .iter()
        .map(|(id, dir)| {
            Ok(Installed {
                model: senses::model(id).ok_or(format!("unknown model {id}"))?,
                dir: dir.as_str().ok_or("a model folder is not a string")?.into(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let work = path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("voice-work");
    let exe = if cfg!(windows) {
        "audiocpp_server.exe"
    } else {
        "audiocpp_server"
    };
    Ok((
        VoiceSetup {
            runtime: runtime.into(),
            exe,
            backend,
            threads: v["threads"].as_u64().unwrap_or(2) as usize,
            models,
            work,
            voices: v["voices"].as_str().map(Into::into),
        },
        v["echo"].as_bool().unwrap_or(false),
    ))
}
