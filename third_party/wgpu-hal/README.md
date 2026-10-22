# Horizon's copy of wgpu-hal 30.0.1

This is wgpu-hal 30.0.1 from crates.io, patched into the build with `[patch.crates-io]` in the
workspace `Cargo.toml`. Changes from the release:

1. **Vulkan descriptor pool** (`src/vulkan/descriptor.rs`): the upstream fix
   [gfx-rs/wgpu#10124](https://github.com/gfx-rs/wgpu/pull/10124) for Intel's Windows driver
   crashing at device creation (#746).
2. **Metal multi-draw through indirect command buffers** (`src/metal/icb.rs`, plus hooks in
   `src/metal/{adapter,command,device,mod}.rs` marked "Horizon patch", and the
   `MTLArgumentEncoder` and `MTLIndirectCommandBuffer` features of `objc2-metal` in
   `Cargo.toml`). The release encodes one draw call per entry of an indirect multi-draw; on Apple
   GPUs (M1 and later) a multi-draw of 16 or more entries under a pipeline whose layout binds no
   texture or sampler directly is now one `executeCommandsInBuffer`, with the commands written on
   the GPU from the indirect arguments before the pass. `WGPU_METAL_ICB=0` turns it off. Upstream
   proposes the same in [gfx-rs/wgpu#9640](https://github.com/gfx-rs/wgpu/pull/9640), with a
   wgpu-core hook where this copy relies on wgpu-core 30 encoding each render pass's pre-pass
   right after it (see the comment at the top of `icb.rs`).

Drop each change once a wgpu release has it, and the folder once none is left.

# `wgpu_hal`: a cross-platform unsafe graphics abstraction

This crate defines a set of traits abstracting over modern graphics APIs,
with implementations ("backends") for Vulkan, Metal, Direct3D, and GL.

`wgpu_hal` is a spiritual successor to
[gfx-hal](https://github.com/gfx-rs/gfx), but with reduced scope, and
oriented towards WebGPU implementation goals. It has no overhead for
validation or tracking, and the API translation overhead is kept to the bare
minimum by the design of WebGPU. This API can be used for resource-demanding
applications and engines.

The `wgpu_hal` crate's main design choices:

- Our traits are meant to be _portable_: proper use
  should get equivalent results regardless of the backend.

- Our traits' contracts are _unsafe_: implementations perform minimal
  validation, if any, and incorrect use will often cause undefined behavior.
  This allows us to minimize the overhead we impose over the underlying
  graphics system. If you need safety, the [`wgpu-core`] crate provides a
  safe API for driving `wgpu_hal`, implementing all necessary validation,
  resource state tracking, and so on. (Note that `wgpu-core` is designed for
  use via FFI; the [`wgpu`] crate provides more idiomatic Rust bindings for
  `wgpu-core`.) Or, you can do your own validation.

- In the same vein, returned errors _only cover cases the user can't
  anticipate_, like running out of memory or losing the device. Any errors
  that the user could reasonably anticipate are their responsibility to
  avoid. For example, `wgpu_hal` returns no error for mapping a buffer that's
  not mappable: as the buffer creator, the user should already know if they
  can map it.

- We use _static dispatch_. The traits are not
  generally object-safe. You must select a specific backend type
  like [`vulkan::Api`] or [`metal::Api`], and then use that
  according to the main traits, or call backend-specific methods.

- We use _idiomatic Rust parameter passing_,
  taking objects by reference, returning them by value, and so on,
  unlike `wgpu-core`, which refers to objects by ID.

- We map buffer contents _persistently_. This means that the buffer
  can remain mapped on the CPU while the GPU reads or writes to it.
  You must explicitly indicate when data might need to be
  transferred between CPU and GPU, if `wgpu_hal` indicates that the
  mapping is not coherent (that is, automatically synchronized
  between the two devices).

- You must record _explicit barriers_ between different usages of a
  resource. For example, if a buffer is written to by a compute
  shader, and then used as and index buffer to a draw call, you
  must use [`CommandEncoder::transition_buffers`] between those two
  operations.

- Pipeline layouts are _explicitly specified_ when setting bind
  group. Incompatible layouts disturb groups bound at higher indices.

- The API _accepts collections as iterators_, to avoid forcing the user to
  store data in particular containers. The implementation doesn't guarantee
  that any of the iterators are drained, unless stated otherwise by the
  function documentation. For this reason, we recommend that iterators don't
  do any mutating work.

Unfortunately, `wgpu_hal`'s safety requirements are not fully documented.
Ideally, all trait methods would have doc comments setting out the
requirements users must meet to ensure correct and portable behavior. If you
are aware of a specific requirement that a backend imposes that is not
ensured by the traits' documented rules, please file an issue. Or, if you are
a capable technical writer, please file a pull request!

[`wgpu-core`]: https://crates.io/crates/wgpu-core
[`wgpu`]: https://crates.io/crates/wgpu
[`vulkan::Api`]: vulkan/struct.Api.html
[`metal::Api`]: metal/struct.Api.html

## Primary backends

The `wgpu_hal` crate has full-featured backends implemented on the following
platform graphics APIs:

- Vulkan, available on Linux, Android, and Windows, using the [`ash`] crate's
  Vulkan bindings. It's also available on macOS, if you install [MoltenVK].

- Metal on macOS, using the [`metal`] crate's bindings.

- Direct3D 12 on Windows, using the [`windows`] crate's bindings.

[`ash`]: https://crates.io/crates/ash
[MoltenVK]: https://github.com/KhronosGroup/MoltenVK
[`metal`]: https://crates.io/crates/metal
[`windows`]: https://crates.io/crates/windows

## Secondary backends

The `wgpu_hal` crate has a partial implementation based on the following
platform graphics API:

- The GL backend is available anywhere OpenGL, OpenGL ES, or WebGL are
  available. See the [`gles`] module documentation for details.

[`gles`]: gles/index.html

You can see what capabilities an adapter is missing by checking the
[`DownlevelCapabilities`][tdc] in [`ExposedAdapter::capabilities`], available
from [`Instance::enumerate_adapters`].

The API is generally designed to fit the primary backends better than the
secondary backends, so the latter may impose more overhead.

[tdc]: wgt::DownlevelCapabilities

## Debugging

Most of the information on the wiki [Debugging wgpu Applications][wiki-debug]
page still applies to this API, with the exception of API tracing/replay
functionality, which is only available in `wgpu-core`.

[wiki-debug]: https://github.com/gfx-rs/wgpu/wiki/Debugging-wgpu-Applications
