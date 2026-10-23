use super::*;
use ratatui::style::{Color, Style};

const TINT: Color = Color::Rgb(0x20, 0x21, 0x22);
const REVERSE_BG: Color = Color::Rgb(0xaa, 0xbb, 0xcc);

fn pane(pane_id: &str, x: u16, focused: bool) -> PaneSurfacePane {
    let rect = SurfaceRect {
        x,
        y: 0,
        width: 4,
        height: 3,
    };
    PaneSurfacePane {
        pane_id: pane_id.into(),
        content_revision: 0,
        rect,
        inner_rect: rect,
        scrollbar_rect: None,
        scroll: None,
        focused,
        mouse_reporting: false,
        sgr_pixel_mouse: false,
        alternate_screen_active: false,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Two 4x3 panes split by a divider column. Each pane's first row holds a default-background
/// cell, explicit indexed and RGB backgrounds, and a reverse-video cell as the server resolves it
/// (concrete background); the remaining rows are blank default-background cells.
fn split_surface(first_focused: bool) -> PaneSurfaceFrame {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 9, 3));
    for origin in [0u16, 5] {
        buffer[(origin, 0)]
            .set_symbol("a")
            .set_style(Style::default().fg(Color::Green).bg(Color::Reset));
        buffer[(origin + 1, 0)]
            .set_symbol("b")
            .set_style(Style::default().bg(Color::Indexed(1)));
        buffer[(origin + 2, 0)]
            .set_symbol("c")
            .set_style(Style::default().bg(Color::Rgb(1, 2, 3)));
        buffer[(origin + 3, 0)].set_symbol("d").set_style(
            Style::default()
                .fg(Color::Rgb(0x11, 0x22, 0x33))
                .bg(REVERSE_BG),
        );
    }
    for y in 0..3 {
        buffer[(4, y)]
            .set_symbol("│")
            .set_style(Style::default().fg(Color::Blue));
    }
    let mut surface = surface();
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    surface.panes = vec![
        pane("pane_1", 0, first_focused),
        pane("pane_2", 5, !first_focused),
    ];
    surface
}

/// Two blank panes filling the 80x19 desktop pane surface; the left one is unfocused.
fn half_surface() -> PaneSurfaceFrame {
    let mut pane_surface = surface();
    let buffer = Buffer::empty(Rect::new(0, 0, 80, 19));
    pane_surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    let half = |pane_id: &str, x: u16, focused: bool| {
        let mut half = pane(pane_id, x, focused);
        half.rect.width = 40;
        half.rect.height = 19;
        half.inner_rect = half.rect;
        half
    };
    pane_surface.panes = vec![half("pane_1", 0, false), half("pane_2", 40, true)];
    pane_surface
}

fn compose_with(
    pane_surface: PaneSurfaceFrame,
    tint: Option<Color>,
    selection: Option<crate::selection::Selection<String>>,
) -> (FrameData, Vec<PaneHit>) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.palette.pane_inactive_bg = tint;
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(pane_surface);
    state.selection = selection;
    let frame = state.compose(106, 20).expect("pane frame").frame;
    (frame, state.hits.panes.clone())
}

fn cell_at(frame: &FrameData, x: u16, y: u16) -> &crate::protocol::CellData {
    &frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)]
}

/// Asserts that `tinted` equals `baseline` everywhere except default-background cells inside
/// `tinted_rects`, which differ only by carrying `TINT` as their background.
fn assert_only_default_cells_tinted(
    baseline: &FrameData,
    tinted: &FrameData,
    tinted_rects: &[Rect],
) -> usize {
    assert_eq!(
        (baseline.width, baseline.height),
        (tinted.width, tinted.height)
    );
    let reset = crate::protocol::color_to_u32(Color::Reset);
    let mut changed = 0;
    for y in 0..baseline.height {
        for x in 0..baseline.width {
            let before = cell_at(baseline, x, y);
            let after = cell_at(tinted, x, y);
            let inside = tinted_rects
                .iter()
                .any(|rect| rect.contains(ratatui::layout::Position::new(x, y)));
            if inside && before.bg == reset {
                let mut expected = before.clone();
                expected.bg = crate::protocol::color_to_u32(TINT);
                assert_eq!(after, &expected, "cell ({x}, {y})");
                changed += 1;
            } else {
                assert_eq!(after, before, "cell ({x}, {y})");
            }
        }
    }
    assert_eq!(baseline.cursor, tinted.cursor);
    changed
}

#[test]
fn inactive_pane_bg_unset_and_reset_compose_baseline_frame() {
    let (baseline, _) = compose_with(split_surface(true), None, None);
    let (reset, _) = compose_with(split_surface(true), Some(Color::Reset), None);
    assert_eq!(reset, baseline);
}

