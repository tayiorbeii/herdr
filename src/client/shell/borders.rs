use super::*;

/// Pane surfaces remain byte-for-byte server data. Restyle only their genuine
/// border corners in the client's composed frame, at most four cells per pane.
pub(super) fn round_pane_corners(
    frame: &mut FrameData,
    panes: &[crate::protocol::PaneSurfacePane],
    area: Rect,
) {
    for pane in panes {
        let rect = pane.rect;
        let inner = pane.inner_rect;
        if rect.width < 2 || rect.height < 2 {
            continue;
        }
        let right = rect.x.saturating_add(rect.width).saturating_sub(1);
        let bottom = rect.y.saturating_add(rect.height).saturating_sub(1);
        let left_edge = inner.x > rect.x;
        let top_edge = inner.y > rect.y;
        let right_edge = inner.x.saturating_add(inner.width) <= right;
        let bottom_edge = inner.y.saturating_add(inner.height) <= bottom;
        for (x, y, present, expected) in [
            (rect.x, rect.y, left_edge && top_edge, "┌"),
            (right, rect.y, right_edge && top_edge, "┐"),
            (rect.x, bottom, left_edge && bottom_edge, "└"),
            (right, bottom, right_edge && bottom_edge, "┘"),
        ] {
            if !present || x >= area.width || y >= area.height {
                continue;
            }
            let Some(x) = area.x.checked_add(x) else {
                continue;
            };
            let Some(y) = area.y.checked_add(y) else {
                continue;
            };
            if x >= frame.width || y >= frame.height {
                continue;
            }
            let index = usize::from(y) * usize::from(frame.width) + usize::from(x);
            let Some(cell) = frame.cells.get_mut(index) else {
                continue;
            };
            if cell.symbol == expected {
                if let Some(symbol) = crate::ui::rounded_light_corner(expected) {
                    cell.symbol.clear();
                    cell.symbol.push_str(symbol);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{PaneSurfacePane, SurfaceRect};
    use ratatui::widgets::{Block, Borders, Widget};

    fn pane(rect: Rect, inner: Rect) -> PaneSurfacePane {
        PaneSurfacePane {
            pane_id: "pane".into(),
            content_revision: 1,
            rect: SurfaceRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            },
            inner_rect: SurfaceRect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: inner.height,
            },
            scrollbar_rect: None,
            scroll: None,
            focused: true,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    #[test]
    fn rounded_borders_only_touch_real_pane_corners_and_preserve_effects() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 10, 6));
        let rect = Rect::new(2, 1, 6, 4);
        let block = Block::default().borders(Borders::ALL);
        let inner = block.inner(rect);
        block.render(rect, &mut buffer);
        buffer[(inner.x, inner.y)].set_symbol("┌"); // terminal-content negative control
        let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
        frame.cells[12].modifier = 0x1234;
        frame.cells[12].hyperlink = Some(0);
        frame.hyperlinks.push("https://example.test".into());
        let before = frame.clone();
        round_pane_corners(&mut frame, &[pane(rect, inner)], buffer.area);
        assert_eq!(frame.cells[12].symbol, "╭");
        assert_eq!(frame.cells[17].symbol, "╮");
        assert_eq!(frame.cells[42].symbol, "╰");
        assert_eq!(frame.cells[47].symbol, "╯");
        for (i, cell) in frame.cells.iter().enumerate() {
            let mut expected = before.cells[i].clone();
            if [12, 17, 42, 47].contains(&i) {
                expected.symbol = crate::ui::rounded_light_corner(&expected.symbol)
                    .expect("corner")
                    .into();
            }
            assert_eq!(cell, &expected);
        }
        assert_eq!(frame.hyperlinks, before.hyperlinks);
        assert_eq!(frame.cursor, before.cursor);
        assert_eq!(frame.graphics, before.graphics);
    }

    #[test]
    fn rounded_borders_preserve_unframed_tiny_junction_and_heavy_controls() {
        for symbol in ["┌", "┼", "┬", "╔", "┏", "╭"] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 4));
            buffer[(0, 0)].set_symbol(symbol);
            let mut frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
            let before = frame.clone();
            let unframed = pane(buffer.area, buffer.area);
            round_pane_corners(&mut frame, &[unframed], buffer.area);
            assert_eq!(frame, before);
            let tiny = pane(Rect::new(0, 0, 1, 1), Rect::new(1, 1, 0, 0));
            round_pane_corners(&mut frame, &[tiny], buffer.area);
            assert_eq!(frame, before);
            if symbol != "┌" {
                let framed = pane(buffer.area, Rect::new(1, 1, 2, 2));
                round_pane_corners(&mut frame, &[framed], buffer.area);
                assert_eq!(frame, before);
            }
        }
    }
}
