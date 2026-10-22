//! Indirect multi-draws draw exactly what their entries say, on whatever
//! path the backend takes: on Metal, the vendored wgpu-hal lowers multi-draws
//! of 16 or more entries under pipelines without direct textures to indirect
//! command buffers generated on the GPU (third_party/wgpu-hal, `metal/icb.rs`).
//!
//! Each entry draws one quad covering one pixel of a 1-row target, chosen by
//! its instance (first_instance), so the target shows which entries drew, with
//! which instance and which vertices: entries with no instances or no indices
//! must leave their pixel clear, offsets into the index buffer must be kept.

use wgpu::util::DeviceExt;

const WIDTH: u32 = 96;

const SHADER: &str = r"
struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) value: u32,
}
// Vertex v is corner v % 4 of quad v / 4 (indexed draws), at pixel
// `instance`; the value written tells the pixel which instance and which
// quad drew it.
@vertex fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) instance: u32) -> Out {
    return corner(v % 4u, v / 4u, instance);
}
// Non-indexed draws: vertex v is corner 0 1 2 0 2 3 [v % 6] of quad v / 6.
@vertex fn vs_list(@builtin(vertex_index) v: u32, @builtin(instance_index) instance: u32) -> Out {
    let corners = array(0u, 1u, 2u, 0u, 2u, 3u);
    return corner(corners[v % 6u], v / 6u, instance);
}
fn corner(c: u32, quad: u32, instance: u32) -> Out {
    let x0 = (f32(instance) / f32(WIDTH)) * 2.0 - 1.0;
    let x1 = (f32(instance + 1u) / f32(WIDTH)) * 2.0 - 1.0;
    let x = select(x0, x1, c == 1u || c == 2u);
    let y = select(-1.0, 1.0, c >= 2u);
    var out: Out;
    out.clip = vec4(x, y, 0.0, 1.0);
    out.value = (instance + 1u) | (quad << 16u);
    return out;
}
@fragment fn fs(in: Out) -> @location(0) vec4<u32> {
    return vec4(in.value, 0u, 0u, 1u);
}
";

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    if !adapter
        .features()
        .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
    {
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::INDIRECT_FIRST_INSTANCE,
        ..Default::default()
    }))
    .ok()
}

/// Draws `entries` (indexed with `format` when given, else non-indexed) as
/// one multi-draw per run of `runs` (first entry, entries) and returns the
/// target's row.
fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    entries: &[[u32; 5]],
    indexed: Option<wgpu::IndexFormat>,
    runs: &[(u32, u32)],
) -> Vec<u32> {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.replace("WIDTH", &format!("{WIDTH}.0")).into()),
    });
    let format = wgpu::TextureFormat::R32Uint;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("multi-draw test"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some(if indexed.is_some() { "vs" } else { "vs_list" }),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    // Two quads per 4 vertices: corners 0 1 2, 0 2 3. Index i of quad q is
    // 4q + corner, so first_index and base_vertex both show in the result.
    let quads: Vec<u32> = (0..64u32)
        .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|c| q * 4 + c))
        .collect();
    // Two padding indices in front: the index buffer is bound at an offset.
    let mut index_bytes = Vec::new();
    match indexed {
        Some(wgpu::IndexFormat::Uint16) => {
            for i in [0u32, 0].iter().chain(&quads) {
                index_bytes.extend_from_slice(&(*i as u16).to_le_bytes());
            }
        }
        _ => {
            for i in [0u32, 0].iter().chain(&quads) {
                index_bytes.extend_from_slice(&i.to_le_bytes());
            }
        }
    }
    let index_offset = match indexed {
        Some(wgpu::IndexFormat::Uint16) => 4,
        _ => 8,
    };
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &index_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });
    let args: Vec<u32> = match indexed {
        Some(_) => entries.iter().flatten().copied().collect(),
        // Non-indexed: vertex count, instances, first vertex, first instance.
        None => entries
            .iter()
            .flat_map(|e| [e[0], e[1], e[2], e[4]])
            .collect(),
    };
    // One unused entry in front: the multi-draw starts at an offset.
    let stride = if indexed.is_some() { 5 } else { 4 };
    let mut padded = vec![7u32; stride];
    padded.extend(args);
    let indirect = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&padded),
        usage: wgpu::BufferUsages::INDIRECT,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(WIDTH) * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        if let Some(format) = indexed {
            pass.set_index_buffer(indices.slice(index_offset..), format);
        }
        for &(first, count) in runs {
            let offset = (u64::from(first) + 1) * stride as u64 * 4;
            match indexed {
                Some(_) => pass.multi_draw_indexed_indirect(&indirect, offset, count),
                None => pass.multi_draw_indirect(&indirect, offset, count),
            }
        }
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: None,
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = readback.slice(..).get_mapped_range().expect("mapped");
    bytemuck::cast_slice(&mapped).to_vec()
}

