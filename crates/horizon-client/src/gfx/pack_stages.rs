//! A Horizon shader pack on the GPU (docs/shader-packs.md): each stage is one
//! full-screen pass. `deferred` stages run between the opaque and the
//! translucent world passes and their `color` output becomes the world's
//! colour; `composite` stages run after the world, and the last `color`
//! output goes to the screen through `world_post` (copy or FXAA) under the
//! hand and HUD. A stage can also write one of the pack's own targets, which
//! the others read (with last frame's copy when it keeps a history). Only
//! exists while a pack is selected.
use super::pack_world::PackWorld;
use crate::shader_pack::{ShaderPack, StageKind, Target, VERTEX_ENTRY};

/// The `Frame` uniform of the prelude (`shader_pack_common.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FrameUniform {
    pub view: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub inverse_view: [[f32; 4]; 4],
    pub inverse_projection: [[f32; 4]; 4],
    pub camera_position: [f32; 4],
    pub resolution: [f32; 4],
    pub time: [f32; 4],
    pub sun: [f32; 4],
    pub moon: [f32; 4],
    pub sky_color: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog: [f32; 4],
    pub weather: [f32; 4],
    pub camera_state: [f32; 4],
    pub view_params: [f32; 4],
    pub dimension: [f32; 4],
    pub previous_view_projection: [[f32; 4]; 4],
    pub shadow_view_projection: [[f32; 4]; 4],
    pub camera_delta: [f32; 4],
    pub shadow: [f32; 4],
    pub shadow_light: [f32; 4],
    pub world: [f32; 4],
}

struct Pass {
    pipeline: wgpu::RenderPipeline,
    kind: StageKind,
    scale: f32,
    /// The pack target it writes, or the `color` chain.
    target: Option<usize>,
    uniform: wgpu::Buffer,
}
/// A pack target at the world's size: two textures when it keeps a history.
struct PackTarget {
    size: [u32; 2],
    views: Vec<wgpu::TextureView>,
}
/// What one pass reads and writes at this world size.
struct Bound {
    size: [u32; 2],
    /// Its own `color` output (chain passes).
    output: Option<wgpu::TextureView>,
    /// Group 0, then group 1 for each history parity.
    group: wgpu::BindGroup,
    resources: [wgpu::BindGroup; 2],
}

/// The world's views a frame's stages read.
pub struct WorldViews<'a> {
    pub color: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
    pub size: [u32; 2],
}

pub struct PackStages {
    pub id: String,
    layout: wgpu::BindGroupLayout,
    resource_layout: wgpu::BindGroupLayout,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    repeat: wgpu::Sampler,
    compare: wgpu::Sampler,
    passes: Vec<Pass>,
    format: wgpu::TextureFormat,
    targets: Vec<Target>,
    textures: Vec<wgpu::TextureView>,
    /// 1x1 stand-ins: for what a pack lacks, and for a stage's own output.
    blank: wgpu::TextureView,
    blank_depth: wgpu::TextureView,
    blank_uint: wgpu::TextureView,
    /// Built for `world_size`.
    sized: Vec<PackTarget>,
    bound: Vec<Bound>,
    /// The opaque world, copied for the deferred stages to read.
    deferred_scene: Option<wgpu::TextureView>,
    /// `world_post`'s bindings of the last composite `color` output.
    output: Option<wgpu::BindGroup>,
    world_size: [u32; 2],
    /// The world resources' version the bindings were made with.
    world_generation: u64,
    started: std::time::Instant,
    frames: u32,
    /// The G-buffer, shadow map and world hooks (`[gbuffer]`, `[shadow]`).
    pub world: Option<PackWorld>,
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn sampler_entry(binding: u32, ty: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
        count: None,
    }
}
fn view_entry(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}
fn texture(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}
pub(super) const TARGET_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

