//! EXPERIMENTAL ray-traced sun shadows, a research spike (docs/raytracing.md): off unless
//! `HORIZON_RT_SHADOWS=1` asks for them at startup and the GPU has hardware ray queries
//! (wgpu's `EXPERIMENTAL_RAY_QUERY`). Without either, nothing here exists and the game keeps the
//! vanilla look.
//!
//! Each terrain section's solid quads become one bottom-level acceleration structure (BLAS),
//! built straight from the section arena's vertices (no copy). Every frame one top-level
//! structure (TLAS) places every built section around the camera, and the solid terrain shader
//! (`terrain_rt.wgsl`) traces one ray per pixel towards the sun. A shadowed pixel reads the
//! lightmap with less sky light, so block light (torches) and caves stay as vanilla draws them.
use std::time::Instant;

use glam::{DVec3, Vec3};

use super::{SectionGpu, SectionKey, arena::Arena, resident::Resident, timing::GpuTimer};

/// The ray query feature.
const RAY_QUERY: wgpu::Features = wgpu::Features::EXPERIMENTAL_RAY_QUERY;
/// BLAS builds per frame at most: a world that just loaded builds over a few frames.
const BUILDS_PER_FRAME: usize = 512;
/// Recent frames kept in `Stats`' series.
const KEEP: usize = 600;
/// Shadow rays stop this far from the pixel, in blocks.
const RAY_LENGTH: f32 = 256.0;

/// Sections farther than this many chunks from the camera (horizontally) stay out of the TLAS:
/// `HORIZON_RT_SHADOW_CHUNKS`, all of them when unset or 0.
fn shadow_chunks() -> Option<i32> {
    std::env::var("HORIZON_RT_SHADOW_CHUNKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &i32| n > 0)
}

/// Whether the player asked for them (the device needs the feature from its creation).
pub fn requested() -> bool {
    std::env::var("HORIZON_RT_SHADOWS").as_deref() == Ok("1")
}

/// The features to request: ray queries, when asked for and the adapter has them.
pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    if !requested() {
        return wgpu::Features::empty();
    }
    if adapter.features().contains(RAY_QUERY) {
        return RAY_QUERY;
    }
    let info = adapter.get_info();
    log::warn!(
        "Horizon: ray-traced shadows asked for, but {} on {:?} has no ray queries: \
         the vanilla look stays",
        info.name,
        info.backend
    );
    wgpu::Features::empty()
}

/// The device's opt-in to `features`' experimental ones.
pub fn experimental(features: wgpu::Features) -> wgpu::ExperimentalFeatures {
    if features.contains(RAY_QUERY) {
        horizon_gpu_experimental::experimental_features()
    } else {
        wgpu::ExperimentalFeatures::disabled()
    }
}

/// What buffers that BLAS builds read need on top of their own usage.
pub fn blas_input(device: &wgpu::Device) -> wgpu::BufferUsages {
    if device.features().contains(RAY_QUERY) {
        wgpu::BufferUsages::BLAS_INPUT
    } else {
        wgpu::BufferUsages::empty()
    }
}

/// `terrain_rt.wgsl`'s `ShadowParams`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    /// xyz: towards the sun, world space; w: shadow strength (0 at night and when off).
    sun: [f32; 4],
    /// x: ray length, yzw unused.
    misc: [f32; 4],
}

/// Figures for the control socket's `rt-shadows` reply and the report.
#[derive(Default, Clone, serde::Serialize)]
pub struct Stats {
    pub blas_count: usize,
    pub blas_triangles: u64,
    pub blas_built_total: u64,
    pub triangles_built_total: u64,
    pub pending: usize,
    pub tlas_instances: u32,
    pub tlas_capacity: u32,
    /// CPU time of the last frame's BLAS creation and TLAS fill, in milliseconds.
    pub cpu_ms: f32,
    /// GPU milliseconds of recent timed frames' BLAS and TLAS builds, oldest first.
    pub build_gpu_ms: Vec<f32>,
    /// BLAS builds of those same frames (a frame without any still rebuilds the TLAS).
    pub builds_per_frame: Vec<u32>,
}

pub struct RayShadows {
    /// Drawn with them (the control socket's `rt-shadows on|off` for A/B runs).
    pub on: bool,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    tlas: wgpu::Tlas,
    group: wgpu::BindGroup,
    pub solid: wgpu::RenderPipeline,
    /// Sections meshed since their BLAS was built.
    pending: Vec<SectionKey>,
    /// `shadow_chunks`.
    reach: Option<i32>,
    timer: Option<GpuTimer>,
    /// BLAS builds of frames whose build time has not been read yet.
    timed_builds: std::collections::VecDeque<u32>,
    pub stats: Stats,
}

