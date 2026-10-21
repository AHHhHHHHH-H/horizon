//! Long distances cheaply: legs of about 20 blocks over standable ground
//! near its own height (the surface, a cave floor, the Nether's ledges),
//! each walked with the vanilla-style path search (`companion_steer`: no
//! digging, no building, never into lava or off a big drop), sprinting.
//! Where no leg is found or the path search gives up, the caller falls back
//! to the walker that digs and builds (`companion_travel`) for a while.
//!
//! An addition to the planner's movement code (game/companion_walk.rs,
//! companion/moves.rs); nothing there changes.
use super::{Game, companion::Body, companion_body::Steer, companion_work::feet, lan::LanHost};
use horizon_client::{blocks::Fluid, companion as c};

/// How far a leg reaches.
const LEG: f64 = 28.0;
/// How far up or down from its feet a leg's end may be.
const RISE: i32 = 10;

/// The leg it walks, and the goal it was planned for.
#[derive(Default, Debug)]
pub(super) struct Cruise {
    leg: Option<[i32; 3]>,
    goal: [i32; 2],
    /// Legs walked (for the status line).
    pub legs: u32,
}

impl Cruise {
    pub fn reset(&mut self) {
        self.leg = None;
    }
}

impl Game<'_> {
    /// Toward `goal` (horizontal) a leg at a time; Arrived within `near`.
    /// Stuck when no leg is found or the path search gave up on one.
    pub(super) fn companion_cruise(
        &mut self,
        host: &LanHost,
        b: &mut Body,
        cruise: &mut Cruise,
        goal: [i32; 2],
        near: f64,
        pos: [f64; 3],
    ) -> Steer {
        let target = [f64::from(goal[0]) + 0.5, pos[1], f64::from(goal[1]) + 0.5];
        if c::horizontal(pos, target) <= near {
            cruise.leg = None;
            return Steer::Arrived;
        }
        if cruise.goal != goal {
            cruise.goal = goal;
            cruise.leg = None;
        }
        let here = feet(pos);
        if cruise
            .leg
            .is_some_and(|l| c::horizontal(pos, centre(l)) < 1.5)
        {
            cruise.leg = None;
            cruise.legs += 1;
        }
        if cruise.leg.is_none() {
            cruise.leg = self.cruise_leg(here, goal);
            b.nav.reset();
        }
        let Some(leg) = cruise.leg else {
            return Steer::Stuck;
        };
        let steer = self.companion_steer(host, b, centre(leg), 1.2, true);
        // Its own look at the ground ahead: never a step toward lava below
        // or a drop of more than three blocks (the path may round a corner
        // close to an edge; a jump carries it further than a step).
        if b.keys.forward && !self.cruise_safe(pos, b.yaw) {
            b.keys.forward = false;
            b.keys.sprint = false;
            b.keys.jump = false;
            cruise.leg = None;
            return Steer::Stuck;
        }
        match steer {
            Steer::Arrived => {
                cruise.leg = None;
                cruise.legs += 1;
                Steer::Moving
            }
            Steer::Moving => Steer::Moving,
            Steer::Stuck => {
                cruise.leg = None;
                Steer::Stuck
            }
        }
    }

    /// The next leg's end: straight on, else turning further each way, a
    /// standable spot near its height in a loaded chunk.
    fn cruise_leg(&self, here: [i32; 3], goal: [i32; 2]) -> Option<[i32; 3]> {
        let (dx, dz) = (f64::from(goal[0] - here[0]), f64::from(goal[1] - here[2]));
        let left = dx.hypot(dz);
        let step = LEG.min(left).max(2.0);
        let toward = dz.atan2(dx);
        for turn in [0.0f64, 25.0, -25.0, 50.0, -50.0, 80.0, -80.0] {
            let a = toward + turn.to_radians();
            let x = here[0] + (a.cos() * step).round() as i32;
            let z = here[2] + (a.sin() * step).round() as i32;
            if !self.world.chunks.contains_key(&(x >> 4, z >> 4)) {
                continue;
            }
            if let Some(y) = self.cruise_stand(x, here[1], z) {
                return Some([x, y, z]);
            }
        }
        None
    }

    /// The ground 0.5 to 2 blocks ahead along `yaw`: no lava in it or
    /// under it, no drop past three blocks.
    pub(super) fn cruise_safe(&self, pos: [f64; 3], yaw: f32) -> bool {
        let facts = self.res.facts();
        let r = f64::from(yaw).to_radians();
        let (dx, dz) = (-r.sin(), r.cos());
        let y0 = pos[1].floor() as i32;
        [0.5, 1.0, 1.5, 2.0].iter().all(|d| {
            let (x, z) = (
                (pos[0] + dx * d).floor() as i32,
                (pos[2] + dz * d).floor() as i32,
            );
            for k in -1..=1 {
                let f = facts.get(self.world.block(x, y0 + k, z));
                if f.fluid == Fluid::Lava || bad(&f.name) {
                    return false;
                }
            }
            // Down to the first floor: lava, or too far, is unsafe.
            for drop in 1..=12 {
                let f = facts.get(self.world.block(x, y0 - drop, z));
                if f.fluid == Fluid::Lava || bad(&f.name) {
                    return false;
                }
                if f.full_collision || low_floor(&f.name) || f.fluid == Fluid::Water {
                    return drop <= 4;
                }
            }
            false
        })
    }

    /// Feet height of a dry, safe spot in column (x, z) nearest `y`.
    pub(super) fn cruise_stand(&self, x: i32, y: i32, z: i32) -> Option<i32> {
        let facts = self.res.facts();
        let at = |y: i32| facts.get(self.world.block(x, y, z));
        let open = |y: i32| {
            let f = at(y);
            !f.full_collision && f.fluid == Fluid::None && !bad(&f.name)
        };
        (0..=RISE).flat_map(|d| [y + d, y - d]).find(|&fy| {
            let floor = at(fy - 1);
            (floor.full_collision || low_floor(&floor.name))
                && floor.fluid == Fluid::None
                && !bad(&floor.name)
                && !floor.name.ends_with("_leaves")
                && open(fy)
                && open(fy + 1)
        })
    }
}

