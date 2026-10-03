//! Presentation-only de-emphasis of inactive pane text (`ui.inactive_pane_dim_percent`).
//!
//! Eligible cells sit on the terminal-default background (`Reset`), are not ANSI faint and
//! have a foreground the host terminal's replies let us resolve to RGB. Their foreground is
//! blended toward the host default background. Backgrounds, symbols and modifiers are never
//! written, and the retained pane surface is never mutated: callers transform composed copies.

use super::*;
use crate::protocol::CellData;
use crate::terminal_theme::RgbColor;
use ratatui::style::Color;

const RESET_FG: usize = 256;
const DIM_BIT: u16 = Modifier::DIM.bits();

pub(super) struct InactivePaneDim {
    percent: u16,
    background: RgbColor,
    /// Blended packed foreground per host palette index, plus the host default foreground at
    /// `RESET_FG`. `None` where the host did not report the color.
    resolved: [Option<u32>; 257],
}

impl InactivePaneDim {
    pub(super) fn new(
        percent: u8,
        background: Option<RgbColor>,
        foreground: Option<RgbColor>,
        palette: &[Option<RgbColor>; 256],
    ) -> Option<Self> {
        let background = background.filter(|_| percent > 0)?;
        let percent = u16::from(percent.min(100));
        let mut resolved = [None; 257];
        for (slot, color) in resolved.iter_mut().zip(palette.iter()) {
            *slot = color.map(|color| pack(blend(color, background, percent)));
        }
        resolved[RESET_FG] = foreground.map(|color| pack(blend(color, background, percent)));
        Some(Self {
            percent,
            background,
            resolved,
        })
    }

    /// The blended packed foreground for a packed wire foreground, or `None` when the
    /// displayed color cannot be resolved.
    fn foreground(&self, fg: u32) -> Option<u32> {
        match fg >> 24 {
            0x00 => match fg & 0xFF {
                0x00 => self.resolved[RESET_FG],
                // Named ANSI colors (wire codes 1..=16) are palette entries 0..=15.
                code @ 0x01..=0x10 => self.resolved[code as usize - 1],
                _ => None,
            },
            0x01 => self.resolved[(fg & 0xFF) as usize],
            0x02 => Some(pack(blend(
                RgbColor {
                    r: (fg >> 16) as u8,
                    g: (fg >> 8) as u8,
                    b: fg as u8,
                },
                self.background,
                self.percent,
            ))),
            _ => None,
        }
    }

    pub(super) fn apply_cell(&self, cell: &mut CellData) {
        if cell.bg != 0 || cell.modifier & DIM_BIT != 0 {
            return;
        }
        if let Some(fg) = self.foreground(cell.fg) {
            cell.fg = fg;
        }
    }

    pub(super) fn apply_rect(&self, frame: &mut FrameData, rect: Rect) {
        let rect = rect.intersection(Rect::new(0, 0, frame.width, frame.height));
        for y in rect.top()..rect.bottom() {
            let start = usize::from(y) * usize::from(frame.width);
            let row = start + usize::from(rect.left())..start + usize::from(rect.right());
            if let Some(cells) = frame.cells.get_mut(row) {
                cells.iter_mut().for_each(|cell| self.apply_cell(cell));
            }
        }
    }
}

fn blend(fg: RgbColor, bg: RgbColor, percent: u16) -> RgbColor {
    let channel = |fg: u8, bg: u8| {
        ((u16::from(fg) * (100 - percent) + u16::from(bg) * percent + 50) / 100) as u8
    };
    RgbColor {
        r: channel(fg.r, bg.r),
        g: channel(fg.g, bg.g),
        b: channel(fg.b, bg.b),
    }
}

fn pack(color: RgbColor) -> u32 {
    crate::protocol::color_to_u32(Color::Rgb(color.r, color.g, color.b))
}

