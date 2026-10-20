//! Small signal processing for the voice: PCM and WAV conversion, the microphone's resampling to
//! 16 kHz mono, and the "radio" effect the companion's voice gets far away or in another dimension.
//! All of it runs on audio or worker threads without allocating per sample.

/// Little-endian signed 16-bit PCM to samples in [-1, 1).
pub fn pcm16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .collect()
}

/// A mono 16-bit PCM WAV file.
pub fn wav_pcm16(samples: &[f32], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// The samples and rate of a 16-bit PCM WAV file (mono, or the first channel).
pub fn read_wav_pcm16(bytes: &[u8]) -> Option<(Vec<f32>, u32)> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut channels, mut bits) = (0u32, 1usize, 0u16);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8 + size).min(bytes.len()))?;
        if id == b"fmt " && body.len() >= 16 {
            channels = usize::from(u16::from_le_bytes([body[2], body[3]])).max(1);
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            if bits != 16 {
                return None;
            }
            let pcm = pcm16_to_f32(body);
            return Some((pcm.into_iter().step_by(channels).collect(), rate));
        }
        at += 8 + size + (size & 1);
    }
    None
}

/// Peak and RMS of a signal, for listen-free checks.
pub fn peak_rms(samples: &[f32]) -> (f32, f32) {
    let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    (peak, rms)
}

/// A second-order IIR section (RBJ cookbook), direct form I.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Biquad {
        Biquad {
            b: [
                (b[0] / a[0]) as f32,
                (b[1] / a[0]) as f32,
                (b[2] / a[0]) as f32,
            ],
            a: [(a[1] / a[0]) as f32, (a[2] / a[0]) as f32],
            ..Biquad::default()
        }
    }

    fn omega(cutoff: f64, rate: f64, q: f64) -> (f64, f64) {
        let w = std::f64::consts::TAU * (cutoff / rate).min(0.49);
        (w.cos(), w.sin() / (2.0 * q))
    }

    pub fn low_pass(cutoff: f64, rate: f64, q: f64) -> Biquad {
        let (cos, alpha) = Self::omega(cutoff, rate, q);
        Biquad::new(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    pub fn high_pass(cutoff: f64, rate: f64, q: f64) -> Biquad {
        let (cos, alpha) = Self::omega(cutoff, rate, q);
        Biquad::new(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        )
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1]
            - self.a[0] * self.y[0]
            - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        // Flush denormals: a silent tail must not slow the audio thread down.
        let y = if y.abs() < 1e-20 { 0.0 } else { y };
        self.y = [y, self.y[0]];
        y
    }
}

/// Turns any input (rate, channels) into 16 kHz mono for speech recognition, as the microphone
/// delivers it: downmix, a low-pass under the new Nyquist (two cascaded sections at 7 kHz), then
/// linear interpolation. Streaming: state carries across calls.
pub struct To16k {
    channels: usize,
    step: f64,
    filters: [Biquad; 2],
    /// Position of the next output sample, in input samples after `last`.
    phase: f64,
    last: f32,
}

impl To16k {
    pub const RATE: u32 = 16000;

    pub fn new(input_rate: u32, channels: usize) -> To16k {
        let rate = f64::from(input_rate.max(1));
        let cutoff = 7000.0f64.min(rate * 0.45);
        To16k {
            channels: channels.max(1),
            step: rate / f64::from(Self::RATE),
            filters: [
                Biquad::low_pass(cutoff, rate, 0.541),
                Biquad::low_pass(cutoff, rate, 1.307),
            ],
            phase: 0.0,
            last: 0.0,
        }
    }

    /// Append the 16 kHz samples of an interleaved input buffer to `out`.
    pub fn push(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for frame in interleaved.chunks_exact(self.channels) {
            let mono = frame.iter().sum::<f32>() / self.channels as f32;
            let x = self.filters.iter_mut().fold(mono, |x, f| f.process(x));
            // Output samples falling between `last` (at 0) and `x` (at 1).
            while self.phase < 1.0 {
                let t = self.phase as f32;
                out.push(self.last + (x - self.last) * t);
                self.phase += self.step;
            }
            self.phase -= 1.0;
            self.last = x;
        }
    }
}

/// The "radio": a band-pass to the 300–3 000 Hz of a walkie-talkie, soft clipping, and a light hiss
/// with crackles. One per voice; runs in the mixer at the output rate.
pub struct Radio {
    filters: [Biquad; 4],
    noise: u32,
    crackle: f32,
}

