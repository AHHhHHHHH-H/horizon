struct Effect { params: vec4<f32>, light: vec4<f32>, color: vec4<f32> }
struct Inputs { effects: array<Effect,4>, count: vec4<u32> }
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var<uniform> inputs: Inputs;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv:vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) i:u32) -> Vertex {
    let xy = vec2(f32((i<<1u)&2u), f32(i&2u));
    var out:Vertex; out.position=vec4(xy*2.0-1.0,0.0,1.0); out.uv=vec2(xy.x,1.0-xy.y); return out;
}
fn color(uv:vec2<f32>) -> vec4<f32> { return textureSampleLevel(scene,linear_sampler,clamp(uv,vec2(0.0),vec2(1.0)),0.0); }
@fragment fn fs_main(in:Vertex) -> @location(0) vec4<f32> {
    let base = color(in.uv); var result = base.rgb;
    let size = vec2<f32>(textureDimensions(scene));
    for (var e=0u; e<min(inputs.count.x,4u); e++) {
        let effect=inputs.effects[e];
        let strength=clamp(effect.params.y,0.0,1.0);
        if effect.params.x < 0.5 {
            var bloom=vec3(0.0);
            let radius=clamp(effect.params.w,0.5,8.0)/size;
            for (var i=0u;i<8u;i++) {
                let angle=f32(i)*0.7853981634;
                let sample=color(in.uv+vec2(cos(angle),sin(angle))*radius).rgb;
                bloom+=max(sample-vec3(effect.params.z),vec3(0.0));
            }
            result+=bloom*(strength/8.0);
        } else {
            var rays=0.0;
            let direction=effect.light.xy-in.uv;
            for (var i=0u;i<16u;i++) {
                let uv=clamp(in.uv+direction*(f32(i)+0.5)/16.0,vec2(0.0),vec2(1.0));
                let pixel=min(vec2<i32>(uv*size),vec2<i32>(size)-vec2(1));
                let open=select(0.0,1.0,textureLoad(depth,pixel,0)>=0.9999);
                rays+=open*dot(color(uv).rgb,vec3(0.2126,0.7152,0.0722));
            }
            result+=vec3(effect.light.zw,effect.color.x)*(strength*rays/16.0);
        }
    }
    return vec4(clamp(result,vec3(0.0),vec3(1.0)),base.a);
}
