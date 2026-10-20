//! The End of the Speedrunner's route (docs/companion-speedrun.md, "The
//! End"), the part without the game: the rules and the geometry the body's
//! End skill decides by (game/companion_end.rs). Arrows' arcs to the
//! crystals, what an explosion does at a distance (vanilla's formula and
//! armour), the plan for a caged crystal (two pillars: one beside the cage
//! to open a window in its bars, one far enough off to shoot through it),
//! and where to stand by a perched dragon: in reach of its head, out of its
//! wings' knockback, its head's and neck's bite and its breath.

/// An arrow at full draw leaves at 3 blocks a tick (BowItem: power 1 × 3).
pub const ARROW_SPEED: f64 = 3.0;
/// AbstractArrow: gravity 0.05 a tick, 0.99 of its speed kept in air.
pub const ARROW_GRAVITY: f64 = 0.05;
pub const ARROW_DRAG: f64 = 0.99;
/// The arrow starts 0.1 below the eye (`Projectile.shootFromRotation`).
pub const ARROW_DROP: f64 = 0.1;
/// End crystals blow up with power 6, beds in the End with power 5.
pub const CRYSTAL_POWER: f32 = 6.0;
pub const BED_POWER: f32 = 5.0;
/// Where the exit portal's podium is (`EndPodiumFeature`, at the origin).
pub const FOUNTAIN: [f64; 2] = [0.5, 0.5];

/// One sample of an arrow's flight: where it is after `tick` ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowPoint {
    pub tick: u32,
    pub at: [f64; 3],
}

/// The pitch (Minecraft degrees, negative up) whose arc at `speed` passes
/// `rise` blocks above its start after `distance` blocks: the flat one and
/// the lob, when they exist. The flat arc first.
pub fn aim_pitches(distance: f64, rise: f64, speed: f64) -> Vec<f32> {
    // The height the arc has when it is `distance` out (None: it never gets there).
    let height_at = |angle: f64| -> Option<f64> {
        let (mut x, mut y) = (0.0, 0.0);
        let (mut vx, mut vy) = (speed * angle.cos(), speed * angle.sin());
        for _ in 0..400 {
            if x + vx >= distance {
                return Some(y + vy * (distance - x) / vx);
            }
            x += vx;
            y += vy;
            vx *= ARROW_DRAG;
            vy = vy * ARROW_DRAG - ARROW_GRAVITY;
            if vx < 1e-3 || y < -400.0 {
                return None;
            }
        }
        None
    };
    let h = |a: f64| height_at(a).unwrap_or(f64::NEG_INFINITY);
    let (lo, hi) = (-1.45, 1.45);
    // The highest the arc gets at that distance: a golden-section search.
    let (mut a, mut b) = (lo, hi);
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..80 {
        let (c, d) = (b - g * (b - a), a + g * (b - a));
        if h(c) < h(d) {
            a = c;
        } else {
            b = d;
        }
    }
    let best = (a + b) / 2.0;
    if h(best) < rise {
        return Vec::new();
    }
    let solve = |mut below: f64, mut above: f64| {
        // `below` misses low, `above` reaches: bisect between.
        for _ in 0..60 {
            let mid = (below + above) / 2.0;
            if h(mid) >= rise {
                above = mid;
            } else {
                below = mid;
            }
        }
        -(above.to_degrees()) as f32
    };
    let mut out = Vec::new();
    if h(lo) < rise {
        out.push(solve(lo, best));
    }
    if h(hi) < rise {
        // On the far side the arc falls as the angle rises.
        let (mut reach, mut miss) = (best, hi);
        for _ in 0..60 {
            let mid = (reach + miss) / 2.0;
            if h(mid) >= rise {
                reach = mid;
            } else {
                miss = mid;
            }
        }
        let lob = -(reach.to_degrees()) as f32;
        if out.iter().all(|p| (p - lob).abs() > 0.5) {
            out.push(lob);
        }
    }
    out
}

