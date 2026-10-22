//! Multi-draw indirect through Metal indirect command buffers (ICBs).
//!
//! Horizon patch (not in wgpu-hal 30.0.1). Metal has no multi-draw, so the
//! backend used to encode one `drawIndexedPrimitives:...indirectBuffer:` per
//! entry of a multi-draw: thousands of driver calls a frame for a terrain
//! pass. Here a multi-draw of [`MIN_DRAWS`] or more under a pipeline that can
//! run in an ICB is encoded as one `executeCommandsInBuffer:withRange:`, and
//! a compute kernel writes the range's commands from the indirect arguments
//! on the GPU, so the draw count costs the CPU nothing.
//!
//! The kernels must run after whatever wrote the arguments and before the
//! render pass. wgpu-core 30 records every render pass in its own hal command
//! buffer and then, on the same hal encoder, the pass's "Pre Pass" buffer
//! (barriers, query resets, indirect validation), which it submits before the
//! pass. The generation for a pass is therefore encoded at the end of the
//! buffer that follows it (see `CommandEncoder::end_encoding`): after the
//! indirect validation, before the pass. This is the same place as upstream's
//! proposal (gfx-rs/wgpu#9640), which gets an explicit wgpu-core hook for it;
//! drop this file once a wgpu release lowers multi-draws to ICBs.
//!
//! Commands inherit the pipeline and every buffer binding from the render
//! encoder; only the index buffer travels in the command, so it is declared
//! with `useResource`. Metal accepts such pipelines only when no stage binds a
//! texture or sampler directly (argument buffers are fine), so only pipeline
//! layouts without direct textures and samplers in the vertex and fragment
//! stages get ICB-capable pipelines; everything else keeps the loop.

use alloc::{string::ToString as _, sync::Arc, vec::Vec};
use std::sync::OnceLock;

use objc2::{rc::Retained, runtime::ProtocolObject};
use objc2_foundation::{NSRange, NSString};
use objc2_metal::{
    MTLArgumentEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCompileOptions,
    MTLComputeCommandEncoder, MTLComputePipelineState, MTLDevice, MTLDispatchType, MTLFunction,
    MTLGPUFamily, MTLIndexType, MTLIndirectCommandBuffer, MTLIndirectCommandBufferDescriptor,
    MTLIndirectCommandType, MTLLanguageVersion, MTLLibrary, MTLPrimitiveType,
    MTLRenderCommandEncoder, MTLRenderStages, MTLResource, MTLResourceOptions, MTLResourceUsage,
    MTLSize,
};
use parking_lot::Mutex;

/// Multi-draws with fewer draws keep the per-draw loop: below this the
/// kernel dispatch costs about what the draws do.
pub(super) const MIN_DRAWS: u32 = 16;

/// Commands per ICB; a multi-draw larger than this gets an ICB of its own.
const CHUNK: u32 = 1 << 14;

/// ICBs kept for reuse once the GPU is done with them.
const POOL: usize = 16;

const SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct DrawArgs { uint vertex_count; uint instance_count; uint first_vertex; uint first_instance; };
struct DrawIndexedArgs { uint index_count; uint instance_count; uint first_index; int base_vertex; uint first_instance; };
struct Icb { command_buffer commands [[id(0)]]; };
// first: first command; index_base: index buffer elements before the bound offset.
struct Params { uint count; uint first; uint primitive; uint index_base; };

static primitive_type primitive(uint p) {
    switch (p) {
        case 0: return primitive_type::point;
        case 1: return primitive_type::line;
        case 2: return primitive_type::line_strip;
        case 4: return primitive_type::triangle_strip;
        default: return primitive_type::triangle;
    }
}

kernel void draws(const device DrawArgs* args [[buffer(0)]], device Icb& icb [[buffer(1)]],
                  constant Params& p [[buffer(2)]], uint i [[thread_position_in_grid]]) {
    if (i >= p.count) return;
    render_command cmd(icb.commands, p.first + i);
    DrawArgs a = args[i];
    if (a.vertex_count == 0 || a.instance_count == 0) { cmd.reset(); return; }
    cmd.draw_primitives(primitive(p.primitive), a.first_vertex, a.vertex_count, a.instance_count,
                        a.first_instance);
}

template <typename I>
static void indexed(const device DrawIndexedArgs* args, device Icb& icb, constant Params& p,
                    const device I* indices, uint i) {
    if (i >= p.count) return;
    render_command cmd(icb.commands, p.first + i);
    DrawIndexedArgs a = args[i];
    if (a.index_count == 0 || a.instance_count == 0) { cmd.reset(); return; }
    cmd.draw_indexed_primitives(primitive(p.primitive), a.index_count,
                                indices + p.index_base + a.first_index, a.instance_count,
                                uint(a.base_vertex), a.first_instance);
}