impl Radio {
    pub fn new(rate: u32) -> Radio {
        let rate = f64::from(rate.max(8000));
        Radio {
            filters: [
                Biquad::high_pass(300.0, rate, 0.707),
                Biquad::high_pass(300.0, rate, 0.707),
                Biquad::low_pass(3000.0, rate, 0.707),
                Biquad::low_pass(3000.0, rate, 0.707),
            ],
            noise: 0x9E37_79B9,
            crackle: 0.0,
        }
    }

    /// White noise in [-1, 1) (xorshift: cheap and deterministic).
    fn white(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        (self.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let band = self.filters.iter_mut().fold(x * 2.0, |x, f| f.process(x));
        // Soft clip: the overdriven speaker of a cheap radio.
        let clipped = band.tanh() * 0.6;
        let hiss = self.white() * 0.012;
        // A crackle now and then: a short decaying click.
        if self.noise.is_multiple_of(20000) {
            self.crackle = 0.25 * self.white().signum();
        }
        self.crackle *= 0.9;
        clipped + hiss + self.crackle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin() * 0.5)
            .collect()
    }

    /// RMS after the first tenth (filters settle).
    fn steady_rms(samples: &[f32]) -> f32 {
        peak_rms(&samples[samples.len() / 10..]).1
    }

    #[test]
    fn wav_round_trips() {
        let s = sine(440.0, 16000, 0.1);
        let wav = wav_pcm16(&s, 16000);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + s.len() * 2);
        let (back, rate) = read_wav_pcm16(&wav).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(back.len(), s.len());
        assert!(back.iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-4));
        assert_eq!(
            pcm16_to_f32(&[0, 0x80, 0xff, 0x7f]),
            vec![-1.0, 32767.0 / 32768.0]
        );
    }

    #[test]
    fn resampling_keeps_speech_and_removes_what_16_khz_cannot_hold() {
        for (rate, channels) in [(48000, 2), (44100, 1), (16000, 1)] {
            let mut r = To16k::new(rate, channels);
            let mut out = Vec::new();
            for tone in [
                sine(1000.0, rate, 1.0),
                sine(12000.0f32.min(rate as f32 * 0.45), rate, 1.0),
            ] {
                let interleaved: Vec<f32> = tone
                    .iter()
                    .flat_map(|s| std::iter::repeat_n(*s, channels))
                    .collect();
                out.clear();
                // In uneven pieces, as audio callbacks come.
                for piece in interleaved.chunks(channels * 517) {
                    r.push(piece, &mut out);
                }
                assert!(
                    (out.len() as i64 - 16000).abs() <= 2,
                    "{rate}: {}",
                    out.len()
                );
            }
            let mut speech = Vec::new();
            let tone: Vec<f32> = sine(1000.0, rate, 1.0)
                .iter()
                .flat_map(|s| std::iter::repeat_n(*s, channels))
                .collect();
            To16k::new(rate, channels).push(&tone, &mut speech);
            assert!((steady_rms(&speech) - 0.3535).abs() < 0.03, "{rate}");
            if rate > 16000 {
                let mut alias = Vec::new();
                let high: Vec<f32> = sine(12000.0, rate, 1.0)
                    .iter()
                    .flat_map(|s| std::iter::repeat_n(*s, channels))
                    .collect();
                To16k::new(rate, channels).push(&high, &mut alias);
                // 12 kHz would fold onto 4 kHz; it is cut by more than 20 dB.
                assert!(steady_rms(&alias) < 0.035, "{rate}: {}", steady_rms(&alias));
            }
        }
    }

    #[test]
    fn the_radio_keeps_the_voice_band_and_cuts_lows_and_highs() {
        let rate = 48000;
        let through = |freq: f32| {
            let mut radio = Radio::new(rate);
            let out: Vec<f32> = sine(freq, rate, 0.5)
                .iter()
                .map(|s| radio.process(s * 0.2))
                .collect();
            steady_rms(&out)
        };
        let voice = through(1000.0);
        let low = through(80.0);
        let high = through(9000.0);
        assert!(voice > 0.05, "{voice}");
        assert!(low < voice * 0.15, "{low} vs {voice}");
        assert!(high < voice * 0.15, "{high} vs {voice}");
        // Silence in: only the hiss and the odd crackle come out, quietly.
        let mut radio = Radio::new(rate);
        let quiet: Vec<f32> = (0..rate).map(|_| radio.process(0.0)).collect();
        let (peak, rms) = peak_rms(&quiet);
        assert!(rms > 0.002 && rms < 0.03, "{rms}");
        assert!(peak <= 1.0);
    }

    #[test]
    fn biquads_settle_to_silence() {
        let mut f = Biquad::low_pass(3000.0, 48000.0, 0.707);
        f.process(1.0);
        let tail = (0..48000).map(|_| f.process(0.0)).last().unwrap();
        assert_eq!(tail, 0.0);
    }
}
