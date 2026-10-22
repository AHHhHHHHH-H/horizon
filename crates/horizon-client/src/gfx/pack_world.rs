//! A Horizon shader pack inside the world pass (docs/shader-packs.md,
//! "Horizon shaders v1"): the `[gbuffer]` pipelines (Horizon's terrain,
//! entity and sky shaders with the pack's hooks, writing the world colour and
//! the G-buffer), the `[shadow]` map and its pass, and the copy of the opaque
//! depth. Only exists while a pack that asks for them is selected; the
//! vanilla pipelines are untouched.
use crate::shader_pack::{Gbuffer, ShaderPack, Shadow};
use glam::{DMat4, DVec3, Mat4, Vec3};

/// The G-buffer: octahedral normal (2 x 16 bits), block and sky light (8
/// bits each), material.
pub const GBUFFER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;

/// What Horizon's world pipelines are built from, for the pack's twins.
pub struct WorldLayouts<'a> {
    pub terrain: &'a wgpu::BindGroupLayout,
    /// The flat section draws' group 1 (Metal's terrain path), if there is one.
    pub flat: Option<&'a wgpu::BindGroupLayout>,
    pub entity: &'a wgpu::BindGroupLayout,
    pub sprite_constants: &'a [(&'a str, f64)],
    pub terrain_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub flat_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub entity_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub color: wgpu::TextureFormat,
}

/// The `[gbuffer]` twins of Horizon's world pipelines.
pub struct GbufferPipelines {
    pub solid: wgpu::RenderPipeline,
    pub translucent: wgpu::RenderPipeline,
    pub flat_solid: Option<wgpu::RenderPipeline>,
    pub flat_translucent: Option<wgpu::RenderPipeline>,
    pub entity: wgpu::RenderPipeline,
    /// The pack's sky (`sky_color`), drawn instead of vanilla's sky disc.
    pub sky: Option<wgpu::RenderPipeline>,
    pub sun_moon: bool,
    pub stars: bool,
    pub clouds: bool,
}

pub struct ShadowMap {
    pub settings: Shadow,
    pub view: wgpu::TextureView,
    pub pipeline: wgpu::RenderPipeline,
}

/// Textures at the world's size.
struct Sized {
    size: [u32; 2],
    /// With `[gbuffer]` (the world pass is split around the opaque geometry).
    gbuffer: Option<(wgpu::TextureView, wgpu::TextureView)>,
}

pub struct PackWorld {
    /// Group 2 of every pack world pipeline.
    pub group: wgpu::BindGroup,
    /// Group 1 where a pipeline has none (all but the flat terrain ones).
    pub empty_group: wgpu::BindGroup,
    /// `group` with a stand-in shadow map, for the shadow pass.
    pub shadow_group: wgpu::BindGroup,
    /// This frame's shadow view (`shadow_view`'s key).
    pub shadow_now: u64,
    /// What the shadow map holds: the view, the casters and the terrain
    /// uploads it was drawn with.
    pub shadow_drawn: Option<u64>,
    uniform: wgpu::Buffer,
    pub gbuffer: Option<GbufferPipelines>,
    pub shadow: Option<ShadowMap>,
    sized: Option<Sized>,
    generation: u64,
    previous: Option<(Mat4, DVec3)>,
    started: std::time::Instant,
    frames: u32,
    /// The shadow pass's section origins and draws (built by the renderer).
    pub shadow_instances: wgpu::Buffer,
    pub shadow_indirect: wgpu::Buffer,
    pub shadow_capacity: u64,
}

pub(super) fn compare_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Shader pack shadow"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::GreaterEqual),
        ..Default::default()
    })
}