#[test]
fn inactive_pane_bg_tints_only_default_background_cells_of_unfocused_panes() {
    let (baseline, hits) = compose_with(split_surface(true), None, None);
    let (tinted, _) = compose_with(split_surface(true), Some(TINT), None);
    let inactive = hits
        .iter()
        .find(|hit| hit.pane_id == "pane_2")
        .expect("inactive pane hit")
        .inner_rect;
    // One default cell on the content row plus two blank rows of four cells.
    assert_eq!(
        assert_only_default_cells_tinted(&baseline, &tinted, &[inactive]),
        9
    );
    let row = |x: u16| cell_at(&tinted, inactive.x + x, inactive.y);
    assert_eq!(row(0).bg, crate::protocol::color_to_u32(TINT));
    assert_eq!(row(0).fg, crate::protocol::color_to_u32(Color::Green));
    assert_eq!(row(1).bg, crate::protocol::color_to_u32(Color::Indexed(1)));
    assert_eq!(
        row(2).bg,
        crate::protocol::color_to_u32(Color::Rgb(1, 2, 3))
    );
    assert_eq!(row(3).bg, crate::protocol::color_to_u32(REVERSE_BG));
}

#[test]
fn inactive_pane_bg_follows_focus_between_panes() {
    let (baseline_first, hits) = compose_with(split_surface(true), None, None);
    let (tinted_first, _) = compose_with(split_surface(true), Some(TINT), None);
    let (baseline_second, _) = compose_with(split_surface(false), None, None);
    let (tinted_second, _) = compose_with(split_surface(false), Some(TINT), None);
    let rect = |id: &str| {
        hits.iter()
            .find(|hit| hit.pane_id == id)
            .expect("pane hit")
            .inner_rect
    };
    assert_only_default_cells_tinted(&baseline_first, &tinted_first, &[rect("pane_2")]);
    assert_only_default_cells_tinted(&baseline_second, &tinted_second, &[rect("pane_1")]);

    // A lone or zoomed surface lists only the focused pane and stays at baseline.
    let (lone_baseline, _) = compose_with(surface(), None, None);
    let (lone_tinted, _) = compose_with(surface(), Some(TINT), None);
    assert_eq!(lone_tinted, lone_baseline);
}

#[test]
fn inactive_pane_bg_leaves_selection_highlight_unchanged() {
    let selection = || {
        Some(crate::selection::Selection::absolute_range(
            "pane_2".into(),
            (1, 0),
            (1, 1),
        ))
    };
    let (baseline, hits) = compose_with(split_surface(true), None, selection());
    let (tinted, _) = compose_with(split_surface(true), Some(TINT), selection());
    let inactive = hits
        .iter()
        .find(|hit| hit.pane_id == "pane_2")
        .expect("inactive pane hit")
        .inner_rect;
    let selected = Rect::new(inactive.x, inactive.y + 1, 2, 1);
    let reset = crate::protocol::color_to_u32(Color::Reset);
    for x in selected.left()..selected.right() {
        let cell = cell_at(&baseline, x, selected.y);
        assert_ne!(cell.bg, reset, "selection paints its own background");
        assert_eq!(cell_at(&tinted, x, selected.y), cell);
    }
    // Everything else follows the ordinary rule.
    assert_eq!(
        assert_only_default_cells_tinted(&baseline, &tinted, &[inactive]),
        7
    );
}

#[test]
fn inactive_pane_bg_does_not_touch_popup_surface() {
    // Two half-surface panes so the centred popup overlaps the inactive one.
    let popup_surface = || {
        let mut pane_surface = half_surface();
        pane_surface.popup = surface_with_popup().popup;
        pane_surface
    };
    let compose = |tint| {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.config.palette.pane_inactive_bg = tint;
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(popup_surface());
        let frame = state.compose(106, 20).expect("popup frame").frame;
        let inactive = state.hits.panes[0].inner_rect;
        let popup = state.hits.popup.as_ref().expect("popup hit").rect;
        (frame, inactive, popup)
    };
    let (baseline, inactive, popup) = compose(None);
    let (tinted, _, _) = compose(Some(TINT));
    // The popup hit rect is the framed outer geometry.
    let popup_outer = popup;
    assert!(
        popup_outer.intersects(inactive),
        "popup must overlap the inactive pane"
    );
    let reset = crate::protocol::color_to_u32(Color::Reset);
    for y in 0..baseline.height {
        for x in 0..baseline.width {
            let position = ratatui::layout::Position::new(x, y);
            let before = cell_at(&baseline, x, y);
            let after = cell_at(&tinted, x, y);
            if inactive.contains(position) && !popup_outer.contains(position) && before.bg == reset
            {
                assert_eq!(after.bg, crate::protocol::color_to_u32(TINT), "({x}, {y})");
            } else {
                assert_eq!(after, before, "cell ({x}, {y})");
            }
        }
    }
}