impl ClientShellState {
    /// The active de-emphasis, or `None` when it is off, outside terminal mode, or the host
    /// default background is unknown.
    pub(super) fn inactive_pane_dim(&self) -> Option<InactivePaneDim> {
        if self.mode != ClientShellMode::Terminal {
            return None;
        }
        InactivePaneDim::new(
            self.config.inactive_pane_dim_percent,
            self.host_background,
            self.host_foreground,
            &self.host_palette,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rgb(r: u8, g: u8, b: u8) -> RgbColor {
        RgbColor { r, g, b }
    }

    fn cell(fg: Color, bg: Color, modifier: Modifier) -> CellData {
        CellData {
            symbol: "x".into(),
            fg: crate::protocol::color_to_u32(fg),
            bg: crate::protocol::color_to_u32(bg),
            modifier: modifier.bits(),
            skip: false,
            hyperlink: None,
        }
    }

    fn palette() -> Box<[Option<RgbColor>; 256]> {
        let mut palette = Box::new([None; 256]);
        palette[1] = Some(rgb(200, 0, 0));
        palette[9] = Some(rgb(255, 80, 80));
        palette[200] = Some(rgb(10, 20, 30));
        palette
    }

    #[test]
    fn blend_rounds_each_channel_to_nearest_and_covers_bounds() {
        let fg = rgb(255, 10, 101);
        let bg = rgb(0, 20, 0);
        assert_eq!(blend(fg, bg, 0), fg);
        assert_eq!(blend(fg, bg, 100), bg);
        // 255*0.5 = 127.5 -> 128; 10 + 10*0.5 = 15; 101*0.5 = 50.5 -> 51.
        assert_eq!(blend(fg, bg, 50), rgb(128, 15, 51));
        // 255*0.67 = 170.85 -> 171; 10*0.67 + 20*0.33 = 13.3 -> 13; 101*0.67 = 67.67 -> 68.
        assert_eq!(blend(fg, bg, 33), rgb(171, 13, 68));
        assert_eq!(blend(rgb(0, 0, 0), rgb(255, 255, 255), 1), rgb(3, 3, 3));
    }

    #[test]
    fn resolves_rgb_indexed_named_and_default_foregrounds() {
        let dim =
            InactivePaneDim::new(50, Some(rgb(0, 0, 0)), Some(rgb(200, 200, 200)), &palette())
                .unwrap();
        let packed = |r, g, b| crate::protocol::color_to_u32(Color::Rgb(r, g, b));
        for (fg, expected) in [
            (Color::Rgb(100, 50, 2), Some(packed(50, 25, 1))),
            (Color::Indexed(200), Some(packed(5, 10, 15))),
            (Color::Indexed(1), Some(packed(100, 0, 0))),
            (Color::Red, Some(packed(100, 0, 0))),
            (Color::LightRed, Some(packed(128, 40, 40))),
            (Color::Reset, Some(packed(100, 100, 100))),
            (Color::Indexed(2), None),
            (Color::Green, None),
        ] {
            assert_eq!(
                dim.foreground(crate::protocol::color_to_u32(fg)),
                expected,
                "{fg:?}"
            );
        }
    }

    #[test]
    fn apply_cell_changes_only_eligible_foregrounds() {
        let dim = InactivePaneDim::new(
            100,
            Some(rgb(10, 10, 10)),
            Some(rgb(250, 250, 250)),
            &palette(),
        )
        .unwrap();
        let mut plain = cell(
            Color::Reset,
            Color::Reset,
            Modifier::BOLD | Modifier::ITALIC,
        );
        let before = plain.clone();
        dim.apply_cell(&mut plain);
        assert_eq!(
            plain.fg,
            crate::protocol::color_to_u32(Color::Rgb(10, 10, 10))
        );
        assert_eq!(
            (plain.bg, plain.modifier, &plain.symbol),
            (before.bg, before.modifier, &before.symbol)
        );

        for untouched in [
            cell(Color::Reset, Color::Reset, Modifier::DIM),
            cell(Color::Reset, Color::Indexed(4), Modifier::empty()),
            cell(Color::Rgb(1, 2, 3), Color::Rgb(9, 9, 9), Modifier::empty()),
            cell(Color::Indexed(77), Color::Reset, Modifier::empty()),
        ] {
            let mut transformed = untouched.clone();
            dim.apply_cell(&mut transformed);
            assert_eq!(transformed, untouched);
        }
    }

    #[test]
    fn unknown_inputs_disable_or_skip() {
        let palette = palette();
        assert!(InactivePaneDim::new(0, Some(rgb(0, 0, 0)), None, &palette).is_none());
        assert!(InactivePaneDim::new(40, None, Some(rgb(9, 9, 9)), &palette).is_none());
        let dim = InactivePaneDim::new(40, Some(rgb(0, 0, 0)), None, &palette).unwrap();
        let mut default_fg = cell(Color::Reset, Color::Reset, Modifier::empty());
        let before = default_fg.clone();
        dim.apply_cell(&mut default_fg);
        assert_eq!(
            default_fg, before,
            "unknown host foreground leaves the cell"
        );
    }
}
