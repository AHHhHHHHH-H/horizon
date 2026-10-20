//! Companions (docs/companions.md): an AI teammate with a player's body. This
//! is the part without the game: what the brain and the reflexes can tell the
//! body to do ([`CompanionCommand`]), what the body tells them back
//! ([`CompanionReport`]), the rules its skills decide by, its file in the
//! world folder and the compact state text the brain reads
//! ([`summary::state_summary`]). The body itself, a LAN guest with no
//! connection, lives in the game (game/companion.rs).
pub mod file;
pub mod gear;
pub mod summary;

use crate::text::{self, Text};

/// What the brain or the reflexes ask of the companion. Commands queue and
/// apply at the start of its next server tick, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum CompanionCommand {
    /// Keep 3 to 6 blocks from the owner; sprint to catch up, teleport past 48.
    Follow,
    /// Hold this spot and fight hostiles that come within 12 blocks.
    Stay,
    /// Come to the owner, then wait there.
    Come,
    /// Walk to a block, or to what the owner looks at.
    GoTo(GoTarget),
    /// Drop the current skill and stand still (no fighting until told).
    Stop,
    /// Fight an entity by id, or the nearest hostile.
    Attack(AttackTarget),
    /// Follow and fight whatever threatens the owner, even before it strikes.
    Defend,
    /// Hold fire (true) blocks every attack, self-defence included, until
    /// "fire" (false).
    HoldFire(bool),
    Jump,
    Crouch(bool),
    /// Eat the best food it carries, whatever its hunger.
    Eat,
}

/// Where [`CompanionCommand::GoTo`] goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GoTarget {
    Block([i32; 3]),
    /// The block the owner's crosshair hits, up to [`CROSSHAIR_RANGE`] away.
    Crosshair,
}

/// Whom [`CompanionCommand::Attack`] fights.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackTarget {
    Entity(u32),
    NearestHostile,
}

/// The skill the body runs: one at a time, each a small state machine ticked
/// on the server (game/companion_body.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skill {
    Idle,
    Follow,
    Stay,
    Come,
    GoTo,
    Attack,
    Defend,
    Retreat,
}

impl Skill {
    pub fn name(self) -> &'static str {
        match self {
            Skill::Idle => "idle",
            Skill::Follow => "follow",
            Skill::Stay => "stay",
            Skill::Come => "come",
            Skill::GoTo => "go to",
            Skill::Attack => "attack",
            Skill::Defend => "defend",
            Skill::Retreat => "retreat",
        }
    }
}

/// What the body tells the brain, in the order it happened.
#[derive(Clone, Debug, PartialEq)]
pub enum CompanionReport {
    SkillStarted(Skill),
    SkillFinished(Skill),
    SkillFailed { skill: Skill, reason: String },
    /// It lost health; `by` names the entity type that hit it, when one did.
    Hurt { amount: f32, by: Option<String> },
    Died { cause: String },
    Respawned,
    /// A hostile came into view: its type, the clock bearing from the
    /// owner's facing (12 ahead, 3 right) and its distance from the owner.
    SawHostile { kind: String, bearing: u8, distance: f32 },
    /// It was too far behind and teleported beside the owner.
    Teleported,
    Ate { item: String },
    Equipped { item: String },
    PickedUp { item: String, count: u32 },
    /// It came along when the owner changed dimension, or stayed behind.
    ChangedDimension { dimension: String },
    StayedBehind { dimension: String },
}