/// An arrow's flight from `from` toward `yaw`/`pitch` (Minecraft degrees),
/// one point a tick, until `ticks` or `max_distance` horizontally.
pub fn arrow_flight(
    from: [f64; 3],
    yaw: f32,
    pitch: f32,
    speed: f64,
    ticks: u32,
    max_distance: f64,
) -> Vec<ArrowPoint> {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    let mut v = [
        -y.sin() * p.cos() * speed,
        -p.sin() * speed,
        y.cos() * p.cos() * speed,
    ];
    let mut at = from;
    let mut out = vec![ArrowPoint { tick: 0, at }];
    for tick in 1..=ticks {
        for k in 0..3 {
            at[k] += v[k];
        }
        out.push(ArrowPoint { tick, at });
        if (at[0] - from[0]).hypot(at[2] - from[2]) > max_distance {
            break;
        }
        v = [
            v[0] * ARROW_DRAG,
            v[1] * ARROW_DRAG - ARROW_GRAVITY,
            v[2] * ARROW_DRAG,
        ];
    }
    out
}

/// Where a segment from `a` to `b` first enters box `lo`..`hi`, as a
/// fraction of the way (the slab method), if it does.
pub fn segment_box(a: [f64; 3], b: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for k in 0..3 {
        let d = b[k] - a[k];
        if d.abs() < 1e-12 {
            if a[k] < lo[k] || a[k] > hi[k] {
                return None;
            }
            continue;
        }
        let (mut ta, mut tb) = ((lo[k] - a[k]) / d, (hi[k] - a[k]) / d);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// `ExplosionDamageCalculator.getEntityDamageAmount`: the damage an
/// explosion of `power` does at `distance` (feet to centre) to an entity
/// `exposure` of whose box it sees.
pub fn explosion_damage(power: f32, distance: f64, exposure: f32) -> f32 {
    let diameter = f64::from(power) * 2.0;
    let d = distance / diameter;
    if d > 1.0 {
        return 0.0;
    }
    let pow = (1.0 - d) * f64::from(exposure);
    ((pow * pow + pow) / 2.0 * 7.0 * diameter + 1.0) as f32
}

/// `CombatRules.getDamageAfterAbsorb`: what armour leaves of `damage`.
pub fn after_armor(damage: f32, armor: f32, toughness: f32) -> f32 {
    let t = 2.0 + toughness / 4.0;
    let a = (armor - damage / t).clamp(armor * 0.2, 20.0);
    damage * (1.0 - a / 25.0)
}

/// The nearest distance from an explosion of `power` at which it leaves a
/// body with `armor` at least `keep` health of `health`, at full exposure.
pub fn safe_distance(power: f32, armor: f32, toughness: f32, health: f32, keep: f32) -> f64 {
    let mut d = 0.0;
    while d < f64::from(power) * 2.0 {
        if health - after_armor(explosion_damage(power, d, 1.0), armor, toughness) >= keep {
            return d;
        }
        d += 0.25;
    }
    f64::from(power) * 2.0
}

/// An obsidian spike under a crystal: its centre column, its top (the
/// crystal stands on bedrock at the top, a block above) and whether iron
/// bars guard it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spike {
    pub x: i32,
    pub z: i32,
    /// `EndSpike.height`: the obsidian ends below it, the bedrock is at it.
    pub height: i32,
    pub caged: bool,
}

impl Spike {
    /// The spike a crystal at `at` stands on (EndSpikeFeature puts it at
    /// the centre column, a block over the top).
    pub fn under(at: [f64; 3], caged: bool) -> Self {
        Self {
            x: at[0].floor() as i32,
            z: at[2].floor() as i32,
            height: at[1].floor() as i32 - 1,
            caged,
        }
    }
    /// The crystal's box (EndCrystal: 2 × 2 × 2 from its feet).
    pub fn crystal_box(&self) -> ([f64; 3], [f64; 3]) {
        let (x, y, z) = (
            f64::from(self.x) + 0.5,
            f64::from(self.height + 1),
            f64::from(self.z) + 0.5,
        );
        ([x - 1.0, y, z - 1.0], [x + 1.0, y + 2.0, z + 1.0])
    }
    pub fn crystal_centre(&self) -> [f64; 3] {
        [
            f64::from(self.x) + 0.5,
            f64::from(self.height) + 2.0,
            f64::from(self.z) + 0.5,
        ]
    }
    /// Toward the island's centre along the nearer axis: the side the
    /// companion works a caged spike from.
    pub fn inward(&self) -> [i32; 2] {
        if self.x.abs() >= self.z.abs() {
            [-self.x.signum(), 0]
        } else {
            [0, -self.z.signum()]
        }
    }
}

/// How to break a caged crystal: pillar up at `near` (beside the cage) to
/// stand level with the cage's floor, break the three bars of `window`,
/// come down, pillar up at `far` to the same height and shoot through the
/// window from where the blast barely reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CagePlan {
    pub near: [i32; 2],
    pub far: [i32; 2],
    /// The feet height on both pillars: level with the top of the obsidian.
    pub stand_y: i32,
    /// The bars to break, bottom up.
    pub window: [[i32; 3]; 3],
}