impl PackStages {
    /// The pipelines of a loaded (already validated) pack, or wgpu's error.
    /// `world`: the pack's world resources, when it has `[gbuffer]` or `[shadow]`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        pack: &ShaderPack,
        world: Option<PackWorld>,
    ) -> Result<Self, String> {
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shader pack stage"),
            entries: &[
                texture_entry(0, float),
                texture_entry(1, wgpu::TextureSampleType::Depth),
                texture_entry(2, float),
                sampler_entry(3, wgpu::SamplerBindingType::Filtering),
                sampler_entry(4, wgpu::SamplerBindingType::NonFiltering),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(6, wgpu::TextureSampleType::Uint),
                texture_entry(7, wgpu::TextureSampleType::Depth),
                texture_entry(8, wgpu::TextureSampleType::Depth),
                sampler_entry(9, wgpu::SamplerBindingType::Comparison),
                sampler_entry(10, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let resources = crate::shader_pack::resources(&pack.targets, &pack.textures);
        let resource_entries: Vec<_> = resources
            .iter()
            .enumerate()
            .map(|(i, (_, filterable))| {
                texture_entry(
                    i as u32,
                    wgpu::TextureSampleType::Float {
                        filterable: *filterable,
                    },
                )
            })
            .collect();
        let resource_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shader pack resources"),
            entries: &resource_entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Shader pack stage"),
            bind_group_layouts: &[Some(&layout), Some(&resource_layout)],
            ..Default::default()
        });
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let passes = pack
            .stages
            .iter()
            .map(|stage| {
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&stage.file),
                    source: wgpu::ShaderSource::Wgsl(stage.source.as_str().into()),
                });
                // A compiled stage's vertex shader comes from the prelude alone.
                let vertex_module = stage.vertex.as_ref().map(|source| {
                    device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("Shader pack stage vertex"),
                        source: wgpu::ShaderSource::Wgsl(source.as_str().into()),
                    })
                });
                let constants: Vec<(&str, f64)> = stage
                    .constants
                    .iter()
                    .map(|(k, v)| (k.as_str(), *v))
                    .collect();
                let target_format = stage
                    .target
                    .map_or(format, |t| pack.targets[t].format.wgpu());
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&stage.file),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: vertex_module.as_ref().unwrap_or(&module),
                        entry_point: Some(VERTEX_ENTRY),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(&stage.entry),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants: &constants,
                            ..Default::default()
                        },
                        targets: &[Some(wgpu::ColorTargetState {
                            format: target_format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
                Pass {
                    pipeline,
                    kind: stage.kind,
                    scale: stage.scale,
                    target: stage.target,
                    uniform: device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Shader pack frame"),
                        size: std::mem::size_of::<FrameUniform>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                }
            })
            .collect();
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(error.to_string());
        }
        let sampler = |filter, address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Shader pack"),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: address,
                address_mode_v: address,
                ..Default::default()
            })
        };
        let clamp = wgpu::AddressMode::ClampToEdge;
        let blank = |format| {
            texture(device, "Shader pack blank", [1, 1], format, TARGET_USAGE)
                .create_view(&Default::default())
        };
        let textures = pack
            .textures
            .iter()
            .map(|t| {
                let size = wgpu::Extent3d {
                    width: t.size[0],
                    height: t.size[1],
                    depth_or_array_layers: 1,
                };
                let tex = texture(
                    device,
                    &t.name,
                    t.size,
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                );
                queue.write_texture(
                    tex.as_image_copy(),
                    &t.pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * t.size[0]),
                        rows_per_image: None,
                    },
                    size,
                );
                tex.create_view(&Default::default())
            })
            .collect();
        Ok(Self {
            id: pack.id.clone(),
            layout,
            resource_layout,
            linear: sampler(wgpu::FilterMode::Linear, clamp),
            nearest: sampler(wgpu::FilterMode::Nearest, clamp),
            repeat: sampler(wgpu::FilterMode::Linear, wgpu::AddressMode::Repeat),
            compare: super::pack_world::compare_sampler(device),
            passes,
            format,
            targets: pack.targets.clone(),
            textures,
            blank: blank(wgpu::TextureFormat::Rgba8Unorm),
            blank_depth: blank(wgpu::TextureFormat::Depth32Float),
            blank_uint: blank(wgpu::TextureFormat::Rgba16Uint),
            sized: Vec::new(),
            bound: Vec::new(),
            deferred_scene: None,
            output: None,
            world_size: [0, 0],
            world_generation: u64::MAX,
            started: std::time::Instant::now(),
            frames: 0,
            world,
        })
    }

    pub fn has_deferred(&self) -> bool {
        self.passes.iter().any(|p| p.kind == StageKind::Deferred)
    }

    /// (Re)make the targets and bindings for the world's views.
    fn bind(&mut self, device: &wgpu::Device, world: &WorldViews, post: &super::world_post::Post) {
        let size = world.size;
        let generation = self.world.as_ref().map_or(0, |w| w.generation());
        if self.world_size == size
            && self.world_generation == generation
            && self.bound.len() == self.passes.len()
        {
            return;
        }
        self.world_size = size;
        self.world_generation = generation;
        let scaled = |scale: f32| size.map(|v| ((v as f32 * scale).round() as u32).max(1));
        self.sized = self
            .targets
            .iter()
            .map(|t| {
                let target_size = scaled(t.scale);
                PackTarget {
                    size: target_size,
                    views: (0..if t.history { 2 } else { 1 })
                        .map(|_| {
                            texture(device, &t.name, target_size, t.format.wgpu(), TARGET_USAGE)
                                .create_view(&Default::default())
                        })
                        .collect(),
                }
            })
            .collect();
        self.deferred_scene = self.has_deferred().then(|| {
            texture(
                device,
                "Shader pack opaque world",
                size,
                self.format,
                TARGET_USAGE | wgpu::TextureUsages::COPY_DST,
            )
            .create_view(&Default::default())
        });
        let pack_world = self.world.as_ref();
        let gbuffer = pack_world
            .and_then(|w| w.gbuffer_view())
            .unwrap_or(&self.blank_uint);
        let shadow = pack_world
            .and_then(|w| w.shadow_view())
            .unwrap_or(&self.blank_depth);
        self.bound.clear();
        // The chain's last output, by kind: what `previous` reads next.
        let mut last_deferred: Option<wgpu::TextureView> = None;
        let mut last_composite: Option<wgpu::TextureView> = None;
        let deferred_count = self
            .passes
            .iter()
            .filter(|p| p.kind == StageKind::Deferred && p.target.is_none())
            .count();
        let mut deferred_seen = 0;
        for pass in &self.passes {
            let deferred = pass.kind == StageKind::Deferred;
            let scene = match (&self.deferred_scene, deferred) {
                (Some(copy), true) => copy,
                _ => world.color,
            };
            let previous = if deferred {
                last_deferred.as_ref()
            } else {
                last_composite.as_ref()
            }
            .unwrap_or(scene)
            .clone();
            // The opaque depth: the world's own while deferred stages run.
            let opaque = match pack_world.and_then(|w| w.opaque_depth_view()) {
                Some(view) if !deferred => view,
                _ => world.depth,
            };
            let (out_size, output) = match pass.target {
                Some(t) => (self.sized[t].size, None),
                None if deferred => {
                    deferred_seen += 1;
                    // The last deferred `color` stage draws into the world itself.
                    let view = (deferred_seen < deferred_count).then(|| {
                        texture(
                            device,
                            "Shader pack deferred",
                            size,
                            self.format,
                            TARGET_USAGE,
                        )
                        .create_view(&Default::default())
                    });
                    (size, view)
                }
                None => {
                    let target_size = scaled(pass.scale);
                    let view = texture(
                        device,
                        "Shader pack stage",
                        target_size,
                        self.format,
                        TARGET_USAGE,
                    )
                    .create_view(&Default::default());
                    (target_size, Some(view))
                }
            };
            if pass.target.is_none() {
                let next = output.clone().unwrap_or_else(|| world.color.clone());
                if deferred {
                    last_deferred = Some(next);
                } else {
                    last_composite = Some(next);
                }
            }
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shader pack stage"),
                layout: &self.layout,
                entries: &[
                    view_entry(0, scene),
                    view_entry(1, world.depth),
                    view_entry(2, &previous),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.linear),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.nearest),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: pass.uniform.as_entire_binding(),
                    },
                    view_entry(6, gbuffer),
                    view_entry(7, opaque),
                    view_entry(8, shadow),
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::Sampler(&self.compare),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::Sampler(&self.repeat),
                    },
                ],
            });
            let resources = [0, 1].map(|parity| {
                let mut views: Vec<&wgpu::TextureView> = Vec::new();
                for (i, (t, sized)) in self.targets.iter().zip(&self.sized).enumerate() {
                    let written = pass.target == Some(i);
                    let current = &sized.views[if t.history { parity } else { 0 }];
                    views.push(if written { &self.blank } else { current });
                    if t.history {
                        views.push(&sized.views[1 - parity]);
                    }
                }
                views.extend(&self.textures);
                let entries: Vec<_> = views
                    .iter()
                    .enumerate()
                    .map(|(i, v)| view_entry(i as u32, v))
                    .collect();
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Shader pack resources"),
                    layout: &self.resource_layout,
                    entries: &entries,
                })
            });
            self.bound.push(Bound {
                size: out_size,
                output,
                group,
                resources,
            });
        }
        self.output = last_composite
            .filter(|v| v != world.color)
            .map(|v| post.bind(device, &v));
    }

    /// One pass's frame uniform and draw.
    fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        i: usize,
        world_color: &wgpu::TextureView,
        mut uniform: FrameUniform,
    ) {
        let (pass, bound) = (&self.passes[i], &self.bound[i]);
        let [w, h] = bound.size.map(|v| v as f32);
        uniform.resolution = [w, h, 1.0 / w, 1.0 / h];
        queue.write_buffer(&pass.uniform, 0, bytemuck::bytes_of(&uniform));
        let parity = (self.frames & 1) as usize;
        let view = match pass.target {
            Some(t) => {
                let views = &self.sized[t].views;
                &views[parity.min(views.len() - 1)]
            }
            None => bound.output.as_ref().unwrap_or(world_color),
        };
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Shader pack stage"),
            color_attachments: &attachments,
            ..Default::default()
        });
        render.set_pipeline(&pass.pipeline);
        render.set_bind_group(0, &bound.group, &[]);
        render.set_bind_group(1, &bound.resources[parity], &[]);
        render.draw(0..3, 0..1);
    }

    fn uniform(&self, mut uniform: FrameUniform, world: [u32; 2]) -> FrameUniform {
        let [w, h] = world.map(|v| v as f32);
        uniform.world = [w, h, 1.0 / w, 1.0 / h];
        // With world resources, they keep the clock (their hooks run first).
        if self.world.is_none() {
            uniform.time[0] = self.started.elapsed().as_secs_f32();
            uniform.time[1] = self.frames as f32;
        }
        uniform
    }

    /// Between the opaque and the translucent world passes: the deferred
    /// stages, the last of which draws into the world's colour.
    pub fn run_deferred(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        world: &WorldViews,
        post: &super::world_post::Post,
        uniform: FrameUniform,
    ) {
        if !self.has_deferred() {
            return;
        }
        self.bind(device, world, post);
        if let Some(copy) = &self.deferred_scene {
            encoder.copy_texture_to_texture(
                world.color.texture().as_image_copy(),
                copy.texture().as_image_copy(),
                copy.texture().size(),
            );
        }
        let uniform = self.uniform(uniform, world.size);
        for i in 0..self.passes.len() {
            if self.passes[i].kind == StageKind::Deferred {
                self.draw(queue, encoder, i, world.color, uniform);
            }
        }
    }

    /// After the world: every composite stage. Returns `post`'s bindings of
    /// the last `color` output, or `None` when the world's colour is the
    /// output (no composite `color` stage).
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        world: &WorldViews,
        post: &super::world_post::Post,
        uniform: FrameUniform,
    ) -> Option<&wgpu::BindGroup> {
        self.bind(device, world, post);
        let uniform = self.uniform(uniform, world.size);
        for i in 0..self.passes.len() {
            if self.passes[i].kind == StageKind::Composite {
                self.draw(queue, encoder, i, world.color, uniform);
            }
        }
        self.frames = self.frames.wrapping_add(1);
        self.output.as_ref()
    }
}

