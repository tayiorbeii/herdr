use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Color,
};

use crate::app::state::Palette;

pub(crate) fn interface_border_type(rounded: bool) -> ratatui::widgets::BorderType {
    if rounded {
        ratatui::widgets::BorderType::Rounded
    } else {
        ratatui::widgets::BorderType::Plain
    }
}

pub(crate) fn rounded_light_corner(symbol: &str) -> Option<&'static str> {
    match symbol {
        "┌" => Some("╭"),
        "┐" => Some("╮"),
        "└" => Some("╰"),
        "┘" => Some("╯"),
        _ => None,
    }
}

/// Restyle only an existing frame's four corners, never its contents or geometry.
pub(crate) fn round_buffer_corners(buffer: &mut ratatui::buffer::Buffer, area: Rect) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let right = area.right().saturating_sub(1);
    let bottom = area.bottom().saturating_sub(1);
    for (x, y) in [
        (area.x, area.y),
        (right, area.y),
        (area.x, bottom),
        (right, bottom),
    ] {
        if x < buffer.area.x
            || y < buffer.area.y
            || x >= buffer.area.right()
            || y >= buffer.area.bottom()
        {
            continue;
        }
        let cell = &mut buffer[(x, y)];
        if let Some(symbol) = rounded_light_corner(cell.symbol()) {
            cell.set_symbol(symbol);
        }
    }
}

pub(super) fn panel_contrast_fg(palette: &Palette) -> Color {
    match palette.panel_bg {
        Color::Reset => palette.surface_dim,
        color => color,
    }
}

pub(crate) fn centered_popup_rect(area: Rect, popup_width: u16, popup_height: u16) -> Option<Rect> {
    let popup_width = popup_width.min(area.width.saturating_sub(4));
    let popup_height = popup_height.min(area.height.saturating_sub(2));
    if popup_width < 4 || popup_height < 4 {
        return None;
    }

    Some(Rect::new(
        area.x + area.width.saturating_sub(popup_width) / 2,
        area.y + area.height.saturating_sub(popup_height) / 2,
        popup_width,
        popup_height,
    ))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModalStackAreas {
    pub header: Rect,
    pub content: Rect,
    pub footer: Option<Rect>,
    pub actions: Option<Rect>,
}

pub(crate) fn modal_stack_areas(
    inner: Rect,
    header_height: u16,
    footer_height: u16,
    actions_height: u16,
    gap: u16,
) -> ModalStackAreas {
    #[derive(Clone, Copy)]
    enum Slot {
        Header,
        Content,
        Footer,
        Actions,
    }

    let mut constraints = Vec::new();
    let mut slots = Vec::new();
    let mut push = |slot: Slot, constraint: Constraint| {
        if !slots.is_empty() {
            constraints.push(Constraint::Length(gap));
        }
        constraints.push(constraint);
        slots.push(slot);
    };

    push(Slot::Header, Constraint::Length(header_height));
    push(Slot::Content, Constraint::Min(0));
    if footer_height > 0 {
        push(Slot::Footer, Constraint::Length(footer_height));
    }
    if actions_height > 0 {
        push(Slot::Actions, Constraint::Length(actions_height));
    }

    let areas = Layout::vertical(constraints).split(inner);
    let mut result = ModalStackAreas {
        header: Rect::default(),
        content: Rect::default(),
        footer: None,
        actions: None,
    };
    for (slot, area) in slots.into_iter().zip(areas.iter().step_by(2).copied()) {
        match slot {
            Slot::Header => result.header = area,
            Slot::Content => result.content = area,
            Slot::Footer => result.footer = Some(area),
            Slot::Actions => result.actions = Some(area),
        }
    }
    result
}

fn action_button_width(hint: Option<&str>, label: &str) -> u16 {
    match hint {
        Some(hint) => format!(" {hint} {label} ").chars().count() as u16,
        None => format!(" {label} ").chars().count() as u16,
    }
}

pub(crate) fn close_button_rect(area: Rect) -> Rect {
    let width = action_button_width(Some("esc"), "close");
    Rect::new(area.x + area.width.saturating_sub(width), area.y, width, 1)
}

pub(crate) fn continue_button_rect(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y,
        action_button_width(Some("↵"), "continue"),
        1,
    )
}

#[cfg(test)]
mod border_tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        style::{Modifier, Style},
        widgets::{Block, Borders, Widget},
    };

    #[test]
    fn rounded_borders_procedural_frame_preserves_every_other_cell() {
        let area = Rect::new(3, 2, 6, 4);
        let mut buffer = Buffer::empty(Rect::new(1, 1, 12, 7));
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            .render(area, &mut buffer);
        buffer[(4, 3)].set_symbol("┌");
        let before = buffer.clone();
        round_buffer_corners(&mut buffer, area);
        for y in buffer.area.y..buffer.area.bottom() {
            for x in buffer.area.x..buffer.area.right() {
                let mut expected = before[(x, y)].clone();
                if [(3, 2), (8, 2), (3, 5), (8, 5)].contains(&(x, y)) {
                    expected.set_symbol(rounded_light_corner(expected.symbol()).unwrap());
                }
                assert_eq!(buffer[(x, y)], expected);
            }
        }
        assert_eq!(
            interface_border_type(false),
            ratatui::widgets::BorderType::Plain
        );
        assert_eq!(
            interface_border_type(true),
            ratatui::widgets::BorderType::Rounded
        );
    }

    #[test]
    fn rounded_borders_helper_preserves_weight_junctions_existing_arcs_and_tiny_area() {
        for glyph in ["─", "│", "┼", "┬", "┤", "╔", "┏", "╭", " "] {
            assert_eq!(rounded_light_corner(glyph), None);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 2));
            buffer[(0, 0)].set_symbol(glyph);
            let before = buffer.clone();
            round_buffer_corners(&mut buffer, Rect::new(0, 0, 2, 2));
            assert_eq!(buffer, before);
        }
        let mut buffer = Buffer::empty(Rect::new(3, 3, 1, 1));
        buffer[(3, 3)].set_symbol("┌");
        let before = buffer.clone();
        let area = buffer.area;
        round_buffer_corners(&mut buffer, area);
        assert_eq!(buffer, before);
        round_buffer_corners(&mut buffer, Rect::new(0, 0, 2, 2));
        assert_eq!(buffer, before);
    }
}
