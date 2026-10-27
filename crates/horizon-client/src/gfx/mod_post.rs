//! Mods' post passes (docs/mods.md, "Shaders from a mod"): the shaders mods register as
//! `post-after-translucent`, run over the world's picture before a shader pack's own post
//! passes and under the hand and HUD. They are built as a pack's post stage is
//! (`NativePost`), each with a `params` uniform the mod sets every frame, and timed on the GPU
//! all the time: a pass that keeps going over [`PASS_BUDGET_MS`] is reported for the game to
//! remove.

use std::time::{Duration, Instant};

use horizon_shader_api::check::CheckedStage;

use super::native_post::{NativeFrame, NativePost};
use super::pack_timing::PassTiming;

/// GPU time one mod pass may take a frame, in milliseconds (the median of its timed frames).
pub const PASS_BUDGET_MS: f32 = 2.0;
/// How long a pass may stay over budget before it is removed.
const OVER_FOR: Duration = Duration::from_secs(5);

/// One mod's pass: its label (`<mod>/<shader>`) and its checked stage.
pub struct ModPass {
    pub label: String,
    pub stage: CheckedStage,
}

pub struct ModPost {
    pub post: NativePost,
    pub frame: NativeFrame,
    pub timing: Option<PassTiming>,
    budget: Budget,
    /// The passes' output when a shader pack's post passes read it after them, of the
    /// world's size.
    out: Option<([u32; 2], wgpu::TextureView)>,
    format: wgpu::TextureFormat,
}

impl ModPost {
    /// Pipelines for `passes`, in order, drawing onto a screen of `format`. Errors name the
    /// pass wgpu refused, by its index.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        passes: Vec<ModPass>,
        format: wgpu::TextureFormat,
    ) -> Result<Self, (usize, String)> {
        let frame = NativeFrame::new(device);
        let labels: Vec<String> = passes.iter().map(|p| p.label.clone()).collect();
        let stages = passes
            .into_iter()
            .map(|p| CheckedStage {
                label: p.label,
                ..p.stage
            })
            .collect();
        let post = NativePost::for_mods(device, stages, format, &frame)?;
        Ok(Self {
            post,
            frame,
            timing: PassTiming::new(device, queue),
            budget: Budget::new(labels),
            out: None,
            format,
        })
    }

    /// The target the passes write when a shader pack's post passes follow them.
    pub fn output(&mut self, device: &wgpu::Device, size: [u32; 2]) -> wgpu::TextureView {
        if self.out.as_ref().is_none_or(|o| o.0 != size) {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Mod post output"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.out = Some((size, view));
        }
        self.out.as_ref().expect("made above").1.clone()
    }

    /// The passes over budget for too long: their index and median GPU time in milliseconds.
    pub fn over_budget(&mut self) -> Vec<(usize, f32)> {
        let Some(timing) = &self.timing else {
            return Vec::new();
        };
        self.budget.check(&timing.medians(), Instant::now())
    }
}

/// Which passes keep going over [`PASS_BUDGET_MS`]: one over it in every check for
/// [`OVER_FOR`] is reported once.
struct Budget {
    /// Each pass's label and since when it has been over.
    passes: Vec<(String, Option<Instant>)>,
}

impl Budget {
    fn new(labels: Vec<String>) -> Self {
        Self {
            passes: labels.into_iter().map(|l| (l, None)).collect(),
        }
    }

    /// The passes, by index, whose median in `medians` has stayed over the budget for
    /// [`OVER_FOR`] by `now`, with that median.
    fn check(&mut self, medians: &[(String, f32)], now: Instant) -> Vec<(usize, f32)> {
        let mut over = Vec::new();
        for (i, (label, since)) in self.passes.iter_mut().enumerate() {
            let Some(&(_, ms)) = medians.iter().find(|(l, _)| l == label) else {
                continue;
            };
            if ms <= PASS_BUDGET_MS {
                *since = None;
                continue;
            }
            let start = *since.get_or_insert(now);
            if now - start >= OVER_FOR {
                over.push((i, ms));
                // Reported once: the game removes it and rebuilds the passes.
                *since = Some(now + Duration::from_secs(3600));
            }
        }
        over
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_that_keeps_going_over_budget_is_dropped() {
        let mut budget = Budget::new(vec!["slow/blur".into(), "fast/tint".into()]);
        let t0 = Instant::now();
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let medians = |slow: f32| {
            vec![
                ("slow/blur".to_owned(), slow),
                ("fast/tint".to_owned(), 0.1),
            ]
        };
        assert!(budget.check(&medians(3.5), at(0.0)).is_empty());
        assert!(budget.check(&medians(3.5), at(4.0)).is_empty());
        // A dip under the budget starts the count again.
        assert!(budget.check(&medians(1.5), at(4.5)).is_empty());
        assert!(budget.check(&medians(3.5), at(5.0)).is_empty());
        assert!(budget.check(&medians(3.5), at(9.0)).is_empty());
        assert_eq!(budget.check(&medians(3.5), at(10.0)), [(0, 3.5)]);
        // Reported once.
        assert!(budget.check(&medians(3.5), at(11.0)).is_empty());
    }
}
