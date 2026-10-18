//! Occlusion culling of solid section draws, in two phases on the GPU.
//!
//! Frustum and cave culling leave many sections that terrain in front hides
//! entirely (about half of the solid vertices drawn from the ground at render
//! distance 16), and drawing fewer vertices is what helps an integrated GPU.
//! Phase 1 draws the solid sections that were in sight last frame. A Hi-Z
//! pyramid of its depth then tests every solid draw's section box: phase 2
//! draws the ones in sight that phase 1 did not, and the result is the next
//! frame's phase 1.
//!
//! The picture stays the one without culling: a section is left out only
//! when its box (grown by a block, the farthest a block model reaches out of
//! its cell) is behind phase 1's depth everywhere it covers, so all its
//! fragments would fail the depth test against the final depth, which is
//! only nearer. Translucent draws are not culled.
//!
//! Draws stay `DrawIndexedIndirectArgs` entries: the culled ones get an
//! instance count of 0, so this is for backends with native multi-draw
//! (Vulkan). Metal replays a multi-draw on the CPU, entry by entry.
use std::num::NonZeroU64;

const WORDS: u64 = 5 * 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    count: u32,
    levels: u32,
    screen: [u32; 2],
}

/// The Hi-Z pyramid for one depth buffer size.
struct Pyramid {
    size: [u32; 2],
    view: wgpu::TextureView,
    levels: u32,
    /// Level l + 1 from level l, with its workgroup counts.
    reduce_groups: Vec<(wgpu::BindGroup, [u32; 2])>,
    level0: wgpu::TextureView,
    level0_groups: [u32; 2],
}

