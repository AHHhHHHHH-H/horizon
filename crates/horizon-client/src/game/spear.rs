//! Spears: the `attack_range`, `piercing_weapon` and `kinetic_weapon`
//! components (`Player.attack`'s STAB after `PiercingWeapon.attack`, and
//! `KineticWeapon.damageEntities` while an item is used).
use super::Game;
use horizon_client::{
    first_person::{Hand, SwingAnimation},
    items::ItemStack,
    mobs::Damage,
};
use horizon_core::geometry::Aabb;
use serde_json::Value;

const ATTACK_RANGE: &str = "minecraft:attack_range";
const PIERCING_WEAPON: &str = "minecraft:piercing_weapon";
const KINETIC_WEAPON: &str = "minecraft:kinetic_weapon";
const ATTACK_ANIMATION: &str = "minecraft:attack_animation";
const MINIMUM_ATTACK_CHARGE: &str = "minecraft:minimum_attack_charge";

/// `AttackRange`.
#[derive(Clone, Copy, Debug)]
pub(super) struct AttackRange {
    min_reach: f64,
    max_reach: f64,
    min_creative_reach: f64,
    max_creative_reach: f64,
    hitbox_margin: f64,
    mob_factor: f64,
}
impl Default for AttackRange {
    /// `AttackRange.defaultFor`: the entity interaction range, no margin.
    fn default() -> Self {
        Self {
            min_reach: 0.0,
            max_reach: 3.0,
            min_creative_reach: 0.0,
            max_creative_reach: 5.0,
            hitbox_margin: 0.3,
            mob_factor: 1.0,
        }
    }
}
impl AttackRange {
    fn parse(value: Option<&Value>) -> Self {
        let number = |key: &str, default: f64| {
            value
                .and_then(|v| v.get(key))
                .and_then(Value::as_f64)
                .unwrap_or(default)
        };
        Self {
            min_reach: number("min_reach", 0.0),
            max_reach: number("max_reach", 3.0),
            min_creative_reach: number("min_creative_reach", 0.0),
            max_creative_reach: number("max_creative_reach", 5.0),
            hitbox_margin: number("hitbox_margin", 0.3),
            mob_factor: number("mob_factor", 1.0),
        }
    }
    /// `effectiveMinRange` for the local player.
    fn min(&self, creative: bool) -> f64 {
        if creative {
            self.min_creative_reach
        } else {
            self.min_reach
        }
    }
    /// `effectiveMaxRange` for the local player.
    fn max(&self, creative: bool) -> f64 {
        if creative {
            self.max_creative_reach
        } else {
            self.max_reach
        }
    }
    fn margin(&self) -> f64 {
        self.hitbox_margin
    }
    #[allow(dead_code)]
    fn mob_factor(&self) -> f64 {
        self.mob_factor
    }
}

/// `KineticWeapon.Condition`.
#[derive(Clone, Copy, Debug)]
struct Condition {
    max_duration_ticks: u32,
    min_speed: f64,
    min_relative_speed: f64,
}
impl Condition {
    fn parse(value: &Value) -> Self {
        Self {
            max_duration_ticks: value
                .get("max_duration_ticks")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            min_speed: value
                .get("min_speed")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            min_relative_speed: value
                .get("min_relative_speed")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        }
    }
    /// `Condition.test`, with the player's `actionFactor` of 1.
    fn test(&self, ticks_used: u32, attacker_speed: f64, relative_speed: f64) -> bool {
        ticks_used <= self.max_duration_ticks
            && attacker_speed >= self.min_speed
            && relative_speed >= self.min_relative_speed
    }
}

/// `KineticWeapon`.
#[derive(Clone, Debug)]
struct KineticWeapon {
    contact_cooldown_ticks: i64,
    delay_ticks: u32,
    dismount_conditions: Option<Condition>,
    knockback_conditions: Option<Condition>,
    damage_conditions: Option<Condition>,
    damage_multiplier: f64,
    sound: Option<String>,
    hit_sound: Option<String>,
}
impl KineticWeapon {
    fn parse(value: &Value) -> Self {
        let condition = |key: &str| value.get(key).map(Condition::parse);
        let sound = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            contact_cooldown_ticks: value
                .get("contact_cooldown_ticks")
                .and_then(Value::as_i64)
                .unwrap_or(10),
            delay_ticks: value
                .get("delay_ticks")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            dismount_conditions: condition("dismount_conditions"),
            knockback_conditions: condition("knockback_conditions"),
            damage_conditions: condition("damage_conditions"),
            damage_multiplier: value
                .get("damage_multiplier")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            sound: sound("sound"),
            hit_sound: sound("hit_sound"),
        }
    }
    /// The longest damage window among the conditions, after the delay.
    fn last_tick(&self) -> u32 {
        [
            self.dismount_conditions,
            self.knockback_conditions,
            self.damage_conditions,
        ]
        .into_iter()
        .flatten()
        .map(|c| c.max_duration_ticks)
        .max()
        .unwrap_or(0)
    }
}

