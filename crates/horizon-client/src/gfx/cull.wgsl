// Occlusion culling of solid section draws (gfx/cull.rs).

struct Params {
    // Solid section draws.
    count: u32,
    // Hi-Z mip levels.
    levels: u32,
    // The depth buffer's size in pixels.
    screen: vec2<u32>,
}
// The leading member of terrain.wgsl's Globals.
struct Globals {
    view_proj: mat4x4<f32>,
}

// The frame's indirect draws, 5 words each (index count, instance count,
// first index, base vertex, first instance = the visible section).
@group(0) @binding(0) var<storage, read> draws: array<u32>;
// Resident slot of each visible section.
@group(0) @binding(1) var<storage, read> slots: array<u32>;
// Per resident slot: 1 when its box was in sight at the last test.
@group(0) @binding(2) var<storage, read_write> visibility: array<u32>;
@group(0) @binding(3) var<storage, read_write> phase1: array<u32>;
@group(0) @binding(4) var<storage, read_write> phase2: array<u32>;
// Camera-relative origin of each visible section.
@group(0) @binding(5) var<storage, read> origins: array<vec4<f32>>;
@group(0) @binding(6) var<uniform> globals: Globals;
@group(0) @binding(7) var<uniform> params: Params;
// Farthest depth (reverse Z: the smallest value) of each texel's pixels.
@group(0) @binding(8) var hiz: texture_2d<f32>;

// Phase 1 draws the sections in sight last frame; phase 2 starts empty.
@compute @workgroup_size(64)
fn prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= params.count {
        return;
    }
    let b = j * 5u;
    for (var k = 0u; k < 5u; k++) {
        phase1[b + k] = draws[b + k];
        phase2[b + k] = draws[b + k];
    }
    phase1[b + 1u] = min(visibility[slots[draws[b + 4u]]], 1u);
    phase2[b + 1u] = 0u;
}

// After phase 1's depth: phase 2 draws what is in sight and phase 1 did not
// draw; the result is the next frame's phase 1.
@compute @workgroup_size(64)
fn test(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= params.count {
        return;
    }
    let b = j * 5u;
    let section = draws[b + 4u];
    let seen = box_in_sight(origins[section].xyz);
    visibility[slots[section]] = select(0u, 1u, seen);
    phase2[b + 1u] = select(0u, 1u, seen && phase1[b + 1u] == 0u);
}

// Whether the section's box, grown by a block (models reach one block out
// of their cell), may have a pixel in front of the depth buffer: false only
// when the box's nearest depth is behind the farthest depth everywhere it
// covers, so every fragment of the section would fail the depth test.
fn box_in_sight(origin: vec3<f32>) -> bool {
    var lo = vec2(1e30);
    var hi = vec2(-1e30);
    var front = 0.0;
    for (var c = 0u; c < 8u; c++) {
        let corner = origin + vec3(
            select(-1.0, 17.0, (c & 1u) != 0u),
            select(-1.0, 17.0, (c & 2u) != 0u),
            select(-1.0, 17.0, (c & 4u) != 0u),
        );
        let clip = globals.view_proj * vec4(corner, 1.0);
        if clip.w <= 1e-4 {
            // Crosses the camera plane.
            return true;
        }
        let ndc = clip.xyz / clip.w;
        lo = min(lo, ndc.xy);
        hi = max(hi, ndc.xy);
        front = max(front, ndc.z);
    }
    let size = vec2<f32>(params.screen);
    // Pixels whose centres the box can cover (y down).
    let p0 = floor(vec2((lo.x + 1.0) * 0.5, (1.0 - hi.y) * 0.5) * size);
    let p1 = floor(vec2((hi.x + 1.0) * 0.5, (1.0 - lo.y) * 0.5) * size);
    if p1.x < 0.0 || p1.y < 0.0 || p0.x >= size.x || p0.y >= size.y {
        return false;
    }
    let a = vec2<u32>(clamp(p0, vec2(0.0), size - 1.0));
    let z = vec2<u32>(clamp(p1, vec2(0.0), size - 1.0));
    // Level 0 texel t covers pixels 2t and 2t + 1, level l + 1 texel t its
    // level-l texels 2t and 2t + 1; each level's last texel also covers an
    // odd remainder, hence the clamps.
    var level = 0u;
    var t0 = a >> vec2(1u);
    var t1 = z >> vec2(1u);
    while level + 1u < params.levels && (t1.x - t0.x > 3u || t1.y - t0.y > 3u) {
        level++;
        t0 >>= vec2(1u);
        t1 >>= vec2(1u);
    }
    let last = textureDimensions(hiz, level) - 1u;
    t1 = min(t1, last);
    t0 = min(t0, t1);
    var far = 1.0;
    for (var y = t0.y; y <= t1.y; y++) {
        for (var x = t0.x; x <= t1.x; x++) {
            far = min(far, textureLoad(hiz, vec2(x, y), i32(level)).r);
        }
    }
    return front >= far;
}
