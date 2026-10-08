//! `MoveControl`, `LookControl`, `JumpControl` and `BodyRotationControl`.
use super::{
    Mob, MobFlags,
    path::{PathLevel, PathType, path_type_static},
    wrap_degrees,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Operation {
    #[default]
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}
#[derive(Clone, Debug, Default)]
pub struct MoveControl {
    pub wanted: [f64; 3],
    pub speed_modifier: f64,
    pub strafe_forwards: f32,
    pub strafe_right: f32,
    pub operation: Operation,
}
impl MoveControl {
    pub fn has_wanted(&self) -> bool {
        self.operation == Operation::MoveTo
    }
    pub fn set_wanted_position(&mut self, at: [f64; 3], speed: f64) {
        self.wanted = at;
        self.speed_modifier = speed;
        if self.operation != Operation::Jumping {
            self.operation = Operation::MoveTo;
        }
    }
    pub fn strafe(&mut self, forwards: f32, right: f32) {
        self.operation = Operation::Strafe;
        self.strafe_forwards = forwards;
        self.strafe_right = right;
        self.speed_modifier = 0.25;
    }
}
#[derive(Clone, Debug, Default)]
pub struct LookControl {
    pub y_max_rot_speed: f32,
    pub x_max_rot_angle: f32,
    pub look_at_cooldown: i32,
    pub wanted: [f64; 3],
}
impl LookControl {
    pub fn set_look_at(&mut self, at: [f64; 3], y_max: f32, x_max: f32) {
        self.wanted = at;
        self.y_max_rot_speed = y_max;
        self.x_max_rot_angle = x_max;
        self.look_at_cooldown = 2;
    }
    pub fn is_looking_at_target(&self) -> bool {
        self.look_at_cooldown > 0
    }
}
#[derive(Clone, Debug, Default)]
pub struct BodyRotationControl {
    head_stable_time: i32,
    last_stable_y_head_rot: f32,
}

/// `Mth.degreesDifference` clamped step (`Control.rotateTowards`).
pub fn rotate_towards(from: f32, to: f32, max: f32) -> f32 {
    from + wrap_degrees(to - from).clamp(-max, max)
}
/// `Mth.rotateIfNecessary`.
pub fn rotate_if_necessary(base: f32, target: f32, max: f32) -> f32 {
    let delta = wrap_degrees(base - target);
    let clamped = delta.clamp(-max, max);
    target + clamped
}
/// `MoveControl.rotlerp` (result wrapped into 0..360).
pub fn rotlerp(a: f32, b: f32, max: f32) -> f32 {
    let diff = wrap_degrees(b - a).clamp(-max, max);
    let mut r = a + diff;
    if r < 0.0 {
        r += 360.0;
    } else if r > 360.0 {
        r -= 360.0;
    }
    r
}
fn atan2_deg(y: f64, x: f64) -> f32 {
    (y.atan2(x) * (180.0 / std::f64::consts::PI)) as f32
}

impl Mob {
    /// Speed the move control applies: `speedModifier * MOVEMENT_SPEED`.
    fn modified_speed(&self, modifier: f64) -> f32 {
        (modifier * self.attributes.movement_speed) as f32
    }
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.zza = speed;
    }
    /// `MoveControl.tick`.
    pub(super) fn tick_move_control<L: PathLevel>(&mut self, level: &L) {
        if self.flags.contains(MobFlags::FLYING_MOVE) {
            return self.tick_flying_move_control();
        }
        let mc = &mut self.move_control;
        match mc.operation {
            Operation::Strafe => {
                let speed = self.attributes.movement_speed as f32;
                let modified = mc.speed_modifier as f32 * speed;
                let (mut xa, mut za) = (mc.strafe_forwards, mc.strafe_right);
                let mut dist = (xa * xa + za * za).sqrt();
                if dist < 1.0 {
                    dist = 1.0;
                }
                dist = modified / dist;
                xa *= dist;
                za *= dist;
                let (s, c) = (self.yaw.to_radians().sin(), self.yaw.to_radians().cos());
                let dx = xa * c - za * s;
                let dz = za * c + xa * s;
                let (bx, by, bz) = ((self.position.x + f64::from(dx)).floor() as i32, self.position.y.floor() as i32, (self.position.z + f64::from(dz)).floor() as i32);
                if path_type_static(level, bx, by, bz) != PathType::Walkable {
                    self.move_control.strafe_forwards = 1.0;
                    self.move_control.strafe_right = 0.0;
                }
                let (f, r) = (self.move_control.strafe_forwards, self.move_control.strafe_right);
                self.set_speed(modified);
                self.zza = f;
                self.xxa = r;
                self.move_control.operation = Operation::Wait;
            }
            Operation::MoveTo => {
                mc.operation = Operation::Wait;
                let xd = mc.wanted[0] - self.position.x;
                let zd = mc.wanted[2] - self.position.z;
                let yd = mc.wanted[1] - self.position.y;
                let dd = xd * xd + yd * yd + zd * zd;
                if dd < f64::from(2.500_000_3e-7_f32) {
                    self.zza = 0.0;
                    return;
                }
                let y_rot_d = atan2_deg(zd, xd) - 90.0;
                self.yaw = rotlerp(self.yaw, y_rot_d, 90.0);
                let speed = self.modified_speed(self.move_control.speed_modifier);
                self.set_speed(speed);
                let (bx, by, bz) = (self.position.x.floor() as i32, self.position.y.floor() as i32, self.position.z.floor() as i32);
                let top = level.collision_top(bx, by, bz);
                let step_up = yd > f64::from(self.max_up_step()) && xd * xd + zd * zd < f64::from(self.width().max(1.0));
                let inside = top.is_some_and(|t| self.position.y < t + f64::from(by)) && !level.is_door_or_fence(bx, by, bz);
                if step_up || inside {
                    self.jump_control = true;
                    self.move_control.operation = Operation::Jumping;
                }
            }
            Operation::Jumping => {
                let speed = self.modified_speed(mc.speed_modifier);
                self.set_speed(speed);
                if self.on_ground || (self.in_liquid() && !self.flags.contains(MobFlags::IGNORES_FLUIDS)) {
                    self.move_control.operation = Operation::Wait;
                }
            }
            Operation::Wait => {
                self.zza = 0.0;
            }
        }
    }
    /// `FlyingMoveControl`-style steering used by ghasts and blazes'
    /// custom controls (simplified: straight-line wanted-position seeking).
    fn tick_flying_move_control(&mut self) {
        let mc = &mut self.move_control;
        if mc.operation == Operation::MoveTo {
            mc.operation = Operation::Wait;
            let xd = mc.wanted[0] - self.position.x;
            let yd = mc.wanted[1] - self.position.y;
            let zd = mc.wanted[2] - self.position.z;
            let dd = xd * xd + yd * yd + zd * zd;
            if dd < f64::from(2.500_000_3e-7_f32) {
                self.yya = 0.0;
                self.zza = 0.0;
                return;
            }
            let y_rot_d = atan2_deg(zd, xd) - 90.0;
            self.yaw = rotlerp(self.yaw, y_rot_d, 90.0);
            let speed = if self.on_ground { self.attributes.movement_speed } else { self.attributes.flying_speed } * mc.speed_modifier;
            self.speed = speed as f32;
            let sd = (xd * xd + zd * zd).sqrt();
            if yd.abs() > 1.0e-5 || sd.abs() > 1.0e-5 {
                let x_rot_d = -atan2_deg(yd, sd);
                self.pitch = rotlerp(self.pitch, x_rot_d, 20.0);
                self.yya = if yd > 0.0 { self.speed } else { -self.speed };
            }
            self.zza = self.speed;
        } else {
            self.yya = 0.0;
            self.zza = 0.0;
        }
    }
    /// `LookControl.tick` (plus `clampHeadRotationToBody`).
    pub(super) fn tick_look_control(&mut self) {
        if self.flags.contains(MobFlags::KEEP_PITCH) {
            // SmoothSwimming/flying look controls keep their pitch.
        } else {
            self.pitch = 0.0;
        }
        if self.look_control.look_at_cooldown > 0 {
            self.look_control.look_at_cooldown -= 1;
            let lc = self.look_control.clone();
            let xd = lc.wanted[0] - self.position.x;
            let zd = lc.wanted[2] - self.position.z;
            if zd.abs() > 1.0e-5 || xd.abs() > 1.0e-5 {
                let y = atan2_deg(zd, xd) - 90.0;
                self.head_yaw = rotate_towards(self.head_yaw, y, lc.y_max_rot_speed);
            }
            let yd = lc.wanted[1] - self.eye_y();
            let sd = (xd * xd + zd * zd).sqrt();
            if yd.abs() > 1.0e-5 || sd.abs() > 1.0e-5 {
                let x = -atan2_deg(yd, sd);
                self.pitch = rotate_towards(self.pitch, x, lc.x_max_rot_angle);
            }
        } else {
            self.head_yaw = rotate_towards(self.head_yaw, self.body_yaw, 10.0);
        }
        if !self.nav.is_done() {
            self.head_yaw = rotate_if_necessary(self.head_yaw, self.body_yaw, self.max_head_y_rot() as f32);
        }
    }
    /// `JumpControl.tick`.
    pub(super) fn tick_jump_control(&mut self) {
        self.jumping = self.jump_control;
        self.jump_control = false;
    }
    /// `BodyRotationControl.clientTick` (`Mob.tickHeadTurn`).
    pub(super) fn tick_body_rotation(&mut self) {
        let xd = self.position.x - self.previous.x;
        let zd = self.position.z - self.previous.z;
        let max = self.max_head_y_rot() as f32;
        let bc = &mut self.body_control;
        if xd * xd + zd * zd > f64::from(2.500_000_3e-7_f32) {
            self.body_yaw = self.yaw;
            self.head_yaw = rotate_if_necessary(self.head_yaw, self.body_yaw, max);
            bc.last_stable_y_head_rot = self.head_yaw;
            bc.head_stable_time = 0;
        } else if (self.head_yaw - bc.last_stable_y_head_rot).abs() > 15.0 {
            bc.head_stable_time = 0;
            bc.last_stable_y_head_rot = self.head_yaw;
            self.body_yaw = rotate_if_necessary(self.body_yaw, self.head_yaw, max);
        } else {
            bc.head_stable_time += 1;
            if bc.head_stable_time > 10 {
                let t = bc.head_stable_time - 10;
                let fraction = (t as f32 / 10.0).clamp(0.0, 1.0);
                let remaining = max * (1.0 - fraction);
                self.body_yaw = rotate_if_necessary(self.body_yaw, self.head_yaw, remaining);
            }
        }
    }
    /// `Mob.lookAt(entity, yMax, xMax)` (direct rotation, used while strafing).
    pub fn look_at_direct(&mut self, at: [f64; 3], eye_y: f64, y_max: f32, x_max: f32) {
        let xd = at[0] - self.position.x;
        let zd = at[2] - self.position.z;
        let yd = eye_y - self.eye_y();
        let sd = (xd * xd + zd * zd).sqrt();
        let y_rot_d = atan2_deg(zd, xd) - 90.0;
        let x_rot_d = -atan2_deg(yd, sd);
        self.pitch += wrap_degrees(x_rot_d - self.pitch).clamp(-x_max, x_max);
        self.yaw += wrap_degrees(y_rot_d - self.yaw).clamp(-y_max, y_max);
    }
}