/// `count` entries [index count, instances, first index, base vertex,
/// first instance]: entry i draws quad (i * 5) % 61 at pixel i + 3, every
/// seventh draws no instances and every eleventh no indices.
fn entries(count: u32) -> Vec<[u32; 5]> {
    (0..count)
        .map(|i| {
            let quad = (i * 5) % 61;
            let indices = if i % 11 == 10 { 0 } else { 6 };
            let instances = if i % 7 == 6 { 0 } else { 1 };
            // Half the entries reach their quad through base_vertex.
            let (first, base) = if i % 2 == 0 {
                (quad * 6, 0)
            } else {
                (0, quad * 4)
            };
            [indices, instances, first, base, i + 3]
        })
        .collect()
}

/// What `entries` must leave in the row: entry i's pixel holds its
/// instance + 1 and its quad, the others nothing.
fn expected(entries: &[[u32; 5]], non_indexed: bool) -> Vec<u32> {
    let mut row = vec![0; WIDTH as usize];
    for (i, e) in entries.iter().enumerate() {
        if e[0] == 0 || e[1] == 0 {
            continue;
        }
        let quad = if non_indexed {
            e[2] / 6
        } else {
            (i as u32 * 5) % 61
        };
        row[e[4] as usize] = (e[4] + 1) | (quad << 16);
    }
    row
}

#[test]
fn multi_draws_draw_every_entry_as_given() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU with indirect first instance: skipped");
        return;
    };
    for count in [3, 16, 90] {
        let entries = entries(count);
        for format in [wgpu::IndexFormat::Uint16, wgpu::IndexFormat::Uint32] {
            let row = draw(&device, &queue, &entries, Some(format), &[(0, count)]);
            assert_eq!(row, expected(&entries, false), "{count} {format:?} draws");
        }
    }
    // Consecutive multi-draws of one list, with short ones between (which
    // Metal draws one by one): the long ones share a generation dispatch.
    let entries = entries(90);
    let runs = [(0, 20), (20, 3), (23, 30), (53, 1), (54, 36)];
    let row = draw(
        &device,
        &queue,
        &entries,
        Some(wgpu::IndexFormat::Uint32),
        &runs,
    );
    assert_eq!(row, expected(&entries, false), "runs {runs:?}");
}

#[test]
fn non_indexed_multi_draws_draw_every_entry_as_given() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU with indirect first instance: skipped");
        return;
    };
    for count in [3, 16, 90] {
        // Non-indexed: vertex count, instances, first vertex (a quad's
        // first), first instance.
        let entries: Vec<[u32; 5]> = entries(count)
            .into_iter()
            .map(|[n, inst, _, _, first_instance]| {
                let quad = (first_instance * 3) % 61;
                [n, inst, quad * 6, 0, first_instance]
            })
            .collect();
        let row = draw(&device, &queue, &entries, None, &[(0, count)]);
        assert_eq!(row, expected(&entries, true), "{count} non-indexed draws");
    }
}
