//! The companion's voice in the game (docs/companions.md, "Voice"): push-to-talk in (the microphone,
//! opened only while the key is held and resampled to 16 kHz, then speech to text on a worker thread)
//! and its lines out (speech streamed into the mixer from where the companion stands). Nothing here
//! exists until voice is turned on: the game holds none of it otherwise.
use crate::audio::{Audio, VoiceId};
use horizon_companion::senses::SenseModel;
use horizon_senses::{
    dsp::To16k,
    voice::{RecognitionClient, Speak, SpeechClient, SpeechEvent, Transcript},
};
use std::{
    io,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

/// The longest clip: held longer, push-to-talk lets go by itself. Parakeet's memory grows with the
/// clip (2.6 GB resident after 3.5 s, 3.3 GB after 14 s on the Mac mini).
pub const MAX_CLIP: Duration = Duration::from_secs(15);
/// Shorter than this is a tap of the key, not speech: nothing is sent.
pub const MIN_CLIP: Duration = Duration::from_millis(300);

/// What push-to-talk understood.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// The words, and their language when the model says (ISO 639-1).
    Said {
        text: String,
        language: Option<String>,
    },
    /// Too short, or nothing recognisable in it.
    Nothing,
    Failed(String),
}

/// What the microphone callback fills.
#[derive(Default)]
struct Capture {
    resampler: Option<To16k>,
    samples: Vec<f32>,
    /// The loudest sample of the last callback, for the HUD's level.
    level: f32,
}

impl Capture {
    #[cfg_attr(not(feature = "audio"), allow(dead_code))]
    fn push(&mut self, interleaved: &[f32]) {
        self.level = interleaved.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let max = (MAX_CLIP.as_secs_f32() * To16k::RATE as f32) as usize;
        if self.samples.len() < max
            && let Some(r) = self.resampler.as_mut()
        {
            r.push(interleaved, &mut self.samples);
        }
    }
}

/// The open microphone: the default input device in its own format, mixed to mono 16 kHz.
pub struct Mic {
    #[cfg(feature = "audio")]
    _stream: cpal::Stream,
    capture: Arc<Mutex<Capture>>,
}

impl Mic {
    /// Open the default input. On macOS the first time asks the player for permission (the app's
    /// `NSMicrophoneUsageDescription`, packaging/macos/Info.plist).
    #[cfg(feature = "audio")]
    pub fn open() -> Result<Mic, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host()
            .default_input_device()
            .ok_or("no microphone")?;
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let capture = Arc::new(Mutex::new(Capture {
            resampler: Some(To16k::new(config.sample_rate().0, config.channels().into())),
            ..Capture::default()
        }));
        let stream = build_input(&device, &config, capture.clone())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Mic {
            _stream: stream,
            capture,
        })
    }

    #[cfg(not(feature = "audio"))]
    pub fn open() -> Result<Mic, String> {
        Err("this build has no sound (built without the `audio` feature)".into())
    }

    /// How loud the player is, 0..1.
    pub fn level(&self) -> f32 {
        self.capture.lock().map_or(0.0, |c| c.level)
    }

    /// Close the device and take the clip (16 kHz mono).
    pub fn close(self) -> Vec<f32> {
        let capture = self.capture.clone();
        drop(self);
        capture
            .lock()
            .map(|mut c| std::mem::take(&mut c.samples))
            .unwrap_or_default()
    }
}

