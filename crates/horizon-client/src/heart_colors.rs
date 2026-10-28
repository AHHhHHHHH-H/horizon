//! The colored hearts extra (`coloredHearts`): past ten hearts, health and
//! absorption tint the HUD's heart row one color per row of ten instead of
//! adding rows. The colors are `#RRGGBB` lists in `horizon-extras.txt`
//! (`heartColors`, `absorptionColors`). The tinted copies of the heart
//! sprites are generated when the atlas is packed: each sprite's distinct
//! opaque colors ranked by luminance (the heart's shade, body and highlight)
//! map to darker, plain and lighter copies of the target color, so any
//! recolor of the vanilla sprites keeps their shading. Nothing here runs
//! while the extra is off.

/// How many rows the atlas bakes a tinted sprite for. The configured colors
/// fill the first rows; past them the passes continue (see `expand`), so a
/// player with far more than ten hearts still sees the row change. Rows past
/// this many repeat the last baked color.
pub const MAX_ROWS: usize = 32;

/// How many colors each list may hold in `horizon-extras.txt`.
pub const COLORS: usize = MAX_ROWS;

/// The heart sprites the extra recolors, in the vanilla table's order:
/// `[full, full_blinking, half, half_blinking] x [normal, hardcore]`.
pub const VARIANT_SOURCES: [&str; 8] = [
    "gui/sprites/hud/heart/full",
    "gui/sprites/hud/heart/full_blinking",
    "gui/sprites/hud/heart/half",
    "gui/sprites/hud/heart/half_blinking",
    "gui/sprites/hud/heart/hardcore_full",
    "gui/sprites/hud/heart/hardcore_full_blinking",
    "gui/sprites/hud/heart/hardcore_half",
    "gui/sprites/hud/heart/hardcore_half_blinking",
];

/// The default health rows: the first is the heart sprite's own body color,
/// so one row of health looks vanilla.
pub const DEFAULT_HEALTH: [u32; 8] = [
    0xFF_1313, 0xFF_B832, 0x3E_E64C, 0x38_C8FF, 0xC8_6BFF, 0xFF_69B4, 0xFF_F135, 0xF0_F0F0,
];
/// The default absorption rows: the first is the absorbing heart's gold.
pub const DEFAULT_ABSORPTION: [u32; 8] = [
    0xFF_C832, 0xFF_E08A, 0xFF_FFFF, 0x8A_E0FF, 0xB4_69FF, 0x69_FF95, 0xF1_8A35, 0xA0_A0A0,
];

/// The colors the atlas bakes the tinted sprites with: `MAX_ROWS` of them,
/// the configured `#RRGGBB` list first and the passes of `expand` after. A
/// list is at least one color: a missing list or an empty one keeps the
/// defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub health: Vec<[u8; 3]>,
    pub absorption: Vec<[u8; 3]>,
}

impl Palette {
    pub fn new(health: &[u32], absorption: &[u32]) -> Self {
        Self {
            health: colors(health, &DEFAULT_HEALTH),
            absorption: colors(absorption, &DEFAULT_ABSORPTION),
        }
    }
}

fn colors(given: &[u32], default: &[u32]) -> Vec<[u8; 3]> {
    let parse = |c: &u32| [(c >> 16) as u8, (c >> 8) as u8, (c & 0xFF) as u8];
    let taken: Vec<[u8; 3]> = given.iter().take(COLORS).map(parse).collect();
    let base: Vec<[u8; 3]> = if taken.is_empty() {
        default.iter().map(parse).collect()
    } else {
        taken
    };
    expand(&base)
}

/// `MAX_ROWS` colors out of a shorter list: the list repeats in order, and
/// each full pass through it shifts one step lighter (odd passes) or darker
/// (even passes), a step further each time — 12% of the way to white or
/// black. Deterministic, so a player with hundreds of hearts climbs through
/// the same colors every time.
fn expand(base: &[[u8; 3]]) -> Vec<[u8; 3]> {
    let step = 12u32;
    let mut out = Vec::with_capacity(MAX_ROWS);
    for row in 0..MAX_ROWS {
        let pass = (row / base.len()) as u32;
        let c = base[row % base.len()];
        out.push(if pass == 0 {
            c
        } else {
            let (amount, lighter) = (step * pass.div_ceil(2), pass % 2 == 1);
            c.map(|v| {
                if lighter {
                    (u32::from(v) + (255 - u32::from(v)) * amount / 100).min(255) as u8
                } else {
                    (u32::from(v) * (100 - amount.min(100)) / 100) as u8
                }
            })
        });
    }
    out
}

