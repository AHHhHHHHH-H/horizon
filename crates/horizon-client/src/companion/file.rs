//! `companions/<uuid>.json` in the world folder: who a companion is. Its body
//! (inventory, position, health) is saved like a guest's player data,
//! `players/data/<uuid>.dat` (lan/playerdata.rs).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's format version.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionFile {
    pub version: u32,
    /// The body's profile UUID (`8-4-4-4-12` hex).
    pub uuid: String,
    pub name: String,
    /// The personality preset (docs/companions.md: veteran, coach, expert,
    /// joker, quiet, custom).
    pub personality: String,
    /// helper, super_strong or partner.
    pub level: String,
    /// The owner's profile UUID.
    pub owner: String,
    /// The brain's rolling memory (slice 3 fills it; "forget everything" clears it).
    pub notes: Vec<String>,
    /// It was in the world when the world was saved: it comes back with it.
    pub active: bool,
    /// What it was doing (follow, stay, defend or idle), where it stood
    /// guard, and whether it held its fire: a reloaded world resumes them.
    pub skill: String,
    pub post: Option<[f64; 3]>,
    pub hold_fire: bool,
    /// The work it had on (collect, give, craft, smelt), as command words
    /// with what is left to do: a reloaded world takes it up again.
    pub task: Option<String>,
}

impl Default for CompanionFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            uuid: String::new(),
            name: String::new(),
            personality: "coach".into(),
            level: "helper".into(),
            owner: String::new(),
            notes: Vec::new(),
            active: true,
            skill: "follow".into(),
            post: None,
            hold_fire: false,
            task: None,
        }
    }
}

/// The companions folder of a world.
pub fn dir(world: &Path) -> PathBuf {
    world.join("companions")
}

impl CompanionFile {
    pub fn path(&self, world: &Path) -> PathBuf {
        dir(world).join(format!("{}.json", self.uuid))
    }
    /// The body's UUID as a number, if the file's is well formed.
    pub fn uuid(&self) -> Option<u128> {
        parse_uuid(&self.uuid)
    }
    pub fn save(&self, world: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir(world))?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Written beside, then renamed: a crash never leaves half a file.
        let path = self.path(world);
        let partial = path.with_extension("json.tmp");
        std::fs::write(&partial, text)?;
        std::fs::rename(partial, path)
    }
    /// Every companion of a world, by name.
    pub fn load_all(world: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(dir(world)) else {
            return Vec::new();
        };
        let mut all: Vec<Self> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| serde_json::from_str::<Self>(&t).ok())
            .filter(|f| f.uuid().is_some())
            .collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));
        all
    }
}

/// A UUID in its `8-4-4-4-12` hex form.
pub fn parse_uuid(text: &str) -> Option<u128> {
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_in_the_world_folder() {
        let world = std::env::temp_dir().join(format!("horizon-companion-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&world);
        let uuid = 0x1234_5678_9abc_4def_8123_4567_89ab_cdefu128;
        let file = CompanionFile {
            uuid: horizon_protocol::crypto::uuid_string(uuid),
            name: "Ari".into(),
            owner: horizon_protocol::crypto::uuid_string(7),
            notes: vec!["likes oak".into()],
            task: Some("collect 3 oak_log".into()),
            ..Default::default()
        };
        file.save(&world).unwrap();
        assert!(
            world
                .join("companions")
                .join(format!("{}.json", file.uuid))
                .is_file()
        );
        let all = CompanionFile::load_all(&world);
        assert_eq!(all, vec![file.clone()]);
        assert_eq!(all[0].uuid(), Some(uuid));
        // Fields a later version adds or drops don't break loading.
        std::fs::write(
            file.path(&world),
            r#"{"uuid":"00000000-0000-4000-8000-000000000001","name":"Zed","mood":1}"#,
        )
        .unwrap();
        let all = CompanionFile::load_all(&world);
        assert_eq!(all[0].name, "Zed");
        assert_eq!(all[0].level, "helper");
        assert!(all[0].active);
        let _ = std::fs::remove_dir_all(&world);
    }
}
