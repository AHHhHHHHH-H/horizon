//! Sandboxed effects reuse vanilla sprite sets, not vanilla constructors.
use super::*;

/// A fixed-size sprite with explicit base-particle physics. Texture assets are
/// not accepted here: `kind` selects an existing vanilla sprite set.
pub struct ModParticle {
    pub kind: ParticleType,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub rgba: [f32; 4],
    /// Quad half-size in blocks (0.005..1).
    pub size: f32,
    /// Ticks, 1..200.
    pub lifetime: u32,
    /// Vertical acceleration is -0.04 * gravity blocks/tick².
    pub gravity: f32,
    /// Velocity multiplier after motion, 0..1.
    pub friction: f32,
    pub collision: bool,
    pub full_bright: bool,
}

impl ModParticle {
    fn valid(&self) -> bool {
        matches!(
            self.kind,
            ParticleType::Smoke
                | ParticleType::Flame
                | ParticleType::Bubble
                | ParticleType::Splash
                | ParticleType::EndRod
                | ParticleType::CherryLeaves
                | ParticleType::WhiteAsh
                | ParticleType::SporeBlossomAir
        ) && self.position.into_iter().all(f64::is_finite)
            && self
                .velocity
                .into_iter()
                .all(|v| v.is_finite() && v.abs() <= 1.0)
            && self
                .rgba
                .into_iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            && self.size.is_finite()
            && (0.005..=1.0).contains(&self.size)
            && (1..=200).contains(&self.lifetime)
            && self.gravity.is_finite()
            && (-1.0..=1.0).contains(&self.gravity)
            && self.friction.is_finite()
            && (0.0..=1.0).contains(&self.friction)
    }
}

impl ParticleEngine {
    /// Local diagnostics for integration fixtures (live plus pending).
    pub fn mod_particle_count(&self) -> usize {
        self.particles
            .iter()
            .chain(&self.pending)
            .filter(|p| p.mod_owner.is_some())
            .count()
    }
    /// Static packaged sprite already admitted to the atlas. No allocation of
    /// new texture storage per emission, and no sprite from another namespace.
    pub fn add_mod_sprite_particle(
        &mut self,
        owner: &str,
        spec: &ModParticle,
        sprite: &str,
    ) -> bool {
        if !self.atlas.contains_key(sprite) {
            return false;
        }
        if !self.add_mod_particle(owner, spec) {
            return false;
        }
        let p = self.pending.last_mut().expect("just queued a particle");
        p.sprite = Sprite::Named(sprite.to_owned());
        p.set = None;
        p.behaviour = Behaviour::Base;
        true
    }
    /// Retain only particles of live mod owners, including pending particles.
    /// Called after hooks, reload and enable/disable; vanilla particles survive.
    pub fn retain_mod_particles(&mut self, mut live: impl FnMut(&str) -> bool) {
        self.particles
            .retain(|p| p.mod_owner.as_deref().is_none_or(&mut live));
        self.pending
            .retain(|p| p.mod_owner.as_deref().is_none_or(&mut live));
    }