impl Game<'_> {
    /// The nearest dry, safe spot to stand within `radius` of `around`
    /// (rings outwards), near its height: where to start digging down when
    /// `around` is under water.
    /// Spots within 4 blocks of one in `avoid` are passed over.
    pub(super) fn dry_spot_near(
        &self,
        around: [i32; 3],
        radius: i32,
        avoid: &[[i32; 3]],
    ) -> Option<[i32; 3]> {
        (0..=radius).find_map(|r| {
            (-r..=r)
                .flat_map(|dx| (-r..=r).map(move |dz| (dx, dz)))
                .filter(|(dx, dz)| dx.abs().max(dz.abs()) == r)
                .find_map(|(dx, dz)| {
                    let (x, z) = (around[0] + dx, around[2] + dz);
                    if !self.world.chunks.contains_key(&(x >> 4, z >> 4)) {
                        return None;
                    }
                    if avoid
                        .iter()
                        .any(|a| (a[0] - x).abs().max((a[2] - z).abs()) <= 4)
                    {
                        return None;
                    }
                    self.cruise_stand(x, around[1], z)
                        .map(|y| [x, y, z])
                        .filter(|p| self.inland(*p))
                })
        })
    }
}

impl Game<'_> {
    /// No water or lava within two blocks round, from its head down four:
    /// a staircase dug down from here doesn't flood.
    pub(super) fn inland(&self, p: [i32; 3]) -> bool {
        let facts = self.res.facts();
        (-2..=2).all(|dx| {
            (-2..=2).all(|dz| {
                (-4..=1).all(|dy| {
                    facts
                        .get(self.world.block(p[0] + dx, p[1] + dy, p[2] + dz))
                        .fluid
                        == Fluid::None
                })
            })
        })
    }
}

/// Blocks a walk never ends a leg on or in.
/// A floor a little lower than a full block that is walked on like one.
pub(super) fn low_floor(name: &str) -> bool {
    matches!(
        name,
        "minecraft:soul_sand" | "minecraft:mud" | "minecraft:farmland" | "minecraft:dirt_path"
    )
}

fn bad(name: &str) -> bool {
    name.contains("lava")
        || name.contains("fire")
        || name.contains("magma")
        || name.contains("cactus")
        || name.contains("sweet_berry")
        || name.contains("powder_snow")
        || name.contains("campfire")
        || name.contains("pointed_dripstone")
}

fn centre(p: [i32; 3]) -> [f64; 3] {
    [
        f64::from(p[0]) + 0.5,
        f64::from(p[1]),
        f64::from(p[2]) + 0.5,
    ]
}

#[cfg(test)]
mod tests {
    use super::bad;

    #[test]
    fn legs_never_end_on_a_hazard() {
        for name in [
            "minecraft:lava",
            "minecraft:magma_block",
            "minecraft:soul_fire",
            "minecraft:cactus",
        ] {
            assert!(bad(name), "{name}");
        }
        for name in [
            "minecraft:netherrack",
            "minecraft:grass_block",
            "minecraft:warped_nylium",
        ] {
            assert!(!bad(name), "{name}");
        }
    }
}
