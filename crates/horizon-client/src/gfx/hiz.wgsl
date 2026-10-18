// Hi-Z pyramid for occlusion culling (gfx/cull.rs): each texel holds the
// farthest depth (reverse Z: the smallest value) of the pixels it covers.

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var level0: texture_storage_2d<r32float, write>;

// Hi-Z level 0 from the depth buffer.
@compute @workgroup_size(8, 8)
fn reduce_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(level0);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let source = textureDimensions(depth);
    let first = id.xy * 2u;
    let odd = (id.xy == size - 1u) & ((source & vec2(1u)) == vec2(1u));
    let to = min(first + select(vec2(1u), vec2(2u), odd), source - 1u);
    var m = 1.0;
    for (var y = first.y; y <= to.y; y++) {
        for (var x = first.x; x <= to.x; x++) {
            m = min(m, textureLoad(depth, vec2(x, y), 0));
        }
    }
    textureStore(level0, id.xy, vec4(m));
}

@group(0) @binding(2) var source_level: texture_2d<f32>;
@group(0) @binding(3) var next_level: texture_storage_2d<r32float, write>;

// Hi-Z level l + 1 from level l.
@compute @workgroup_size(8, 8)
fn reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(next_level);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let source = textureDimensions(source_level);
    let first = id.xy * 2u;
    let odd = (id.xy == size - 1u) & ((source & vec2(1u)) == vec2(1u));
    let to = min(first + select(vec2(1u), vec2(2u), odd), source - 1u);
    var m = 1.0;
    for (var y = first.y; y <= to.y; y++) {
        for (var x = first.x; x <= to.x; x++) {
            m = min(m, textureLoad(source_level, vec2(x, y), 0).r);
        }
    }
    textureStore(next_level, id.xy, vec4(m));
}
