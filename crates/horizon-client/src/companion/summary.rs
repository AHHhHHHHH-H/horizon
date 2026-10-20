//! The state the brain reads each time it is called: about 250 tokens of
//! plain text, the same layout every time so the model learns where to look.
//! Bearings are clock hours from the owner's facing ("zombie 2 o'clock 6m"),
//! the way a teammate would call them out.
use super::short_id;

/// What the companion knows of the world this tick (game/companion_body.rs
/// gathers it).
#[derive(Clone, Debug, Default)]
pub struct Perception {
    pub name: String,
    pub position: [f64; 3],
    pub dimension: String,
    pub biome: String,
    /// The day time (`World.day_time`).
    pub day_time: i64,
    pub raining: bool,
    pub thundering: bool,
    pub health: f32,
    pub food: i32,
    pub armor: f32,
    pub held: Option<String>,
    /// The inventory, summed by item, most first.
    pub inventory: Vec<(String, u32)>,
    pub owner: Option<OwnerView>,
    /// Entities near the owner, nearest first.
    pub entities: Vec<Nearby>,
    /// Notable blocks near the owner (ores, chests, water, lava, beds),
    /// nearest of each kind first.
    pub blocks: Vec<Nearby>,
    pub skill: String,
    pub hold_fire: bool,
    /// The last few reports, oldest first.
    pub events: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct OwnerView {
    pub name: String,
    pub health: f32,
    pub food: i32,
    pub distance: f64,
    /// The companion's bearing from the owner.
    pub bearing: u8,
    /// What the owner's crosshair rests on: a block or an entity, and how far.
    pub looking_at: Option<(String, f64)>,
}

/// An entity or block, as the owner sees it.
#[derive(Clone, Debug, Default)]
pub struct Nearby {
    pub id: String,
    pub hostile: bool,
    pub bearing: u8,
    pub distance: f64,
}

/// At most this many of each list go in the text.
const ENTITIES: usize = 6;
const BLOCKS: usize = 5;
const ITEMS: usize = 8;
const EVENTS: usize = 5;

/// `ticks` of the day as a clock time and a word (`06:00 morning`).
fn time_of_day(day_time: i64) -> String {
    // Day time 0 is 06:00.
    let t = day_time.rem_euclid(24_000);
    let minutes = (t * 60 / 1000 + 6 * 60) % (24 * 60);
    let word = match t {
        0..=999 => "morning",
        1000..=5999 => "day",
        6000..=11_999 => "afternoon",
        12_000..=13_799 => "dusk",
        13_800..=22_199 => "night",
        _ => "dawn",
    };
    format!("{:02}:{:02} {word}", minutes / 60, minutes % 60)
}

fn nearby(list: &[Nearby], max: usize) -> String {
    let shown: Vec<String> = list
        .iter()
        .take(max)
        .map(|n| {
            let tag = if n.hostile { "!" } else { "" };
            format!("{tag}{} {}h {:.0}m", short_id(&n.id), n.bearing, n.distance)
        })
        .collect();
    let more = list.len().saturating_sub(max);
    match (shown.is_empty(), more) {
        (true, _) => "none".into(),
        (false, 0) => shown.join(", "),
        (false, n) => format!("{} (+{n})", shown.join(", ")),
    }
}

/// The compact state text (docs/companions.md "What it is given", 3).
pub fn state_summary(p: &Perception) -> String {
    let mut out = String::with_capacity(1024);
    let [x, y, z] = p.position.map(|v| v.floor() as i64);
    let weather = if p.thundering {
        "thunder"
    } else if p.raining {
        "rain"
    } else {
        "clear"
    };
    out.push_str(&format!(
        "You: {} at {x} {y} {z}, {}, {}, {}, {weather}.\n",
        p.name,
        short_id(&p.dimension),
        short_id(&p.biome),
        time_of_day(p.day_time)
    ));
    out.push_str(&format!(
        "Health {:.0}/20, food {}/20, armour {:.0}, holding {}.\n",
        p.health,
        p.food,
        p.armor,
        p.held.as_deref().map_or("nothing", short_id)
    ));
    match &p.owner {
        Some(o) => {
            out.push_str(&format!(
                "Owner {}: health {:.0}/20, food {}/20, you are at {}h {:.0}m.",
                o.name, o.health, o.food, o.bearing, o.distance
            ));
            if let Some((what, d)) = &o.looking_at {
                out.push_str(&format!(" Looking at {} {d:.0}m.", short_id(what)));
            }
            out.push('\n');
        }
        None => out.push_str("Owner: not here.\n"),
    }
    let items: Vec<String> = p
        .inventory
        .iter()
        .take(ITEMS)
        .map(|(id, n)| format!("{n} {}", short_id(id)))
        .collect();
    let more = p.inventory.len().saturating_sub(ITEMS);
    out.push_str("Inventory: ");
    if items.is_empty() {
        out.push_str("empty");
    } else {
        out.push_str(&items.join(", "));
        if more > 0 {
            out.push_str(&format!(" (+{more} kinds)"));
        }
    }
    out.push_str(".\n");
    out.push_str(&format!(
        "Near (from owner's view, ! hostile): {}.\n",
        nearby(&p.entities, ENTITIES)
    ));
    out.push_str(&format!("Blocks: {}.\n", nearby(&p.blocks, BLOCKS)));
    out.push_str(&format!(
        "Doing: {}{}.\n",
        p.skill,
        if p.hold_fire { ", holding fire" } else { "" }
    ));
    let start = p.events.len().saturating_sub(EVENTS);
    if p.events.len() > start {
        out.push_str(&format!("Recent: {}.\n", p.events[start..].join("; ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy() -> Perception {
        let near = |id: &str, hostile, bearing, distance| Nearby {
            id: id.into(),
            hostile,
            bearing,
            distance,
        };
        Perception {
            name: "Ari".into(),
            position: [12.7, 64.0, -3.2],
            dimension: "minecraft:overworld".into(),
            biome: "minecraft:old_growth_birch_forest".into(),
            day_time: 13_000,
            raining: true,
            thundering: false,
            health: 14.0,
            food: 17,
            armor: 7.0,
            held: Some("minecraft:iron_sword".into()),
            inventory: (0..12)
                .map(|i| (format!("minecraft:item_number_{i}"), 64 - i))
                .collect(),
            owner: Some(OwnerView {
                name: "Ludovic111".into(),
                health: 9.0,
                food: 12,
                distance: 4.2,
                bearing: 5,
                looking_at: Some(("minecraft:oak_log".into(), 7.9)),
            }),
            entities: (0..9)
                .map(|i| {
                    near(
                        "minecraft:zombie_villager",
                        i % 2 == 0,
                        (i % 12 + 1) as u8,
                        3.0 + f64::from(i),
                    )
                })
                .collect(),
            blocks: (0..8)
                .map(|i| {
                    near(
                        "minecraft:deepslate_diamond_ore",
                        false,
                        2,
                        8.0 + f64::from(i),
                    )
                })
                .collect(),
            skill: "follow".into(),
            hold_fire: true,
            events: (0..9).map(|i| format!("hurt {i} by skeleton")).collect(),
        }
    }

    #[test]
    fn the_summary_stays_compact() {
        let text = state_summary(&busy());
        // About 250 tokens: four characters a token is generous for this text.
        assert!(text.len() < 1000, "{} chars:\n{text}", text.len());
        assert!(text.contains(
            "You: Ari at 12 64 -4, overworld, old_growth_birch_forest, 19:00 dusk, rain."
        ));
        assert!(text.contains(
            "Owner Ludovic111: health 9/20, food 12/20, you are at 5h 4m. Looking at oak_log 8m."
        ));
        assert!(text.contains("!zombie_villager 1h 3m"));
        assert!(text.contains("(+3)"));
        assert!(text.contains("(+4 kinds)"));
        assert!(text.contains("Doing: follow, holding fire."));
        assert!(text.contains("Recent: hurt 4 by skeleton;"));
        assert!(!text.contains("hurt 3 by"));
    }

    #[test]
    fn an_empty_state_still_reads() {
        let text = state_summary(&Perception {
            name: "Kai".into(),
            skill: "idle".into(),
            ..Default::default()
        });
        assert!(text.contains("Owner: not here."));
        assert!(text.contains("Inventory: empty."));
        assert!(text.contains("Near (from owner's view, ! hostile): none."));
        assert!(!text.contains("Recent"));
        assert_eq!(time_of_day(0), "06:00 morning");
        assert_eq!(time_of_day(18_000), "00:00 night");
    }
}