impl RayShadows {
    /// The shadows, when the device has ray queries.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        terrain_bindings: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        constants: &[(&str, f64)],
    ) -> Option<Self> {
        if !device.features().contains(RAY_QUERY) {
            return None;
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ray-traced shadow bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ray-traced shadow parameters"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tlas = Self::create_tlas(device, 1024);
        let group = Self::bind(device, &layout, &tlas, &params);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Horizon terrain, ray-traced shadows"),
            source: wgpu::ShaderSource::Wgsl(shader().into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ray-traced shadow terrain layout"),
            // Group 1 is the flat draws' (off with ray-traced shadows).
            bind_group_layouts: &[Some(terrain_bindings), None, Some(&layout)],
            ..Default::default()
        });
        let instance_attributes = wgpu::vertex_attr_array![5=>Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<crate::mesher::TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &super::TERRAIN_ATTRIBUTES,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            }),
        ];
        // The vanilla solid pipeline's state (gfx.rs's "Terrain solid"), other entry points.
        let solid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Terrain solid, ray-traced shadows"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_terrain_rt"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_solid_rt"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        log::info!("Horizon: experimental ray-traced shadows on (HORIZON_RT_SHADOWS=1)");
        Some(Self {
            on: true,
            layout,
            params,
            tlas,
            group,
            solid,
            pending: Vec::new(),
            reach: shadow_chunks(),
            timer: GpuTimer::new(device, queue),
            timed_builds: Default::default(),
            stats: Stats::default(),
        })
    }

    fn create_tlas(device: &wgpu::Device, capacity: u32) -> wgpu::Tlas {
        device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("Terrain sections"),
            max_instances: capacity,
            // Rebuilt every frame: build speed over trace speed would be the other choice.
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        })
    }

    fn bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        tlas: &wgpu::Tlas,
        params: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ray-traced shadows"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tlas.as_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params.as_entire_binding(),
                },
            ],
        })
    }

    /// A section got a new mesh: its BLAS is (re)built on a coming frame.
    pub fn queue(&mut self, key: SectionKey) {
        self.pending.push(key);
    }

    /// The bind group `solid` reads at group 2.
    pub fn group(&self) -> &wgpu::BindGroup {
        &self.group
    }

    /// Builds the BLAS of sections meshed since the last frame (at most `BUILDS_PER_FRAME`) and
    /// the frame's TLAS of every built section around `camera`, and writes the sun.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        sections: &mut Resident<SectionGpu>,
        arena: &Arena,
        indices: &wgpu::Buffer,
        camera: DVec3,
        sun_angle: f32,
    ) {
        let started = Instant::now();
        // Vanilla's sky turns the sun about the X axis after a quarter turn about Y: it rises
        // in the east (+X) at sun angle 270 degrees (gfx.rs's sky `sun` before that turn).
        let sun = Vec3::new(-sun_angle.sin(), sun_angle.cos(), 0.0);
        let strength = if self.on {
            (sun.y * 8.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let params = Params {
            sun: [sun.x, sun.y, sun.z, strength],
            misc: [RAY_LENGTH, 0.0, 0.0, 0.0],
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
        if let Some(timer) = &mut self.timer {
            let mut times = Vec::new();
            timer.collect(device, &mut times);
            for t in times {
                let builds = self.timed_builds.pop_front().unwrap_or(0);
                self.stats.build_gpu_ms.push(t);
                self.stats.builds_per_frame.push(builds);
            }
            let extra = self.stats.build_gpu_ms.len().saturating_sub(KEEP);
            self.stats.build_gpu_ms.drain(..extra);
            let extra = self.stats.builds_per_frame.len().saturating_sub(KEEP);
            self.stats.builds_per_frame.drain(..extra);
        }

        // New BLAS: one per section, its solid quads (translucent ones cast no shadow here).
        let take = self.pending.len().min(BUILDS_PER_FRAME);
        let mut builds: Vec<(u32, wgpu::BlasTriangleGeometrySizeDescriptor)> = Vec::new();
        for key in self.pending.drain(..take) {
            let Some(slot) = sections.slot(key) else {
                continue;
            };
            let s = sections.get_mut(slot);
            if s.solid == 0 || s.blas.is_some() || builds.iter().any(|b| b.0 == slot) {
                continue;
            }
            builds.push((
                slot,
                wgpu::BlasTriangleGeometrySizeDescriptor {
                    vertex_format: wgpu::VertexFormat::Float32x3,
                    vertex_count: s.solid * 4,
                    index_format: Some(wgpu::IndexFormat::Uint32),
                    index_count: Some(s.solid * 6),
                    flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
                },
            ));
        }
        let blases: Vec<wgpu::Blas> = builds
            .iter()
            .map(|(_, size)| {
                device.create_blas(
                    &wgpu::CreateBlasDescriptor {
                        label: None,
                        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                    },
                    wgpu::BlasGeometrySizeDescriptors::Triangles {
                        descriptors: vec![size.clone()],
                    },
                )
            })
            .collect();
        let stride = std::mem::size_of::<crate::mesher::TerrainVertex>() as u64;
        let entries: Vec<wgpu::BlasBuildEntry<'_>> = builds
            .iter()
            .zip(&blases)
            .map(|((slot, size), blas)| {
                let s = sections.get(*slot);
                wgpu::BlasBuildEntry {
                    blas,
                    geometry: wgpu::BlasGeometries::TriangleGeometries(vec![
                        wgpu::BlasTriangleGeometry {
                            size,
                            vertex_buffer: arena.buffer(s.page),
                            first_vertex: s.start as u32,
                            vertex_stride: stride,
                            index_buffer: Some(indices),
                            first_index: Some(0),
                            transform_buffer: None,
                            transform_buffer_offset: None,
                        },
                    ]),
                }
            })
            .collect();
        for (slot, _) in &builds {
            let solid = sections.get(*slot).solid;
            self.stats.triangles_built_total += u64::from(solid) * 2;
        }
        self.stats.blas_built_total += builds.len() as u64;

        // The TLAS: every section with a BLAS, camera-relative like the terrain's origins.
        let built = sections.entries().filter(|(_, s)| s.blas.is_some()).count() + blases.len();
        if built > self.tlas.get().len() {
            let capacity = (built as u32).next_power_of_two();
            self.tlas = Self::create_tlas(device, capacity);
            self.group = Self::bind(device, &self.layout, &self.tlas, &self.params);
        }
        self.stats.tlas_capacity = self.tlas.get().len() as u32;
        let mut n = 0;
        let mut triangles = 0u64;
        let new = builds
            .iter()
            .zip(&blases)
            .map(|((slot, _), blas)| (sections.key(*slot), blas, sections.get(*slot).solid));
        let column = (
            (camera.x.floor() as i32) >> 4,
            (camera.z.floor() as i32) >> 4,
        );
        let near = |k: &SectionKey| {
            self.reach
                .is_none_or(|r| (k.0 - column.0).abs() <= r && (k.2 - column.1).abs() <= r)
        };
        for (key, blas, solid) in sections
            .entries()
            .filter_map(|(k, s)| s.blas.as_ref().map(|b| (*k, b, s.solid)))
            .chain(new)
            .filter(|(k, _, _)| near(k))
        {
            triangles += u64::from(solid) * 2;
            let o = super::resident::origin(key, camera).as_vec3();
            self.tlas[n] = Some(wgpu::TlasInstance::new(
                blas,
                [1.0, 0.0, 0.0, o.x, 0.0, 1.0, 0.0, o.y, 0.0, 0.0, 1.0, o.z],
                0,
                0xff,
            ));
            n += 1;
        }
        let capacity = self.tlas.get().len();
        for i in n..capacity {
            if self.tlas[i].is_none() {
                break;
            }
            self.tlas[i] = None;
        }
        self.stats.tlas_instances = n as u32;
        self.stats.blas_count = built;
        self.stats.blas_triangles = triangles;
        if let Some(timer) = &mut self.timer {
            timer.begin(encoder);
            if timer.timing() {
                self.timed_builds.push_back(builds.len() as u32);
            }
        }
        encoder.build_acceleration_structures(entries.iter(), std::iter::once(&self.tlas));
        if let Some(timer) = &mut self.timer {
            timer.end(encoder);
        }
        drop(entries);
        for ((slot, _), blas) in builds.into_iter().zip(blases) {
            sections.get_mut(slot).blas = Some(blas);
        }
        self.stats.pending = self.pending.len();
        self.stats.cpu_ms = started.elapsed().as_secs_f32() * 1000.0;
    }

    /// After the frame's submission (the build timer reads back late).
    pub fn submitted(&mut self) {
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
    }
}

/// terrain.wgsl with the ray-traced entry points, ray queries enabled.
pub(super) fn shader() -> String {
    format!(
        "enable wgpu_ray_query;\n{}\n{}",
        include_str!("../terrain.wgsl"),
        include_str!("../terrain_rt.wgsl")
    )
}