kernel void draws_u16(const device DrawIndexedArgs* args [[buffer(0)]], device Icb& icb [[buffer(1)]],
                      constant Params& p [[buffer(2)]], const device ushort* indices [[buffer(3)]],
                      uint i [[thread_position_in_grid]]) {
    indexed(args, icb, p, indices, i);
}

kernel void draws_u32(const device DrawIndexedArgs* args [[buffer(0)]], device Icb& icb [[buffer(1)]],
                      constant Params& p [[buffer(2)]], const device uint* indices [[buffer(3)]],
                      uint i [[thread_position_in_grid]]) {
    indexed(args, icb, p, indices, i);
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    count: u32,
    first: u32,
    primitive: u32,
    index_base: u32,
}

type Raw<T> = Retained<ProtocolObject<T>>;

struct Kernels {
    draws: Raw<dyn MTLComputePipelineState>,
    draws_u16: Raw<dyn MTLComputePipelineState>,
    draws_u32: Raw<dyn MTLComputePipelineState>,
    /// Encodes an ICB into the kernels' `Icb` argument (buffer 1); locked,
    /// as encoders on several threads may lease ICBs.
    arguments: Mutex<Raw<dyn MTLArgumentEncoder>>,
}

/// The device's ICB support: the generation kernels and a pool of ICBs.
pub(super) struct Icbs {
    device: Raw<dyn MTLDevice>,
    msl_version: MTLLanguageVersion,
    kernels: OnceLock<Option<Kernels>>,
    pool: Mutex<Vec<Icb>>,
}

impl core::fmt::Debug for Icbs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Icbs").finish_non_exhaustive()
    }
}

unsafe impl Send for Icbs {}
unsafe impl Sync for Icbs {}

impl Icbs {
    /// ICB support for `device`, where it is known to work: Apple GPUs from
    /// the M1 and A14 on (not the paravirtual device of macOS VMs).
    /// `WGPU_METAL_ICB=0` turns it off.
    pub(super) fn new(
        device: &Raw<dyn MTLDevice>,
        msl_version: MTLLanguageVersion,
    ) -> Option<Arc<Self>> {
        if std::env::var_os("WGPU_METAL_ICB").is_some_and(|v| v == "0") {
            return None;
        }
        let supported = device.supportsFamily(MTLGPUFamily::Apple7)
            && !device.name().to_string().contains("Paravirtual");
        supported.then(|| {
            Arc::new(Self {
                device: device.clone(),
                msl_version,
                kernels: OnceLock::new(),
                pool: Mutex::new(Vec::new()),
            })
        })
    }

    /// The generation kernels, compiled on first use; `None` if they don't
    /// compile (multi-draws then keep the loop).
    fn kernels(&self) -> Option<&Kernels> {
        self.kernels
            .get_or_init(|| {
                let kernels = self.compile();
                if let Err(ref e) = kernels {
                    log::warn!("Metal ICB kernels: {e}; multi-draws use the per-draw loop");
                }
                kernels.ok()
            })
            .as_ref()
    }

    /// Whether multi-draws can be lowered (compiling the kernels now if not
    /// yet: called when an ICB-capable pipeline is created).
    pub(super) fn ready(&self) -> bool {
        self.kernels().is_some()
    }

    fn compile(&self) -> Result<Kernels, alloc::string::String> {
        use alloc::format;
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(self.msl_version.max(MTLLanguageVersion::Version2_1));
        let library = self
            .device
            .newLibraryWithSource_options_error(&NSString::from_str(SOURCE), Some(&options))
            .map_err(|e| format!("{e:?}"))?;
        let pipeline = |name: &str| {
            let function = library
                .newFunctionWithName(&NSString::from_str(name))
                .ok_or_else(|| format!("no function {name}"))?;
            let state = self
                .device
                .newComputePipelineStateWithFunction_error(&function)
                .map_err(|e| format!("{e:?}"))?;
            Ok::<_, alloc::string::String>((function, state))
        };
        let (draws_fn, draws) = pipeline("draws")?;
        let (_, draws_u16) = pipeline("draws_u16")?;
        let (_, draws_u32) = pipeline("draws_u32")?;
        let arguments = unsafe { draws_fn.newArgumentEncoderWithBufferIndex(1) };
        Ok(Kernels {
            draws,
            draws_u16,
            draws_u32,
            arguments: Mutex::new(arguments),
        })
    }

