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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Female,
    Male,
}

/// One designed voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Character {
    /// The id settings and requests name (`Speak::voice`).
    pub id: &'static str,
    /// The name the voice list shows.
    pub label: &'static str,
    /// What it sounds like, in a few words.
    pub description: &'static str,
    pub gender: Gender,
    /// The personality it is the default voice of.
    pub personality: Option<&'static str>,
    /// The VoiceDesign description it was made from (with an emotion appended per clip).
    pub design: &'static str,
    /// The VoiceDesign seed of its anchor clip.
    pub seed: u32,
}

/// Languages with their own clips. Other languages take the voice of the English neutral clip
/// without its delivery (Qwen3-TTS Base would copy its accent).
pub const CLIP_LANGUAGES: &[&str] = &["en", "fr"];

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

macro_rules! character {
    ($id:literal, $label:literal, $gender:ident, $desc:literal, $personality:expr, $design:literal, $seed:literal) => {
        Character {
            id: $id,
            label: $label,
            description: $desc,
            gender: Gender::$gender,
            personality: $personality,
            design: $design,
            seed: $seed,
        }
    };
}

/// The cast, the default voice first.
pub const CAST: &[Character] = &[
    character!(
        "veteran",
        "Veteran",
        Male,
        "Gruff, deep, battle-worn",
        Some("veteran"),
        "A gruff, deep male voice of a battle-worn veteran soldier in his fifties, gravelly and rough, low pitch, a bit hoarse, grumpy and world-weary.",
        3
    ),
    character!(
        "coach",
        "Coach",
        Female,
        "Cheerful, warm, encouraging",
        Some("coach"),
        "A cheerful, energetic young woman in her late twenties, a friendly sports coach, warm mid-high pitch, bright and encouraging, clear articulation.",
        3
    ),
    character!(
        "expert",
        "Expert",
        Female,
        "Calm, precise, composed",
        Some("expert"),
        "A calm, precise woman in her forties, a composed scientist, smooth medium-low pitch, measured pace, clear and confident, unhurried.",
        1
    ),
    character!(
        "joker",
        "Joker",
        Male,
        "Cocky, cheeky, a grin in his voice",
        Some("joker"),
        "A cocky young man in his twenties, a cheeky prankster, playful and smug, medium pitch, quick and bouncy delivery, a grin in his voice.",
        2
    ),
    character!(
        "quiet",
        "Quiet one",
        Female,
        "Soft-spoken, gentle, a little shy",
        Some("quiet"),
        "A soft-spoken shy young woman in her twenties, gentle and airy, light breathy voice, slow and careful, slightly timid.",
        1
    ),
    character!(
        "captain",
        "Old captain",
        Male,
        "Raspy old sea dog",
        None,
        "A raspy old sea captain in his sixties, a pirate-like rough gravelly voice, heavy and husky, rolling and theatrical, a weathered growl.",
        3
    ),
    character!(
        "speedrunner",
        "Speedrunner",
        Male,
        "Hyped, fast-talking streamer",
        None,
        "An energetic young male video game streamer around twenty, fast-talking and hyped, high energy, bright tenor, excited and punchy.",
        1
    ),
    character!(
        "nana",
        "Nana",
        Female,
        "Warm, kind older woman",
        None,
        "A warm, kind older woman in her late sixties, a gentle grandmother, soft and motherly, slightly low and mellow, reassuring and affectionate.",
        2
    ),
    character!(
        "teen",
        "Teen",
        Female,
        "Sarcastic, bored, unimpressed",
        None,
        "A sarcastic teenage girl around sixteen, bored and unimpressed, slightly nasal mid-high pitch, eye-rolling drawl, deadpan.",
        1
    ),
    character!(
        "knight",
        "Knight",
        Male,
        "Deep, stoic, noble",
        None,
        "A deep, stoic male knight in his forties, a resonant bass voice, solemn and steady, slow and noble, firm and controlled.",
        1
    ),
    character!(
        "sunny",
        "Sunny",
        Female,
        "Bright, bubbly, upbeat",
        None,
        "A bright, upbeat young woman around twenty, bubbly and sparkly, high pitch, giggly and enthusiastic, fast and lively.",
        1
    ),
    character!(
        "butler",
        "Butler",
        Male,
        "Dry, refined, British",
        None,
        "A dry, refined older British butler in his fifties, crisp received pronunciation, polite and understated, medium-low pitch, impeccably composed.",
        1
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

impl Character {
    /// Every clip of the character.
    pub fn clips(&self) -> impl Iterator<Item = Clip> + '_ {
        REFERENCE_LINES
            .iter()
            .map(move |&(language, emotion, text)| Clip {
                name: clip_name(self.id, language, emotion),
                language,
                emotion,
                text,
            })
    }

    /// The clip a line in `language` with this emotion is said from: that language's clip, else
    /// the English one (see `SpeechClient::request` for how a cloning model takes it then).
    pub fn reference(&self, language: &str, emotion: Emotion) -> Clip {
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
    pub fn reference_name(&self, language: &str, tone: &Tone) -> String {
        self.reference(language, Emotion::for_tone(tone)).name
    }
}

fn clip_name(id: &str, language: &str, emotion: Emotion) -> String {
    format!("hz-{id}-{language}-{}", emotion.id())
}

pub fn character(id: &str) -> Option<&'static Character> {
    CAST.iter().find(|c| c.id == id)
}

/// The voice a personality starts with (the custom personality and unknown ones get the first).
pub fn default_for(personality: &str) -> &'static Character {
    CAST.iter()
        .find(|c| c.personality == Some(personality))
        .unwrap_or(&CAST[0])
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
    fn the_cast_is_well_formed() {
        assert!(CAST.len() >= 10);
        for c in CAST {
            assert_eq!(CAST.iter().filter(|o| o.id == c.id).count(), 1, "{}", c.id);
            assert!(
                c.id.bytes().all(|b| b.is_ascii_lowercase()),
                "{}: ids are file names",
                c.id
            );
            assert!(!c.label.is_empty() && !c.description.is_empty() && !c.design.is_empty());
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
        let men = CAST.iter().filter(|c| c.gender == Gender::Male).count();
        assert_eq!(men * 2, CAST.len(), "as many women as men");
    }

    #[test]
    fn every_personality_has_its_own_default_voice() {
        for p in ["veteran", "coach", "expert", "joker", "quiet"] {
            assert_eq!(default_for(p).personality, Some(p));
        }
        assert_eq!(default_for("custom").id, CAST[0].id);
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