/// `PiercingWeapon`.
#[derive(Clone, Debug)]
struct PiercingWeapon {
    deals_knockback: bool,
    dismounts: bool,
    sound: Option<String>,
    hit_sound: Option<String>,
}
impl PiercingWeapon {
    fn parse(value: &Value) -> Self {
        let sound = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            deals_knockback: value
                .get("deals_knockback")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            dismounts: value
                .get("dismounts")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            sound: sound("sound"),
            hit_sound: sound("hit_sound"),
        }
    }
}

fn distance_sq(from: [f64; 3], to: [f64; 3]) -> f64 {
    (to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2)
}

/// The entry and exit distances of a ray through `box`, if it overlaps.
fn ray_box_interval(eye: [f64; 3], dir: [f64; 3], reach: f64, b: Aabb) -> Option<(f64, f64)> {
    let min = b.min_position();
    let max = b.max_position();
    let (lo, hi) = ([min.x, min.y, min.z], [max.x, max.y, max.z]);
    let (mut t0, mut t1) = (0.0f64, reach);
    for a in 0..3 {
        if dir[a].abs() < 1e-9 {
            if eye[a] < lo[a] || eye[a] > hi[a] {
                return None;
            }
            continue;
        }
        let (mut ta, mut tb) = ((lo[a] - eye[a]) / dir[a], (hi[a] - eye[a]) / dir[a]);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
    }
    (t0 <= t1).then_some((t0, t1))
}