impl PackWorld {
    /// The pack's world resources, or `None` when it has neither `[gbuffer]`
    /// nor `[shadow]` nor deferred stages. `materials`: the material of every
    /// sprite layer (`shader_pack::material_table`).
    pub fn new(
        device: &wgpu::Device,
        pack: &ShaderPack,
        layouts: &WorldLayouts,
        materials: &[u32],
    ) -> Result<Option<Self>, String> {
        if pack.gbuffer.is_none() && pack.shadow.is_none() {
            return Ok(None);
        }
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shader pack world"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    4,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shader pack world (unused)"),
            entries: &[],
        });
        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                ..Default::default()
            })
        };
        let terrain_layout = pipeline_layout(
            "Shader pack terrain",
            &[Some(layouts.terrain), Some(&empty), Some(&layout)],
        );
        let constants = pack.constants();
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = |label, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        // Horizon's sprite constants, then the pack's options the module declares.
        let terrain_source = pack
            .gbuffer
            .as_ref()
            .map_or_else(crate::shader_pack::shadow_only_terrain, |g| {
                g.terrain.clone()
            });
        let mut terrain_constants: Vec<(&str, f64)> = layouts.sprite_constants.to_vec();
        terrain_constants.extend(super::pack_stages::declared(&constants, &terrain_source));
        let terrain = module("Shader pack terrain", &terrain_source);
        let color_target = |blend| {
            Some(wgpu::ColorTargetState {
                format: layouts.color,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let gbuffer_target = Some(wgpu::ColorTargetState {
            format: GBUFFER_FORMAT,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        });
        let depth = |write, compare, bias| {
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias,
            })
        };
        let ge = wgpu::CompareFunction::GreaterEqual;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        #[allow(clippy::too_many_arguments)]
        let world_pipeline =
            |label: &str,
             layout: &wgpu::PipelineLayout,
             module: &wgpu::ShaderModule,
             constants: &[(&str, f64)],
             vertex: &str,
             fragment: &str,
             buffers: &[Option<wgpu::VertexBufferLayout>],
             cull: Option<wgpu::Face>,
             targets: &[Option<wgpu::ColorTargetState>],
             depth_stencil: Option<wgpu::DepthStencilState>| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some(vertex),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants,
                            ..Default::default()
                        },
                        buffers,
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: cull,
                        ..Default::default()
                    },
                    depth_stencil,
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(fragment),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants,
                            ..Default::default()
                        },
                        targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let back = Some(wgpu::Face::Back);
        let gbuffer = match &pack.gbuffer {
            None => None,
            Some(Gbuffer {
                entity,
                sky,
                sun_moon,
                stars,
                clouds,
                ..
            }) => {
                let solid_targets = [color_target(None), gbuffer_target.clone()];
                let translucent_targets = [color_target(alpha), gbuffer_target.clone()];
                let flat_layout = layouts.flat.map(|flat| {
                    pipeline_layout(
                        "Shader pack terrain (flat)",
                        &[Some(layouts.terrain), Some(flat), Some(&layout)],
                    )
                });
                let flat = |label, fragment, targets: &[Option<wgpu::ColorTargetState>]| {
                    flat_layout.as_ref().map(|l| {
                        world_pipeline(
                            label,
                            l,
                            &terrain,
                            &terrain_constants,
                            "vs_terrain_flat_pack",
                            fragment,
                            layouts.flat_buffers,
                            back,
                            targets,
                            depth(true, ge, Default::default()),
                        )
                    })
                };
                let entity_module = module("Shader pack entities", entity);
                let entity_constants = super::pack_stages::declared(&constants, entity);
                let entity_layout = pipeline_layout(
                    "Shader pack entities",
                    &[Some(layouts.entity), Some(&empty), Some(&layout)],
                );
                Some(GbufferPipelines {
                    solid: world_pipeline(
                        "Shader pack terrain solid",
                        &terrain_layout,
                        &terrain,
                        &terrain_constants,
                        "vs_terrain_pack",
                        "fs_solid_pack",
                        layouts.terrain_buffers,
                        back,
                        &solid_targets,
                        depth(true, ge, Default::default()),
                    ),
                    translucent: world_pipeline(
                        "Shader pack terrain translucent",
                        &terrain_layout,
                        &terrain,
                        &terrain_constants,
                        "vs_terrain_pack",
                        "fs_translucent_pack",
                        layouts.terrain_buffers,
                        back,
                        &translucent_targets,
                        depth(true, ge, Default::default()),
                    ),
                    flat_solid: flat(
                        "Shader pack terrain solid (flat)",
                        "fs_solid_pack",
                        &solid_targets,
                    ),
                    flat_translucent: flat(
                        "Shader pack terrain translucent (flat)",
                        "fs_translucent_pack",
                        &translucent_targets,
                    ),
                    entity: world_pipeline(
                        "Shader pack entities",
                        &entity_layout,
                        &entity_module,
                        &entity_constants,
                        "vs_entity_pack",
                        "fs_entity_pack",
                        layouts.entity_buffers,
                        None,
                        &[color_target(alpha), gbuffer_target.clone()],
                        depth(true, ge, Default::default()),
                    ),
                    sky: sky.then(|| {
                        world_pipeline(
                            "Shader pack sky",
                            &terrain_layout,
                            &terrain,
                            &terrain_constants,
                            "vs_sky",
                            "fs_sky_pack",
                            &[],
                            None,
                            &[color_target(None)],
                            depth(false, wgpu::CompareFunction::Always, Default::default()),
                        )
                    }),
                    sun_moon: *sun_moon,
                    stars: *stars,
                    clouds: *clouds,
                })
            }
        };
        let shadow = pack.shadow.map(|settings| {
            let view = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Shader pack shadow map"),
                    size: wgpu::Extent3d {
                        width: settings.resolution,
                        height: settings.resolution,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth32Float,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let pipeline = world_pipeline(
                "Shader pack shadow",
                &terrain_layout,
                &terrain,
                &terrain_constants,
                "vs_shadow",
                "fs_shadow",
                layouts.terrain_buffers,
                // Both faces: thin walls and leaves cast from either side.
                None,
                &[],
                depth(
                    true,
                    ge,
                    wgpu::DepthBiasState {
                        constant: -2,
                        slope_scale: -1.5,
                        clamp: 0.0,
                    },
                ),
            );
            ShadowMap {
                settings,
                view,
                pipeline,
            }
        });
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(error.to_string());
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shader pack world frame"),
            size: std::mem::size_of::<super::pack_stages::FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let materials = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("Shader pack materials"),
                contents: bytemuck::cast_slice(if materials.is_empty() {
                    &[0x0101_0101]
                } else {
                    materials
                }),
                usage: wgpu::BufferUsages::STORAGE,
            },
        );
        // A stand-in for the shadow map: when the pack has none, and in the
        // shadow pass itself, which can't read the map it writes.
        let blank_shadow = {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("Shader pack no shadow map"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let shadow_view = shadow.as_ref().map_or(&blank_shadow, |s| &s.view);
        let compare = compare_sampler(device);
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shader pack repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let world_group = |label, shadow_view| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(shadow_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&compare),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: materials.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&repeat),
                    },
                ],
            })
        };
        let group = world_group("Shader pack world", shadow_view);
        let shadow_group = world_group("Shader pack world (shadow pass)", &blank_shadow);
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 20 * 64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shader pack world (unused)"),
            layout: &empty,
            entries: &[],
        });
        Ok(Some(Self {
            group,
            shadow_group,
            shadow_now: 0,
            shadow_drawn: None,
            empty_group,
            uniform,
            gbuffer,
            shadow,
            sized: None,
            generation: 0,
            previous: None,
            started: std::time::Instant::now(),
            frames: 0,
            shadow_instances: buffer("Shader pack shadow origins", wgpu::BufferUsages::VERTEX),
            shadow_indirect: buffer("Shader pack shadow draws", wgpu::BufferUsages::INDIRECT),
            shadow_capacity: 64,
        }))
    }

    /// Room for `capacity` shadow draws.
    pub fn grow_shadow(&mut self, device: &wgpu::Device, capacity: u64) {
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        self.shadow_instances = buffer(
            "Shader pack shadow origins",
            capacity * 16,
            wgpu::BufferUsages::VERTEX,
        );
        self.shadow_indirect = buffer(
            "Shader pack shadow draws",
            capacity * 20,
            wgpu::BufferUsages::INDIRECT,
        );
        self.shadow_capacity = capacity;
    }
    /// Bumped whenever the world-sized textures are made again.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn gbuffer_view(&self) -> Option<&wgpu::TextureView> {
        self.sized.as_ref()?.gbuffer.as_ref().map(|g| &g.0)
    }
    /// The depth before translucent geometry (only where the world pass is split).
    pub fn opaque_depth_view(&self) -> Option<&wgpu::TextureView> {
        self.sized.as_ref()?.gbuffer.as_ref().map(|g| &g.1)
    }
    pub fn shadow_view(&self) -> Option<&wgpu::TextureView> {
        self.shadow.as_ref().map(|s| &s.view)
    }
    /// The G-buffer and opaque depth for a world of `size`.
    pub fn prepare(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.sized.as_ref().is_some_and(|s| s.size == size) {
            return;
        }
        let texture = |label, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
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
                .create_view(&Default::default())
        };
        let binding = wgpu::TextureUsages::TEXTURE_BINDING;
        self.sized = Some(Sized {
            size,
            gbuffer: self.gbuffer.is_some().then(|| {
                (
                    texture(
                        "Shader pack G-buffer",
                        GBUFFER_FORMAT,
                        binding | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    ),
                    texture(
                        "Shader pack opaque depth",
                        wgpu::TextureFormat::Depth32Float,
                        binding | wgpu::TextureUsages::COPY_DST,
                    ),
                )
            }),
        });
        self.generation += 1;
    }

    /// After the opaque world: keep its depth for the composite stages.
    pub fn copy_opaque_depth(&self, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView) {
        if let Some(copy) = self.opaque_depth_view() {
            encoder.copy_texture_to_texture(
                depth.texture().as_image_copy(),
                copy.texture().as_image_copy(),
                copy.texture().size(),
            );
        }
    }

    /// Fill in this frame's shadow matrix, light and history in `uniform`
    /// (camera-relative, camera at `camera`; `view_projection`: this frame's
    /// camera) and upload it for the world hooks.
    pub fn frame(
        &mut self,
        queue: &wgpu::Queue,
        uniform: &mut super::pack_stages::FrameUniform,
        camera: DVec3,
        view_projection: Mat4,
        world_size: [u32; 2],
    ) {
        let (previous, previous_camera) = self.previous.unwrap_or((view_projection, camera));
        uniform.previous_view_projection = previous.to_cols_array_2d();
        let delta = camera - previous_camera;
        uniform.camera_delta = [delta.x as f32, delta.y as f32, delta.z as f32, 0.0];
        self.previous = Some((view_projection, camera));
        uniform.time[0] = self.started.elapsed().as_secs_f32();
        uniform.time[1] = self.frames as f32;
        self.frames = self.frames.wrapping_add(1);
        let sun = Vec3::from_slice(&uniform.sun[..3]);
        // The sun by day, the moon by night.
        let light = if sun.y >= 0.0 { sun } else { -sun };
        uniform.shadow_light = [light.x, light.y, light.z, f32::from(u8::from(sun.y >= 0.0))];
        if let Some(shadow) = &self.shadow {
            let s = shadow.settings;
            let (matrix, key) = shadow_view(light, camera, s.distance, s.resolution);
            uniform.shadow_view_projection = matrix.to_cols_array_2d();
            uniform.shadow = [s.resolution as f32, s.distance, s.distortion, 1.0];
            self.shadow_now = key;
        }
        let [w, h] = world_size.map(|v| v as f32);
        uniform.resolution = [w, h, 1.0 / w, 1.0 / h];
        uniform.world = uniform.resolution;
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(uniform));
    }
}

