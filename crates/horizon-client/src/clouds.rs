//! Cloud visibility and flat/fancy cell geometry (`CloudRenderer`).
use crate::mesher::Vertex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Face shades (UP, WEST/EAST, NORTH/SOUTH, DOWN). A cloud vertex's `pos` is
/// its cell's low corner; `light` holds the shade index (byte 0, the colour is
/// `Globals.cloud_colors[i]`) and whether the vertex is on the far side of the
/// cell in x, y and z (bytes 1..4). The shader adds the cell's 12x4x12 size
/// after the offset, so positions round exactly as when built in place.
pub const SHADES: [f32; 4] = [1.0, 0.9, 0.8, 0.7];

/// `CloudRenderer.RelativeCameraPos`: the camera against the cloud layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativePos {
    Above,
    Inside,
    Below,
}

impl RelativePos {
    /// `bottom`: the cloud bottom relative to the camera (`relativeBottomY`).
    pub fn new(bottom: f32) -> Self {
        if bottom + 4.0 < 0.0 {
            Self::Above
        } else if bottom > 0.0 {
            Self::Below
        } else {
            Self::Inside
        }
    }
}

/// Everything the cell mesh depends on: the camera's cell, its position
/// against the layer, the cloud type and the radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MeshKey {
    cell: (i64, i64),
    pos: RelativePos,
    fancy: bool,
    radius: i32,
}

/// Mesh generations are unique across reloads, so a renderer never keeps a
/// stale upload.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// clouds.png as filled cells and the cell mesh `CloudRenderer` keeps.
pub struct Clouds {
    width: i64,
    height: i64,
    cells: Vec<bool>,
    key: Option<MeshKey>,
    /// Quads relative to the camera's cell corner (x, z) and the cloud bottom
    /// (y), nearest first, encoded as `SHADES` says; moved into place by
    /// `Prepared::offset`.
    pub mesh: Vec<Vertex>,
    /// Changes whenever `mesh` is rebuilt.
    pub generation: u64,
}

/// One frame's clouds: the cached mesh's generation and where it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prepared {
    pub generation: u64,
    /// `CloudInfo`'s offset: (-xInCell, relativeBottomY, -zInCell).
    pub offset: [f32; 3],
}

impl Clouds {
    /// `CloudRenderer.prepare` (resources): cells with alpha 10 or more are filled.
    pub fn load(root: &std::path::Path) -> Self {
        let (width, height, cells) =
            crate::hud::decode(&root.join("assets/minecraft/textures/environment/clouds.png"))
                .map(|(w, h, px)| {
                    (
                        w,
                        h,
                        px.as_chunks::<4>().0.iter().map(|p| p[3] >= 10).collect(),
                    )
                })
                .unwrap_or((1, 1, vec![false]));
        Self::new(width, height, cells)
    }

    pub fn new(width: u32, height: u32, cells: Vec<bool>) -> Self {
        Self {
            width: i64::from(width),
            height: i64::from(height),
            cells,
            key: None,
            mesh: Vec::new(),
            generation: 0,
        }
    }

    fn filled(&self, cx: i64, cz: i64) -> bool {
        self.cells[(cz.rem_euclid(self.height) * self.width + cx.rem_euclid(self.width)) as usize]
    }

