//! What a companion picks from its inventory: the best weapon, armour piece,
//! tool for a block and food, scored from the items' own data components.
use crate::items::{ItemStack, Items};

/// Foods a player would rather not eat: they poison, nauseate or teleport.
const BAD_FOOD: [&str; 6] = [
    "minecraft:rotten_flesh",
    "minecraft:spider_eye",
    "minecraft:poisonous_potato",
    "minecraft:pufferfish",
    "minecraft:chorus_fruit",
    "minecraft:suspicious_stew",
];

/// A melee weapon's worth: its attack damage, then its attack speed (the
/// modifiers' amounts; a bare hand is 0). None for what isn't a weapon.
pub fn weapon_score(items: &Items, stack: &ItemStack) -> Option<f32> {
    let info = items.get(&stack.item)?;
    (info.attack_damage > 0.0).then_some(info.attack_damage * 10.0 + info.attack_speed)
}

/// An armour piece's worth and its slot (0 head .. 3 feet): its armour, then
/// its toughness and knockback resistance.
pub fn armor_score(items: &Items, stack: &ItemStack) -> Option<(usize, f64)> {
    let info = items.get(&stack.item)?;
    let slot = match info.equip_slot.as_deref()? {
        "head" => 0,
        "chest" => 1,
        "legs" => 2,
        "feet" => 3,
        _ => return None,
    };
    // An elytra is worn for gliding, not chosen over a chestplate.
    if info.glider {
        return None;
    }
    let amount = |attribute: &str| {
        info.modifiers
            .iter()
            .filter(|m| m.0 == attribute)
            .map(|m| m.2)
            .sum::<f64>()
    };
    let score = amount("minecraft:armor") * 100.0
        + amount("minecraft:armor_toughness") * 10.0
        + amount("minecraft:knockback_resistance");
    // Curse of binding sticks: never put it on by choice.
    (score > 0.0 && stack.enchantment_level("minecraft:binding_curse") == 0)
        .then_some((slot, score))
}

/// A food's worth when hungry: nutrition then saturation; None for what isn't
/// food or is a food players avoid.
pub fn food_score(items: &Items, stack: &ItemStack) -> Option<f32> {
    if BAD_FOOD.contains(&&*stack.item) {
        return None;
    }
    let (nutrition, saturation) = items.get(&stack.item)?.food?;
    Some(nutrition as f32 * 10.0 + saturation)
}

/// The best of `stacks` by `score`, as an index.
pub fn best<T: PartialOrd + Copy>(
    stacks: &[Option<ItemStack>],
    mut score: impl FnMut(&ItemStack) -> Option<T>,
) -> Option<(usize, T)> {
    let mut best: Option<(usize, T)> = None;
    for (i, s) in stacks.iter().enumerate() {
        let Some(v) = s.as_ref().and_then(&mut score) else {
            continue;
        };
        if best.is_none_or(|(_, b)| v > b) {
            best = Some((i, v));
        }
    }
    best
}

/// The tools a companion knows to ask for, cheapest first within each kind.
pub const TOOLS: [&str; 25] = [
    "minecraft:wooden_pickaxe",
    "minecraft:stone_pickaxe",
    "minecraft:copper_pickaxe",
    "minecraft:iron_pickaxe",
    "minecraft:diamond_pickaxe",
    "minecraft:netherite_pickaxe",
    "minecraft:wooden_shovel",
    "minecraft:stone_shovel",
    "minecraft:copper_shovel",
    "minecraft:iron_shovel",
    "minecraft:diamond_shovel",
    "minecraft:netherite_shovel",
    "minecraft:wooden_axe",
    "minecraft:stone_axe",
    "minecraft:copper_axe",
    "minecraft:iron_axe",
    "minecraft:diamond_axe",
    "minecraft:netherite_axe",
    "minecraft:wooden_hoe",
    "minecraft:stone_hoe",
    "minecraft:copper_hoe",
    "minecraft:iron_hoe",
    "minecraft:diamond_hoe",
    "minecraft:netherite_hoe",
    "minecraft:shears",
];

/// How well a stack digs `block` (`Tool.getMiningSpeed`), and whether the
/// block drops with it (`Tool.isCorrectForDrops`, or the block needs no tool).
pub fn dig_score(
    items: &Items,
    stack: Option<&ItemStack>,
    block: &str,
    requires_tool: bool,
) -> (bool, f32) {
    let speed = items.mining_speed(stack, block);
    let drops = !requires_tool || items.correct_for_drops(stack, block);
    (drops, speed)
}

/// The best stack of `stacks` to dig `block` with: one it drops with first,
/// then the fastest; a slot with no tool (or an empty hand) when nothing
/// beats the hand, so that a sword or a pickaxe isn't worn for nothing. None
/// when the hand is as good and no slot is free of tools.
pub fn best_tool(
    items: &Items,
    stacks: &[Option<ItemStack>],
    block: &str,
    requires_tool: bool,
) -> Option<usize> {
    let hand = dig_score(items, None, block, requires_tool);
    let mut best: Option<(usize, (bool, f32))> = None;
    for (i, s) in stacks.iter().enumerate() {
        let Some(s) = s else { continue };
        let score = dig_score(items, Some(s), block, requires_tool);
        let better = |a: (bool, f32), b: (bool, f32)| a.0 && !b.0 || a.0 == b.0 && a.1 > b.1;
        if better(score, hand) && best.is_none_or(|(_, b)| better(score, b)) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i).or_else(|| {
        // Bare hands: an empty slot, else something that takes no wear.
        stacks.iter().take(9).position(Option::is_none).or_else(|| {
            stacks.iter().take(9).position(|s| {
                s.as_ref()
                    .is_some_and(|s| items.get(&s.item).is_none_or(|i| i.max_damage == 0))
            })
        })
    })
}

/// The cheapest tool that makes `block` drop (`needs a stone pickaxe`).
pub fn tool_needed(items: &Items, block: &str) -> Option<&'static str> {
    TOOLS
        .into_iter()
        .find(|t| items.correct_for_drops(Some(&ItemStack::new(t, 1)), block))
}
