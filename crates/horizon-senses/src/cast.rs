//! The companion's voice cast: designed characters the player picks from, each with one reference
//! clip per emotion and language (docs/companions.md, "Voice cast").
//!
//! The voices are made by us with Qwen3-TTS 1.7B VoiceDesign (Apache-2.0) from a written description
//! and a fixed seed: no real person's voice is recorded or cloned. One English neutral clip is the
//! character's anchor; the other emotions and languages are its closest match, picked by speaker
//! similarity among VoiceDesign takes and VoxCPM2 clones of the anchor, with a checked transcript
//! (`tools/companion_voices/`). At run time a cloning model (Qwen3-TTS Base, Pocket TTS) is given the
//! clip whose emotion matches the line's tone, so the delivery comes from the clip and the timbre
//! stays the character's.
//!
//! The clips are not in the repository: they ship as the voice pack, one archive downloaded with the
//! expressive or natural voice (see [`VOICE_PACK`]) and unpacked into
//! `<data>/companion/voices/<version>/`, which becomes audio.cpp's `voice_dir`. A clip is selected by
//! its library name ([`Character::reference_name`]), and audio.cpp reads its transcript from the
//! pack's `prompt_text` file.
use crate::voice::Tone;

/// The delivery a reference clip carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Emotion {
    Neutral,
    Shout,
    Whisper,
    Laugh,
    Sarcastic,
}

impl Emotion {
    pub const ALL: [Emotion; 5] = [
        Emotion::Neutral,
        Emotion::Shout,
        Emotion::Whisper,
        Emotion::Laugh,
        Emotion::Sarcastic,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Emotion::Neutral => "neutral",
            Emotion::Shout => "shout",
            Emotion::Whisper => "whisper",
            Emotion::Laugh => "laugh",
            Emotion::Sarcastic => "sarcastic",
        }
    }

    /// A tone that is said from this emotion's clip.
    pub fn tone(self) -> Tone {
        match self {
            Emotion::Neutral => Tone::Normal,
            Emotion::Shout => Tone::Shout,
            Emotion::Whisper => Tone::Whisper,
            Emotion::Laugh => Tone::Laugh,
            Emotion::Sarcastic => Tone::Sarcastic,
        }
    }

    /// The clip a line's tone is said from. A scared line borrows the shout (urgent, loud); calm and
    /// free-form tones the neutral clip.
    pub fn for_tone(tone: &Tone) -> Emotion {
        match tone {
            Tone::Shout | Tone::Scared => Emotion::Shout,
            Tone::Whisper => Emotion::Whisper,
            Tone::Laugh => Emotion::Laugh,
            Tone::Sarcastic => Emotion::Sarcastic,
            Tone::Normal | Tone::Calm | Tone::Other(_) => Emotion::Neutral,
        }
    }
}

pub use horizon_companion::cast::{
    CAST, CLIP_LANGUAGES, Character, Gender, character, clip_name, default_for,
};

/// What every reference clip says, by language and emotion (5–8 s each). The transcript must match
/// the audio: Qwen3-TTS Base conditions on both.
pub const REFERENCE_LINES: &[(&str, Emotion, &str)] = &[
    (
        "en",
        Emotion::Neutral,
        "Alright, I've got the torches. Let's head down the mine and see what we can find today.",
    ),
    (
        "en",
        Emotion::Shout,
        "Watch out! There's a whole horde coming over the hill! Get behind the wall, right now, go!",
    ),
    (
        "en",
        Emotion::Whisper,
        "Quiet... I can hear something moving in the dark. Stay close to me and don't make a sound.",
    ),
    (
        "en",
        Emotion::Laugh,
        "Ha ha ha! Did you see that? The pig just jumped off the cliff and landed right in the water!",
    ),
    (
        "en",
        Emotion::Sarcastic,
        "Oh, wonderful. Another hole full of gravel. This is exactly how I wanted to spend my evening.",
    ),
    (
        "fr",
        Emotion::Neutral,
        "Bon, j'ai les torches. On descend dans la mine et on va voir ce qu'on trouve aujourd'hui.",
    ),
    (
        "fr",
        Emotion::Shout,
        "Attention ! Il y a toute une horde qui arrive par la colline ! Derrière le mur, tout de suite, vite !",
    ),
    (
        "fr",
        Emotion::Whisper,
        "Doucement... j'entends quelque chose bouger dans le noir. Reste près de moi et ne fais pas de bruit.",
    ),
    (
        "fr",
        Emotion::Laugh,
        "Ha ha ha ! T'as vu ça ? Le cochon a sauté de la falaise et il est tombé en plein dans l'eau !",
    ),
    (
        "fr",
        Emotion::Sarcastic,
        "Oh, génial. Encore un trou rempli de gravier. C'est exactement comme ça que je voulais passer ma soirée.",
    ),
];