/// The cage plan for `spike`, `near` blocks out from its centre (just off
/// the obsidian: its radius + 1) and shooting from `far` blocks out.
pub fn cage_plan(spike: &Spike, near: i32, far: i32) -> CagePlan {
    let [dx, dz] = spike.inward();
    let at = |n: i32| [spike.x + dx * n, spike.z + dz * n];
    let wall = at(2);
    CagePlan {
        near: at(near.max(3)),
        far: at(far),
        stand_y: spike.height,
        window: [0, 1, 2].map(|dy| [wall[0], spike.height + dy, wall[1]]),
    }
}

/// A box as min and max corners.
pub type Box3 = ([f64; 3], [f64; 3]);

fn inflate(b: Box3, x: f64, y: f64, z: f64) -> Box3 {
    (
        [b.0[0] - x, b.0[1] - y, b.0[2] - z],
        [b.1[0] + x, b.1[1] + y, b.1[2] + z],
    )
}

fn intersects(a: Box3, b: Box3) -> bool {
    (0..3).all(|k| a.0[k] < b.1[k] && a.1[k] > b.0[k])
}

/// A player's box at `feet`.
pub fn player_box(feet: [f64; 3]) -> Box3 {
    (
        [feet[0] - 0.3, feet[1], feet[2] - 0.3],
        [feet[0] + 0.3, feet[1] + 1.8, feet[2] + 0.3],
    )
}

/// The dragon's parts as a companion sees them, for where it may stand.
#[derive(Clone, Copy, Debug)]
pub struct DragonParts {
    pub head: Box3,
    pub neck: Box3,
    pub wings: [Box3; 2],
}

impl DragonParts {
    /// `EnderDragon.knockBack`'s zone round each wing: inflated 4, 2, 4 and
    /// moved 2 down.
    pub fn wing_zones(&self) -> [Box3; 2] {
        self.wings.map(|w| {
            let z = inflate(w, 4.0, 2.0, 4.0);
            (
                [z.0[0], z.0[1] - 2.0, z.0[2]],
                [z.1[0], z.1[1] - 2.0, z.1[2]],
            )
        })
    }
    /// Whether a body at `feet` is safe from its parts: no wing knockback,
    /// no bite from the head or neck (inflated 1), with `margin` to spare.
    /// Its jump (a critical hit's) counts: up to 1.3 blocks above `feet`.
    pub fn clear_of(&self, feet: [f64; 3], margin: f64) -> bool {
        let (lo, hi) = inflate(player_box(feet), margin, 0.0, margin);
        let me = (lo, [hi[0], hi[1] + 1.3, hi[2]]);
        let bite = [self.head, self.neck].map(|b| inflate(b, 1.0, 1.0, 1.0));
        !self
            .wing_zones()
            .iter()
            .chain(bite.iter())
            .any(|z| intersects(me, *z))
    }
}

/// How far along the line from `eye` to the centre of box `b` the line
/// enters it (a crosshair aimed at the centre: the reach a hit needs).
pub fn reach_to(eye: [f64; 3], b: Box3) -> f64 {
    let c = [
        (b.0[0] + b.1[0]) / 2.0,
        (b.0[1] + b.1[1]) / 2.0,
        (b.0[2] + b.1[2]) / 2.0,
    ];
    let d = ((c[0] - eye[0]).powi(2) + (c[1] - eye[1]).powi(2) + (c[2] - eye[2]).powi(2)).sqrt();
    segment_box(eye, c, b.0, b.1).map_or(d, |t| t * d)
}

