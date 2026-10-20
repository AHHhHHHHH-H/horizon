//! The Partner level's milestone planner: where a run stands on the way to the dragon, read from
//! what the owner and the companion carry and where they are. The game puts the step in the brain's
//! state ("Plan: 3/10 ...") and tells the brain when it moves on, so the plan survives a small model
//! that forgets it.

/// The steps of a run, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    Wood,
    StoneTools,
    Iron,
    Diamonds,
    Nether,
    BlazeRods,
    EnderPearls,
    Stronghold,
    End,
    Dragon,
}

impl Step {
    pub const ALL: [Step; 10] = [
        Step::Wood,
        Step::StoneTools,
        Step::Iron,
        Step::Diamonds,
        Step::Nether,
        Step::BlazeRods,
        Step::EnderPearls,
        Step::Stronghold,
        Step::End,
        Step::Dragon,
    ];

    /// 1 to 10.
    pub fn number(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1
    }

    /// What to do, for the brain (English: the state is English whatever the game's language).
    pub fn goal(self) -> &'static str {
        match self {
            Step::Wood => "get wood (about 16 logs), craft a crafting table and wooden tools",
            Step::StoneTools => "mine cobblestone, craft stone tools and a furnace",
            Step::Iron => "mine and smelt iron: iron pickaxe, then armour and a bucket",
            Step::Diamonds => "find diamonds (deep, y -50 to 16) for a diamond pickaxe",
            Step::Nether => "get obsidian and flint and steel, build a nether portal and go in",
            Step::BlazeRods => "find a nether fortress and kill blazes for 6 or more blaze rods",
            Step::EnderPearls => "get 12 or more ender pearls (endermen, or barter gold with piglins)",
            Step::Stronghold => "craft eyes of ender, throw them to find the stronghold",
            Step::End => "fill the end portal frame and jump in",
            Step::Dragon => "destroy the end crystals and beat the ender dragon",
        }
    }
}

/// The step a run is on: `items` is what the owner and the companion carry together (`oak_log`,
/// `minecraft:oak_log`), `dimension` where the owner is (`minecraft:the_nether`).
pub fn step(items: &[(String, u32)], dimension: &str) -> Step {
    let count = |want: &dyn Fn(&str) -> bool| -> u32 {
        items
            .iter()
            .filter(|(id, _)| want(id.strip_prefix("minecraft:").unwrap_or(id)))
            .map(|(_, n)| n)
            .sum()
    };
    let has = |id: &str| count(&|i| i == id) > 0;
    let dimension = dimension.strip_prefix("minecraft:").unwrap_or(dimension);
    if dimension == "the_end" {
        return Step::Dragon;
    }
    let eyes = count(&|i| i == "ender_eye");
    if eyes >= 12 {
        return Step::End;
    }
    let pearls = count(&|i| i == "ender_pearl") + eyes;
    let rods = count(&|i| i == "blaze_rod") * 2 + count(&|i| i == "blaze_powder");
    if rods >= 12 && pearls >= 12 {
        return Step::Stronghold;
    }
    if rods >= 12 {
        return Step::EnderPearls;
    }
    if dimension == "the_nether" {
        return Step::BlazeRods;
    }
    let diamond_tier = ["diamond", "diamond_pickaxe", "netherite_pickaxe"]
        .iter()
        .any(|i| has(i));
    if diamond_tier {
        return Step::Nether;
    }
    let iron_tier = has("iron_pickaxe") || count(&|i| i == "iron_ingot") >= 3;
    if iron_tier {
        return Step::Diamonds;
    }
    let stone_tier = has("stone_pickaxe") || has("raw_iron") || has("iron_ore");
    if stone_tier {
        return Step::Iron;
    }
    let wood = count(&|i| i.ends_with("_log") || i.ends_with("_stem"))
        + count(&|i| i.ends_with("_planks")) / 4;
    if has("wooden_pickaxe") || wood >= 16 {
        return Step::StoneTools;
    }
    Step::Wood
}

/// The line the brain's state gets.
pub fn state_line(step: Step) -> String {
    format!("Plan: step {}/10, {}.", step.number(), step.goal())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(list: &[(&str, u32)]) -> Vec<(String, u32)> {
        list.iter().map(|(i, n)| ((*i).to_owned(), *n)).collect()
    }

    #[test]
    fn a_run_moves_through_the_steps() {
        let overworld = "minecraft:overworld";
        assert_eq!(step(&[], overworld), Step::Wood);
        assert_eq!(step(&items(&[("minecraft:oak_log", 6)]), overworld), Step::Wood);
        assert_eq!(step(&items(&[("oak_log", 16)]), overworld), Step::StoneTools);
        assert_eq!(step(&items(&[("wooden_pickaxe", 1)]), overworld), Step::StoneTools);
        assert_eq!(step(&items(&[("stone_pickaxe", 1)]), overworld), Step::Iron);
        assert_eq!(step(&items(&[("iron_ingot", 3)]), overworld), Step::Diamonds);
        assert_eq!(step(&items(&[("minecraft:diamond", 2)]), overworld), Step::Nether);
        assert_eq!(step(&items(&[("diamond_pickaxe", 1)]), "minecraft:the_nether"), Step::BlazeRods);
        assert_eq!(step(&items(&[("blaze_rod", 6)]), overworld), Step::EnderPearls);
        assert_eq!(
            step(&items(&[("blaze_rod", 3), ("blaze_powder", 6), ("ender_pearl", 12)]), overworld),
            Step::Stronghold
        );
        assert_eq!(step(&items(&[("ender_eye", 12)]), overworld), Step::End);
        assert_eq!(step(&[], "minecraft:the_end"), Step::Dragon);
    }

    #[test]
    fn the_state_line_numbers_the_step() {
        assert!(state_line(Step::Iron).starts_with("Plan: step 3/10, mine and smelt iron"));
        assert_eq!(Step::Dragon.number(), 10);
    }
}
