//! The target details extra (`targetDetails`): a small readout of what the
//! crosshair is over, centered above the hotbar — the looked-at block's name,
//! or the looked-at mob's name and its health. The idea is ported, not a
//! mod's code; nothing here runs while the extra is off.

use crate::hud_layout::{GuiDraw, fill};
use crate::text::Text;

/// The readout's background, like the corner panels' (`extras_hud`).
pub const BACKGROUND: u32 = 0x7800_0000;
/// The name line's color; the second line's grays.
pub const NAME: u32 = 0xFFFF_FFFF;
pub const DETAIL: u32 = 0xFFAA_AAAA;
/// The line height in GUI pixels and the box's padding.
pub const LINE: i32 = 10;
pub const PAD: i32 = 2;

/// One readout for a frame: the lines' texts with their colors, and the
/// block or mob's position when it should show its coordinates.
pub struct Inputs {
    pub w: i32,
    pub h: i32,
    /// The HUD's height above the hotbar (`bar_height`): the box sits above it.
    pub bar_height: i32,
    pub lines: Vec<(String, u32)>,
    pub at: Option<[i32; 3]>,
}

/// The health line's color: green at full health toward red at none.
pub fn health_color(health: f32, max: f32) -> u32 {
    let f = if max <= 0.0 {
        0.0
    } else {
        (health / max).clamp(0.0, 1.0)
    };
    // lerp(a, b, f); red drains as green rises, blue (minor) follows.
    let lerp = |a: u32, b: u32| (a as f32 + (b as f32 - a as f32) * f).round() as u32;
    let (r, g) = (lerp(255, 85), lerp(0, 255));
    crate::hud_layout::argb(255, r, g, lerp(0, 85))
}

impl Inputs {
    /// The box's lines: the readout's own, then the coordinates line.
    fn lines(&self) -> Vec<(String, u32)> {
        let mut lines = self.lines.clone();
        if let Some([x, y, z]) = self.at {
            lines.push((format!("{x} {y} {z}"), DETAIL));
        }
        lines
    }
}

/// The readout's draws for a screen of `w` x `h` GUI pixels; `width` measures
/// the texts with the font (lines wider than the box's place are left as is).
pub fn layout(i: &Inputs, width: &dyn Fn(&str) -> i32) -> Vec<GuiDraw> {
    let lines = i.lines();
    if lines.is_empty() {
        return Vec::new();
    }
    let box_w = lines
        .iter()
        .map(|(text, _)| width(text) + 4)
        .max()
        .unwrap_or(0);
    let box_h = LINE * lines.len() as i32 + PAD;
    let x0 = i.w / 2 - box_w / 2;
    let y0 = i.h - i.bar_height - box_h - 4;
    let mut out = vec![fill(x0, y0, x0 + box_w, y0 + box_h, BACKGROUND)];
    for (n, (text, color)) in lines.iter().enumerate() {
        out.push(GuiDraw::Text {
            text: Text::literal(text),
            x: x0 + 2,
            y: y0 + PAD + n as i32 * LINE,
            color: *color,
            shadow: true,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(s: &str) -> i32 {
        s.len() as i32 * 5
    }

    #[test]
    fn the_box_centers_above_the_hotbar() {
        let i = Inputs {
            w: 400,
            h: 300,
            bar_height: 49,
            lines: vec![("Stone".into(), NAME)],
            at: None,
        };
        let draws = layout(&i, &width);
        // 5x9 text is 25 wide: box 29x12 at x (200 - 14), bottom at
        // 300 - 49 - 12 - 4 = 235.
        assert_eq!(draws.len(), 2);
        match &draws[0] {
            GuiDraw::Fill {
                x0,
                y0,
                x1,
                y1,
                color,
            } => {
                assert_eq!((*x0, *y0, *x1, *y1), (186, 235, 186 + 29, 235 + 12));
                assert_eq!(color, &BACKGROUND);
            }
            _ => panic!("the first draw is the box"),
        }
        match &draws[1] {
            GuiDraw::Text {
                text, x, y, color, ..
            } => {
                assert_eq!(text.plain(), "Stone");
                assert_eq!((*x, *y), (188, 237));
                assert_eq!(color, &NAME);
            }
            _ => panic!("the second draw is the name"),
        }
    }

    #[test]
    fn the_coordinates_line_sits_under_the_name() {
        let i = Inputs {
            w: 400,
            h: 300,
            bar_height: 49,
            lines: vec![("Stone".into(), NAME)],
            at: Some([1, 64, -3]),
        };
        let draws = layout(&i, &width);
        assert_eq!(draws.len(), 3);
        match &draws[2] {
            GuiDraw::Text { text, y, color, .. } => {
                assert_eq!(text.plain(), "1 64 -3");
                // The box grew a line: its top moved up, the second line at
                // y0 + PAD + LINE = 225 + 2 + 10.
                assert_eq!(*y, 237);
                assert_eq!(color, &DETAIL);
            }
            _ => panic!("the third draw is the coordinates"),
        }
    }

    #[test]
    fn nothing_draws_without_lines() {
        let i = Inputs {
            w: 400,
            h: 300,
            bar_height: 49,
            lines: Vec::new(),
            at: None,
        };
        assert!(layout(&i, &width).is_empty());
    }

    #[test]
    fn the_health_line_greens_and_reds_by_the_fraction() {
        assert_eq!(health_color(20.0, 20.0), 0xFF55FF55);
        assert_eq!(health_color(0.0, 20.0), 0xFFFF0000);
        assert_eq!(health_color(10.0, 20.0), 0xFFAA802B);
        assert_eq!(health_color(5.0, 0.0), 0xFFFF0000);
    }
}