    /// `CloudRenderer.prepare`: the cell under the camera and the offset into
    /// it; the mesh is rebuilt only when the cell, the camera's position
    /// against the layer, the cloud type or the radius changes.
    pub fn prepare(
        &mut self,
        camera: glam::DVec3,
        game_time: i64,
        partial_tick: f64,
        fancy: bool,
        radius: i32,
        uv: [f32; 2],
    ) -> Prepared {
        let cloud_height = 192.33;
        let offset = (game_time.rem_euclid(self.width * 400)) as f64 + partial_tick;
        let mut cloud_x = camera.x + offset * 0.030_000_001;
        let mut cloud_z = camera.z + 3.96;
        let (tw, th) = (self.width as f64 * 12.0, self.height as f64 * 12.0);
        cloud_x -= (cloud_x / tw).floor() * tw;
        cloud_z -= (cloud_z / th).floor() * th;
        let cell_x = (cloud_x / 12.0).floor() as i64;
        let cell_z = (cloud_z / 12.0).floor() as i64;
        let x_in = (cloud_x - cell_x as f64 * 12.0) as f32;
        let z_in = (cloud_z - cell_z as f64 * 12.0) as f32;
        let bottom = (cloud_height - camera.y) as f32;
        let key = MeshKey {
            cell: (cell_x, cell_z),
            pos: RelativePos::new(bottom),
            fancy,
            radius,
        };
        if self.key != Some(key) {
            self.key = Some(key);
            self.build(key, bottom, uv);
        }
        Prepared {
            generation: self.generation,
            offset: [-x_in, bottom, -z_in],
        }
    }

    fn build(&mut self, key: MeshKey, bottom: f32, uv: [f32; 2]) {
        let mut mesh = std::mem::take(&mut self.mesh);
        mesh.clear();
        let (cell_x, cell_z) = key.cell;
        for (dx, dz) in cells(key.radius) {
            let (cx, cz) = (cell_x + i64::from(dx), cell_z + i64::from(dz));
            if !self.filled(cx, cz) {
                continue;
            }
            let x0 = dx as f32 * 12.0;
            let z0 = dz as f32 * 12.0;
            let (x1, z1, y0, y1) = (x0 + 12.0, z0 + 12.0, 0.0, 4.0);
            let mut quad = |p: [[f32; 3]; 4], shade: u32| {
                for v in p {
                    let far = |axis: usize, near: f32| u32::from(v[axis] != near) << (8 + 8 * axis);
                    // Overlay 4: clouds (unlit, faded by the cloud fog end).
                    mesh.push(Vertex {
                        normal: [0; 4],
                        pos: [x0, y0, z0],
                        uv,
                        layer: 4,
                        color: u32::MAX,
                        light: shade | far(0, x0) | far(1, y0) | far(2, z0),
                    });
                }
            };
            if !key.fancy {
                for p in flat_cell(x0, z0, y0) {
                    quad(p, 0);
                }
                continue;
            }
            let sides = [
                ([[x1, y0, z0], [x1, y0, z1], [x0, y0, z1], [x0, y0, z0]], 3),
                ([[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]], 0),
                ([[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]], 2),
                ([[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]], 2),
                ([[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]], 1),
                ([[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]], 1),
            ];
            let mask = face_mask(
                dx,
                dz,
                bottom,
                [
                    !self.filled(cx, cz - 1),
                    !self.filled(cx, cz + 1),
                    !self.filled(cx - 1, cz),
                    !self.filled(cx + 1, cz),
                ],
            );
            for (direction, inside) in faces(mask) {
                let (p, shade) = sides[usize::from(direction)];
                quad(if inside { [p[3], p[2], p[1], p[0]] } else { p }, shade);
            }
        }
        self.mesh = mesh;
        self.generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    }
}

/// The cloud colour times each of `SHADES`, packed RGBA8 like vertex colours.
pub fn shaded_colors(color: [f32; 4]) -> [u32; 4] {
    SHADES.map(|shade| {
        let ch = |v: f32| ((v * shade).clamp(0.0, 1.0) * 255.0) as u32;
        ch(color[3].clamp(0.0, 1.0)) << 24 | ch(color[2]) << 16 | ch(color[1]) << 8 | ch(color[0])
    })
}
/// `LevelRenderer`: skip OFF clouds and colors with no packed alpha.
pub fn visible(mode: &str, alpha: f32) -> bool {
    mode != "false" && (alpha.clamp(0.0, 1.0) * 255.0) as u8 > 0
}