/// Whether feet at `at` stand in a dragon's breath (an `AreaEffectCloud` of
/// `radius` at `cloud`: it hurts within the radius and less than a block
/// above or below), with `margin` blocks to spare.
pub fn in_breath(at: [f64; 3], cloud: [f64; 3], radius: f32, margin: f64) -> bool {
    (at[0] - cloud[0]).hypot(at[2] - cloud[2]) <= f64::from(radius) + margin
        && (at[1] - cloud[1]).abs() < 1.0
}

/// Spots around a perched dragon's head to hit it from: the middles of the
/// blocks round the head (no edge to slip off when it jumps for a critical
/// hit) at heights `floors` (feet), clear of its parts with `margin`, the
/// head within `reach` of the eye. Nearest `from` first.
pub fn perch_spots(
    parts: &DragonParts,
    floors: &[f64],
    from: [f64; 3],
    reach: f64,
    margin: f64,
) -> Vec<[f64; 3]> {
    let h = parts.head;
    let c = [(h.0[0] + h.1[0]) / 2.0, (h.0[2] + h.1[2]) / 2.0];
    let mut out: Vec<(f64, [f64; 3])> = Vec::new();
    let (cx, cz) = (c[0].floor() as i32, c[1].floor() as i32);
    for &y in floors {
        for dx in -4..=4 {
            for dz in -4..=4 {
                let feet = [f64::from(cx + dx) + 0.5, y, f64::from(cz + dz) + 0.5];
                let eye = [feet[0], feet[1] + 1.62, feet[2]];
                if !parts.clear_of(feet, margin) || reach_to(eye, h) > reach {
                    continue;
                }
                let d = (feet[0] - from[0]).hypot(feet[2] - from[2]) + (feet[1] - from[1]).abs();
                out.push((d, feet));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.into_iter().map(|(_, p)| p).collect()
}

/// A dragon fireball heading at `me`: the way to step aside, if it would
/// land within `radius` of it. `at` and `velocity` per tick.
pub fn dodge(me: [f64; 3], at: [f64; 3], velocity: [f64; 3], radius: f64) -> Option<[f64; 2]> {
    let speed2 = velocity.iter().map(|v| v * v).sum::<f64>();
    if speed2 < 1e-6 {
        return None;
    }
    // Its nearest approach to me along its straight line.
    let rel = [me[0] - at[0], me[1] + 0.9 - at[1], me[2] - at[2]];
    let t = (rel[0] * velocity[0] + rel[1] * velocity[1] + rel[2] * velocity[2]) / speed2;
    // (It speeds up as it flies: only the line counts, not when.)
    if t < 0.0 {
        return None;
    }
    let near = [
        at[0] + velocity[0] * t,
        at[1] + velocity[1] * t,
        at[2] + velocity[2] * t,
    ];
    let miss = (near[0] - me[0]).hypot(near[2] - me[2]);
    if miss > radius {
        return None;
    }
    // Sideways from its course, to the side it misses on already.
    let (vx, vz) = (velocity[0], velocity[2]);
    let len = vx.hypot(vz).max(1e-6);
    let mut side = [-vz / len, vx / len];
    if (me[0] - near[0]) * side[0] + (me[2] - near[2]) * side[1] < 0.0 {
        side = [-side[0], -side[1]];
    }
    Some(side)
}

/// What the End route is doing, for the Watch HUD and the run summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndPhase {
    /// Eyes into the portal room's frames, then in.
    Portal,
    /// On the arrival platform: up and over to the island.
    Arrive,
    /// Looking round for the spikes and their crystals.
    Scout,
    /// Shooting the crystals, then the caged ones.
    Crystals,
    /// Waiting for the dragon to perch, then at its head.
    Dragon,
    /// The dragon is dead: into the exit portal.
    Exit,
    Done,
    Failed,
}

impl EndPhase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::Arrive => "arrive",
            Self::Scout => "scout",
            Self::Crystals => "crystals",
            Self::Dragon => "dragon",
            Self::Exit => "exit",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the arc at `pitch` is when it is `distance` out.
    fn height(distance: f64, pitch: f32) -> f64 {
        let f = arrow_flight([0.0; 3], -90.0, pitch, ARROW_SPEED, 400, 1e9);
        let i = f.iter().position(|p| p.at[0] >= distance).unwrap();
        let (a, b) = (f[i - 1].at, f[i].at);
        a[1] + (b[1] - a[1]) * (distance - a[0]) / (b[0] - a[0])
    }

    #[test]
    fn arcs_reach_the_crystals_on_the_spikes() {
        // 15 to 45 blocks off and 10 to 40 up, as the spikes stand from the island.
        for (d, rise) in [
            (15.0, 10.0),
            (20.0, 30.0),
            (30.0, 25.0),
            (45.0, 40.0),
            (12.0, 0.5),
        ] {
            let pitches = aim_pitches(d, rise, ARROW_SPEED);
            assert!(!pitches.is_empty(), "{d} {rise}");
            for p in &pitches {
                assert!((height(d, *p) - rise).abs() < 0.05, "{d} {rise} {p}");
            }
            // The flat arc first, aimed up.
            assert!(pitches[0] < 0.0 && pitches[0] > -80.0, "{pitches:?}");
            if let Some(lob) = pitches.get(1) {
                assert!(lob < &pitches[0]);
            }
        }
        // Out of a bow's range.
        assert!(aim_pitches(200.0, 60.0, ARROW_SPEED).is_empty());
    }

    #[test]
    fn blasts_by_distance_and_armour() {
        // Point blank a crystal kills anyone; at 12 blocks it does 1.
        assert!(explosion_damage(CRYSTAL_POWER, 1.0, 1.0) > 70.0);
        assert!((explosion_damage(CRYSTAL_POWER, 12.0, 1.0) - 1.0).abs() < 1e-4);
        assert_eq!(explosion_damage(CRYSTAL_POWER, 12.5, 1.0), 0.0);
        // Iron armour (15 points) takes a share off.
        let raw = explosion_damage(CRYSTAL_POWER, 8.0, 1.0);
        let worn = after_armor(raw, 15.0, 0.0);
        assert!(worn < raw && worn > raw * 0.6, "{raw} {worn}");
        // A full-health body in iron should stand 8 to 10 blocks off.
        let safe = safe_distance(CRYSTAL_POWER, 15.0, 0.0, 20.0, 6.0);
        assert!((8.0..=11.0).contains(&safe), "{safe}");
        // With no armour, further.
        assert!(safe_distance(CRYSTAL_POWER, 0.0, 0.0, 20.0, 6.0) > safe);
    }

    #[test]
    fn a_caged_spike_is_worked_from_the_island_side() {
        // Spikes 42 blocks out; caged ones are the shortest (heights 79, 82).
        let s = Spike::under([-12.5, 80.0, -39.5], true);
        assert_eq!((s.x, s.z, s.height), (-13, -40, 79));
        assert_eq!(s.inward(), [0, 1]);
        let p = cage_plan(&s, 3, 12);
        assert_eq!(p.near, [-13, -37]);
        assert_eq!(p.far, [-13, -28]);
        assert_eq!(p.stand_y, 79);
        assert_eq!(p.window, [[-13, 79, -38], [-13, 80, -38], [-13, 81, -38]]);
        let s = Spike::under([42.5, 101.0, 0.5], false);
        assert_eq!(s.inward(), [-1, 0]);
        // The shot from the far pillar: level, through the window, onto the crystal.
        let s = Spike::under([33.5, 83.0, -24.5], true);
        let p = cage_plan(&s, 3, 12);
        let feet = [
            f64::from(p.far[0]) + 0.5,
            f64::from(p.stand_y),
            f64::from(p.far[1]) + 0.5,
        ];
        let eye = [feet[0], feet[1] + 1.62, feet[2]];
        let target = s.crystal_centre();
        let d = (target[0] - eye[0]).hypot(target[2] - eye[2]);
        let pitch = aim_pitches(d, target[1] - (eye[1] - ARROW_DROP), ARROW_SPEED)[0];
        let yaw = (-(target[0] - eye[0]).atan2(target[2] - eye[2])).to_degrees() as f32;
        let from = [eye[0], eye[1] - ARROW_DROP, eye[2]];
        let flight = arrow_flight(from, yaw, pitch, ARROW_SPEED, 40, 20.0);
        let (lo, hi) = s.crystal_box();
        let mut through_window = false;
        let mut hit = false;
        for w in flight.windows(2) {
            for bar in p.window {
                let b = [f64::from(bar[0]), f64::from(bar[1]), f64::from(bar[2])];
                if segment_box(w[0].at, w[1].at, b, [b[0] + 1.0, b[1] + 1.0, b[2] + 1.0]).is_some()
                {
                    through_window = true;
                }
            }
            if segment_box(w[0].at, w[1].at, lo, hi).is_some() {
                hit = true;
                break;
            }
        }
        assert!(through_window && hit);
        // And the blast there barely reaches.
        let at = [target[0], target[1] - 1.0, target[2]];
        let dist =
            ((at[0] - feet[0]).powi(2) + (at[1] - feet[1]).powi(2) + (at[2] - feet[2]).powi(2))
                .sqrt();
        assert!(explosion_damage(CRYSTAL_POWER, dist, 1.0) <= 2.0, "{dist}");
    }

    fn dragon_facing_south() -> DragonParts {
        // Perched at the podium (0.5, 67, 0.5), yaw 180: facing +z, the head
        // 6.5 out and a block down, the wings 4.5 to the sides.
        let b = |c: [f64; 3], w: f64, h: f64| -> Box3 {
            (
                [c[0] - w / 2.0, c[1], c[2] - w / 2.0],
                [c[0] + w / 2.0, c[1] + h, c[2] + w / 2.0],
            )
        };
        DragonParts {
            head: b([0.5, 66.0, 7.0], 1.0, 1.0),
            neck: b([0.5, 66.0, 6.0], 3.0, 3.0),
            wings: [
                b([5.0, 69.0, 0.5], 4.0, 2.0),
                b([-4.0, 69.0, 0.5], 4.0, 2.0),
            ],
        }
    }

    #[test]
    fn spots_by_a_perched_head_avoid_wings_and_bite() {
        let parts = dragon_facing_south();
        // Right under the head: bitten. Beside the body: the wing throws it.
        assert!(!parts.clear_of([0.5, 64.0, 7.0], 0.0));
        assert!(!parts.clear_of([4.0, 64.0, 1.0], 0.0));
        let spots = perch_spots(&parts, &[64.0, 65.0], [0.5, 64.0, 20.0], 2.9, 0.15);
        assert!(!spots.is_empty());
        for s in &spots {
            assert!(parts.clear_of(*s, 0.15));
            assert!(reach_to([s[0], s[1] + 1.62, s[2]], parts.head) <= 2.9);
        }
        // Nearest the approach first: in front of the head, not behind it.
        assert!(spots[0][2] > 7.0, "{:?}", spots[0]);
    }

    #[test]
    fn breath_and_fireballs() {
        assert!(in_breath([3.0, 64.0, 0.0], [0.0, 64.0, 0.0], 5.0, 0.5));
        // A block up the cloud cannot reach.
        assert!(!in_breath([3.0, 65.0, 0.0], [0.0, 64.0, 0.0], 5.0, 0.0));
        assert!(!in_breath([6.0, 64.0, 0.0], [0.0, 64.0, 0.0], 5.0, 0.5));
        // A fireball coming straight down at it: step aside; one going away: no.
        let side = dodge([0.0, 64.0, 0.0], [0.0, 90.0, -20.0], [0.0, -1.0, 0.8], 5.0).unwrap();
        assert!((side[0].abs() - 1.0).abs() < 1e-6 && side[1].abs() < 1e-6);
        assert!(dodge([0.0, 64.0, 0.0], [0.0, 90.0, -20.0], [0.0, 0.0, -1.0], 5.0).is_none());
    }
}