#[cfg(feature = "audio")]
fn build_input(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    capture: Arc<Mutex<Capture>>,
) -> Result<cpal::Stream, String> {
    use cpal::SampleFormat as F;
    fn stream<T: cpal::SizedSample>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        capture: Arc<Mutex<Capture>>,
    ) -> Result<cpal::Stream, String>
    where
        f32: cpal::FromSample<T>,
    {
        use cpal::traits::DeviceTrait;
        let mut scratch = Vec::new();
        device
            .build_input_stream(
                &config.config(),
                move |data: &[T], _| {
                    scratch.clear();
                    scratch.extend(data.iter().map(|s| s.to_sample::<f32>()));
                    if let Ok(mut c) = capture.lock() {
                        c.push(&scratch);
                    }
                },
                |e| log::warn!("Horizon: microphone error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())
    }
    match config.sample_format() {
        F::F32 => stream::<f32>(device, config, capture),
        F::I16 => stream::<i16>(device, config, capture),
        F::I32 => stream::<i32>(device, config, capture),
        F::U16 => stream::<u16>(device, config, capture),
        F::U8 => stream::<u8>(device, config, capture),
        F::I8 => stream::<i8>(device, config, capture),
        F::F64 => stream::<f64>(device, config, capture),
        other => Err(format!("unsupported microphone format {other}")),
    }
}

/// Push-to-talk: `press` opens the microphone, `release` closes it and sends the clip to speech to
/// text on a worker thread, `poll` (each tick) brings the result.
pub struct PushToTalk {
    recognition: RecognitionClient,
    /// The game's language (ISO 639-1): a hint for the models that need one.
    pub language: Option<String>,
    mic: Option<(Mic, Instant)>,
    pending: Option<mpsc::Receiver<io::Result<Transcript>>>,
}

impl PushToTalk {
    pub fn new(recognition: RecognitionClient, language: Option<String>) -> PushToTalk {
        PushToTalk {
            recognition,
            language,
            mic: None,
            pending: None,
        }
    }

    pub fn model(&self) -> &'static SenseModel {
        self.recognition.model()
    }

    /// The key went down: open the microphone.
    pub fn press(&mut self) -> Result<(), String> {
        if self.mic.is_none() {
            self.mic = Some((Mic::open()?, Instant::now()));
        }
        Ok(())
    }

    /// The key came up (or the clip is full): close the microphone and transcribe what it heard.
    /// `None` when the clip was too short to send.
    pub fn release(&mut self) -> Option<Heard> {
        let (mic, opened) = self.mic.take()?;
        let held = opened.elapsed();
        let clip = mic.close();
        if held < MIN_CLIP || clip.len() < (MIN_CLIP.as_secs_f32() * To16k::RATE as f32) as usize {
            return Some(Heard::Nothing);
        }
        self.transcribe(clip);
        None
    }

    /// Send a 16 kHz clip to speech to text on a worker thread.
    pub fn transcribe(&mut self, clip: Vec<f32>) {
        let (send, receive) = mpsc::channel();
        let recognition = self.recognition.clone();
        let language = self.language.clone();
        let spawned = std::thread::Builder::new()
            .name("companion ears".into())
            .spawn(move || {
                let _ = send.send(recognition.transcribe(&clip, language.as_deref()));
            });
        if spawned.is_ok() {
            self.pending = Some(receive);
        }
    }

    /// Each tick: lets go of a clip that reached `MAX_CLIP`, and brings what speech to text heard.
    pub fn poll(&mut self) -> Option<Heard> {
        if self
            .mic
            .as_ref()
            .is_some_and(|(_, opened)| opened.elapsed() >= MAX_CLIP)
            && let Some(heard) = self.release()
        {
            return Some(heard);
        }
        let result = match self.pending.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err(io::Error::other("speech to text stopped"))
            }
        };
        self.pending = None;
        Some(match result {
            Ok(t) if t.text.chars().any(char::is_alphanumeric) => Heard::Said {
                text: t.text,
                language: t.language,
            },
            Ok(_) => Heard::Nothing,
            Err(e) => Heard::Failed(e.to_string()),
        })
    }

    /// The microphone is open.
    pub fn listening(&self) -> bool {
        self.mic.is_some()
    }

    /// The microphone's level while it is open.
    pub fn level(&self) -> f32 {
        self.mic.as_ref().map_or(0.0, |(m, _)| m.level())
    }

    /// A clip is being transcribed.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

/// A line waiting for its turn or being generated.
struct Line {
    speak: Speak,
    events: Option<mpsc::Receiver<SpeechEvent>>,
    gain: f32,
    /// Its subtitle went up (with its first sound).
    shown: bool,
    /// What it sounded like, kept for the WAV tap.
    recorded: Vec<f32>,
}

/// One companion talking: its lines are generated one after the other into one stream, so the
/// sentences of a reply follow without gaps.
struct Talker {
    name: String,
    lines: std::collections::VecDeque<Line>,
    voice: Option<VoiceId>,
    at: [f64; 3],
    elsewhere: bool,
}

/// The companions' voices: `say` queues a line for a speaker, `tick` moves what the speech model
/// generated into the mixer and keeps each voice where its speaker is.
pub struct Speakers {
    client: SpeechClient,
    talkers: Vec<Talker>,
    /// `HORIZON_COMPANION_VOICE_WAV=<folder>` (tests and demos): every line is also written there
    /// as `<n>-<speaker>.wav`, so a voice can be checked without playing it out loud.
    tap: Option<(std::path::PathBuf, u32)>,
}

/// Lines a speaker may have waiting for their turn.
const MAX_WAITING: usize = 4;

/// The longest subtitle; the whole line is in the chat.
const SUBTITLE_CHARS: usize = 48;