/// `buildFlatCell`: bottom plane, in both windings for the culled cloud pipeline.
pub fn flat_cell(x: f32, z: f32, bottom: f32) -> [[[f32; 3]; 4]; 2] {
    let p = [
        [x + 12.0, bottom, z],
        [x + 12.0, bottom, z + 12.0],
        [x, bottom, z + 12.0],
        [x, bottom, z],
    ];
    [p, [p[3], p[2], p[1], p[0]]]
}

/// Direction order is DOWN, UP, NORTH, SOUTH, WEST, EAST; inward faces use bits 6..12.
pub fn face_mask(x: i32, z: i32, bottom: f32, empty: [bool; 4]) -> u16 {
    let mut mask = 0;
    if bottom <= 0.0 {
        mask |= 1 << 1;
    }
    if bottom + 4.0 >= 0.0 {
        mask |= 1;
    }
    for (i, visible) in [z > 0, z < 0, x > 0, x < 0].into_iter().enumerate() {
        if empty[i] && visible {
            mask |= 1 << (i + 2);
        }
    }
    if x.abs() <= 1 && z.abs() <= 1 {
        mask |= 63 << 6;
    }
    mask
}

/// Exterior faces first, then reversed interior faces, in vanilla emission order.
pub fn faces(mask: u16) -> impl Iterator<Item = (u8, bool)> {
    [1, 0, 2, 3, 4, 5]
        .into_iter()
        .filter(move |d| mask & (1 << d) != 0)
        .map(|d| (d, false))
        .chain(
            (0..6)
                .filter(move |d| mask & (1 << (d + 6)) != 0)
                .map(|d| (d, true)),
        )
}

/// `CloudRenderer.prepare`: the cell radius for the Cloud Distance option
/// (in chunks), independent of the render distance.
pub fn radius_cells(cloud_range_chunks: i32) -> i32 {
    ((cloud_range_chunks * 16) as f32 / 12.0).ceil() as i32
}