impl Game<'_> {
    /// One merged item component of `stack`.
    fn item_component(&self, stack: &ItemStack, key: &str) -> Option<Value> {
        let components = self
            .pack
            .item_renderer
            .models
            .components(&horizon_client::item_render::stack_view(stack));
        components.get(key).cloned()
    }
    fn attack_range_of(&self, stack: &ItemStack) -> AttackRange {
        AttackRange::parse(self.item_component(stack, ATTACK_RANGE).as_ref())
    }
    fn piercing_of(&self, stack: &ItemStack) -> Option<PiercingWeapon> {
        self.item_component(stack, PIERCING_WEAPON)
            .as_ref()
            .map(PiercingWeapon::parse)
    }
    fn kinetic_of(&self, stack: &ItemStack) -> Option<KineticWeapon> {
        self.item_component(stack, KINETIC_WEAPON)
            .as_ref()
            .map(KineticWeapon::parse)
    }
    /// `ItemStack.getAttackAnimation` of the main-hand item.
    pub(super) fn attack_animation(&self) -> SwingAnimation {
        let Some(stack) = self.held() else {
            return SwingAnimation::DEFAULT;
        };
        let Some(value) = self.item_component(stack, ATTACK_ANIMATION) else {
            return SwingAnimation::DEFAULT;
        };
        let duration = value
            .get("duration")
            .and_then(Value::as_i64)
            .unwrap_or(6)
            .max(0) as i32;
        let kind = match value.get("type").and_then(Value::as_str) {
            Some("none") => 0,
            Some("stab") => 2,
            Some("whack") | None => 1,
            Some(_) => 1,
        };
        SwingAnimation { kind, duration }
    }
    /// The main-hand item has a `piercing_weapon`.
    pub(super) fn piercing_weapon(&self) -> bool {
        self.held().is_some_and(|s| self.piercing_of(s).is_some())
    }
    /// `Player.cannotAttackWithItem(held, 0)`: the item's minimum attack
    /// charge is not reached yet.
    pub(super) fn cannot_attack_with_item(&self) -> bool {
        let Some(stack) = self.held() else {
            return false;
        };
        let required = self
            .item_component(stack, MINIMUM_ATTACK_CHARGE)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if required <= 0.0 {
            return false;
        }
        let speed = self.attack_item_info().map_or(0.0, |i| i.attack_speed);
        let delay = 20.0 / (4.0 + speed).max(0.1);
        (f64::from(self.attack_ticks) / f64::from(delay)) < required
    }
    fn player_at(&self) -> Option<[f64; 3]> {
        self.player
            .as_ref()
            .map(|p| [p.position.x, p.position.y, p.position.z])
    }
    fn play_at_player(&mut self, sound: &str) {
        if let Some(at) = self.player_at() {
            self.audio.play(sound, Some(at), 1.0, 1.0);
        }
    }
    /// The entities a spear's ray can reach: from `effectiveMinRange` to
    /// `effectiveMaxRange` plus the attacker's forward movement, with the
    /// range's hitbox margin, clipped by colliding blocks.
    fn spear_targets(
        &self,
        eye: [f64; 3],
        dir: [f64; 3],
        range: &AttackRange,
    ) -> Vec<(usize, f64)> {
        let min = range.min(self.creative);
        let forward = self.player.as_ref().map_or(0.0, |p| {
            p.velocity.x * dir[0] + p.velocity.y * dir[1] + p.velocity.z * dir[2]
        });
        let to = range.max(self.creative) + forward.max(0.0);
        let limit = match self.clip_ray(eye, dir, to) {
            Some(hit) => {
                let d = distance_sq(eye, hit.point).sqrt();
                if d < min {
                    return Vec::new();
                }
                d.min(to)
            }
            None => to,
        };
        let margin = range.margin();
        let mut out = Vec::new();
        for (i, mob) in self.mobs.iter().enumerate() {
            if !mob.alive() || mob.removed {
                continue;
            }
            let b = mob
                .bounds()
                .inflate(horizon_core::geometry::Vec3::new(margin, margin, margin));
            if let Some((t0, t1)) = ray_box_interval(eye, dir, limit, b)
                && t1 >= min
                && t0 <= limit
            {
                out.push((i, t0.max(min)));
            }
        }
        out.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        out.into_iter().map(|(i, _)| (i, 0.0)).collect()
    }
    /// `PiercingWeapon.attack`'s damage for `target_kind`, before the attack
    /// strength and enchantments are applied by `stab_mob`.
    fn attack_damage(&self) -> f32 {
        1.0 + self.attack_item_info().map_or(0.0, |i| i.attack_damage) + self.effect_attack_bonus()
    }
    /// `EnchantmentHelper.modifyDamage` for a spear hit on `target_kind`.
    fn spear_enchant_damage(&mut self, stack: &ItemStack, target_kind: &str, base: f32) -> f32 {
        let ctx = horizon_client::enchant::EffectContext {
            this: Some(target_kind),
            attacker: Some("minecraft:player"),
            direct_attacker: Some("minecraft:player"),
            is_direct: true,
            damage_type: Some("minecraft:spear"),
            items: Some(self.res.items()),
            ..Default::default()
        };
        self.res.stations().enchantments.modify_value(
            Some(stack),
            "minecraft:damage",
            &ctx,
            &mut self.world.random,
            base,
        ) - base
    }
    /// `Player.stabAttack` on the mob at `index`: damage (when
    /// `deals_damage`), the 0.4 plus the weapon's knockback, and item wear.
    fn stab_mob(
        &mut self,
        index: usize,
        base_damage: f32,
        damage_scale: f32,
        magic_scale: f32,
        deals_damage: bool,
        deals_knockback: bool,
        dismounts: bool,
    ) -> bool {
        let Some(stack) = self.held().cloned() else {
            return false;
        };
        let target_kind = self.mobs[index].kind.id.clone();
        let target_kind = format!("minecraft:{}", target_kind.trim_start_matches("minecraft:"));
        let magic = if deals_damage {
            self.spear_enchant_damage(&stack, &target_kind, base_damage) * magic_scale
        } else {
            0.0
        };
        let amount = if deals_damage {
            base_damage * damage_scale + magic
        } else {
            0.0
        };
        let old_health = self.mobs[index].health;
        let from = self.player_at().unwrap_or([
            self.mobs[index].position.x,
            self.mobs[index].position.y,
            self.mobs[index].position.z,
        ]);
        let source = Damage::player(from);
        let landed = deals_damage && self.hurt_by_player(index, amount, source);
        let mut affected = landed;
        if deals_knockback {
            let (xd, zd) = self.extra_knockback_direction();
            self.mobs[index].knockback(0.4, xd, zd);
            self.slow_after_stab();
            // `LivingEntity.getKnockback`: the ATTACK_KNOCKBACK attribute
            // and the weapon's enchantment, halved (scale 0 adds no sprinting
            // bonus).
            let (knockback, _) = self.extra_knockback(&target_kind, 0.0);
            if knockback > 0.0 {
                self.mobs[index].knockback(f64::from(knockback), xd, zd);
                self.slow_after_stab();
            }
            affected = true;
        }
        // Dismounting is left out: no spear in the game has the condition
        // for a target this engine can dismount.
        let _ = dismounts;
        if !affected {
            return false;
        }
        if !self.creative
            && let Some(p) = self.player.as_mut()
        {
            p.cause_food_exhaustion(0.1);
        }
        let id = self.mobs[index].id;
        self.combat.last_hurt_mob = Some((id, self.world.game_time));
        if landed {
            let actual = old_health - self.mobs[index].health;
            self.stats
                .custom("damage_dealt", (actual * 10.0).round() as i32);
            self.last_hurt_by_player.insert(id, self.lan_acting);
        }
        if self.hand == Hand::Main {
            self.weapon_hit(index);
        }
        true
    }
    /// `causeExtraKnockback`'s attacker half: a 0.6 horizontal speed cut.
    fn slow_after_stab(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.velocity.x *= 0.6;
            p.velocity.z *= 0.6;
        }
    }
    /// `MultiPlayerGameMode.piercingAttack` on the host: the ray, every
    /// entity it reaches, then the swing and sounds.
    pub(super) fn piercing_attack(&mut self, animation: SwingAnimation) {
        let Some(stack) = self.held().cloned() else {
            return;
        };
        let Some(weapon) = self.piercing_of(&stack) else {
            return;
        };
        let (Some(eye), Some(_)) = (self.eye_position(), self.player.as_ref()) else {
            return;
        };
        let dir = super::view_vector(self.yaw, self.pitch);
        let range = self.attack_range_of(&stack);
        let targets = self.spear_targets(eye, dir, &range);
        let base = self.attack_damage();
        let scale = self.attack_strength_scale();
        let mut hit_something = false;
        for (index, _) in targets {
            hit_something |= self.stab_mob(
                index,
                base,
                0.2 + scale * scale * 0.8,
                scale,
                true,
                weapon.deals_knockback,
                weapon.dismounts,
            );
        }
        if hit_something && let Some(sound) = weapon.hit_sound.as_deref() {
            self.play_at_player(sound);
        }
        if let Some(sound) = weapon.sound.as_deref() {
            self.play_at_player(sound);
        }
        self.swing_hand(Hand::Main, animation);
        self.reset_attack_strength();
    }
    /// `Item.use` for a `kinetic_weapon`: starts the charge and its sound.
    pub(super) fn use_spear(&mut self, stack: &ItemStack) -> bool {
        let Some(weapon) = self.kinetic_of(stack) else {
            return false;
        };
        if self.using.is_none() {
            self.using = Some((self.held_slot(), 0));
            self.spear_hits.clear();
        }
        if let Some(sound) = weapon.sound.as_deref() {
            self.play_at_player(sound);
        }
        true
    }
    /// `KineticWeapon.damageEntities` for the item in use: one tick of the
    /// charge's damage window. Returns whether the item is a kinetic weapon.
    pub(super) fn tick_spear(&mut self, slot: usize, ticks: u32, stack: &ItemStack) -> bool {
        let Some(weapon) = self.kinetic_of(stack) else {
            return false;
        };
        let ticks_used = ticks + 1;
        self.using = Some((slot, ticks_used));
        if ticks_used < weapon.delay_ticks {
            return true;
        }
        let sweep_tick = ticks_used - weapon.delay_ticks;
        if sweep_tick > weapon.last_tick() {
            return true;
        }
        let Some(eye) = self.eye_position() else {
            return true;
        };
        let dir = super::view_vector(self.yaw, self.pitch);
        let range = self.attack_range_of(stack);
        let attacker_speed = self.player.as_ref().map_or(0.0, |p| {
            (p.velocity.x * dir[0] + p.velocity.y * dir[1] + p.velocity.z * dir[2]) * 20.0
        });
        let targets = self.spear_targets(eye, dir, &range);
        let now = self.world.game_time;
        let mut affected = false;
        for (index, _) in targets {
            let id = self.mobs[index].id;
            if self
                .spear_hits
                .get(&id)
                .is_some_and(|last| now - last < weapon.contact_cooldown_ticks)
            {
                continue;
            }
            self.spear_hits.insert(id, now);
            let target_speed = {
                let v = self.mobs[index].velocity;
                (v.x * dir[0] + v.y * dir[1] + v.z * dir[2]) * 20.0
            };
            let relative = (attacker_speed - target_speed).max(0.0);
            let dismounts = weapon
                .dismount_conditions
                .is_some_and(|c| c.test(sweep_tick, attacker_speed, relative));
            let deals_knockback = weapon
                .knockback_conditions
                .is_some_and(|c| c.test(sweep_tick, attacker_speed, relative));
            let deals_damage = weapon
                .damage_conditions
                .is_some_and(|c| c.test(sweep_tick, attacker_speed, relative));
            if !(dismounts || deals_knockback || deals_damage) {
                continue;
            }
            let base = 1.0 + (relative * weapon.damage_multiplier).floor() as f32;
            affected |= self.stab_mob(
                index,
                base,
                1.0,
                1.0,
                deals_damage,
                deals_knockback,
                dismounts,
            );
        }
        if affected && now - self.kinetic_hit_time > 10 {
            self.kinetic_hit_time = now;
            if let Some(sound) = weapon.hit_sound.as_deref() {
                self.play_at_player(sound);
            }
        }
        true
    }
}
