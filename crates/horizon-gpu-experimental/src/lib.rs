//! wgpu gates its `EXPERIMENTAL_*` features (ray queries among them) behind an `unsafe` opt-in.
//! horizon-client forbids unsafe code, so the opt-in lives here, used only by the experimental
//! ray-traced shadows (docs/raytracing.md), which are off unless asked for.

/// The token that lets a device request wgpu's experimental features.
pub fn experimental_features() -> wgpu_types::ExperimentalFeatures {
    // SAFETY: wgpu asks callers to accept that its experimental features may have bugs that
    // reach undefined behaviour through safe calls. Horizon requests them only when a player
    // turns on the experimental ray-traced shadows, and only `EXPERIMENTAL_RAY_QUERY`.
    unsafe { wgpu_types::ExperimentalFeatures::enabled() }
}
