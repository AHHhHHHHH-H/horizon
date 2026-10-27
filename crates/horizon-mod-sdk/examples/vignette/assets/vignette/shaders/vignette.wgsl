// The vignette's post pass (docs/mods.md, "Shaders from a mod"): the picture darkens toward its
// edges by `params.strength` (0 to 1), and the edges tighten and pulse red as
// `params.low_health` (0 full health to 1 none) rises.
fn post(input: PostInput) -> vec4<f32> {
    let aspect = frame.resolution.x / max(frame.resolution.y, 1.0);
    let from_centre = length((input.uv - 0.5) * vec2<f32>(aspect, 1.0));
    let inner = mix(0.55, 0.3, params.low_health);
    let edge = smoothstep(inner, inner + 0.6, from_centre) * params.strength;
    let pulse = 0.75 + 0.25 * sin(frame.time * 5.0);
    let red = vec3<f32>(0.55, 0.02, 0.0) * edge * params.low_health * pulse;
    return vec4<f32>(input.color.rgb * (1.0 - edge) + red, input.color.a);
}