impl Speakers {
    pub fn new(client: SpeechClient) -> Speakers {
        // Numbered after the session's start, so a second run doesn't overwrite the first.
        let start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() % 100_000);
        let tap = std::env::var_os("HORIZON_COMPANION_VOICE_WAV")
            .map(|dir| (dir.into(), start as u32 * 1000));
        Speakers {
            client,
            talkers: Vec::new(),
            tap,
        }
    }

    /// Queue a line for `speaker` (a sentence: the brain's reply comes sentence by sentence).
    pub fn say(&mut self, speaker: &str, speak: Speak) {
        let gain = self.client.request(&speak).2;
        let line = Line {
            speak,
            events: None,
            gain,
            shown: false,
            recorded: Vec::new(),
        };
        match self.talkers.iter_mut().find(|t| t.name == speaker) {
            Some(t) => {
                // A line waiting behind too many others is stale by the time its turn comes (a run of
                // quick orders): the oldest not yet started goes; the chat still has it.
                let waiting = t.lines.iter().filter(|l| l.events.is_none()).count();
                if waiting >= MAX_WAITING
                    && let Some(i) = t.lines.iter().position(|l| l.events.is_none())
                {
                    t.lines.remove(i);
                }
                t.lines.push_back(line);
            }
            None => self.talkers.push(Talker {
                name: speaker.to_owned(),
                lines: [line].into(),
                voice: None,
                at: [0.0; 3],
                elsewhere: true,
            }),
        }
    }

    /// Where a speaker is: its position, and whether it is in another dimension than the player.
    pub fn place(&mut self, speaker: &str, at: [f64; 3], elsewhere: bool) {
        if let Some(t) = self.talkers.iter_mut().find(|t| t.name == speaker) {
            t.at = at;
            t.elsewhere = elsewhere;
        }
    }

    /// Whether `speaker` has something left to say or play.
    pub fn talking(&self, speaker: &str, audio: &Audio) -> bool {
        self.talkers.iter().any(|t| {
            t.name == speaker
                && (!t.lines.is_empty() || t.voice.is_some_and(|v| audio.voice_playing(v)))
        })
    }

    /// Silence a speaker (its lines still generating are dropped).
    pub fn hush(&mut self, speaker: &str, audio: &mut Audio) {
        self.talkers.retain(|t| {
            if t.name != speaker {
                return true;
            }
            if let Some(v) = t.voice {
                audio.voice_stop(v);
            }
            false
        });
    }

    /// Each tick: start the next line of each speaker, move generated samples into the mixer, place
    /// the voices, and forget speakers that are done. Returns the errors of failed lines.
    pub fn tick(&mut self, audio: &mut Audio) -> Vec<String> {
        let mut failed = Vec::new();
        let client = &self.client;
        let tap = &mut self.tap;
        self.talkers.retain_mut(|t| {
            if let Some(v) = t.voice {
                audio.voice_place(v, t.at, t.elsewhere);
            }
            while let Some(line) = t.lines.front_mut() {
                let events = line
                    .events
                    .get_or_insert_with(|| client.speak(line.speak.clone()));
                let mut finished = false;
                for event in events.try_iter() {
                    match event {
                        SpeechEvent::Audio(mut samples) => {
                            let rate = client.model_for(&line.speak.language).sample_rate;
                            let voice = match t.voice.filter(|v| audio.voice_playing(*v)) {
                                Some(v) => Some(v),
                                None => audio.voice_start(rate, 1.0, t.at, t.elsewhere),
                            };
                            t.voice = voice;
                            if !line.shown {
                                line.shown = true;
                                let subtitle = subtitle(&t.name, &line.speak.text);
                                audio.voice_subtitle(&subtitle, (!t.elsewhere).then_some(t.at));
                            }
                            samples.iter_mut().for_each(|s| *s *= line.gain);
                            if tap.is_some() {
                                line.recorded.extend_from_slice(&samples);
                            }
                            if let Some(v) = voice {
                                audio.voice_push(v, &samples);
                            }
                        }
                        SpeechEvent::Done(_) => {
                            finished = true;
                            if let Some((dir, n)) = tap.as_mut() {
                                *n += 1;
                                let rate = client.model_for(&line.speak.language).sample_rate;
                                let path = dir.join(format!("{n:08}-{}.wav", t.name));
                                let wav = horizon_senses::dsp::wav_pcm16(&line.recorded, rate);
                                if let Err(e) = std::fs::create_dir_all(dir)
                                    .and_then(|()| std::fs::write(&path, wav))
                                {
                                    failed.push(format!("{}: {e}", path.display()));
                                }
                            }
                        }
                        SpeechEvent::Failed(why) => {
                            failed.push(why);
                            finished = true;
                        }
                    }
                }
                if !finished {
                    break;
                }
                t.lines.pop_front();
            }
            if t.lines.is_empty() {
                match t.voice {
                    Some(v) if audio.voice_playing(v) => {
                        audio.voice_finish(v);
                        true
                    }
                    _ => false,
                }
            } else {
                true
            }
        });
        failed
    }
}