impl CompanionReport {
    /// One short line, for the state summary's recent events and the logs.
    pub fn text(&self) -> String {
        match self {
            Self::SkillStarted(s) => format!("started {}", s.name()),
            Self::SkillFinished(s) => format!("finished {}", s.name()),
            Self::SkillFailed { skill, reason } => format!("{} failed: {reason}", skill.name()),
            Self::Hurt { amount, by } => match by {
                Some(by) => format!("hurt {amount:.0} by {}", short_id(by)),
                None => format!("hurt {amount:.0}"),
            },
            Self::Died { cause } => format!("died ({cause})"),
            Self::Respawned => "respawned".into(),
            Self::SawHostile {
                kind,
                bearing,
                distance,
            } => format!("{} at {bearing} o'clock {distance:.0}m", short_id(kind)),
            Self::Teleported => "teleported to owner".into(),
            Self::Ate { item } => format!("ate {}", short_id(item)),
            Self::Equipped { item } => format!("equipped {}", short_id(item)),
            Self::PickedUp { item, count } => format!("picked up {count} {}", short_id(item)),
            Self::ChangedDimension { dimension } => format!("came along to {}", short_id(dimension)),
            Self::StayedBehind { dimension } => format!("stayed in {}", short_id(dimension)),
        }
    }
}

/// A command in words, as the dev command and the control socket take it:
/// `follow`, `stay` (`guard`), `come`, `goto <x> <y> <z>`, `goto there`
/// (the owner's crosshair), `stop`, `attack [<entity id>|nearest]`,
/// `defend`, `hold` / `fire` (`holdfire on|off`), `jump`, `crouch [on|off]`,
/// `eat`.
pub fn parse_command(words: &[&str]) -> Result<CompanionCommand, String> {
    let on_off = |w: Option<&&str>, default: bool| match w.copied() {
        None => Ok(default),
        Some("on" | "true" | "1") => Ok(true),
        Some("off" | "false" | "0") => Ok(false),
        Some(other) => Err(format!("on or off, not {other}")),
    };
    let Some(first) = words.first() else {
        return Err("which command?".into());
    };
    Ok(match first.to_ascii_lowercase().as_str() {
        "follow" => CompanionCommand::Follow,
        "stay" | "guard" | "wait" => CompanionCommand::Stay,
        "come" => CompanionCommand::Come,
        "goto" | "go" => match &words[1..] {
            [] | ["there"] | ["crosshair"] => CompanionCommand::GoTo(GoTarget::Crosshair),
            [x, y, z] => {
                let n = |v: &str| v.parse::<i32>().map_err(|_| format!("not a block coordinate: {v}"));
                CompanionCommand::GoTo(GoTarget::Block([n(x)?, n(y)?, n(z)?]))
            }
            _ => return Err("goto <x> <y> <z>, or goto there".into()),
        },
        "stop" => CompanionCommand::Stop,
        "attack" | "fight" => match words.get(1).copied() {
            None | Some("nearest") => CompanionCommand::Attack(AttackTarget::NearestHostile),
            Some(id) => CompanionCommand::Attack(AttackTarget::Entity(
                id.parse().map_err(|_| format!("not an entity id: {id}"))?,
            )),
        },
        "defend" | "protect" => CompanionCommand::Defend,
        "hold" => CompanionCommand::HoldFire(true),
        "fire" => CompanionCommand::HoldFire(false),
        "holdfire" => CompanionCommand::HoldFire(on_off(words.get(1), true)?),
        "jump" => CompanionCommand::Jump,
        "crouch" | "sneak" => CompanionCommand::Crouch(on_off(words.get(1), true)?),
        "uncrouch" | "stand" => CompanionCommand::Crouch(false),
        "eat" => CompanionCommand::Eat,
        other => return Err(format!("unknown companion command: {other}")),
    })
}

/// `minecraft:zombie` as `zombie`.
pub fn short_id(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

// ---- the rules the skills follow ------------------------------------------

/// Follow keeps between these distances from the owner.
pub const FOLLOW_NEAR: f64 = 3.0;
pub const FOLLOW_FAR: f64 = 6.0;
/// Past this it sprints to catch up.
pub const FOLLOW_SPRINT: f64 = 10.0;
/// Past this it teleports beside the owner (tamed animals do it at 12; the
/// companion walks much further first, as a player would).
pub const FOLLOW_TELEPORT: f64 = 48.0;
/// Stay / guard fights hostiles within this radius of its post.
pub const GUARD_RADIUS: f64 = 12.0;
/// The owner's crosshair ray for "go there".
pub const CROSSHAIR_RANGE: f64 = 128.0;
/// Hostiles this close to the owner or the companion are noticed.
pub const NOTICE_RADIUS: f64 = 16.0;
/// It eats below this food level (players can't sprint at 6 and stop healing below 18).
pub const EAT_BELOW: i32 = 14;
/// It backs off below this health, and is brave again above `BRAVE_AGAIN`.
pub const RETREAT_BELOW: f32 = 6.0;
pub const BRAVE_AGAIN: f32 = 12.0;
/// Its body respawns this many ticks after it died (10 s).
pub const RESPAWN_TICKS: u32 = 200;
/// It comes along through a dimension change within this distance of the owner.
pub const COME_ALONG: f64 = 16.0;

/// What Follow does at a distance from the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FollowStep {
    /// Close enough: stand (or wander to pick up items).
    Hold,
    Walk,
    Sprint,
    Teleport,
}

