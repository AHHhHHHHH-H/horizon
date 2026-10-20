//! Sound playback: vanilla's `SoundEngine` bookkeeping (`sound_engine`) over a
//! small mixer standing in for OpenAL: one channel per source with its gain,
//! pitch, linear distance attenuation (`AL_LINEAR_DISTANCE`, reference 0,
//! rolloff 1), looping, relative flag and source state, Ogg/Vorbis decoding
//! (streams on a worker thread) and stereo panning. Companions' voices play
//! beside the sources as appendable streams (`voice_start`).
use crate::Result;
use crate::sound_engine::{
    Attenuation, Backend, Bound, ChannelId, InstanceId, PlayResult, SoundEngine, SoundInstance,
    Sounds, TickContext,
};
use crate::sound_events::{Sound, SoundRegistry};
#[cfg(feature = "audio")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use horizon_core::random::{LegacyRandom, RandomSource};
use horizon_senses::dsp::Radio;
use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// An OpenAL source's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Initial,
    Playing,
    Paused,
    Stopped,
}
/// An OpenAL source.
struct Channel {
    samples: Option<(Arc<Vec<f32>>, u32)>,
    position: f64,
    state: State,
    gain: f32,
    pitch: f32,
    /// `AL_LINEAR_DISTANCE` max distance, or no attenuation.
    linear: Option<f32>,
    looping: bool,
    at: [f32; 3],
    relative: bool,
}
#[derive(Default)]
struct Mixer {
    channels: HashMap<ChannelId, Channel>,
    listener: [f64; 3],
    listener_yaw: f32,
    limiter_peak: f32,
    /// Companions' voices; empty (and skipped) unless one is talking.
    voices: HashMap<VoiceId, Voice>,
    /// The Voice/Speech slider times the master volume.
    voice_volume: f32,
    voices_paused: bool,
}
/// A companion's voice line in the mixer.
pub type VoiceId = u32;
/// The companions' sound category (`SoundSource.VOICE`, "Voice/Speech").
const VOICE: usize = 9;
/// How far a companion is heard directly: as far as a sound at volume 3 (16 blocks per unit of
/// volume, linear). Past it, or from another dimension, the voice comes over the radio.
pub const VOICE_RANGE: f32 = 48.0;
/// The radio fades in over the last blocks of the range, so walking away never switches abruptly.
const RADIO_FADE: f32 = 8.0;
/// The radio's level against the direct voice at full volume.
const RADIO_GAIN: f32 = 0.8;
/// Audio held back before a line starts, or resumes after running dry, against generation jitter.
const VOICE_PREBUFFER: f32 = 0.12;
/// A companion's voice: samples appended while the speech model generates them, played from the
/// companion's position (docs/companions.md, "Playback").
struct Voice {
    queue: VecDeque<f32>,
    rate: u32,
    /// Between `previous` and the front of the queue, 0..1.
    phase: f64,
    previous: f32,
    /// The tone's gain (a shout is louder, a whisper quieter).
    gain: f32,
    at: [f32; 3],
    /// In another dimension than the listener: only the radio carries it.
    elsewhere: bool,
    /// How much of it comes over the radio, eased toward its target so it never clicks.
    wet: f32,
    radio: Option<Radio>,
    /// Buffering before it plays (at the start, or after the generator fell behind).
    waiting: bool,
    /// No more samples will come: it ends when the queue runs out.
    finished: bool,
}
/// A sound the player could have heard (`SoundEventListener.onPlaySound`):
/// what the subtitle overlay needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Heard {
    /// The `subtitle` translation key of the sound event.
    pub subtitle: String,
    /// The instance's position (relative sounds report their raw coordinates).
    pub at: Option<[f64; 3]>,
    /// `SoundEngine.getRange`: infinite for relative and non-attenuated sounds.
    pub range: f32,
    pub looping: bool,
}
/// The mixer side of the engine (`Backend`).
pub struct MixerBackend {
    mixer: Arc<Mutex<Mixer>>,
    root: PathBuf,
    cache: Arc<Mutex<SoundCache>>,
    /// The decoder thread for static sounds, started with the first one.
    decoder: Option<std::sync::mpsc::Sender<Decode>>,
    next: ChannelId,
    live: bool,
}
/// Decoded samples and their rate (`None`: the file did not load).
type Samples = Option<(Arc<Vec<f32>>, u32)>;
/// Decoded static sounds (`SoundBufferLibrary`'s buffers), dropped least
/// recently used first past `CACHE_BYTES`: kept forever, every sound heard in
/// a session stayed (up to 1.4 GiB of samples, villages playing hundreds).
#[derive(Default)]
struct SoundCache {
    entries: HashMap<String, (Samples, u64)>,
    bytes: usize,
    clock: u64,
}
const CACHE_BYTES: usize = 64 << 20;
impl SoundCache {
    fn get(&mut self, key: &str) -> Option<Samples> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.1 = self.clock;
        Some(entry.0.clone())
    }
    fn insert(&mut self, key: String, samples: Samples) {
        self.clock += 1;
        self.bytes += Self::size(&samples);
        if let Some((old, _)) = self.entries.insert(key, (samples, self.clock)) {
            self.bytes -= Self::size(&old);
        }
        while self.bytes > CACHE_BYTES && self.entries.len() > 1 {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= Self::size(&gone);
            }
        }
    }
    fn size(samples: &Samples) -> usize {
        samples
            .as_ref()
            .map_or(0, |(s, _)| s.len() * std::mem::size_of::<f32>())
    }
}
/// A static sound to decode for a channel waiting on it.
struct Decode {
    key: String,
    file: PathBuf,
    channel: ChannelId,
}
/// Give a channel waiting on its sound the decoded samples and start it, as
/// the buffer future attaching and playing the source. A source stopped or
/// released meanwhile stays so.
fn attach(mixer: &Mutex<Mixer>, channel: ChannelId, samples: Samples, looping: bool) {
    if let Some(samples) = samples
        && let Ok(mut m) = mixer.lock()
        && let Some(c) = m.channels.get_mut(&channel)
        && c.state == State::Initial
    {
        c.samples = Some(samples);
        c.position = 0.0;
        c.state = State::Playing;
        c.looping |= looping;
    }
}
/// The open output stream. A build without the `audio` feature (the server build, which must not
/// need libasound) never has one.
#[cfg(feature = "audio")]
type Stream = cpal::Stream;
#[cfg(not(feature = "audio"))]
type Stream = std::convert::Infallible;
pub struct Audio {
    _stream: Option<Stream>,
    pub engine: SoundEngine,
    backend: MixerBackend,
    /// Record `Heard` sounds (the subtitle overlay is listening).
    pub record_heard: bool,
    pub listener: [f64; 3],
    pub listener_yaw: f32,
    pub master: f32,
    /// Seeds of positional plays (the client level random's `nextLong`).
    random: LegacyRandom,
    /// A `SoundSource` index the next sound plays on (`/playsound`).
    source_override: Option<usize>,
    /// Positional sounds played since the last drain, kept while the world
    /// is open to the network so guests hear them too.
    pub capture: Option<Vec<(String, [f64; 3], f32, f32)>>,
    /// The voice volume the mixer has (`Mixer::voice_volume`).
    voice_volume: f32,
    next_voice: VoiceId,
    /// Companions' lines for the subtitle overlay, since the last `take_heard`.
    voice_heard: Vec<Heard>,
}
fn decode(path: &Path) -> Option<(Vec<f32>, u32)> {
    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
        meta::MetadataOptions, probe::Hint,
    };
    let file = fs::File::open(path).ok()?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probed.format;
    let track = format.default_track()?;
    let id = track.id;
    let rate = track.codec_params.sample_rate?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .ok()?;
    let mut mono = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != id {
            continue;
        }
        let Ok(buffer) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *buffer.spec();
        let channels = spec.channels.count().max(1);
        let mut samples = SampleBuffer::<f32>::new(buffer.capacity() as u64, spec);
        samples.copy_interleaved_ref(buffer);
        for frame in samples.samples().chunks(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }
    }
    Some((mono, rate))
}
impl MixerBackend {
    fn with(&self, channel: ChannelId, f: impl FnOnce(&mut Channel)) {
        if let Ok(mut m) = self.mixer.lock()
            && let Some(c) = m.channels.get_mut(&channel)
        {
            f(c);
        }
    }
}
impl Backend for MixerBackend {
    fn create(&mut self, _streaming: bool) -> Option<ChannelId> {
        self.next += 1;
        let id = self.next;
        self.mixer.lock().ok()?.channels.insert(
            id,
            Channel {
                samples: None,
                position: 0.0,
                state: State::Initial,
                gain: 1.0,
                pitch: 1.0,
                linear: None,
                looping: false,
                at: [0.0; 3],
                relative: false,
            },
        );
        Some(id)
    }
    fn destroy(&mut self, channel: ChannelId) {
        if let Ok(mut m) = self.mixer.lock() {
            m.channels.remove(&channel);
        }
    }
    fn set_pitch(&mut self, channel: ChannelId, pitch: f32) {
        self.with(channel, |c| c.pitch = pitch);
    }
    fn set_volume(&mut self, channel: ChannelId, volume: f32) {
        self.with(channel, |c| c.gain = volume);
    }
    fn set_attenuation(&mut self, channel: ChannelId, linear: Option<f32>) {
        self.with(channel, |c| c.linear = linear);
    }
    fn set_looping(&mut self, channel: ChannelId, looping: bool) {
        self.with(channel, |c| c.looping = looping);
    }
    fn set_position(&mut self, channel: ChannelId, p: [f64; 3]) {
        self.with(channel, |c| c.at = [p[0] as f32, p[1] as f32, p[2] as f32]);
    }
    fn set_relative(&mut self, channel: ChannelId, relative: bool) {
        self.with(channel, |c| c.relative = relative);
    }
    /// `getCompleteBuffer` (cached, else decoded on the decoder thread) or
    /// `getStream` (decoded on a thread of its own), then attach and
    /// `alSourcePlay`. A sound that cannot load leaves its source initial,
    /// as a failed buffer future does in vanilla.
    fn start(&mut self, channel: ChannelId, sound: &Sound, looping: bool) {
        if !self.live {
            return;
        }
        let (namespace, path) = sound.path();
        let file = self.root.join("assets").join(namespace).join(&path);
        if sound.stream {
            // A streamed sound loops by restarting its stream.
            let mixer = self.mixer.clone();
            std::thread::spawn(move || {
                let samples = decode(&file).map(|(s, r)| (Arc::new(s), r));
                attach(&mixer, channel, samples, looping);
            });
            return;
        }
        let key = format!("{namespace}:{path}");
        let cached = self.cache.lock().ok().and_then(|mut c| c.get(&key));
        if let Some(samples) = cached {
            attach(&self.mixer, channel, samples, false);
            return;
        }
        let decoder = self.decoder.get_or_insert_with(|| {
            let (send, receive) = std::sync::mpsc::channel::<Decode>();
            let (mixer, cache) = (self.mixer.clone(), self.cache.clone());
            std::thread::Builder::new()
                .name("sound decoder".into())
                .spawn(move || {
                    for job in receive {
                        // Queued twice before the first decode finished.
                        let cached = cache.lock().ok().and_then(|mut c| c.get(&job.key));
                        let samples = cached.unwrap_or_else(|| {
                            let samples = decode(&job.file).map(|(s, r)| (Arc::new(s), r));
                            if let Ok(mut c) = cache.lock() {
                                c.insert(job.key, samples.clone());
                            }
                            samples
                        });
                        attach(&mixer, job.channel, samples, false);
                    }
                })
                .expect("sound decoder thread");
            send
        });
        let _ = decoder.send(Decode { key, file, channel });
    }
    /// `alSourceStop`: any state becomes stopped, an initial source too (a
    /// sound still decoding then never starts).
    fn stop(&mut self, channel: ChannelId) {
        self.with(channel, |c| c.state = State::Stopped);
    }
    fn pause(&mut self, channel: ChannelId) {
        self.with(channel, |c| {
            if c.state == State::Playing {
                c.state = State::Paused;
            }
        });
    }
    fn unpause(&mut self, channel: ChannelId) {
        self.with(channel, |c| {
            if c.state == State::Paused {
                c.state = State::Playing;
            }
        });
    }
    fn stopped(&mut self, channel: ChannelId) -> bool {
        self.mixer
            .lock()
            .map(|m| {
                m.channels
                    .get(&channel)
                    .is_none_or(|c| c.state == State::Stopped)
            })
            .unwrap_or(true)
    }
}
/// The gain OpenAL applies for distance (`AL_LINEAR_DISTANCE` with reference
/// distance 0 and rolloff 1: the distance clamps to the max distance).
fn distance_gain(linear: Option<f32>, distance: f32) -> f32 {
    match linear {
        None => 1.0,
        Some(max) if max > 0.0 => (1.0 - distance.min(max) / max).clamp(0.0, 1.0),
        Some(_) => 0.0,
    }
}
/// Mix the companions' voices into `data`: direct and panned within `VOICE_RANGE`, then over the
/// radio, centred. A voice that runs dry waits for `VOICE_PREBUFFER` more, and one that is
/// finished leaves when it has played everything.
fn mix_voices(mixer: &mut Mixer, data: &mut [f32], channels: usize, rate: u32) {
    let listener = mixer.listener;
    let yaw = mixer.listener_yaw.to_radians();
    let volume = mixer.voice_volume;
    // The radio eases in and out over about 150 ms.
    let ease = 1.0 - (-1.0 / (0.15 * rate as f32)).exp();
    mixer.voices.retain(|_, v| {
        if v.waiting {
            let ready = v.queue.len() as f32 >= VOICE_PREBUFFER * v.rate as f32;
            if !ready && !v.finished {
                return true;
            }
            v.waiting = false;
        }
        let d = [
            f64::from(v.at[0]) - listener[0],
            f64::from(v.at[1]) - listener[1],
            f64::from(v.at[2]) - listener[2],
        ];
        let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
        let target = if v.elsewhere {
            1.0
        } else {
            ((distance - (VOICE_RANGE - RADIO_FADE)) / RADIO_FADE).clamp(0.0, 1.0)
        };
        let gain = v.gain.clamp(0.0, 2.0) * volume;
        let direct = gain * distance_gain(Some(VOICE_RANGE), distance);
        let side =
            (-(yaw.cos()) as f64 * d[0] - (yaw.sin()) as f64 * d[2]) as f32 / distance.max(0.001);
        let pan = side.clamp(-1.0, 1.0);
        let left = direct * (1.0 - pan.max(0.0) * 0.6);
        let right = direct * (1.0 + pan.min(0.0) * 0.6);
        let step = f64::from(v.rate) / f64::from(rate);
        for frame in data.chunks_mut(channels) {
            while v.phase >= 1.0 {
                let Some(x) = v.queue.pop_front() else {
                    break;
                };
                v.previous = x;
                v.phase -= 1.0;
            }
            let Some(&next) = v.queue.front().filter(|_| v.phase < 1.0) else {
                if v.finished {
                    return false;
                }
                v.waiting = true;
                break;
            };
            let x = v.previous + (next - v.previous) * v.phase as f32;
            v.phase += step;
            v.wet += (target - v.wet) * ease;
            let dry = 1.0 - v.wet;
            let (mut l, mut r) = (x * left * dry, x * right * dry);
            if v.wet > 0.001 {
                let radio = v.radio.get_or_insert_with(|| Radio::new(rate));
                let y = radio.process(x) * v.wet * gain * RADIO_GAIN;
                l += y;
                r += y;
            }
            if channels >= 2 {
                frame[0] += l;
                frame[1] += r;
            } else {
                frame[0] += (l + r) / 2.0;
            }
        }
        true
    });
}
// Only the output stream mixes; a build without one (no `audio` feature) never does.
#[cfg_attr(not(feature = "audio"), allow(dead_code))]
fn mix(mixer: &mut Mixer, data: &mut [f32], channels: usize, rate: u32) {
    data.fill(0.0);
    let listener = mixer.listener;
    let yaw = mixer.listener_yaw.to_radians();
    // Gains and pans per source for this buffer.
    let mut voices: Vec<(ChannelId, f32, f32, f64)> = Vec::new();
    for (id, c) in &mixer.channels {
        let Some((_, source_rate)) = &c.samples else {
            continue;
        };
        if c.state != State::Playing {
            continue;
        }
        let d = if c.relative {
            [f64::from(c.at[0]), f64::from(c.at[1]), f64::from(c.at[2])]
        } else {
            [
                f64::from(c.at[0]) - listener[0],
                f64::from(c.at[1]) - listener[1],
                f64::from(c.at[2]) - listener[2],
            ]
        };
        let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
        let gain = c.gain.clamp(0.0, 1.0) * distance_gain(c.linear, distance);
        // The listener's right vector for a Minecraft yaw.
        let side = if c.relative {
            (d[0] as f32) / distance.max(0.001)
        } else {
            (-(yaw.cos()) as f64 * d[0] - (yaw.sin()) as f64 * d[2]) as f32 / distance.max(0.001)
        };
        let pan = side.clamp(-1.0, 1.0);
        let left = gain * (1.0 - pan.max(0.0) * 0.6);
        let right = gain * (1.0 + pan.min(0.0) * 0.6);
        let step = f64::from(*source_rate) / f64::from(rate) * f64::from(c.pitch.max(0.0));
        voices.push((*id, left, right, step));
    }
    for (id, left, right, step) in voices {
        let Some(c) = mixer.channels.get_mut(&id) else {
            continue;
        };
        let Some((samples, _)) = c.samples.clone() else {
            continue;
        };
        for frame in data.chunks_mut(channels) {
            let mut i = c.position as usize;
            if i + 1 >= samples.len() {
                if c.looping && samples.len() > 1 {
                    c.position = 0.0;
                    i = 0;
                } else {
                    c.state = State::Stopped;
                    break;
                }
            }
            let t = (c.position - i as f64) as f32;
            let s = samples[i] * (1.0 - t) + samples[i + 1] * t;
            if channels >= 2 {
                frame[0] += s * left;
                frame[1] += s * right;
            } else {
                frame[0] += s * (left + right) / 2.0;
            }
            c.position += step;
        }
    }
    if !mixer.voices.is_empty() && !mixer.voices_paused {
        mix_voices(mixer, data, channels, rate);
    }
    // Limit the whole buffer together, preserving waveform and stereo balance
    // instead of flattening each overloaded sample; release over 50 ms.
    let peak = data.iter().fold(1.0f32, |peak, s| peak.max(s.abs()));
    mixer.limiter_peak = mixer.limiter_peak.max(peak);
    let release = (-1.0 / (0.05 * rate as f32)).exp();
    for frame in data.chunks_mut(channels) {
        let gain = mixer.limiter_peak.recip();
        for s in frame {
            *s *= gain;
        }
        mixer.limiter_peak = (mixer.limiter_peak * release).max(peak);
    }
}
/// Why the game should not open an output, if it should not:
/// `HORIZON_NO_AUDIO`, or a virtual screen (`vscreen`), where agents and
/// harnesses run the game and must stay off the user's speakers.
/// `HORIZON_AUDIO=1` opens the output anyway (testing into a null sink).
#[cfg(any(feature = "audio", test))]
fn output_disabled(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<String> {
    if var("HORIZON_NO_AUDIO").is_some() {
        return Some("audio off: HORIZON_NO_AUDIO is set".into());
    }
    if var("HORIZON_AUDIO").is_some_and(|v| v == "1") {
        return None;
    }
    let display = var("WAYLAND_DISPLAY")?;
    Path::new(&display)
        .components()
        .any(|c| c.as_os_str() == "vscreen")
        .then(|| "audio off on a virtual screen (vscreen); HORIZON_AUDIO=1 opens it".into())
}
/// An output stream on `device` in its default configuration, whatever its
/// sample format: the mixer works in f32 and converts.
#[cfg(feature = "audio")]
fn open_device(
    device: &cpal::Device,
    mixer: Arc<Mutex<Mixer>>,
) -> Result<(cpal::Stream, cpal::SupportedStreamConfig)> {
    use cpal::SampleFormat as F;
    let config = device.default_output_config()?;
    let stream = match config.sample_format() {
        F::F32 => build_stream::<f32>(device, &config, mixer),
        F::F64 => build_stream::<f64>(device, &config, mixer),
        F::I16 => build_stream::<i16>(device, &config, mixer),
        F::I32 => build_stream::<i32>(device, &config, mixer),
        F::I8 => build_stream::<i8>(device, &config, mixer),
        F::U16 => build_stream::<u16>(device, &config, mixer),
        F::U32 => build_stream::<u32>(device, &config, mixer),
        F::U8 => build_stream::<u8>(device, &config, mixer),
        other => Err(format!("unsupported sample format {other}").into()),
    }?;
    stream.play()?;
    Ok((stream, config))
}
#[cfg(feature = "audio")]
fn build_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    mixer: Arc<Mutex<Mixer>>,
) -> Result<cpal::Stream> {
    let rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let mut scratch = Vec::new();
    let stream = device.build_output_stream(
        &config.config(),
        move |data: &mut [T], _| {
            scratch.resize(data.len(), 0.0f32);
            if let Ok(mut m) = mixer.lock() {
                mix(&mut m, &mut scratch, channels, rate);
            } else {
                scratch.fill(0.0);
            }
            for (out, s) in data.iter_mut().zip(&scratch) {
                *out = T::from_sample(*s);
            }
        },
        |e| log::warn!("Horizon: audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}
impl Audio {
    pub fn new(root: &Path) -> Self {
        Self::with_output(root, true)
    }
    /// No output device: the dedicated server.
    pub fn silent(root: &Path) -> Self {
        Self::with_output(root, false)
    }
    fn with_output(root: &Path, output: bool) -> Self {
        let registry = SoundRegistry::load(root);
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let opened = if output {
            Self::open(mixer.clone())
        } else {
            Err("no output on a dedicated server".into())
        };
        let (stream, live) = match opened {
            Ok(v) => (Some(v), true),
            Err(e) => {
                if output {
                    log::warn!("Horizon: no sound: {e}");
                }
                (None, false)
            }
        };
        let mut engine = SoundEngine::new(registry);
        engine.loaded = live;
        Self {
            _stream: stream,
            engine,
            backend: MixerBackend {
                mixer,
                root: root.to_owned(),
                cache: Arc::default(),
                decoder: None,
                next: 0,
                live,
            },
            record_heard: false,
            listener: [0.0; 3],
            listener_yaw: 0.0,
            master: 1.0,
            random: LegacyRandom::new(crate::sound_engine::unique_seed()),
            source_override: None,
            capture: None,
            voice_volume: 0.0,
            next_voice: 0,
            voice_heard: Vec::new(),
        }
    }
    #[cfg(not(feature = "audio"))]
    fn open(_: Arc<Mutex<Mixer>>) -> Result<Stream> {
        Err("this build has no sound output (built without the `audio` feature)".into())
    }
    #[cfg(feature = "audio")]
    fn open(mixer: Arc<Mutex<Mixer>>) -> Result<Stream> {
        if let Some(reason) = output_disabled(|name| std::env::var_os(name)) {
            return Err(reason.into());
        }
        let host = cpal::default_host();
        let mut errors = Vec::new();
        // The default device first, then any other output a host lists:
        // a default that cannot open (busy, or a stale ALSA default) should
        // not leave the game silent when another device works.
        let default = host.default_output_device();
        let others = host.output_devices().into_iter().flatten();
        let mut tried = Vec::new();
        for device in default.into_iter().chain(others) {
            let name = device.name().unwrap_or_else(|_| "unnamed device".into());
            if tried.contains(&name) {
                continue;
            }
            tried.push(name.clone());
            match open_device(&device, mixer.clone()) {
                Ok((stream, config)) => {
                    log::info!(
                        "Horizon: audio output {name} ({} Hz, {} channels, {})",
                        config.sample_rate().0,
                        config.channels(),
                        config.sample_format()
                    );
                    return Ok(stream);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        if errors.is_empty() {
            return Err(format!("no audio output device on the {} host", host.id().name()).into());
        }
        Err(format!("no audio output device opened ({})", errors.join("; ")).into())
    }
    fn bound(&mut self) -> Bound<'_, MixerBackend> {
        Bound {
            engine: &mut self.engine,
            backend: &mut self.backend,
        }
    }
    /// `SoundManager.updateSource`: the camera position and yaw.
    pub fn set_listener(&mut self, position: [f64; 3], yaw: f32) {
        self.listener = position;
        self.listener_yaw = yaw;
        if let Ok(mut m) = self.backend.mixer.lock() {
            m.listener = position;
            m.listener_yaw = yaw;
        }
    }
    /// Sync the volume options, refreshing the categories that changed
    /// (the option sliders' `refreshCategoryVolume`).
    fn sync_volumes(&mut self) {
        let mut volumes = [self.master; 11];
        for (i, v) in volumes.iter_mut().enumerate().skip(1) {
            *v = crate::settings::source_volume(i);
        }
        for source in 0..11 {
            if self.engine.source_volumes[source] != volumes[source] {
                self.engine.source_volumes[source] = volumes[source];
                self.engine
                    .refresh_category_volume(source, &mut self.backend);
            }
        }
        self.engine.listening = self.record_heard;
        let voice = self.master * volumes[VOICE];
        if voice != self.voice_volume
            && let Ok(mut m) = self.backend.mixer.lock()
        {
            m.voice_volume = voice;
            self.voice_volume = voice;
        }
    }
    /// `SoundManager.tick(paused)`, once per client tick.
    pub fn tick(&mut self, paused: bool, ctx: &TickContext) {
        self.sync_volumes();
        self.engine.tick(paused, ctx, &mut self.backend);
    }
    /// `SoundManager.pauseAllExcept(MUSIC, UI)`.
    pub fn pause(&mut self) {
        self.engine.pause_all_except(
            &[crate::sound_engine::MUSIC, crate::sound_engine::UI],
            &mut self.backend,
        );
        self.pause_voices(true);
    }
    /// `SoundManager.resume`.
    pub fn resume(&mut self) {
        self.engine.resume(&mut self.backend);
        self.pause_voices(false);
    }
    fn pause_voices(&mut self, paused: bool) {
        if let Ok(mut m) = self.backend.mixer.lock() {
            m.voices_paused = paused;
        }
    }
    /// `SoundManager.stop()`: every sound.
    pub fn stop_all(&mut self) {
        self.engine.stop_all(&mut self.backend);
        if let Ok(mut m) = self.backend.mixer.lock() {
            m.voices.clear();
        }
    }
    /// `SoundManager.reload` (resource packs changed): stop every sound, then
    /// read sound events and sound files from the new resource root.
    pub fn reload(&mut self, root: &Path) {
        self.stop_all();
        self.engine.registry = SoundRegistry::load(root);
        self.backend.root = root.to_owned();
        let mut cache = self.backend.cache.lock().unwrap();
        cache.entries.clear();
        cache.bytes = 0;
    }
    /// Play a sound event at a world position, as `ClientLevel.playLocalSound`
    /// (a `SimpleSoundInstance` seeded from the level random), or relative to
    /// the listener without attenuation (`None`).
    pub fn play(&mut self, event: &str, at: Option<[f64; 3]>, volume: f32, pitch: f32) {
        let seed = self.random.next_long();
        self.play_seeded(event, at, volume, pitch, seed);
    }
    /// `LivingEntity.handleDamageEvent`'s pitch, `(r - r) * 0.2 + 1`, drawn on the
    /// client (the entity's own random there, never the server's world random).
    pub fn hurt_pitch(&mut self) -> f32 {
        (self.random.next_float() - self.random.next_float()) * 0.2 + 1.0
    }
    /// `AbstractWidget.playButtonClickSound`: an unseeded, relative UI sound.
    pub fn play_button_click(&mut self) {
        self.play_instance(
            SoundInstance::for_ui(
                "minecraft:ui.button.click",
                1.0,
                0.25,
                SoundEngine::unseeded(),
            ),
            false,
        );
    }
    /// `play` with the seed a sound packet carries.
    pub fn play_seeded(
        &mut self,
        event: &str,
        at: Option<[f64; 3]>,
        volume: f32,
        pitch: f32,
        seed: i64,
    ) {
        if let (Some(capture), Some(at)) = (self.capture.as_mut(), at) {
            capture.push((event.to_owned(), at, volume, pitch));
        }
        self.sync_volumes();
        let event = crate::sound_events::normalize(event);
        let source = self
            .source_override
            .unwrap_or_else(|| crate::settings::sound_source(&event));
        let random = LegacyRandom::new(seed);
        let instance = match at {
            Some(at) => SoundInstance::at(&event, source, volume, pitch, random, at),
            None => SoundInstance::new(
                &event,
                source,
                volume,
                pitch,
                random,
                false,
                0,
                Attenuation::None,
                [0.0; 3],
                true,
            ),
        };
        self.play_instance(instance, false);
    }
    /// Play a sound on a given `SoundSource` index (`/playsound`).
    pub fn play_on(
        &mut self,
        source: usize,
        event: &str,
        at: Option<[f64; 3]>,
        volume: f32,
        pitch: f32,
    ) {
        self.source_override = Some(source);
        self.play(event, at, volume, pitch);
        self.source_override = None;
    }
    /// `SimpleSoundInstance.forJukeboxSong` at a jukebox; the handle stays
    /// until `release`.
    pub fn play_jukebox_song(&mut self, event: &str, pos: [i32; 3]) -> InstanceId {
        let at = [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]) + 0.5,
            f64::from(pos[2]) + 0.5,
        ];
        let event = crate::sound_events::normalize(event);
        let instance = SoundInstance::for_jukebox_song(&event, at, SoundEngine::unseeded());
        self.play_instance(instance, true).0
    }
    /// Stop and drop a held instance.
    pub fn stop_and_release(&mut self, id: InstanceId) {
        self.stop_instance(id);
        self.release(id);
    }
    /// Every sound event id (for command completion).
    /// The engine's table sizes and the mixer's channels (`status`).
    pub fn census(&self) -> serde_json::Value {
        let mut census = self.engine.census();
        census["channels"] = self
            .backend
            .mixer
            .lock()
            .map_or(0, |m| m.channels.len())
            .into();
        if let Ok(cache) = self.backend.cache.lock() {
            census["decoded"] = cache.entries.len().into();
            census["decoded_mib"] = (cache.bytes >> 20).into();
        }
        census
    }
    pub fn event_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.engine.registry.names().cloned().collect();
        v.sort();
        v
    }
    /// `ClientboundStopSoundPacket`: stop the playing sounds of an event
    /// and/or a source.
    pub fn stop_sounds(&mut self, event: Option<&str>, source: Option<usize>) {
        let event = event.map(crate::sound_events::normalize);
        self.engine
            .stop_matching(event.as_deref(), source, &mut self.backend);
    }
    /// The sounds played since the last call, for the subtitle overlay.
    pub fn take_heard(&mut self) -> Vec<Heard> {
        std::mem::take(&mut self.engine.notifications)
            .into_iter()
            .filter_map(|n| {
                Some(Heard {
                    subtitle: n.subtitle?,
                    at: Some(n.position),
                    range: n.range,
                    looping: n.looping,
                })
            })
            .chain(self.voice_heard.drain(..))
            .collect()
    }
    /// Start a companion's line: samples at `rate` come with `voice_push`, from `at` (in another
    /// dimension when `elsewhere`), louder or quieter by `gain` (the tone). `None` without an output.
    pub fn voice_start(
        &mut self,
        rate: u32,
        gain: f32,
        at: [f64; 3],
        elsewhere: bool,
    ) -> Option<VoiceId> {
        if !self.backend.live {
            return None;
        }
        self.sync_volumes();
        self.next_voice += 1;
        let id = self.next_voice;
        self.backend.mixer.lock().ok()?.voices.insert(
            id,
            Voice {
                queue: VecDeque::new(),
                rate: rate.max(1),
                phase: 0.0,
                previous: 0.0,
                gain,
                at: at.map(|c| c as f32),
                elsewhere,
                wet: if elsewhere { 1.0 } else { 0.0 },
                radio: None,
                waiting: true,
                finished: false,
            },
        );
        Some(id)
    }
    fn with_voice(&self, id: VoiceId, f: impl FnOnce(&mut Voice)) {
        if let Ok(mut m) = self.backend.mixer.lock()
            && let Some(v) = m.voices.get_mut(&id)
        {
            f(v);
        }
    }
    /// More of a line, as the speech model generates it.
    pub fn voice_push(&mut self, id: VoiceId, samples: &[f32]) {
        self.with_voice(id, |v| v.queue.extend(samples));
    }
    /// Where the speaker is now (each tick while it talks).
    pub fn voice_place(&mut self, id: VoiceId, at: [f64; 3], elsewhere: bool) {
        self.with_voice(id, |v| {
            v.at = at.map(|c| c as f32);
            v.elsewhere = elsewhere;
        });
    }
    /// No more samples will come: the line ends when it has played them.
    pub fn voice_finish(&mut self, id: VoiceId) {
        self.with_voice(id, |v| v.finished = true);
    }
    /// Cut a line off.
    pub fn voice_stop(&mut self, id: VoiceId) {
        if let Ok(mut m) = self.backend.mixer.lock() {
            m.voices.remove(&id);
        }
    }
    /// Whether a line is still buffering or playing.
    pub fn voice_playing(&self, id: VoiceId) -> bool {
        self.backend
            .mixer
            .lock()
            .is_ok_and(|m| m.voices.contains_key(&id))
    }
    /// Show a companion's line in the subtitles while the overlay listens: at the speaker within
    /// `VOICE_RANGE`, everywhere over the radio (`at` none).
    pub fn voice_subtitle(&mut self, text: &str, at: Option<[f64; 3]>) {
        if self.record_heard {
            self.voice_heard.push(Heard {
                // The overlay reads it as a translation key: `%` would start an argument.
                subtitle: text.replace('%', "%%"),
                at: Some(at.unwrap_or(self.listener)),
                range: if at.is_some() {
                    VOICE_RANGE
                } else {
                    f32::INFINITY
                },
                looping: false,
            });
        }
    }
}
impl Sounds for Audio {
    fn engine(&mut self) -> &mut SoundEngine {
        &mut self.engine
    }
    fn play_instance(&mut self, instance: SoundInstance, retain: bool) -> (InstanceId, PlayResult) {
        self.sync_volumes();
        self.bound().play_instance(instance, retain)
    }
    fn play_id(&mut self, id: InstanceId) -> PlayResult {
        self.sync_volumes();
        self.bound().play_id(id)
    }
    fn stop_instance(&mut self, id: InstanceId) {
        self.bound().stop_instance(id);
    }
    fn update_category_volume(&mut self, source: usize, gain: f32) {
        self.bound().update_category_volume(source, gain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_click_uses_vanilla_ui_parameters_without_advancing_world_random() {
        let mut audio = Audio::silent(Path::new("/nonexistent/horizon-test-assets"));
        audio.engine.registry.add_file(
            "minecraft",
            br#"{"ui.button.click":{"sounds":["gui/button"]}}"#,
            &|_, _| true,
        );
        // Exercise the mixer without opening an output device. The missing
        // sample is harmless: this test checks the source the engine creates.
        audio.engine.loaded = true;
        audio.backend.live = true;
        let mut random = audio.random.clone();
        audio.play_button_click();
        let instance = audio.engine.instance(1).expect("click reached the engine");
        assert_eq!(instance.identifier, "minecraft:ui.button.click");
        assert_eq!(instance.source, crate::sound_engine::UI);
        assert_eq!(instance.volume, 0.25);
        assert_eq!(instance.pitch, 1.0);
        assert_eq!(instance.attenuation, Attenuation::None);
        assert_eq!(instance.position, [0.0; 3]);
        assert!(instance.relative);
        assert!(!instance.looping);
        assert_eq!(audio.random.next_long(), random.next_long());
        audio.pause();
        assert_eq!(
            audio.backend.mixer.lock().unwrap().channels[&1].state,
            State::Initial
        );
        audio.stop_all();
    }

    fn channel(samples: Vec<f32>, linear: Option<f32>, at: [f32; 3], looping: bool) -> Channel {
        Channel {
            samples: Some((Arc::new(samples), 48_000)),
            position: 0.0,
            state: State::Playing,
            gain: 1.0,
            pitch: 1.0,
            linear,
            looping,
            at,
            relative: false,
        }
    }

    #[test]
    fn decoded_sounds_stay_within_the_budget_least_recently_used_out() {
        let mut cache = SoundCache::default();
        let quarter = CACHE_BYTES / 4 / std::mem::size_of::<f32>();
        let sound = || Some((Arc::new(vec![0.0f32; quarter]), 44100));
        for key in ["a", "b", "c", "d"] {
            cache.insert(key.into(), sound());
        }
        assert!(cache.get("a").is_some());
        cache.insert("e".into(), sound());
        assert!(cache.bytes <= CACHE_BYTES);
        assert!(
            cache.get("b").is_none(),
            "the least recently used goes first"
        );
        assert!(cache.get("a").is_some() && cache.get("e").is_some());
        // A failed load is remembered without counting.
        cache.insert("missing".into(), None);
        assert_eq!(cache.get("missing"), Some(None));
    }
    #[test]
    fn output_stays_off_on_a_virtual_screen_unless_asked() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| std::ffi::OsString::from(*v))
            }
        };
        assert!(output_disabled(env(&[("WAYLAND_DISPLAY", "wayland-1")])).is_none());
        assert!(output_disabled(env(&[])).is_none());
        const VSCREEN: &str = "/run/user/1000/vscreen/main/wayland-1";
        assert!(output_disabled(env(&[("WAYLAND_DISPLAY", VSCREEN)])).is_some());
        assert!(
            output_disabled(env(&[("WAYLAND_DISPLAY", VSCREEN), ("HORIZON_AUDIO", "1")])).is_none()
        );
        assert!(
            output_disabled(env(&[("HORIZON_NO_AUDIO", "1"), ("HORIZON_AUDIO", "1")])).is_some()
        );
    }
    #[test]
    fn linear_attenuation_reaches_zero_at_the_max_distance() {
        assert_eq!(distance_gain(None, 100.0), 1.0);
        assert_eq!(distance_gain(Some(16.0), 0.0), 1.0);
        assert_eq!(distance_gain(Some(16.0), 8.0), 0.5);
        assert_eq!(distance_gain(Some(16.0), 16.0), 0.0);
        assert_eq!(distance_gain(Some(16.0), 40.0), 0.0);
    }

    #[test]
    fn overlapping_voices_are_limited_without_flattening_or_changing_stereo_balance() {
        let mut m = Mixer::default();
        let wave = vec![0.25, 0.5, 0.75, 1.0, -1.0, -0.75, -0.5, -0.25, 0.0];
        for id in 1..=4 {
            m.channels
                .insert(id, channel(wave.clone(), None, [-1.0, 0.0, 0.0], false));
        }
        let mut data = [0.0; 16];
        mix(&mut m, &mut data, 2, 48_000);
        for (frame, sample) in data.as_chunks::<2>().0.iter().zip(wave) {
            assert!((frame[0] - sample * 0.4).abs() < 1e-6, "{frame:?}");
            assert!((frame[1] - sample).abs() < 1e-6, "{frame:?}");
        }
        assert!(data.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn limiter_leaves_quiet_audio_alone_and_recovers_across_callbacks() {
        let mut m = Mixer::default();
        m.channels
            .insert(1, channel(vec![0.25; 2], None, [0.0; 3], true));
        let mut data = [0.0; 16];
        mix(&mut m, &mut data, 2, 48_000);
        assert_eq!(data, [0.25; 16]);

        for id in 2..=5 {
            m.channels
                .insert(id, channel(vec![1.0; 2], None, [0.0; 3], true));
        }
        mix(&mut m, &mut data, 2, 48_000);
        assert!(data.iter().all(|s| s.abs() <= 1.0));
        m.channels.retain(|&id, _| id == 1);
        let mut recovery = vec![0.0; 2 * 48_000];
        mix(&mut m, &mut recovery, 2, 48_000);
        assert!(recovery[0] > 0.0 && recovery[0] < 0.1);
        assert!(recovery.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*recovery.last().unwrap(), 0.25);
    }

    fn voice(at: [f32; 3], elsewhere: bool) -> Voice {
        Voice {
            queue: VecDeque::new(),
            rate: 24_000,
            phase: 0.0,
            previous: 0.0,
            gain: 1.0,
            at,
            elsewhere,
            wet: if elsewhere { 1.0 } else { 0.0 },
            radio: None,
            waiting: true,
            finished: false,
        }
    }

    fn voice_mixer() -> Mixer {
        Mixer {
            voice_volume: 1.0,
            ..Mixer::default()
        }
    }

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    fn rms(data: &[f32]) -> f32 {
        (data.iter().map(|s| s * s).sum::<f32>() / data.len() as f32).sqrt()
    }

    #[test]
    fn a_voice_waits_for_its_prebuffer_plays_appended_samples_and_ends_when_finished() {
        let mut m = voice_mixer();
        m.voices.insert(1, voice([0.0; 3], false));
        let mut out = vec![0.0f32; 2 * 4800];
        // 50 ms is less than the prebuffer: silence.
        m.voices.get_mut(&1).unwrap().queue.extend(vec![0.25; 1200]);
        mix(&mut m, &mut out, 2, 48_000);
        assert!(out.iter().all(|s| *s == 0.0));
        // 150 ms more: it plays (at 24 kHz into 48 kHz, 0.1 s of output takes 2 400 samples).
        m.voices.get_mut(&1).unwrap().queue.extend(vec![0.25; 3600]);
        mix(&mut m, &mut out, 2, 48_000);
        assert!((out[2 * 4000] - 0.25).abs() < 1e-4, "{}", out[2 * 4000]);
        assert!(m.voices.contains_key(&1));
        // It ran dry while the generator is behind: it waits again, then ends once finished.
        mix(&mut m, &mut out, 2, 48_000);
        mix(&mut m, &mut out, 2, 48_000);
        assert!(m.voices[&1].waiting);
        m.voices.get_mut(&1).unwrap().finished = true;
        m.voices.get_mut(&1).unwrap().queue.extend(vec![0.25; 100]);
        mix(&mut m, &mut out, 2, 48_000);
        assert!(!m.voices.contains_key(&1));
    }

    #[test]
    fn a_voice_is_heard_directly_nearby_and_over_the_radio_far_away_or_elsewhere() {
        let speech = sine(1000.0, 24_000, 0.5);
        let hum = sine(100.0, 24_000, 0.5);
        let play = |at: [f32; 3], elsewhere: bool, samples: &[f32]| {
            let mut m = voice_mixer();
            let mut v = voice(at, elsewhere);
            v.queue.extend(samples);
            v.finished = true;
            m.voices.insert(1, v);
            let mut out = vec![0.0f32; 2 * 24_000];
            mix_voices(&mut m, &mut out, 2, 48_000);
            out
        };
        // On the listener's right (yaw 0 faces +z, so -x is right): louder in the right ear.
        let near = play([-8.0, 0.0, 0.0], false, &speech);
        let (left, right): (Vec<f32>, Vec<f32>) =
            near.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).unzip();
        assert!(rms(&right) > rms(&left) * 1.5);
        // Linear over VOICE_RANGE: at 24 blocks, half as loud as at the listener.
        let close = play([0.0; 3], false, &speech);
        let mid = play([0.0, 0.0, 24.0], false, &speech);
        assert!((rms(&mid) / rms(&close) - 0.5).abs() < 0.02);
        // Past the range, the radio: the voice band stays, a 100 Hz hum is cut, centred.
        let far = play([0.0, 0.0, 100.0], false, &speech);
        assert!(rms(&far) > 0.1, "{}", rms(&far));
        let far_hum = play([0.0, 0.0, 100.0], false, &hum);
        assert!(rms(&far_hum) < rms(&far) * 0.3);
        let (l, r): (Vec<f32>, Vec<f32>) =
            far.as_chunks::<2>().0.iter().map(|f| (f[0], f[1])).unzip();
        assert_eq!(l, r);
        // Another dimension, right beside: the radio too.
        let elsewhere = play([1.0, 0.0, 0.0], true, &hum);
        assert!(rms(&elsewhere) < rms(&close) * 0.3);
    }

    /// The mixer's cost for one voice (run with `--ignored --nocapture`, in release).
    #[test]
    #[ignore]
    fn voice_mixing_cost() {
        let rounds = 2000;
        let time = |voices: usize, at: [f32; 3]| {
            let mut m = voice_mixer();
            for id in 0..voices as u32 {
                let mut v = voice(at, false);
                v.queue.extend(sine(300.0, 24_000, 30.0));
                v.waiting = false;
                m.voices.insert(id, v);
            }
            let mut out = vec![0.0f32; 2 * 512];
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                mix(&mut m, &mut out, 2, 48_000);
            }
            start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds)
        };
        let none = time(0, [0.0; 3]);
        let direct = time(1, [4.0, 0.0, 4.0]);
        let radio = time(1, [0.0, 0.0, 100.0]);
        eprintln!(
            "512-frame stereo callback at 48 kHz (10.7 ms of audio): no voice {none:.2} µs, \
             a voice nearby {direct:.2} µs, over the radio {radio:.2} µs"
        );
    }

    #[test]
    fn sources_play_attenuated_then_stop_or_loop() {
        let mut m = Mixer::default();
        m.channels
            .insert(1, channel(vec![1.0; 4], Some(16.0), [8.0, 0.0, 0.0], false));
        m.channels
            .insert(2, channel(vec![1.0; 4], None, [0.0; 3], true));
        let mut data = [0.0f32; 16];
        mix(&mut m, &mut data, 2, 48_000);
        assert_eq!(m.channels[&1].state, State::Stopped);
        assert_eq!(m.channels[&2].state, State::Playing);
        // Mono sum of the two at the first frame: the far one is half as loud.
        let first = data[0] + data[1];
        assert!(first > 1.4 && first < 3.1, "{first}");
    }
}