/// A reference clip of the pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    /// Its library name: `<voice_dir>/<name>.wav`, and its line in `prompt_text`.
    pub name: String,
    pub language: &'static str,
    pub emotion: Emotion,
    pub text: &'static str,
}

/// A character's clips in the voice pack.
pub trait Clips {
    fn clips(&self) -> impl Iterator<Item = Clip> + '_;
    fn reference(&self, language: &str, emotion: Emotion) -> Clip;
    fn reference_name(&self, language: &str, tone: &Tone) -> String;
}

impl Clips for Character {
    /// Every clip of the character.
    fn clips(&self) -> impl Iterator<Item = Clip> + '_ {
        REFERENCE_LINES
            .iter()
            .map(move |&(language, emotion, text)| Clip {
                name: clip_name(self.id, language, emotion.id()),
                language,
                emotion,
                text,
            })
    }

    /// The clip a line in `language` with this emotion is said from: that language's clip, else
    /// the English one (see `SpeechClient::request` for how a cloning model takes it then).
    fn reference(&self, language: &str, emotion: Emotion) -> Clip {
        let language = CLIP_LANGUAGES
            .iter()
            .copied()
            .find(|l| *l == language)
            .unwrap_or("en");
        self.clips()
            .find(|c| c.language == language && c.emotion == emotion)
            .expect("every character has a clip per clip language and emotion")
    }

    /// The library name of [`Character::reference`].
    fn reference_name(&self, language: &str, tone: &Tone) -> String {
        self.reference(language, Emotion::for_tone(tone)).name
    }
}

/// The archive of every clip, downloaded once beside the voice models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoicePack {
    /// Its folder under `<data>/companion/voices/`.
    pub version: &'static str,
    /// Unpacked: one 24 kHz mono 16-bit WAV per clip, `prompt_text` and `manifest.json`.
    pub clips: usize,
    /// Measured size of the unpacked WAVs.
    pub size: u64,
    /// The archive once it is hosted: its URL and SHA-256 are pinned like the models'. `None` until
    /// the pack is published with the release that turns the cast on.
    pub archive: Option<horizon_companion::catalogue::ModelFile>,
}

pub const VOICE_PACK: VoicePack = VoicePack {
    version: "v1",
    clips: CAST.len() * REFERENCE_LINES.len(),
    size: 49_189_848,
    archive: None,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_character_has_a_clip_per_language_and_emotion() {
        for c in CAST {
            let clips: Vec<Clip> = c.clips().collect();
            assert_eq!(clips.len(), CLIP_LANGUAGES.len() * Emotion::ALL.len());
            for language in CLIP_LANGUAGES {
                for emotion in Emotion::ALL {
                    let clip = c.reference(language, emotion);
                    assert_eq!((clip.language, clip.emotion), (*language, emotion));
                    assert!(!clip.text.is_empty());
                }
            }
        }
    }

    #[test]
    fn the_tone_picks_the_clip_and_other_languages_use_english() {
        let vet = character("veteran").unwrap();
        assert_eq!(
            vet.reference_name("fr", &Tone::Shout),
            "hz-veteran-fr-shout"
        );
        assert_eq!(
            vet.reference_name("en", &Tone::Scared),
            "hz-veteran-en-shout"
        );
        assert_eq!(
            vet.reference_name("de", &Tone::Whisper),
            "hz-veteran-en-whisper"
        );
        assert_eq!(
            vet.reference_name("en", &Tone::Other("sleepy".into())),
            "hz-veteran-en-neutral"
        );
        assert_eq!(
            vet.reference("fr", Emotion::Laugh).text,
            REFERENCE_LINES[8].2
        );
        assert_eq!(VOICE_PACK.clips, CAST.len() * 10);
        for emotion in Emotion::ALL {
            assert_eq!(Emotion::for_tone(&emotion.tone()), emotion);
        }
    }
}