/// Follow's choice at `distance` blocks, `moving` when it is already on its
/// way: it starts walking past [`FOLLOW_FAR`] and keeps going until within
/// [`FOLLOW_NEAR`], so it doesn't stop and start at one distance.
pub fn follow_step(distance: f64, moving: bool) -> FollowStep {
    if distance > FOLLOW_TELEPORT {
        FollowStep::Teleport
    } else if distance > FOLLOW_SPRINT {
        FollowStep::Sprint
    } else if distance > FOLLOW_FAR || moving && distance > FOLLOW_NEAR {
        FollowStep::Walk
    } else {
        FollowStep::Hold
    }
}

/// Mth.wrapDegrees.
pub fn wrap_degrees(degrees: f64) -> f64 {
    let d = degrees % 360.0;
    if d >= 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// The yaw (Minecraft's: 0 south, 90 west) from `from` toward `to`.
pub fn yaw_toward(from: [f64; 3], to: [f64; 3]) -> f64 {
    (-(to[0] - from[0])).atan2(to[2] - from[2]).to_degrees()
}

/// The clock bearing of `to` seen from `from` facing `yaw`: 12 straight
/// ahead, 3 to the right, 6 behind, 9 to the left.
pub fn clock_bearing(from: [f64; 3], yaw: f32, to: [f64; 3]) -> u8 {
    let relative = wrap_degrees(yaw_toward(from, to) - f64::from(yaw));
    // Turning right raises the yaw, so a positive angle is to the right.
    let hour = (relative / 30.0).round().rem_euclid(12.0) as u8;
    if hour == 0 { 12 } else { hour }
}

/// The horizontal distance between two points.
pub fn horizontal(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The distance between two points.
pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

// ---- looks ----------------------------------------------------------------

/// The name tag and player list name: the name, then a small aqua AI marker.
pub fn name_text(name: &str) -> Text {
    let mut t = Text::literal(name);
    t.push(marker());
    t
}

/// The marker after a companion's name: ` [AI]`, the brackets in aqua.
pub fn marker() -> Text {
    let mut t = Text::literal(" ");
    t.push(Text::literal("[AI]").with_color(text::AQUA));
    t
}

/// [`name_text`] as the player list's display name for the guests.
pub fn listed_name(name: &str) -> horizon_world::nbt::Tag {
    crate::lan::data::component_nbt(&name_text(name).component())
}

/// The default skins a companion can wear, as vanilla picks them for a
/// profile without a skin (`DefaultPlayerSkin.get(UUID)`): wearing the one its
/// UUID gives, it looks the same to vanilla clients.
pub const SKINS: [&str; 9] = [
    "alex", "ari", "efe", "kai", "makena", "noor", "steve", "sunny", "zuri",
];

/// A random UUID (version 4) that vanilla's default skin choice maps to
/// `skin` (`slim/alex`, `wide/steve`...): its low bits are walked until the
/// skin matches, so no skin needs to be sent.
pub fn uuid_for_skin(seed: u64, skin: Option<&str>) -> u128 {
    let mut state = seed | 1;
    let mut next = || {
        // SplitMix64.
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    loop {
        let raw = u128::from(next()) << 64 | u128::from(next());
        // Version 4, IETF variant.
        let uuid = raw & !(0xF << 76) | (0x4 << 76);
        let uuid = uuid & !(0x3 << 62) | (0x2 << 62);
        let wanted = skin.is_none_or(|s| {
            let got = crate::tab_list::default_skin(uuid);
            got.ends_with(&format!("/{s}")) || got == format!("entity/player/{s}")
        });
        if wanted {
            return uuid;
        }
    }
}

/// A profile name for a new companion: 3 to 16 letters, digits or `_`
/// (`StringUtil.isValidPlayerName`), from `wanted` or a list of its own.
pub fn valid_name(wanted: &str) -> Option<String> {
    let ok = (3..=16).contains(&wanted.len())
        && wanted
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then(|| wanted.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_bearings_follow_the_facing() {
        let o = [0.0, 64.0, 0.0];
        // Facing south (+Z): ahead is 12, west (-X) is on the right.
        assert_eq!(clock_bearing(o, 0.0, [0.0, 64.0, 10.0]), 12);
        assert_eq!(clock_bearing(o, 0.0, [-10.0, 64.0, 0.0]), 3);
        assert_eq!(clock_bearing(o, 0.0, [0.0, 64.0, -10.0]), 6);
        assert_eq!(clock_bearing(o, 0.0, [10.0, 64.0, 0.0]), 9);
        // Facing west (yaw 90): north (-Z) is on the right.
        assert_eq!(clock_bearing(o, 90.0, [0.0, 64.0, -10.0]), 3);
        assert_eq!(clock_bearing(o, 90.0, [-10.0, 64.0, 10.0]), 10);
    }

    #[test]
    fn follow_keeps_its_distance_without_dithering() {
        assert_eq!(follow_step(2.0, false), FollowStep::Hold);
        assert_eq!(follow_step(5.0, false), FollowStep::Hold);
        assert_eq!(follow_step(5.0, true), FollowStep::Walk);
        assert_eq!(follow_step(2.9, true), FollowStep::Hold);
        assert_eq!(follow_step(7.0, false), FollowStep::Walk);
        assert_eq!(follow_step(20.0, false), FollowStep::Sprint);
        assert_eq!(follow_step(49.0, true), FollowStep::Teleport);
    }

    #[test]
    fn a_skin_is_chosen_through_the_uuid() {
        for skin in ["slim/alex", "wide/steve", "wide/zuri"] {
            let uuid = uuid_for_skin(7, Some(skin));
            assert!(crate::tab_list::default_skin(uuid).ends_with(skin));
            assert_eq!((uuid >> 76) & 0xF, 4, "version 4");
        }
        assert_ne!(uuid_for_skin(1, None), uuid_for_skin(2, None));
    }

    #[test]
    fn commands_parse_from_words() {
        let p = |line: &str| parse_command(&line.split(' ').collect::<Vec<_>>());
        assert_eq!(p("follow"), Ok(CompanionCommand::Follow));
        assert_eq!(p("guard"), Ok(CompanionCommand::Stay));
        assert_eq!(p("goto 1 64 -3"), Ok(CompanionCommand::GoTo(GoTarget::Block([1, 64, -3]))));
        assert_eq!(p("goto there"), Ok(CompanionCommand::GoTo(GoTarget::Crosshair)));
        assert_eq!(p("attack 42"), Ok(CompanionCommand::Attack(AttackTarget::Entity(42))));
        assert_eq!(p("attack"), Ok(CompanionCommand::Attack(AttackTarget::NearestHostile)));
        assert_eq!(p("holdfire off"), Ok(CompanionCommand::HoldFire(false)));
        assert_eq!(p("crouch"), Ok(CompanionCommand::Crouch(true)));
        assert!(p("dance").is_err());
        assert!(p("goto 1 2").is_err());
    }

    #[test]
    fn names_are_profile_names() {
        assert_eq!(valid_name("Kira_2").as_deref(), Some("Kira_2"));
        assert_eq!(valid_name("ab"), None);
        assert_eq!(valid_name("no spaces"), None);
        assert_eq!(valid_name("waytoolongforaprofile"), None);
        assert_eq!(name_text("Ari").plain(), "Ari [AI]");
    }
}