    /// An ICB for at least `count` commands, from the pool if one fits.
    fn lease(self: &Arc<Self>, count: u32) -> Option<Lease> {
        let kernels = self.kernels()?;
        let reused = {
            let mut pool = self.pool.lock();
            pool.iter()
                .position(|icb| icb.capacity >= count)
                .map(|i| pool.swap_remove(i))
        };
        let icb = match reused {
            Some(icb) => icb,
            None => {
                let capacity = count.max(CHUNK).next_power_of_two();
                let descriptor = MTLIndirectCommandBufferDescriptor::new();
                descriptor.setCommandTypes(
                    MTLIndirectCommandType::Draw | MTLIndirectCommandType::DrawIndexed,
                );
                descriptor.setInheritPipelineState(true);
                descriptor.setInheritBuffers(true);
                // Inheriting buffers, these are unused; some Apple GPUs
                // (A12, A13) fault unless they are at the maximum.
                descriptor.setMaxVertexBufferBindCount(31);
                descriptor.setMaxFragmentBufferBindCount(31);
                let raw = unsafe {
                    self.device
                        .newIndirectCommandBufferWithDescriptor_maxCommandCount_options(
                            &descriptor,
                            capacity as usize,
                            MTLResourceOptions::StorageModePrivate,
                        )
                }?;
                raw.setLabel(Some(&NSString::from_str("wgpu multi-draw ICB")));
                let arguments = kernels.arguments.lock();
                let argument = self.device.newBufferWithLength_options(
                    arguments.encodedLength(),
                    MTLResourceOptions::StorageModeShared,
                )?;
                unsafe {
                    arguments.setArgumentBuffer_offset(Some(&argument), 0);
                    arguments.setIndirectCommandBuffer_atIndex(Some(&raw), 0);
                }
                drop(arguments);
                Icb {
                    raw,
                    argument,
                    capacity,
                }
            }
        };
        Some(Lease {
            icb: Some(icb),
            used: 0,
            pool: Arc::clone(self),
        })
    }
}

struct Icb {
    raw: Raw<dyn MTLIndirectCommandBuffer>,
    /// The kernels' `Icb` argument, pointing at `raw`.
    argument: Raw<dyn MTLBuffer>,
    capacity: u32,
}

/// An ICB in use by one command buffer; back to the pool when that command
/// buffer is dropped, which wgpu-core does only once the GPU is done with it.
pub(super) struct Lease {
    icb: Option<Icb>,
    used: u32,
    pool: Arc<Icbs>,
}

impl core::fmt::Debug for Lease {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Lease").field("used", &self.used).finish()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(icb) = self.icb.take() {
            let mut pool = self.pool.pool.lock();
            if pool.len() < POOL {
                pool.push(icb);
            }
        }
    }
}

unsafe impl Send for Lease {}
unsafe impl Sync for Lease {}

/// One multi-draw's commands to write before its pass runs.
pub(super) struct Generation {
    icb: Raw<dyn MTLIndirectCommandBuffer>,
    argument: Raw<dyn MTLBuffer>,
    first: u32,
    count: u32,
    args: Raw<dyn MTLBuffer>,
    args_offset: u64,
    /// Index buffer, its offset and type, for indexed draws.
    index: Option<(Raw<dyn MTLBuffer>, u64, MTLIndexType)>,
    primitive: MTLPrimitiveType,
}

unsafe impl Send for Generation {}
unsafe impl Sync for Generation {}

/// Where a multi-draw's commands go.
pub(super) struct Slot {
    icb: Raw<dyn MTLIndirectCommandBuffer>,
    first: u32,
}

/// Most entries skipped between two multi-draws whose commands one dispatch
/// still writes together (it writes the skipped entries' commands too, which
/// never run).
const MAX_GAP: u64 = 64;