/// "Name: the line", cut to `SUBTITLE_CHARS`.
fn subtitle(name: &str, text: &str) -> String {
    let line = format!("{name}: {}", text.trim());
    if line.chars().count() <= SUBTITLE_CHARS {
        return line;
    }
    let cut: String = line.chars().take(SUBTITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_companion::senses::model;
    use std::io::{Read, Write};

    /// A stand-in server answering one request with `reply`.
    fn one_shot(reply: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = vec![0; 1 << 20];
            let mut got = Vec::new();
            // Read the headers and the body (Content-Length), then answer.
            loop {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if got.len() >= end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            s.write_all(reply.as_bytes()).unwrap();
        });
        base
    }

    fn json_reply(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn wait(ptt: &mut PushToTalk) -> Heard {
        let start = Instant::now();
        loop {
            if let Some(h) = ptt.poll() {
                return h;
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_clip_comes_back_as_words_or_nothing() {
        let parakeet = model("parakeet-tdt-0.6b-v3-q8_0").unwrap();
        let base = one_shot(json_reply(r#"{"text":"suis-moi","timing":{}}"#));
        let mut ptt = PushToTalk::new(RecognitionClient::new(&base, parakeet), Some("fr".into()));
        assert!(!ptt.listening() && !ptt.busy());
        ptt.transcribe(vec![0.1; 16000]);
        assert!(ptt.busy());
        assert_eq!(
            wait(&mut ptt),
            Heard::Said {
                text: "suis-moi".into(),
                language: None
            }
        );
        assert!(!ptt.busy());
        // Canary returns nothing for some clips: not a line.
        let base = one_shot(json_reply(r#"{"text":" . ","language":"es"}"#));
        let mut ptt = PushToTalk::new(RecognitionClient::new(&base, parakeet), None);
        ptt.transcribe(vec![0.1; 16000]);
        assert_eq!(wait(&mut ptt), Heard::Nothing);
        // A server that is gone.
        let mut ptt = PushToTalk::new(RecognitionClient::new("http://127.0.0.1:1", parakeet), None);
        ptt.transcribe(vec![0.1; 16000]);
        assert!(matches!(wait(&mut ptt), Heard::Failed(_)));
    }

    #[test]
    fn the_capture_resamples_to_16_khz_and_stops_at_the_longest_clip() {
        let mut c = Capture {
            resampler: Some(To16k::new(48000, 2)),
            ..Capture::default()
        };
        c.push(&vec![0.5; 48000 * 2]);
        assert!(
            (c.samples.len() as i32 - 16000).abs() < 10,
            "{}",
            c.samples.len()
        );
        assert_eq!(c.level, 0.5);
        for _ in 0..20 {
            c.push(&vec![0.0; 48000 * 2]);
        }
        let max = (MAX_CLIP.as_secs_f32() * 16000.0) as usize;
        assert!(c.samples.len() <= max + 16000, "{}", c.samples.len());
    }

    #[test]
    fn subtitles_name_the_speaker_and_stay_short() {
        assert_eq!(subtitle("Kira", " Creeper! "), "Kira: Creeper!");
        let long = subtitle("Kira", &"word ".repeat(30));
        assert_eq!(long.chars().count(), SUBTITLE_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn a_speakers_lines_play_one_after_the_other_in_one_stream() {
        // Two lines of 0.25 s at 24 kHz from a stand-in speech server (one connection each).
        // 12 000 zero bytes in base64.
        let chunk = "A".repeat(16000);
        let sse = format!(
            "data: {{\"type\":\"speech.audio.delta\",\"audio\":\"{chunk}\"}}\n\ndata: {{\"type\":\"speech.audio.done\"}}\n\ndata: [DONE]\n\n"
        );
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
            sse.len()
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0; 4096];
                let _ = s.read(&mut buf).unwrap();
                s.write_all(reply.as_bytes()).unwrap();
            }
        });
        let pocket = model("pocket-tts-en-q8_0").unwrap();
        let mut speakers = Speakers::new(SpeechClient::new(&base, vec![pocket]));
        let line = |text: &str| Speak {
            text: text.into(),
            voice: None,
            tone: horizon_senses::voice::Tone::Shout,
            language: "en".into(),
        };
        speakers.say("Kira", line("One."));
        speakers.say("Kira", line("Two."));
        // A silent Audio has no output: the lines are generated, nothing is mixed.
        let mut audio = Audio::silent(std::path::Path::new("/nonexistent/horizon-test-assets"));
        let start = Instant::now();
        while speakers.talking("Kira", &audio) {
            assert!(speakers.tick(&mut audio).is_empty());
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        served.join().unwrap();
        speakers.say("Kira", line("Three."));
        speakers.hush("Kira", &mut audio);
        assert!(!speakers.talking("Kira", &audio));
    }
}