/// The atlas slot of one tinted sprite, as the atlas knows it and as a
/// `hud_layout` draw names it (the paint adds the `gui/sprites/` prefix).
pub fn slot_atlas_name(absorption: bool, variant: usize, color: usize) -> String {
    format!(
        "gui/sprites/horizon/{}heart{}x{}",
        if absorption { "a" } else { "" },
        variant,
        color
    )
}
pub fn slot_id(absorption: bool, variant: usize, color: usize) -> String {
    format!(
        "horizon/{}heart{}x{}",
        if absorption { "a" } else { "" },
        variant,
        color
    )
}

/// Parses a `heartColors` value: `#RRGGBB` values separated by commas.
/// Entries that aren't a color are dropped; nothing parses to no colors.
pub fn parse_colors(value: &str) -> Vec<u32> {
    value
        .split(',')
        .filter_map(|entry| {
            let hex = entry.trim().trim_start_matches('#');
            if hex.len() != 6 {
                return None;
            }
            u32::from_str_radix(hex, 16).ok().map(|c| c & 0xFF_FFFF)
        })
        .take(COLORS)
        .collect()
}

/// The settings file's form of a color list.
pub fn colors_to_text(colors: &[u32]) -> String {
    colors
        .iter()
        .map(|c| format!("#{c:06X}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A sprite's pixels recolored to `target`: the sprite's distinct opaque
/// colors sorted by luminance spread evenly from a darker to a lighter copy
/// of the target, so a three-tone heart keeps its shade, body and highlight.
pub fn recolored(pixels: &[u8], target: [u8; 3]) -> Vec<u8> {
    let mut out = pixels.to_vec();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    for px in out.as_chunks::<4>().0 {
        if px[3] == 0 {
            continue;
        }
        let c = [px[0], px[1], px[2]];
        if !colors.contains(&c) {
            colors.push(c);
        }
    }
    colors.sort_by_key(|c| u16::from(c[0]) + u16::from(c[1]) + u16::from(c[2]));
    if colors.len() < 2 {
        return out;
    }
    // The tone each rank maps to, in 1/256 steps from shade (0) through the
    // target (mid) to a lighter copy of it: the vanilla heart's highlight
    // sits 78% of the way to white (FFC8C8 of FF1313) and its shade at 73%
    // of the body (BB1313).
    let mix = |a: [u8; 3], b: [u8; 3], t: u32| -> [u8; 3] {
        std::array::from_fn(|i| {
            ((u16::from(a[i]) * (256 - t) as u16 + u16::from(b[i]) * t as u16) / 256) as u8
        })
    };
    let steps = (colors.len() - 1) as u32;
    let tone = |rank: u32| -> [u8; 3] {
        // Twice the steps across shade -> target -> light.
        let pos = rank * 512 / steps;
        if pos <= 256 {
            mix(shade(target), target, pos)
        } else {
            mix(target, light(target), pos - 256)
        }
    };
    for px in out.as_chunks_mut::<4>().0 {
        if px[3] == 0 {
            continue;
        }
        let c = [px[0], px[1], px[2]];
        if let Some(rank) = colors.iter().position(|&x| x == c) {
            let t = tone(rank as u32);
            px[0] = t[0];
            px[1] = t[1];
            px[2] = t[2];
        }
    }
    out
}

fn shade(t: [u8; 3]) -> [u8; 3] {
    t.map(|v| (u16::from(v) * 187 / 255) as u8)
}
fn light(t: [u8; 3]) -> [u8; 3] {
    t.map(|v| (u16::from(v) + u16::from(255u8.wrapping_sub(v)) * 200 / 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vanilla sprites' pixels (the pinned 26.3 atlas sources).
    const FULL: [u8; 4] = [0xFF, 0x13, 0x13, 0xFF];
    const FULL_SHADE: [u8; 4] = [0xBB, 0x13, 0x13, 0xFF];
    const FULL_HIGHLIGHT: [u8; 4] = [0xFF, 0xC8, 0xC8, 0xFF];
    const BLINK_BODY: [u8; 4] = [0xFF, 0xA1, 0xA1, 0xFF];
    const BLINK_SHADE: [u8; 4] = [0xDE, 0xA1, 0xA1, 0xFF];
    const BLINK_HIGHLIGHT: [u8; 4] = [0xFF, 0xE3, 0xE3, 0xFF];

    fn sprite(body: [u8; 4], shade: [u8; 4], highlight: [u8; 4]) -> Vec<u8> {
        [body, shade, highlight].concat()
    }

    #[test]
    fn the_body_takes_the_color_and_the_tones_keep_their_order() {
        let out = recolored(
            &sprite(FULL, FULL_SHADE, FULL_HIGHLIGHT),
            [0x38, 0xC8, 0xFF],
        );
        // The body is the color itself; the shade is darker, the highlight
        // lighter, and their order is kept.
        assert_eq!(&out[0..4], &[0x38, 0xC8, 0xFF, 0xFF]);
        assert_eq!(&out[4..8], &[0x29, 0x92, 0xBB, 0xFF]);
        assert_eq!(&out[8..12], &[0xD4, 0xF3, 0xFF, 0xFF]);
    }

    #[test]
    fn the_blinking_sprites_recolor_by_their_own_ranking() {
        let out = recolored(
            &sprite(BLINK_BODY, BLINK_SHADE, BLINK_HIGHLIGHT),
            [0x38, 0xC8, 0xFF],
        );
        assert_eq!(&out[0..4], &[0x38, 0xC8, 0xFF, 0xFF]);
        assert_eq!(&out[4..8], &[0x29, 0x92, 0xBB, 0xFF]);
        assert_eq!(&out[8..12], &[0xD4, 0xF3, 0xFF, 0xFF]);
    }

    #[test]
    fn a_vanilla_red_target_stays_vanilla_red() {
        // The body is exact; the shade and highlight land within a few steps
        // of the sprite's own tones (their saturated channels can't multiply
        // lower).
        let out = recolored(
            &sprite(FULL, FULL_SHADE, FULL_HIGHLIGHT),
            [0xFF, 0x13, 0x13],
        );
        assert_eq!(&out[0..4], &FULL);
        let close = |a: [u8; 4], b: [u8; 4]| {
            assert!(
                a.iter().zip(b.iter()).all(|(x, y)| x.abs_diff(*y) <= 6),
                "{a:?} vs {b:?}"
            );
        };
        close(out[4..8].try_into().unwrap(), FULL_SHADE);
        close(out[8..12].try_into().unwrap(), FULL_HIGHLIGHT);
    }

    #[test]
    fn transparent_pixels_and_flat_sprites_are_left_alone() {
        let mut px = vec![0, 0, 0, 0];
        px.extend_from_slice(&[40, 40, 40, 255]);
        assert_eq!(recolored(&px, [1, 2, 3]), px);
    }

    #[test]
    fn colors_parse_and_round_trip() {
        assert_eq!(parse_colors("#FF1313, #FFB832"), vec![0xFF_1313, 0xFF_B832]);
        assert_eq!(parse_colors("heartColors"), Vec::<u32>::new());
        assert_eq!(colors_to_text(&[0xFF_1313, 0xB]), "#FF1313,#00000B");
        // The file's lists stop at the baked rows' worth of colors.
        assert_eq!(parse_colors(&"#010203,".repeat(40)).len(), MAX_ROWS);
    }

    #[test]
    fn palettes_fall_back_to_their_defaults_and_expand() {
        let p = Palette::new(&[], &[]);
        assert_eq!(p.health[0], [0xFF, 0x13, 0x13]);
        assert_eq!(p.absorption[0], [0xFF, 0xC8, 0x32]);
        assert_eq!(p.health.len(), MAX_ROWS);
        // One color fills every row; the passes alternate lighter and darker
        // by growing steps, deterministically.
        let p = Palette::new(&[0x80_00_00], &[]);
        assert_eq!(p.health[0], [0x80, 0x00, 0x00], "the list itself first");
        assert!(
            p.health[1][0] > 0x80,
            "the first pass is lighter: {:?}",
            p.health[1]
        );
        assert_eq!(p.health[2], [0x70, 0x00, 0x00], "the second darker by 12%");
        let (first, third) = (p.health[1], p.health[3]);
        assert!(
            third[0] > first[0],
            "the next light pass steps further: {third:?}"
        );
        let p2 = Palette::new(&[0x80_00_00], &[]);
        assert_eq!(p.health, p2.health, "deterministic");
    }

    #[test]
    fn slots_carry_their_kind_variant_and_color() {
        assert_eq!(slot_id(false, 3, 2), "horizon/heart3x2");
        assert_eq!(slot_id(true, 0, 7), "horizon/aheart0x7");
        assert_eq!(slot_atlas_name(true, 0, 7), "gui/sprites/horizon/aheart0x7");
    }
}