/// The option values a module declares an `override` for (wgpu refuses
/// constants a module doesn't have).
pub(super) fn declared<'a>(constants: &[(&'a str, f64)], source: &str) -> Vec<(&'a str, f64)> {
    constants
        .iter()
        .copied()
        .filter(|(key, _)| declares_override(source, key))
        .collect()
}

/// `override <key>` appears in the source (a declaration, not just a use).
fn declares_override(source: &str, key: &str) -> bool {
    source.lines().any(|line| {
        line.trim_start()
            .strip_prefix("override")
            .and_then(|rest| rest.trim_start().strip_prefix(key))
            .is_some_and(|rest| rest.trim_start().starts_with([':', '=']))
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_frame_uniform_matches_the_prelude() {
        // 6 matrices and 16 vectors, as `struct Frame` in the common prelude.
        assert_eq!(std::mem::size_of::<super::FrameUniform>(), 6 * 64 + 16 * 16);
        let prelude = crate::shader_pack::COMMON;
        let body = prelude.split("struct Frame {").nth(1).unwrap();
        let body = &body[..body.find('}').unwrap()];
        assert_eq!(body.matches("mat4x4<f32>").count(), 6);
        assert_eq!(body.matches("vec4<f32>").count(), 16);
    }
    #[test]
    fn only_declared_overrides_are_passed() {
        let source =
            "override saturation: f32 = 1.0;\noverride  vignette = true;\nlet x = saturation_2;";
        assert!(super::declares_override(source, "saturation"));
        assert!(super::declares_override(source, "vignette"));
        assert!(!super::declares_override(source, "saturation_2"));
        assert!(!super::declares_override(source, "haze"));
    }
}
