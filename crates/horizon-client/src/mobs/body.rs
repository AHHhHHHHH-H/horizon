//! `LivingEntity.travel` and `Entity.move` for mobs: air, water and lava
//! movement, step-up collision, climbing, fall damage, step sounds and the
//! fluid state (`updateInWaterStateAndDoFluidPushing`).
use super::{Cx, Damage, DamageKind, Mob, Rand, flags, species};
use crate::{blocks::Fluid, collisions::SceneCollisions};
use horizon_core::geometry::{Aabb, Vec3};

impl Mob {
    /// Fluid heights over the deflated bounding box.
    pub(super) fn update_fluid_state(&mut self, cx: &mut Cx) {
        let b = self.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        let (x0, x1) = ((lo.x + 0.001).floor() as i32, (hi.x - 0.001).ceil() as i32);
        let (y0, y1) = ((lo.y + 0.001).floor() as i32, (hi.y - 0.001).ceil() as i32);
        let (z0, z1) = ((lo.z + 0.001).floor() as i32, (hi.z - 0.001).ceil() as i32);
        let mut water: f64 = 0.0;
        let mut lava: f64 = 0.0;
        let mut flow_water = [0.0f64; 3];
        let mut flow_count = 0;
        for x in x0..x1 {
            for y in y0..y1 {
                for z in z0..z1 {
                    let f = cx.w.facts.get(cx.level.state(x, y, z));
                    if f.fluid == Fluid::None {
                        continue;
                    }
                    let above = cx.w.facts.get(cx.level.state(x, y + 1, z));
                    let height = if above.fluid == f.fluid { 1.0 } else { f64::from(f.fluid_level) / 9.0 };
                    let top = f64::from(y) + height;
                    if top >= lo.y + 0.001 {
                        let depth = top - (lo.y + 0.001);
                        match f.fluid {
                            Fluid::Water => {
                                water = water.max(depth);
                                if f.fluid_falling {
                                    flow_water[1] -= 1.0;
                                    flow_count += 1;
                                }
                            }
                            Fluid::Lava => lava = lava.max(depth),
                            Fluid::None => {}
                        }
                    }
                }
            }
        }
        let was_in_water = self.in_water;
        self.water_height = water;
        self.lava_height = lava;
        self.in_water = water > 0.0;
        self.in_lava = lava > 0.0;
        if self.in_water {
            if !was_in_water && self.fall_distance > 0.0 {
                // Entity.doWaterSplashEffect is cosmetic; falling stops in water.
            }
            self.fall_distance = 0.0;
            if self.fire_ticks > 0 {
                self.fire_ticks = 0;
            }
            if flow_count > 0 && flags_affected(self) {
                // Falling water drags entities down a little (simplified flow).
                self.velocity.y += flow_water[1] / f64::from(flow_count) * 0.014;
            }
        }
        if self.in_lava {
            self.fall_distance *= 0.5;
        }
        let eye = self.eye_y() - 0.111_111_11;
        let (ex, ey, ez) = (self.position.x.floor() as i32, eye.floor() as i32, self.position.z.floor() as i32);
        let f = cx.w.facts.get(cx.level.state(ex, ey, ez));
        self.eye_in_water = f.fluid == Fluid::Water && {
            let above = cx.w.facts.get(cx.level.state(ex, ey + 1, ez));
            let height = if above.fluid == Fluid::Water { 1.0 } else { f64::from(f.fluid_level) / 9.0 };
            eye < f64::from(ey) + height
        };
    }
    /// Feed `xxa/yya/zza` into `travel`.
    pub(super) fn travel_tick(&mut self, cx: &mut Cx) {
        let input = [f64::from(self.xxa), f64::from(self.yya), f64::from(self.zza)];
        if self.flags.contains(flags::FLYING_MOVE) {
            self.travel_flying(cx, input);
        } else if self.in_liquid() && flags_affected(self) && !species::can_stand_on_fluid(self) {
            self.travel_in_fluid(cx, input);
        } else {
            self.travel_in_air(cx, input);
        }
    }
    /// `Entity.moveRelative`.
    pub fn move_relative(&mut self, speed: f32, input: [f64; 3]) {
        let len = input[0] * input[0] + input[1] * input[1] + input[2] * input[2];
        if len < 1.0e-7 {
            return;
        }
        let mut m = input;
        if len > 1.0 {
            let l = len.sqrt();
            m = m.map(|v| v / l);
        }
        let m = m.map(|v| v * f64::from(speed));
        let yaw = self.yaw.to_radians();
        let (s, c) = (f64::from(yaw.sin()), f64::from(yaw.cos()));
        self.velocity = Vec3::new(self.velocity.x + (m[0] * c - m[2] * s), self.velocity.y + m[1], self.velocity.z + (m[2] * c + m[0] * s));
    }
    fn friction_below(&self, cx: &Cx) -> f32 {
        let (x, y, z) = (self.position.x.floor() as i32, (self.position.y - 0.500_000_1).floor() as i32, self.position.z.floor() as i32);
        cx.w.facts.get(cx.level.state(x, y, z)).friction
    }
    fn on_climbable(&self, cx: &Cx) -> bool {
        if self.flags.contains(flags::CLIMBER) && self.data.climbing {
            return true;
        }
        let [x, y, z] = self.block_pos();
        cx.w.rules.block_in_tag("minecraft:climbable", cx.level.state(x, y, z))
    }
    /// `travelInAir` (ground friction, gravity, air drag).
    fn travel_in_air(&mut self, cx: &mut Cx, input: [f64; 3]) {
        let block_friction = if self.on_ground { self.friction_below(cx) } else { 1.0 };
        let speed = if self.on_ground {
            if block_friction > 0.6 { self.speed * (0.216_000_02 / (block_friction * block_friction * block_friction)) } else { self.speed }
        } else {
            species::flying_speed(self)
        };
        self.move_relative(speed, input);
        let climbable = self.on_climbable(cx);
        if climbable {
            self.fall_distance = 0.0;
            let v = self.velocity;
            self.velocity = Vec3::new(v.x.clamp(-0.15, 0.15), v.y.max(-0.15), v.z.clamp(-0.15, 0.15));
        }
        self.move_self(cx);
        let mut m = self.velocity;
        if (self.horizontal_collision || self.jumping) && climbable {
            m.y = 0.2;
        }
        let mut y = m.y;
        if self.flags.contains(flags::NO_GRAVITY) {
        } else if cx.level.world.loaded(self.position.x.floor() as i32, self.position.z.floor() as i32) {
            y -= self.attributes.gravity;
        } else if self.position.y > f64::from(cx.level.min_y_value()) {
            y = -0.1;
        } else {
            y = 0.0;
        }
        let friction = f64::from(block_friction * 0.91);
        self.velocity = Vec3::new(m.x * friction, y * f64::from(0.98f32), m.z * friction);
    }
    /// `travelInFluid` (water and lava).
    fn travel_in_fluid(&mut self, cx: &mut Cx, input: [f64; 3]) {
        let falling = self.velocity.y <= 0.0;
        let old_y = self.position.y;
        let gravity = if self.flags.contains(flags::NO_GRAVITY) { 0.0 } else { self.attributes.gravity };
        if self.in_water {
            let mut slow = species::water_slow_down(self);
            let mut speed = 0.02f32;
            let mut efficiency = self.attributes.water_movement_efficiency as f32;
            if !self.on_ground {
                efficiency *= 0.5;
            }
            if efficiency > 0.0 {
                slow += (0.546_000_06 - slow) * efficiency;
                speed += (self.speed - speed) * efficiency;
            }
            self.move_relative(speed, input);
            self.move_self(cx);
            let mut m = self.velocity;
            if self.horizontal_collision && self.on_climbable(cx) {
                m.y = 0.2;
            }
            m = Vec3::new(m.x * f64::from(slow), m.y * f64::from(0.8f32), m.z * f64::from(slow));
            self.velocity = fluid_falling_adjusted(gravity, falling, m);
        } else {
            self.move_relative(0.02, input);
            self.move_self(cx);
            if self.lava_height <= self.fluid_jump_threshold() {
                let v = self.velocity;
                self.velocity = fluid_falling_adjusted(gravity, falling, Vec3::new(v.x * 0.5, v.y * f64::from(0.8f32), v.z * 0.5));
            } else {
                self.velocity = Vec3::new(self.velocity.x * 0.5, self.velocity.y * 0.5, self.velocity.z * 0.5);
            }
            if gravity != 0.0 {
                self.velocity.y -= gravity / 4.0;
            }
        }
        // jumpOutOfFluid
        let v = self.velocity;
        if self.horizontal_collision && self.is_free(cx, Vec3::new(v.x, v.y + 0.6 - self.position.y + old_y, v.z)) {
            self.velocity.y = 0.3;
        }
    }
    /// `travelFlying` (ghasts, phantoms): no gravity, uniform drag.
    fn travel_flying(&mut self, cx: &mut Cx, input: [f64; 3]) {
        let (speed, drag) = if self.in_water {
            (0.02, 0.8)
        } else if self.in_lava {
            (0.02, 0.5)
        } else {
            (species::flying_speed(self), 0.91)
        };
        self.move_relative(speed, input);
        self.move_self(cx);
        self.velocity = Vec3::new(self.velocity.x * drag, self.velocity.y * drag, self.velocity.z * drag);
    }
    /// `Entity.isFree(dx, dy, dz)`: no collision and no liquid.
    fn is_free(&self, cx: &Cx, d: Vec3) -> bool {
        let b = self.bounds();
        let (lo, hi) = (b.min_position(), b.max_position());
        let moved = Aabb::new(Vec3::new(lo.x + d.x, lo.y + d.y, lo.z + d.z), Vec3::new(hi.x + d.x, hi.y + d.y, hi.z + d.z));
        let mut scene = SceneCollisions { world: cx.w.world, binding: cx.w.collision };
        if !scene.unobstructed(moved).unwrap_or(false) {
            return false;
        }
        let (ml, mh) = (moved.min_position(), moved.max_position());
        for x in ml.x.floor() as i32..mh.x.ceil() as i32 {
            for y in ml.y.floor() as i32..mh.y.ceil() as i32 {
                for z in ml.z.floor() as i32..mh.z.ceil() as i32 {
                    if cx.w.facts.get(cx.level.state(x, y, z)).fluid != Fluid::None {
                        return false;
                    }
                }
            }
        }
        true
    }
    /// `Entity.move(SELF, deltaMovement)` with collisions, step-up, block
    /// speed factors, fall damage and step sounds.
    fn move_self(&mut self, cx: &mut Cx) {
        let wanted = self.velocity;
        // Cobweb / sweet berry bush stuck multipliers.
        let [bx, by, bz] = self.block_pos();
        let inside = cx.level.name_at(bx, by, bz);
        let stuck = match inside {
            "minecraft:cobweb" if !self.flags.contains(flags::CLIMBER) => Some([0.25, 0.05, 0.25]),
            "minecraft:sweet_berry_bush" => Some([0.8, 0.75, 0.8]),
            "minecraft:powder_snow" => Some([0.9, 1.5, 0.9]),
            _ => None,
        };
        let wanted = match stuck {
            Some(m) => {
                self.velocity = Vec3::ZERO;
                Vec3::new(wanted.x * m[0], wanted.y * m[1], wanted.z * m[2])
            }
            None => wanted,
        };
        let mut scene = SceneCollisions { world: cx.w.world, binding: cx.w.collision };
        let moved = scene.resolve(self.bounds(), wanted, self.max_up_step(), self.on_ground).unwrap_or(Vec3::ZERO);
        self.position = Vec3::new(self.position.x + moved.x, self.position.y + moved.y, self.position.z + moved.z);
        let x_col = (moved.x - wanted.x).abs() > 1.0e-5;
        let z_col = (moved.z - wanted.z).abs() > 1.0e-5;
        self.horizontal_collision = x_col || z_col;
        self.vertical_collision = (moved.y - wanted.y).abs() > 1.0e-5;
        self.on_ground = self.vertical_collision && wanted.y < 0.0;
        if self.horizontal_collision {
            self.velocity = Vec3::new(if x_col { 0.0 } else { self.velocity.x }, self.velocity.y, if z_col { 0.0 } else { self.velocity.z });
        }
        // Fall damage (Entity.checkFallDamage / LivingEntity.causeFallDamage).
        if self.on_ground {
            if self.fall_distance > 0.0 {
                let d = self.fall_distance;
                self.fall_distance = 0.0;
                species::on_land(self, cx, d);
                self.cause_fall_damage(cx, d);
            }
        } else if moved.y < 0.0 {
            self.fall_distance -= moved.y;
        }
        if self.vertical_collision {
            // Block.updateEntityMovementAfterFallOn: slime blocks bounce.
            let below = cx.level.name_at(self.position.x.floor() as i32, (self.position.y - 0.2).floor() as i32, self.position.z.floor() as i32);
            if below == "minecraft:slime_block" && wanted.y < 0.0 {
                self.velocity.y = -wanted.y;
            } else {
                self.velocity.y = 0.0;
            }
        }
        // Block speed factor (soul sand, honey).
        let factor = block_speed_factor(cx, self);
        if factor != 1.0 {
            self.velocity = Vec3::new(self.velocity.x * f64::from(factor), self.velocity.y, self.velocity.z * f64::from(factor));
        }
        // Step sounds (Entity.move: moveDist / nextStep).
        let horizontal = (moved.x * moved.x + moved.z * moved.z).sqrt();
        if self.on_ground && horizontal > 0.0 {
            self.data.move_dist += (horizontal * 0.6) as f32;
            if self.data.move_dist > self.data.next_step {
                self.data.next_step = self.data.move_dist + 1.0;
                species::play_step_sound(self, cx);
            }
        }
    }
    fn cause_fall_damage(&mut self, cx: &mut Cx, distance: f64) {
        if species::ignores_fall_damage(self) {
            return;
        }
        let damage = ((distance - self.attributes.safe_fall_distance) * 1.0).ceil();
        if damage <= 0.0 {
            return;
        }
        let at = self.pos();
        let sound = if damage > 4.0 { species::fall_sound(self, true) } else { species::fall_sound(self, false) };
        cx.sound(sound, at, 1.0, 1.0);
        if self.hurt_by(Damage::plain(damage as f32, DamageKind::Fall), cx.w.game_time) {
            let s = if self.dead { self.death_sound() } else { self.hurt_sound() };
            let pitch = self.voice_pitch();
            cx.sound(s, at, species::sound_volume(self), pitch);
        }
    }
    /// Hop over small obstacles when strafing or idle: `JumpControl` helper.
    pub fn random_hop(&mut self) -> bool {
        self.random.int(2) == 0
    }
}
fn flags_affected(mob: &Mob) -> bool {
    !mob.flags.contains(flags::IGNORES_FLUIDS)
}
/// `getFluidFallingAdjustedMovement`.
fn fluid_falling_adjusted(gravity: f64, falling: bool, m: Vec3) -> Vec3 {
    if gravity != 0.0 {
        let y = if falling && (m.y - 0.005).abs() >= 0.003 && (m.y - gravity / 16.0).abs() < 0.003 { -0.003 } else { m.y - gravity / 16.0 };
        Vec3::new(m.x, y, m.z)
    } else {
        m
    }
}
/// `getBlockSpeedFactor`: the block at the feet, else the block below.
fn block_speed_factor(cx: &Cx, mob: &Mob) -> f32 {
    let [x, y, z] = mob.block_pos();
    let factor = |name: &str| match name {
        "minecraft:soul_sand" | "minecraft:honey_block" => 0.4,
        _ => 1.0,
    };
    let here = cx.level.name_at(x, y, z);
    if here == "minecraft:water" || here == "minecraft:bubble_column" {
        return 1.0;
    }
    let f = factor(here);
    if f != 1.0 {
        return f;
    }
    factor(cx.level.name_at(x, (mob.position.y - 0.500_000_1).floor() as i32, z))
}