/// CloudRenderer builds cells in expanding Manhattan rings, nearest first.
pub fn cells(radius: i32) -> impl Iterator<Item = (i32, i32)> {
    (0..=2 * radius).flat_map(move |ring| {
        (-ring..=ring).flat_map(move |x| {
            let z = ring - x.abs();
            let visible = z <= radius && x * x + z * z <= radius * radius;
            (visible && z != 0)
                .then_some((x, -z))
                .into_iter()
                .chain(visible.then_some((x, z)))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_and_packed_alpha_gate_clouds() {
        for mode in ["false", "fast", "true"] {
            assert_eq!(visible(mode, 1.0), mode != "false");
            assert!(!visible(mode, 0.0));
            assert!(!visible(mode, 0.5 / 255.0));
        }
        assert!(visible("fast", 1.0 / 255.0));
    }

    #[test]
    fn fast_cells_are_flat_at_the_bottom_and_visible_from_both_sides() {
        for bottom in [-10.0, 0.0, 10.0] {
            let [down, up] = flat_cell(-3.0, 7.0, bottom);
            assert_eq!(
                down,
                [
                    [9.0, bottom, 7.0],
                    [9.0, bottom, 19.0],
                    [-3.0, bottom, 19.0],
                    [-3.0, bottom, 7.0],
                ]
            );
            assert_eq!(up, [down[3], down[2], down[1], down[0]]);
            for (quad, sign) in [(down, -1.0), (up, 1.0)] {
                let [a, b, c] = [quad[0], quad[1], quad[2]].map(glam::Vec3::from_array);
                assert!((b - a).cross(c - a).y * sign > 0.0);
            }
        }
    }

    #[test]
    fn cells_are_nearest_first_and_unique() {
        let cells: Vec<_> = cells(2).collect();
        assert_eq!(cells[0], (0, 0));
        assert_eq!(cells.len(), 13);
        assert!(
            cells
                .windows(2)
                .all(|w| w[0].0.abs() + w[0].1.abs() <= w[1].0.abs() + w[1].1.abs())
        );
        assert_eq!(
            cells.iter().collect::<std::collections::HashSet<_>>().len(),
            cells.len()
        );
    }
    #[test]
    fn only_camera_facing_exterior_faces() {
        assert_eq!(face_mask(2, 2, 10.0, [true; 4]), 1 | 4 | 16);
        assert_eq!(face_mask(-2, -2, -5.0, [true; 4]), 2 | 8 | 32);
        assert_eq!(face_mask(2, 2, 0.0, [false; 4]), 3);
    }
    #[test]
    fn cloud_distance_sets_the_radius() {
        assert_eq!(radius_cells(2), 3);
        assert_eq!(radius_cells(12), 16);
        assert_eq!(radius_cells(128), 171);
    }
    #[test]
    fn connected_cells_have_no_exterior_walls() {
        assert_eq!(face_mask(2, 2, 10.0, [false; 4]), 1);
    }
    #[test]
    fn nearby_cells_keep_inward_faces_for_camera_inside_clouds() {
        assert_eq!(face_mask(0, 0, 0.0, [false; 4]), 3 | (63 << 6));
    }

    fn sky() -> Clouds {
        // A 16x16 sky with a few clouds of various shapes.
        let cells = (0..256).map(|i| (i * 7 + i / 16 * 3) % 5 < 2).collect();
        Clouds::new(16, 16, cells)
    }

    /// The per-frame builder this cache replaced, kept as the oracle:
    /// camera-relative positions and face shades.
    fn built_in_place(
        clouds: &Clouds,
        camera: glam::DVec3,
        time: f64,
        fancy: bool,
        radius: i32,
    ) -> Vec<([f32; 3], f32)> {
        let (w, h) = (clouds.width, clouds.height);
        let offset = (time.floor() as i64).rem_euclid(w * 400) as f64 + time.fract();
        let mut cloud_x = camera.x + offset * 0.030_000_001;
        let mut cloud_z = camera.z + 3.96;
        let (tw, th) = (w as f64 * 12.0, h as f64 * 12.0);
        cloud_x -= (cloud_x / tw).floor() * tw;
        cloud_z -= (cloud_z / th).floor() * th;
        let cell_x = (cloud_x / 12.0).floor() as i64;
        let cell_z = (cloud_z / 12.0).floor() as i64;
        let x_in = (cloud_x - cell_x as f64 * 12.0) as f32;
        let z_in = (cloud_z - cell_z as f64 * 12.0) as f32;
        let bottom = (192.33 - camera.y) as f32;
        let mut out = Vec::new();
        for (dx, dz) in cells(radius) {
            let (cx, cz) = (cell_x + i64::from(dx), cell_z + i64::from(dz));
            if !clouds.filled(cx, cz) {
                continue;
            }
            let x0 = dx as f32 * 12.0 - x_in;
            let z0 = dz as f32 * 12.0 - z_in;
            let (x1, z1, y0, y1) = (x0 + 12.0, z0 + 12.0, bottom, bottom + 4.0);
            let mut quad = |p: [[f32; 3]; 4], shade: f32| out.extend(p.map(|v| (v, shade)));
            if !fancy {
                for p in flat_cell(x0, z0, bottom) {
                    quad(p, 1.0);
                }
                continue;
            }
            let sides = [
                (
                    [[x1, y0, z0], [x1, y0, z1], [x0, y0, z1], [x0, y0, z0]],
                    0.7,
                ),
                (
                    [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
                    1.0,
                ),
                (
                    [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
                    0.8,
                ),
                (
                    [[x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1]],
                    0.8,
                ),
                (
                    [[x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0]],
                    0.9,
                ),
                (
                    [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
                    0.9,
                ),
            ];
            let empty = [
                !clouds.filled(cx, cz - 1),
                !clouds.filled(cx, cz + 1),
                !clouds.filled(cx - 1, cz),
                !clouds.filled(cx + 1, cz),
            ];
            for (direction, inside) in faces(face_mask(dx, dz, bottom, empty)) {
                let (p, shade) = sides[usize::from(direction)];
                quad(if inside { [p[3], p[2], p[1], p[0]] } else { p }, shade);
            }
        }
        out
    }

    #[test]
    fn cached_mesh_moved_into_place_matches_building_in_place() {
        let mut clouds = sky();
        for (x, y, z) in [
            (0.3, 150.0, 0.7),
            (0.3, 194.2, 0.7),
            (-1234.56, 240.0, 987.65),
            (23.99, 192.33, 35.99),
            (11.9, 196.33, -0.1),
        ] {
            for time in [0.0_f64, 51_234.5, 6_399.999, 1e7 + 0.25] {
                for fancy in [true, false] {
                    let camera = glam::DVec3::new(x, y, z);
                    let prepared = clouds.prepare(
                        camera,
                        time.floor() as i64,
                        time.fract(),
                        fancy,
                        6,
                        [0.5; 2],
                    );
                    // entity.wgsl: (corner + offset) + the cell size on the far sides.
                    let [ox, oy, oz] = prepared.offset;
                    let moved: Vec<_> = clouds
                        .mesh
                        .iter()
                        .map(|v| {
                            let far = |axis: usize, size: f32| {
                                ((v.light >> (8 + 8 * axis)) & 255) as f32 * size
                            };
                            let p = [
                                v.pos[0] + ox + far(0, 12.0),
                                v.pos[1] + oy + far(1, 4.0),
                                v.pos[2] + oz + far(2, 12.0),
                            ];
                            (p, SHADES[(v.light & 255) as usize])
                        })
                        .collect();
                    let expected = built_in_place(&clouds, camera, time, fancy, 6);
                    assert!(!expected.is_empty());
                    let bits = |m: &[([f32; 3], f32)]| -> Vec<_> {
                        m.iter()
                            .map(|(p, s)| (p.map(f32::to_bits), s.to_bits()))
                            .collect()
                    };
                    assert_eq!(
                        bits(&moved),
                        bits(&expected),
                        "camera {camera} time {time} fancy {fancy}"
                    );
                }
            }
        }
    }

    #[test]
    fn mesh_is_rebuilt_only_when_vanilla_rebuilds_it() {
        let mut clouds = sky();
        let prepare = |c: &mut Clouds, x: f64, y: f64, time: i64, fancy: bool, radius: i32| {
            c.prepare(
                glam::DVec3::new(x, y, 0.5),
                time,
                0.0,
                fancy,
                radius,
                [0.5; 2],
            )
            .generation
        };
        let first = prepare(&mut clouds, 0.5, 100.0, 0, true, 6);
        // Moving and drifting inside the cell: translated, not rebuilt.
        assert_eq!(prepare(&mut clouds, 4.0, 120.0, 20, true, 6), first);
        // Another cell, either by moving or by drifting with time.
        let moved = prepare(&mut clouds, 20.0, 120.0, 20, true, 6);
        assert_ne!(moved, first);
        let drifted = prepare(&mut clouds, 20.0, 120.0, 420, true, 6);
        assert_ne!(drifted, moved);
        // Into the layer, above it, a cloud type and a Cloud Distance change.
        let mut last = drifted;
        for (y, fancy, radius) in [
            (194.0, true, 6),
            (200.0, true, 6),
            (200.0, false, 6),
            (200.0, false, 8),
        ] {
            let generation = prepare(&mut clouds, 20.0, y, 420, fancy, radius);
            assert_ne!(generation, last);
            last = generation;
        }
        assert_eq!(prepare(&mut clouds, 20.0, 300.0, 420, false, 8), last);
    }

    #[test]
    fn shaded_colors_pack_like_vertex_colors() {
        assert_eq!(
            shaded_colors([1.0; 4]),
            [u32::MAX, 0xE5E5_E5E5, 0xCCCC_CCCC, 0xB2B2_B2B2]
        );
        assert_eq!(shaded_colors([0.5, 0.25, 2.0, 0.8])[0], 0xCCFF_3F7F);
    }
}