/// Camera-relative world to the shadow map's clip space: an orthographic
/// box `distance` blocks on each side around the eye, seen along `light`,
/// with reversed depth (1 nearest the light). The box moves in whole texels
/// of the map (from the absolute `camera`), so shadows don't shimmer as the
/// camera moves.
#[cfg(test)]
pub fn shadow_matrix(light: Vec3, camera: DVec3, distance: f32, resolution: u32) -> Mat4 {
    shadow_view(light, camera, distance, resolution).0
}

/// `shadow_matrix`, and what the map it draws depends on: the light's
/// direction and the box's place in whole texels. While both stay, the map
/// is the same (for the same terrain), so the shadow pass can be skipped.
/// The light's direction moves in steps of about 0.015° (a few ticks of the
/// sun's path), too small to see, so a still player redraws it a few times
/// a second instead of every frame.
pub fn shadow_view(light: Vec3, camera: DVec3, distance: f32, resolution: u32) -> (Mat4, u64) {
    let light = (light.normalize_or(Vec3::Y) * 4096.0)
        .round()
        .normalize_or(Vec3::Y);
    let up = if light.z.abs() > 0.99 {
        Vec3::X
    } else {
        Vec3::Z
    };
    let view = Mat4::look_to_rh(Vec3::ZERO, -light, up);
    let texel = f64::from(2.0 * distance / resolution as f32);
    let eye =
        DMat4::from_cols_array(&view.to_cols_array().map(f64::from)).transform_vector3(camera);
    let snap = |v: f64| ((v / texel).round() * texel - v) as f32;
    // Depth moves in whole texels too, so a map stays right as the eye moves.
    let (x, y, z) = (snap(eye.x), snap(eye.y), snap(eye.z));
    // Casters up to 512 blocks towards the light, receivers as far below.
    let reach = 512.0;
    let matrix = Mat4::orthographic_rh(
        x - distance,
        x + distance,
        y - distance,
        y + distance,
        reach - z,
        -reach - z,
    ) * view;
    let mut key = std::hash::DefaultHasher::new();
    std::hash::Hash::hash(&light.to_array().map(f32::to_bits), &mut key);
    std::hash::Hash::hash(
        &[eye.x, eye.y, eye.z].map(|v| (v / texel).round() as i64),
        &mut key,
    );
    std::hash::Hash::hash(&(distance.to_bits(), resolution), &mut key);
    (matrix, std::hash::Hasher::finish(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shadow_matrix_puts_the_eye_in_the_middle_and_the_sun_side_near() {
        let m = shadow_matrix(
            Vec3::new(0.3, 0.9, 0.1),
            DVec3::new(0.0, 64.0, 0.0),
            128.0,
            2048,
        );
        let eye = m.project_point3(Vec3::ZERO);
        assert!(eye.x.abs() < 1e-3 && eye.y.abs() < 1e-3, "{eye}");
        assert!((eye.z - 0.5).abs() < 1e-3, "{eye}");
        // A point towards the sun is nearer the light: deeper (reversed) depth.
        let up = m.project_point3(Vec3::new(0.3, 0.9, 0.1) * 100.0);
        assert!(up.z > eye.z, "{up}");
        // 128 blocks to the side is the edge of the map.
        let light = Vec3::new(0.3, 0.9, 0.1).normalize();
        let side = light.cross(Vec3::Z).normalize() * 128.0;
        let edge = m.project_point3(side);
        assert!(
            (edge.x.abs().max(edge.y.abs()) - 1.0).abs() < 0.01,
            "{edge}"
        );
    }

    #[test]
    fn the_shadow_box_moves_in_whole_texels() {
        let light = Vec3::new(0.2, 0.95, -0.1);
        let texel_clip = 2.0 / 2048.0;
        // A point fixed in the world, seen from two eyes a fraction of a texel apart.
        let world = DVec3::new(10.0, 70.0, -3.0);
        let a = DVec3::new(0.0, 64.0, 0.0);
        let b = a + DVec3::new(0.03, 0.0, 0.01);
        let at = |eye: DVec3| {
            shadow_matrix(light, eye, 128.0, 2048).project_point3((world - eye).as_vec3())
        };
        let (pa, pb) = (at(a), at(b));
        // Where it lands moves by whole texels (or not at all).
        for d in [pa.x - pb.x, pa.y - pb.y] {
            let texels = d / texel_clip;
            assert!((texels - texels.round()).abs() < 0.02, "{texels}");
        }
    }
}