pub struct Cull {
    prepare: wgpu::ComputePipeline,
    test: wgpu::ComputePipeline,
    reduce_depth: wgpu::ComputePipeline,
    reduce: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    pyramid: Option<Pyramid>,
    params: wgpu::Buffer,
    pub phase1: wgpu::Buffer,
    pub phase2: wgpu::Buffer,
    draw_capacity: u64,
    slots: wgpu::Buffer,
    slot_capacity: u64,
    slot_data: Vec<u32>,
    visibility: wgpu::Buffer,
    visibility_capacity: u64,
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

const PHASE_USAGE: wgpu::BufferUsages =
    wgpu::BufferUsages::INDIRECT.union(wgpu::BufferUsages::STORAGE);

impl Cull {
    /// Whether the device draws an indirect multi-draw natively.
    pub fn supported(backend: wgpu::Backend) -> bool {
        backend == wgpu::Backend::Vulkan
    }
    pub fn new(device: &wgpu::Device) -> Self {
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Section culling"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, false),
                storage(3, false),
                storage(4, false),
                storage(5, true),
                uniform(6),
                uniform(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Section culling"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let cull = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Section culling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("cull.wgsl").into()),
        });
        let hiz = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Hi-Z"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hiz.wgsl").into()),
        });
        let pipeline = |module, layout: Option<&wgpu::PipelineLayout>, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout,
                module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let storage_usage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        Self {
            prepare: pipeline(&cull, Some(&pipeline_layout), "prepare"),
            test: pipeline(&cull, Some(&pipeline_layout), "test"),
            reduce_depth: pipeline(&hiz, None, "reduce_depth"),
            reduce: pipeline(&hiz, None, "reduce"),
            layout,
            pyramid: None,
            params: buffer(
                device,
                "Section culling parameters",
                std::mem::size_of::<Params>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            ),
            phase1: buffer(device, "Section draws, phase 1", WORDS, PHASE_USAGE),
            phase2: buffer(device, "Section draws, phase 2", WORDS, PHASE_USAGE),
            draw_capacity: 1,
            slots: buffer(device, "Visible section slots", 4, storage_usage),
            slot_capacity: 1,
            slot_data: Vec::new(),
            visibility: buffer(device, "Section visibility", 4, wgpu::BufferUsages::STORAGE),
            visibility_capacity: 1,
        }
    }

    /// The Hi-Z pyramid for a `size` depth buffer: level 0 is half its size.
    fn pyramid(&mut self, device: &wgpu::Device, size: [u32; 2]) -> &Pyramid {
        if self.pyramid.as_ref().is_some_and(|p| p.size == size) {
            return self.pyramid.as_ref().expect("checked");
        }
        let base = size.map(|v| (v / 2).max(1));
        let levels = 32 - base[0].max(base[1]).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Hi-Z"),
            size: wgpu::Extent3d {
                width: base[0],
                height: base[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let level = |l| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: l,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let groups = |edge: u32| edge.div_ceil(8);
        let layout = self.reduce.get_bind_group_layout(0);
        let reduce_groups = (1..levels)
            .map(|l| {
                let dims = base.map(|v| (v >> l).max(1));
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Hi-Z level"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&level(l - 1)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&level(l)),
                        },
                    ],
                });
                (group, dims.map(groups))
            })
            .collect();
        self.pyramid.insert(Pyramid {
            size,
            view: texture.create_view(&Default::default()),
            levels,
            reduce_groups,
            level0: level(0),
            level0_groups: base.map(groups),
        })
    }

    /// Stage this frame's inputs: the resident slot of each visible section.
    /// `draws` is the number of solid draws.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        uploads: &mut super::upload::Uploads,
        encoder: &mut wgpu::CommandEncoder,
        slots: impl Iterator<Item = u32>,
        draws: u64,
        size: [u32; 2],
    ) {
        self.slot_data.clear();
        self.slot_data.extend(slots);
        let len = self.slot_data.len() as u64;
        if len > self.slot_capacity {
            self.slot_capacity = len.next_power_of_two();
            self.slots = buffer(
                device,
                "Visible section slots",
                self.slot_capacity * 4,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
        }
        let slots_needed = self.slot_data.iter().max().map_or(0, |&m| u64::from(m) + 1);
        if slots_needed > self.visibility_capacity {
            // A new buffer starts with nothing in sight: phase 2 tests all.
            self.visibility_capacity = slots_needed.next_power_of_two();
            self.visibility = buffer(
                device,
                "Section visibility",
                self.visibility_capacity * 4,
                wgpu::BufferUsages::STORAGE,
            );
        }
        if draws > self.draw_capacity {
            self.draw_capacity = draws.next_power_of_two();
            self.phase1 = buffer(
                device,
                "Section draws, phase 1",
                self.draw_capacity * WORDS,
                PHASE_USAGE,
            );
            self.phase2 = buffer(
                device,
                "Section draws, phase 2",
                self.draw_capacity * WORDS,
                PHASE_USAGE,
            );
        }
        uploads.buffer(
            encoder,
            &self.slots,
            0,
            bytemuck::cast_slice(&self.slot_data),
        );
        let levels = self.pyramid(device, size).levels;
        let params = Params {
            count: draws as u32,
            levels,
            screen: size,
        };
        uploads.buffer(encoder, &self.params, 0, bytemuck::bytes_of(&params));
    }

    fn group(
        &self,
        device: &wgpu::Device,
        draws: &wgpu::Buffer,
        origins: &wgpu::Buffer,
        globals: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        fn whole(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        let pyramid = self.pyramid.as_ref().expect("uploaded first");
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Section culling"),
            layout: &self.layout,
            entries: &[
                whole(0, draws),
                whole(1, &self.slots),
                whole(2, &self.visibility),
                whole(3, &self.phase1),
                whole(4, &self.phase2),
                whole(5, origins),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: globals,
                        offset: 0,
                        size: NonZeroU64::new(64),
                    }),
                },
                whole(7, &self.params),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&pyramid.view),
                },
            ],
        })
    }

    /// Before the world pass: fill both phases' draws from `draws`.
    pub fn prepare(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        draws: &wgpu::Buffer,
        origins: &wgpu::Buffer,
        globals: &wgpu::Buffer,
        count: u32,
    ) -> wgpu::BindGroup {
        let group = self.group(device, draws, origins, globals);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Section culling, phase 1"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.prepare);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        group
    }

    /// After phase 1: build the Hi-Z pyramid from `depth` and choose phase 2.
    pub fn test(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        group: &wgpu::BindGroup,
        depth: &wgpu::TextureView,
        count: u32,
    ) {
        let pyramid = self.pyramid.as_ref().expect("uploaded first");
        let level0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Hi-Z level 0"),
            layout: &self.reduce_depth.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&pyramid.level0),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Section culling, phase 2"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.reduce_depth);
        pass.set_bind_group(0, &level0, &[]);
        let [x, y] = pyramid.level0_groups;
        pass.dispatch_workgroups(x, y, 1);
        pass.set_pipeline(&self.reduce);
        for (level, [x, y]) in &pyramid.reduce_groups {
            pass.set_bind_group(0, level, &[]);
            pass.dispatch_workgroups(*x, *y, 1);
        }
        pass.set_pipeline(&self.test);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shaders_validate() {
        for source in [include_str!("cull.wgsl"), include_str!("hiz.wgsl")] {
            let module = naga::front::wgsl::parse_str(source).unwrap();
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap();
        }
    }
}
