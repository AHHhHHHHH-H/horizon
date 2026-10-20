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