/// Reserves the commands of a multi-draw of `count` entries at `args_offset`
/// in `args`, and queues their generation: within the previous multi-draw's
/// dispatch when that one read the same buffers up to a few entries before
/// (consecutive runs of one indirect list, as a frame's terrain draws are),
/// else as a dispatch of its own.
#[allow(clippy::too_many_arguments)]
pub(super) fn lower(
    icbs: &Arc<Icbs>,
    leases: &mut Vec<Lease>,
    pending: &mut Vec<Generation>,
    args: &super::Buffer,
    args_offset: u64,
    count: u32,
    index: Option<(Raw<dyn MTLBuffer>, u64, MTLIndexType)>,
    primitive: MTLPrimitiveType,
) -> Option<Slot> {
    let stride = if index.is_some() { 20 } else { 16 };
    if let (Some(last), Some(lease)) = (pending.last_mut(), leases.last_mut()) {
        let end = last.args_offset + u64::from(last.count) * stride;
        let gap = args_offset
            .checked_sub(end)
            .map(|bytes| (bytes / stride, bytes % stride));
        let same_index = match (&last.index, &index) {
            (None, None) => true,
            (Some((a, a_offset, a_type)), Some((b, b_offset, b_type))) => {
                core::ptr::eq(&**a, &**b) && a_offset == b_offset && a_type == b_type
            }
            _ => false,
        };
        if let (Some((gap, 0)), Some(icb), true) = (gap, &lease.icb, same_index) {
            let first = lease.used + gap as u32;
            if gap <= MAX_GAP
                && core::ptr::eq(&*last.icb, &*icb.raw)
                && core::ptr::eq(&*last.args, &*args.raw)
                && last.primitive == primitive
                && last.first + last.count == lease.used
                && u64::from(first) + u64::from(count) <= u64::from(icb.capacity)
            {
                lease.used = first + count;
                last.count = lease.used - last.first;
                return Some(Slot {
                    icb: icb.raw.clone(),
                    first,
                });
            }
        }
    }
    let fits = leases.last().is_some_and(|lease| {
        lease
            .icb
            .as_ref()
            .is_some_and(|icb| icb.capacity - lease.used >= count)
    });
    if !fits {
        leases.push(icbs.lease(count)?);
    }
    let lease = leases.last_mut()?;
    let icb = lease.icb.as_ref()?;
    let first = lease.used;
    lease.used += count;
    pending.push(Generation {
        icb: icb.raw.clone(),
        argument: icb.argument.clone(),
        first,
        count,
        args: args.raw.clone(),
        args_offset,
        index,
        primitive,
    });
    Some(Slot {
        icb: icb.raw.clone(),
        first,
    })
}

/// Encodes `slot`'s execution into the render pass: `count` commands.
///
/// # Safety
///
/// `index`, if any, must stay alive until the command buffer completes.
pub(super) unsafe fn execute(
    encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
    slot: &Slot,
    count: u32,
    index: Option<&ProtocolObject<dyn MTLBuffer>>,
) {
    if let Some(index) = index {
        encoder.useResource_usage_stages(
            ProtocolObject::<dyn MTLResource>::from_ref(index),
            MTLResourceUsage::Read,
            MTLRenderStages::Vertex,
        );
    }
    unsafe {
        encoder.executeCommandsInBuffer_withRange(
            &slot.icb,
            NSRange::new(slot.first as usize, count as usize),
        )
    };
}

/// Encodes the kernels that write `generations`' commands into `cmd_buf`.
pub(super) fn encode(
    icbs: &Icbs,
    cmd_buf: &ProtocolObject<dyn MTLCommandBuffer>,
    generations: &[Generation],
) {
    let Some(kernels) = icbs.kernels() else {
        return;
    };
    let Some(encoder) = cmd_buf.computeCommandEncoderWithDispatchType(MTLDispatchType::Concurrent)
    else {
        return;
    };
    encoder.setLabel(Some(&NSString::from_str("wgpu multi-draw ICB generation")));
    for g in generations {
        let (pipeline, index_base) = match &g.index {
            None => (&kernels.draws, 0),
            Some((buffer, offset, index_type)) => {
                // Bind at a 4-byte aligned offset; the rest in elements.
                let aligned = offset & !3;
                let stride = if *index_type == MTLIndexType::UInt16 {
                    2
                } else {
                    4
                };
                unsafe { encoder.setBuffer_offset_atIndex(Some(buffer), aligned as usize, 3) };
                let pipeline = if *index_type == MTLIndexType::UInt16 {
                    &kernels.draws_u16
                } else {
                    &kernels.draws_u32
                };
                (pipeline, ((offset - aligned) / stride) as u32)
            }
        };
        let params = Params {
            count: g.count,
            first: g.first,
            primitive: g.primitive.0 as u32,
            index_base,
        };
        encoder.setComputePipelineState(pipeline);
        unsafe {
            encoder.setBuffer_offset_atIndex(Some(&g.args), g.args_offset as usize, 0);
            encoder.setBuffer_offset_atIndex(Some(&g.argument), 0, 1);
            encoder.setBytes_length_atIndex(
                core::ptr::NonNull::from(&params).cast(),
                size_of::<Params>(),
                2,
            );
        }
        encoder.useResource_usage(
            ProtocolObject::<dyn MTLResource>::from_ref(&*g.icb),
            MTLResourceUsage::Write,
        );
        let width = pipeline.threadExecutionWidth().max(1);
        encoder.dispatchThreads_threadsPerThreadgroup(
            MTLSize {
                width: g.count as usize,
                height: 1,
                depth: 1,
            },
            MTLSize {
                width,
                height: 1,
                depth: 1,
            },
        );
    }
    encoder.endEncoding();
}