    /// Queues one configurable local particle. Enforces a separate live cap
    /// (256/owner, 1024 total), the player's setting and camera distance before
    /// allocation. The sandbox additionally limits requests per tick.
    pub fn add_mod_particle(&mut self, owner: &str, spec: &ModParticle) -> bool {
        if owner.is_empty() || owner.len() > 128 || !spec.valid() {
            return false;
        }
        let distance: f64 = spec
            .position
            .into_iter()
            .zip(self.camera)
            .map(|(p, c)| (p - c).powi(2))
            .sum();
        if !distance.is_finite() || distance > 1024.0 {
            return false;
        }
        let mut owned = 0;
        let mut total = 0;
        for p in self.particles.iter().chain(&self.pending) {
            if let Some(id) = &p.mod_owner {
                total += 1;
                owned += usize::from(id == owner);
            }
        }
        if owned >= 256 || total >= 1024 || self.calculate_particle_level(false) == 2 {
            return false;
        }
        let mut p = Particle::blank(spec.kind, spec.position);
        p.mod_owner = Some(owner.to_owned());
        p.velocity = spec.velocity;
        p.rgba = spec.rgba;
        p.size = spec.size;
        p.lifetime = spec.lifetime as i32;
        p.gravity = spec.gravity;
        p.friction = spec.friction;
        p.has_physics = spec.collision;
        p.light = if spec.full_bright {
            Light::FullBright
        } else {
            Light::World
        };
        p.set = self.set_of(spec.kind);
        p.sprite = self.first(spec.kind);
        p.behaviour = Behaviour::Animated;
        self.pending.push(p);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ModParticle {
        ModParticle {
            kind: ParticleType::Flame,
            position: [0.0; 3],
            velocity: [0.1; 3],
            rgba: [0.5; 4],
            size: 0.2,
            lifetime: 40,
            gravity: 0.5,
            friction: 0.9,
            collision: false,
            full_bright: true,
        }
    }

    #[test]
    fn explicit_parameters_do_not_inherit_vanilla_behaviour() {
        let mut engine = ParticleEngine::new(Path::new(""), &HashMap::new(), &HashSet::new());
        assert!(engine.add_mod_particle("owner", &spec()));
        let p = &engine.pending[0];
        assert_eq!(p.velocity, [0.1; 3]);
        assert_eq!(p.rgba, [0.5; 4]);
        assert_eq!(
            (p.size, p.lifetime, p.gravity, p.friction),
            (0.2, 40, 0.5, 0.9)
        );
        assert!(!p.has_physics);
        assert_eq!(p.light, Light::FullBright);
        assert!(matches!(p.behaviour, Behaviour::Animated));
        assert!(!p.pre_tick);
    }
    #[test]
    fn custom_sprite_must_be_loaded_and_is_static_and_owner_scoped() {
        let mut engine = ParticleEngine::new(Path::new(""), &HashMap::new(), &HashSet::new());
        assert!(!engine.add_mod_sprite_particle("a", &spec(), "a:particle/spark"));
        engine.atlas.insert(
            "a:particle/spark".into(),
            (
                Rect {
                    u0: 0.0,
                    v0: 0.0,
                    u1: 1.0,
                    v1: 1.0,
                    w: 8,
                    h: 8,
                },
                true,
            ),
        );
        assert!(engine.add_mod_sprite_particle("a", &spec(), "a:particle/spark"));
        assert!(matches!(&engine.pending[0].sprite, Sprite::Named(n) if n == "a:particle/spark"));
        assert!(engine.pending[0].set.is_none());
        engine.retain_mod_particles(|_| false);
        assert!(engine.pending.is_empty());
    }

    #[test]
    fn validation_caps_settings_and_cleanup_cover_pending_and_live() {
        let mut engine = ParticleEngine::new(Path::new(""), &HashMap::new(), &HashSet::new());
        let mut s = spec();
        s.size = f32::NAN;
        assert!(!engine.add_mod_particle("owner", &s));
        s = spec();
        s.position[0] = 33.0;
        assert!(!engine.add_mod_particle("owner", &s));
        engine.particle_level = 2;
        assert!(!engine.add_mod_particle("owner", &spec()));
        engine.particle_level = 0;
        for owner in ["a", "b", "c", "d"] {
            for _ in 0..256 {
                assert!(engine.add_mod_particle(owner, &spec()));
            }
            assert!(!engine.add_mod_particle(owner, &spec()));
        }
        assert!(!engine.add_mod_particle("e", &spec()));
        engine.particles.push(engine.pending.pop().unwrap());
        engine
            .particles
            .push(Particle::blank(ParticleType::Flame, [0.0; 3]));
        engine.retain_mod_particles(|id| id == "a");
        assert_eq!(engine.pending.len(), 256);
        assert_eq!(engine.particles.len(), 1);
        assert!(engine.particles[0].mod_owner.is_none());
        engine.retain_mod_particles(|_| false);
        assert!(engine.pending.is_empty());
    }
}
