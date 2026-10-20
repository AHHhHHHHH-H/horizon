//! The companion's voice cast, as the game's and the launcher's voice lists show it: designed
//! characters the player picks from (docs/companions.md, "Voice cast"). Their reference clips, one
//! per emotion and language, are the voice pack (`horizon_senses::cast`, which also makes them
//! speak); here are who they are, which personality starts with which, and the voices a
//! text-to-speech model offers.
use crate::{
    prompt::Personality,
    senses::{self, SenseModel},
};
use std::path::{Path, PathBuf};

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

pub fn character(id: &str) -> Option<&'static Character> {
    CAST.iter().find(|c| c.id == id)
}

/// The voice a personality starts with (the custom personality and unknown ones get the first).
pub fn default_for(personality: &str) -> &'static Character {
    CAST.iter()
        .find(|c| c.personality == Some(personality))
        .unwrap_or(&CAST[0])
}

/// The library name of a character's clip: `hz-<id>-<language>-<emotion>`.
pub fn clip_name(id: &str, language: &str, emotion: &str) -> String {
    format!("hz-{id}-{language}-{emotion}")
}

/// A character's own recording in the unpacked voice pack, to hear it before picking it: its
/// neutral clip in `language` (`fr`, `en`…), else the English one.
pub fn sample_clip(pack: &Path, id: &str, language: &str) -> Option<PathBuf> {
    let language = CLIP_LANGUAGES
        .iter()
        .copied()
        .find(|l| *l == language)
        .unwrap_or("en");
    let path = pack.join(format!("{}.wav", clip_name(id, language, "neutral")));
    path.is_file().then_some(path)
}

/// The voices a text-to-speech model offers the player, as (id, label, description): the cast's
/// characters when it clones and the voice pack is installed (`cast`), then its built-in voices.
/// The game's and the launcher's lists both come from here.
pub fn voice_choices(model: &SenseModel, cast: bool) -> Vec<(String, String, String)> {
    let characters = CAST.iter().filter(|_| cast && model.clone).map(|c| {
        (
            c.id.to_owned(),
            c.label.to_owned(),
            c.description.to_owned(),
        )
    });
    let built_in = model.voices.iter().map(|v| {
        (
            v.id.to_owned(),
            v.label.to_owned(),
            v.description.to_owned(),
        )
    });
    characters.chain(built_in).collect()
}

/// The voice a personality starts with on a model: its character of the cast on a cloning model
/// with the voice pack, else its built-in default.
pub fn default_voice_id(
    model: &SenseModel,
    personality: Personality,
    cast: bool,
) -> Option<String> {
    if cast && model.clone {
        let id = personality.label().to_ascii_lowercase();
        return Some(default_for(&id).id.to_owned());
    }
    senses::default_voice(model, personality).map(|v| v.id.to_owned())
}

/// The voice the companion speaks with: the one chosen for this model if it still offers it, else
/// the personality's default.
pub fn chosen_voice_id(
    settings: &crate::settings::CompanionSettings,
    model: &SenseModel,
    cast: bool,
) -> Option<String> {
    settings
        .voices
        .get(model.id)
        .filter(|id| voice_choices(model, cast).iter().any(|(v, ..)| v == *id))
        .cloned()
        .or_else(|| default_voice_id(model, settings.persona.personality, cast))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CompanionSettings;

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
    fn cloning_models_offer_the_cast_once_the_pack_is_here() {
        let base = senses::model("qwen3-tts-12hz-1.7b-base-q8_0").unwrap();
        let pocket = senses::model("pocket-tts-en-q8_0").unwrap();
        assert!(voice_choices(base, false).is_empty());
        let cast = voice_choices(base, true);
        assert_eq!(cast.len(), CAST.len());
        assert_eq!(cast[0].1, CAST[0].label);
        // Pocket clones too: the cast first, then its own voices.
        let both = voice_choices(pocket, true);
        assert_eq!(both.len(), CAST.len() + pocket.voices.len());
        assert_eq!(voice_choices(pocket, false).len(), pocket.voices.len());

        assert_eq!(
            default_voice_id(base, Personality::Coach, true).as_deref(),
            Some("coach")
        );
        assert_eq!(default_voice_id(base, Personality::Coach, false), None);
        assert_eq!(
            default_voice_id(pocket, Personality::Veteran, false).as_deref(),
            Some("javert")
        );

        let mut settings = CompanionSettings::default();
        settings.persona.personality = Personality::Joker;
        assert_eq!(
            chosen_voice_id(&settings, pocket, true).as_deref(),
            Some("joker")
        );
        settings.voices.insert(pocket.id.into(), "butler".into());
        assert_eq!(
            chosen_voice_id(&settings, pocket, true).as_deref(),
            Some("butler")
        );
        // Without the pack the cast's character is gone: back to the built-in default.
        assert_eq!(
            chosen_voice_id(&settings, pocket, false).as_deref(),
            Some("george")
        );
    }

    #[test]
    fn a_sample_is_the_neutral_clip_in_the_language_or_english() {
        let dir = std::env::temp_dir().join(format!("horizon-cast-sample-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["hz-nana-en-neutral.wav", "hz-nana-fr-neutral.wav"] {
            std::fs::write(dir.join(name), b"RIFF").unwrap();
        }
        assert_eq!(
            sample_clip(&dir, "nana", "fr"),
            Some(dir.join("hz-nana-fr-neutral.wav"))
        );
        assert_eq!(
            sample_clip(&dir, "nana", "de"),
            Some(dir.join("hz-nana-en-neutral.wav"))
        );
        assert_eq!(sample_clip(&dir, "butler", "en"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