#[test]
fn inactive_pane_bg_leaves_mode_bar_over_inactive_pane_unchanged() {
    let compose = |tint| {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.config.palette.pane_inactive_bg = tint;
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(half_surface());
        state.mode = ClientShellMode::Resize;
        let frame = state.compose(106, 20).expect("mode bar frame").frame;
        (frame, state.hits.panes[0].inner_rect)
    };
    let (baseline, inactive) = compose(None);
    let (tinted, _) = compose(Some(TINT));
    // The mode bar occupies the last pane-surface row, across the inactive pane.
    let bar_row = inactive.bottom() - 1;
    let rows = frame_rows(&baseline);
    assert_ne!(
        rows[usize::from(bar_row)].trim(),
        "",
        "mode bar drawn on the inactive pane row"
    );
    let bar = Rect::new(0, bar_row, baseline.width, 1);
    assert_eq!(
        assert_only_default_cells_tinted(
            &baseline,
            &tinted,
            &[Rect::new(
                inactive.x,
                inactive.y,
                inactive.width,
                inactive.height - 1
            )]
        ),
        usize::from(inactive.width) * usize::from(inactive.height - 1)
    );
    for x in bar.left()..bar.right() {
        assert_eq!(cell_at(&tinted, x, bar_row), cell_at(&baseline, x, bar_row));
    }
}

#[test]
fn inactive_pane_bg_applies_to_retained_surface_patch_fast_path() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.palette.pane_inactive_bg = Some(TINT);
    state.set_snapshot(Box::new(snapshot()));
    let pane_surface = half_surface();
    let panes = pane_surface.panes.clone();
    state.set_pane_surface(pane_surface);
    let composed = state.compose(106, 20).expect("initial composed frame");
    let composed = composed.frame;
    assert_eq!(state.mode, ClientShellMode::Terminal);
    let cell = |symbol: &str, bg: Color| crate::protocol::CellData {
        symbol: symbol.into(),
        fg: 0,
        bg: crate::protocol::color_to_u32(bg),
        modifier: 0,
        skip: false,
        hyperlink: None,
    };
    let patch = crate::protocol::PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        base_surface_revision: 1,
        surface_revision: 2,
        rows: vec![
            // Unfocused pane_1: default, explicit indexed and explicit RGB backgrounds.
            crate::protocol::PaneSurfacePatchRow {
                x: 0,
                y: 2,
                cells: vec![
                    cell("u", Color::Reset),
                    cell("i", Color::Indexed(1)),
                    cell("r", Color::Rgb(1, 2, 3)),
                ],
            },
            // Focused pane_2: default background stays Reset.
            crate::protocol::PaneSurfacePatchRow {
                x: 40,
                y: 2,
                cells: vec![cell("f", Color::Reset)],
            },
        ],
        panes,
        cursor: None,
    };
    let ClientPaneSurfacePatchOutcome::Applied(Some(patch)) = state.apply_pane_surface_patch(patch)
    else {
        panic!("expected fast retained patch");
    };
    let presented = apply_composed_surface_patch(&composed, patch).expect("apply composed patch");
    let full = state
        .compose(106, 20)
        .expect("full compose after patch")
        .frame;
    let origin = state.layout(106, 20).pane_surface;
    let at = |frame: &FrameData, x: u16| cell_at(frame, origin.x + x, origin.y + 2).clone();
    assert_eq!(at(&presented, 0).symbol, "u");
    assert_eq!(at(&presented, 0).bg, crate::protocol::color_to_u32(TINT));
    assert_eq!(
        at(&presented, 1).bg,
        crate::protocol::color_to_u32(Color::Indexed(1))
    );
    assert_eq!(
        at(&presented, 2).bg,
        crate::protocol::color_to_u32(Color::Rgb(1, 2, 3))
    );
    assert_eq!(at(&presented, 40).symbol, "f");
    assert_eq!(
        at(&presented, 40).bg,
        crate::protocol::color_to_u32(Color::Reset)
    );
    // The fast path presents exactly what a full composition would.
    assert_eq!(presented.cells, full.cells);
}

#[test]
fn inactive_pane_row_tints_only_unfocused_spans_crossed_by_the_row() {
    // Panes at x 0..4 (unfocused), 5..9 (focused) and 10..14 (unfocused); the row starts at x 2
    // and ends at x 12, crossing all three plus the divider columns 4 and 9.
    let mut first = pane("pane_1", 0, false);
    first.inner_rect.width = 4;
    let focused = pane("pane_2", 5, true);
    let third = pane("pane_3", 10, false);
    let mut below = pane("pane_4", 0, false);
    below.inner_rect.y = 3;
    let panes = [first, focused, third, below];
    let default_cell = || crate::protocol::CellData {
        symbol: " ".into(),
        fg: 0,
        bg: crate::protocol::color_to_u32(Color::Reset),
        modifier: 0,
        skip: false,
        hyperlink: None,
    };
    let mut cells = vec![default_cell(); 10];
    cells[1].bg = crate::protocol::color_to_u32(Color::Indexed(1));
    composition::tint_inactive_pane_row(
        &panes,
        2,
        1,
        &mut cells,
        crate::protocol::color_to_u32(TINT),
    );
    let tinted: Vec<u16> = (2u16..)
        .zip(&cells)
        .filter(|(_, cell)| cell.bg == crate::protocol::color_to_u32(TINT))
        .map(|(x, _)| x)
        .collect();
    assert_eq!(tinted, vec![2, 10, 11]);
    assert_eq!(
        cells[1].bg,
        crate::protocol::color_to_u32(Color::Indexed(1))
    );
}
