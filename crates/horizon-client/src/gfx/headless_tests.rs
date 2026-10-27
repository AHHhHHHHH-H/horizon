use super::{Frame, Renderer, lightning, panorama};
use crate::{environment::Sky, hud::HudBatch, weather};
use glam::{DVec3, Mat4};

/// A windowless renderer draws into its offscreen target and reads every frame back, oldest
/// first, as tightly packed rows. No game files: one white sprite and an empty world, so the
/// frame is the sky's clear colour.
#[test]
#[ignore = "requires a Vulkan/Metal/DX12 adapter"]
fn headless_frames_are_read_back_in_order() {
    let white = [255u8; 16 * 16 * 4];
    let panorama = panorama::Faces {
        size: 1,
        pixels: [0u8; 4].repeat(6),
    };
    let mut renderer = pollster::block_on(Renderer::new_headless(
        64,
        64,
        &white,
        16,
        16,
        1,
        &[],
        &[],
        (&white, 16, 16),
        (&[0, 0, 0, 0], 1),
        &panorama,
        None,
        Default::default(),
    ))
    .unwrap();
    renderer.capture_frames = true;
    let hud = HudBatch::new([64, 64]);
    let frame = |sky_color: [f32; 3]| Frame {
        camera: DVec3::new(0.5, 100.0, 0.5),
        yaw: 0.0,
        pitch: 0.0,
        fov_degrees: 70.0,
        bob: Mat4::IDENTITY,
        sky: Sky {
            sky_color,
            fog_color: sky_color,
            sky_light_factor: 1.0,
            sky_light_color: [1.0; 3],
            ..Default::default()
        },
        distance: 32.0,
        detail_distance: 32.0,
        depth_far: 64.0,
        underwater: false,
        cave_culling: false,
        title: false,
        skybox: false,
        end_sky: false,
        end_sky_sprite: None,
        environment_fog: None,
        water_fog: [0.0; 3],
        fog: None,
        dark_disc: false,
        outline: Vec::new(),
        outline_after_terrain: false,
        debug_lines: &[],
        debug_lines_on_top: &[],
        debug_crosshair: &[],
        hud: &hud,
        nether_lighting: false,
        entities: &[],
        entity_spans: &[],
        weather: &[],
        weather_time: weather::Time::new(0, 0.0),
        dynamic_lights: &[],
        clouds: None,
        lightning: lightning::Input {
            bolts: &[],
            uv: [0.0; 2],
        },
        additive: &[],
        see_through: &[],
        celestial: &[],
        stars: None,
        hand: &[],
        hand_fov_degrees: 70.0,
        screen: &[],
        pack: Default::default(),
        border: None,
        game_time: 0.0,
        shader_game_time: 0.0,
        end_portal_sprites: [[0.0; 4]; 2],
        crumbling_sprite: [0.0; 4],
        ticks: 0,
        darkness: (0.0, 0.0),
        night_vision: 0.0,
        block_flicker: 0.0,
        boss_darken: 0.0,
        menu_blur: None,
    };
    let (red, blue) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    renderer.render(&frame(red)).unwrap();
    renderer.render(&frame(blue)).unwrap();
    assert_eq!(renderer.pending_captures(), 2);
    // Red and blue sit at different bytes in either layout.
    let (r, b) = match renderer.capture_format() {
        wgpu::TextureFormat::Bgra8Unorm => (2, 0),
        wgpu::TextureFormat::Rgba8Unorm => (0, 2),
        format => panic!("unexpected capture format {format:?}"),
    };
    let mut pixels = Vec::new();
    for (name, (strong, weak)) in [("red", (r, b)), ("blue", (b, r))] {
        let size = renderer.read_capture(&mut pixels).unwrap();
        assert_eq!(size, Some([64, 64]));
        assert_eq!(pixels.len(), 64 * 64 * 4);
        let center = &pixels[(32 * 64 + 32) * 4..][..4];
        assert!(
            center[strong] > 200 && center[weak] < 50,
            "{name} frame's centre is {center:?}"
        );
    }
    assert_eq!(renderer.pending_captures(), 0);
    assert_eq!(renderer.read_capture(&mut pixels).unwrap(), None);
}
