use super::*;
use crate::protocol::CellData;
use crate::terminal_theme::RgbColor;
use ratatui::style::{Color, Style};

const DARK_BG: RgbColor = RgbColor { r: 0, g: 0, b: 0 };
const DARK_FG: RgbColor = RgbColor {
    r: 200,
    g: 200,
    b: 200,
};
const LIGHT_BG: RgbColor = RgbColor {
    r: 255,
    g: 255,
    b: 255,
};
const LIGHT_FG: RgbColor = RgbColor { r: 0, g: 0, b: 0 };
const BORDER: Color = Color::Cyan;

fn packed(color: Color) -> u32 {
    crate::protocol::color_to_u32(color)
}

fn surface_pane(
    pane_id: &str,
    rect: SurfaceRect,
    inner: SurfaceRect,
    focused: bool,
) -> PaneSurfacePane {
    PaneSurfacePane {
        pane_id: pane_id.into(),
        content_revision: 0,
        rect,
        inner_rect: inner,
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

/// The content row written at the start of each pane's first inner row: default fg, indexed fg,
/// RGB fg, named fg with modifiers, ANSI faint, explicit background, reverse video as the server
/// resolves it (concrete fg/bg, no marker), and an indexed fg the host never reported.
fn content_row(buffer: &mut Buffer, x: u16, y: u16) {
    let cells: [(&str, Style); 8] = [
        ("a", Style::default()),
        ("b", Style::default().fg(Color::Indexed(1))),
        ("c", Style::default().fg(Color::Rgb(200, 100, 50))),
        (
            "d",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ),
        ("e", Style::default().add_modifier(Modifier::DIM)),
        (
            "f",
            Style::default()
                .fg(Color::Rgb(1, 2, 3))
                .bg(Color::Indexed(4)),
        ),
        (
            "g",
            Style::default()
                .fg(Color::Rgb(0, 0, 0))
                .bg(Color::Rgb(220, 220, 220)),
        ),
        ("h", Style::default().fg(Color::Indexed(77))),
    ];
    for (offset, (symbol, style)) in cells.into_iter().enumerate() {
        buffer[(x + offset as u16, y)]
            .set_symbol(symbol)
            .set_style(style);
    }
}

/// A 19x4 surface: a bordered left pane (frame drawn in `BORDER`, inner 8x2 at (1,1)), a divider
/// column, and a borderless right pane (8x4 at (11,0)). Each inner area starts with
/// `content_row`; the rest is blank default cells.
fn split_surface(left_focused: bool) -> PaneSurfaceFrame {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 19, 4));
    let border = Style::default().fg(BORDER);
    for x in 0..10 {
        for y in [0, 3] {
            buffer[(x, y)].set_symbol("─").set_style(border);
        }
    }
    for y in 0..4 {
        buffer[(0, y)].set_symbol("│").set_style(border);
        buffer[(9, y)].set_symbol("│").set_style(border);
        buffer[(10, y)]
            .set_symbol("│")
            .set_style(Style::default().fg(Color::Blue));
    }
    content_row(&mut buffer, 1, 1);
    content_row(&mut buffer, 11, 0);
    let mut surface = surface();
    surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    let rect = |x, y, width, height| SurfaceRect {
        x,
        y,
        width,
        height,
    };
    surface.panes = vec![
        surface_pane("pane_1", rect(0, 0, 10, 4), rect(1, 1, 8, 2), left_focused),
        surface_pane(
            "pane_2",
            rect(11, 0, 8, 4),
            rect(11, 0, 8, 4),
            !left_focused,
        ),
    ];
    surface
}

fn host_palette() -> Box<[Option<RgbColor>; 256]> {
    let mut palette = Box::new([None; 256]);
    palette[1] = Some(RgbColor { r: 255, g: 0, b: 0 });
    palette[2] = Some(RgbColor { r: 0, g: 255, b: 0 });
    palette
}

fn state_with(config_toml: &str, host: Option<(RgbColor, RgbColor)>) -> ClientShellState {
    let config: Config = toml::from_str(config_toml).expect("test config");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    if let Some((background, foreground)) = host {
        state.host_background = Some(background);
        state.host_foreground = Some(foreground);
        state.host_palette = host_palette();
    }
    state.set_snapshot(Box::new(snapshot()));
    state
}

fn compose(state: &mut ClientShellState, pane_surface: PaneSurfaceFrame) -> FrameData {
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("pane frame").frame.clone()
}

fn inner_rect(state: &ClientShellState, pane_id: &str) -> Rect {
    state
        .hits
        .panes
        .iter()
        .find(|hit| hit.pane_id == pane_id)
        .expect("pane hit")
        .inner_rect
}

fn cell_at(frame: &FrameData, x: u16, y: u16) -> &CellData {
    &frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)]
}

/// Independent oracle for the blend: host fg/palette colors used by `content_row`, blended
/// toward `bg` by `percent` with nearest-integer channels.
fn expected_fg(fg: u32, bg: RgbColor, host_fg: RgbColor, percent: u16) -> Option<u32> {
    let source = if fg == packed(Color::Reset) {
        host_fg
    } else if fg == packed(Color::Indexed(1)) {
        RgbColor { r: 255, g: 0, b: 0 }
    } else if fg == packed(Color::Green) {
        RgbColor { r: 0, g: 255, b: 0 }
    } else if fg == packed(Color::Rgb(200, 100, 50)) {
        RgbColor {
            r: 200,
            g: 100,
            b: 50,
        }
    } else {
        return None;
    };
    let mix = |f: u8, b: u8| {
        ((f64::from(f) * f64::from(100 - percent) + f64::from(b) * f64::from(percent)) / 100.0
            + 0.5)
            .floor() as u8
    };
    Some(packed(Color::Rgb(
        mix(source.r, bg.r),
        mix(source.g, bg.g),
        mix(source.b, bg.b),
    )))
}

/// Asserts `dimmed` equals `baseline` everywhere except eligible cells (Reset background, not
/// faint, resolvable fg) inside `rects`, which differ only in the expected foreground. Returns
/// the number of changed cells.
fn assert_only_eligible_fg_dimmed(
    baseline: &FrameData,
    dimmed: &FrameData,
    rects: &[Rect],
    host: (RgbColor, RgbColor),
    percent: u16,
) -> usize {
    assert_eq!(
        (baseline.width, baseline.height),
        (dimmed.width, dimmed.height)
    );
    let mut changed = 0;
    for y in 0..baseline.height {
        for x in 0..baseline.width {
            let before = cell_at(baseline, x, y);
            let after = cell_at(dimmed, x, y);
            let inside = rects
                .iter()
                .any(|rect| rect.contains(ratatui::layout::Position::new(x, y)));
            let eligible = inside
                && before.bg == packed(Color::Reset)
                && before.modifier & Modifier::DIM.bits() == 0;
            match eligible
                .then(|| expected_fg(before.fg, host.0, host.1, percent))
                .flatten()
            {
                Some(fg) => {
                    let mut expected = before.clone();
                    expected.fg = fg;
                    assert_eq!(after, &expected, "cell ({x}, {y})");
                    changed += 1;
                }
                None => assert_eq!(after, before, "cell ({x}, {y})"),
            }
        }
    }
    assert_eq!(baseline.cursor, dimmed.cursor);
    changed
}

#[test]
fn inactive_pane_dim_zero_or_unset_composes_baseline_frame() {
    let host = Some((DARK_BG, DARK_FG));
    let baseline = compose(&mut state_with("", host), split_surface(true));
    let zero = compose(
        &mut state_with("[ui]\ninactive_pane_dim_percent = 0\n", host),
        split_surface(true),
    );
    let malformed = compose(
        &mut state_with("[ui]\ninactive_pane_dim_percent = 250\n", host),
        split_surface(true),
    );
    assert_eq!(zero, baseline);
    assert_eq!(malformed, baseline);
}

#[test]
fn inactive_pane_dim_blends_only_eligible_foregrounds_of_unfocused_panes() {
    // A non-empty exclude list is accepted but cannot apply yet (no process identity).
    let config = "[ui]\ninactive_pane_dim_percent = 50\ninactive_pane_dim_exclude_processes = [\"zsh\", \"vim\"]\n";
    let host = (DARK_BG, DARK_FG);
    for left_focused in [true, false] {
        let mut base_state = state_with("", Some(host));
        let baseline = compose(&mut base_state, split_surface(left_focused));
        let mut state = state_with(config, Some(host));
        let dimmed = compose(&mut state, split_surface(left_focused));
        let (inactive, inactive_id) = if left_focused {
            (inner_rect(&state, "pane_2"), "pane_2")
        } else {
            (inner_rect(&state, "pane_1"), "pane_1")
        };
        // Borderless right pane: 4 eligible content cells + 24 blanks. Bordered left pane:
        // 4 eligible content cells + 12 blanks inside the frame; its frame is untouched.
        let expected = if left_focused { 28 } else { 12 };
        assert_eq!(
            assert_only_eligible_fg_dimmed(&baseline, &dimmed, &[inactive], host, 50),
            expected,
            "{inactive_id}"
        );
        let first = |offset: u16| cell_at(&dimmed, inactive.x + offset, inactive.y);
        assert_eq!(first(0).fg, packed(Color::Rgb(100, 100, 100)), "default fg");
        assert_eq!(first(1).fg, packed(Color::Rgb(128, 0, 0)), "indexed fg");
        assert_eq!(first(2).fg, packed(Color::Rgb(100, 50, 25)), "rgb fg");
        assert_eq!(first(3).fg, packed(Color::Rgb(0, 128, 0)), "named fg");
        assert_eq!(
            first(3).modifier,
            (Modifier::BOLD | Modifier::UNDERLINED).bits(),
            "modifiers kept"
        );
        assert_eq!(first(4).fg, packed(Color::Reset), "faint untouched");
        assert_eq!(
            first(5).fg,
            packed(Color::Rgb(1, 2, 3)),
            "explicit bg untouched"
        );
        assert_eq!(
            first(6).fg,
            packed(Color::Rgb(0, 0, 0)),
            "reverse untouched"
        );
        assert_eq!(
            first(7).fg,
            packed(Color::Indexed(77)),
            "unresolvable untouched"
        );
    }
}

#[test]
fn inactive_pane_dim_blends_toward_light_and_dark_host_backgrounds() {
    for (host, default_fg) in [
        ((DARK_BG, DARK_FG), Color::Rgb(140, 140, 140)),
        ((LIGHT_BG, LIGHT_FG), Color::Rgb(77, 77, 77)),
    ] {
        let baseline = compose(&mut state_with("", Some(host)), split_surface(true));
        let mut state = state_with("[ui]\ninactive_pane_dim_percent = 30\n", Some(host));
        let dimmed = compose(&mut state, split_surface(true));
        let inactive = inner_rect(&state, "pane_2");
        assert_eq!(
            assert_only_eligible_fg_dimmed(&baseline, &dimmed, &[inactive], host, 30),
            28
        );
        assert_eq!(
            cell_at(&dimmed, inactive.x, inactive.y).fg,
            packed(default_fg)
        );
    }
}

#[test]
fn inactive_pane_dim_leaves_cells_unchanged_when_host_colors_are_unknown() {
    let config = "[ui]\ninactive_pane_dim_percent = 60\n";
    let baseline = compose(&mut state_with("", None), split_surface(true));
    assert_eq!(
        compose(&mut state_with(config, None), split_surface(true)),
        baseline,
        "no host background: nothing can be resolved"
    );

    // Background known, foreground and palette unknown: only RGB foregrounds resolve.
    let mut state = state_with(config, None);
    state.host_background = Some(DARK_BG);
    let dimmed = compose(&mut state, split_surface(true));
    let inactive = inner_rect(&state, "pane_2");
    let mut changed = Vec::new();
    for y in 0..baseline.height {
        for x in 0..baseline.width {
            if cell_at(&baseline, x, y) != cell_at(&dimmed, x, y) {
                changed.push((x, y));
            }
        }
    }
    assert_eq!(changed, vec![(inactive.x + 2, inactive.y)]);
    assert_eq!(
        cell_at(&dimmed, inactive.x + 2, inactive.y).fg,
        packed(Color::Rgb(80, 40, 20))
    );
}

#[test]
fn inactive_pane_dim_follows_focus_between_panes() {
    let host = (DARK_BG, DARK_FG);
    let config = "[ui]\ninactive_pane_dim_percent = 50\n";
    let left_baseline = compose(&mut state_with("", Some(host)), split_surface(true));
    let right_baseline = compose(&mut state_with("", Some(host)), split_surface(false));

    let mut state = state_with(config, Some(host));
    let first = compose(&mut state, split_surface(true));
    let right = inner_rect(&state, "pane_2");
    let left = inner_rect(&state, "pane_1");
    assert_eq!(
        assert_only_eligible_fg_dimmed(&left_baseline, &first, &[right], host, 50),
        28
    );

    let mut moved = split_surface(false);
    moved.surface_revision = 2;
    let second = compose(&mut state, moved);
    assert_eq!(
        assert_only_eligible_fg_dimmed(&right_baseline, &second, &[left], host, 50),
        12,
        "newly focused right pane restored, newly inactive left pane dimmed"
    );
}

#[test]
fn inactive_pane_dim_applies_only_in_terminal_mode() {
    let host = (DARK_BG, DARK_FG);
    let mut state = state_with("[ui]\ninactive_pane_dim_percent = 50\n", Some(host));
    let terminal = compose(&mut state, split_surface(true));
    assert_ne!(
        terminal,
        compose(&mut state_with("", Some(host)), split_surface(true)),
        "terminal mode dims the inactive pane"
    );

    for mode in [
        ClientShellMode::Prefix,
        ClientShellMode::Navigate,
        ClientShellMode::Resize,
    ] {
        let mut base_state = state_with("", Some(host));
        base_state.mode = mode;
        let baseline = compose(&mut base_state, split_surface(true));
        state.mode = mode;
        let frame = state.compose(106, 20).expect("frame").frame.clone();
        assert_eq!(frame, baseline, "{mode:?} shows the baseline");
    }

    state.mode = ClientShellMode::Terminal;
    let back = state.compose(106, 20).expect("frame").frame.clone();
    assert_eq!(back, terminal, "returning to terminal mode re-applies");
}

#[test]
fn inactive_pane_dim_leaves_selection_highlight_unchanged() {
    let host = (DARK_BG, DARK_FG);
    let selection = || {
        Some(crate::selection::Selection::absolute_range(
            "pane_2".to_string(),
            (1, 0),
            (1, 1),
        ))
    };
    let mut base_state = state_with("", Some(host));
    base_state.set_pane_surface(split_surface(true));
    base_state.selection = selection();
    let baseline = base_state.compose(106, 20).expect("frame").frame.clone();
    let mut state = state_with("[ui]\ninactive_pane_dim_percent = 50\n", Some(host));
    state.set_pane_surface(split_surface(true));
    state.selection = selection();
    let dimmed = state.compose(106, 20).expect("frame").frame.clone();
    let inactive = inner_rect(&state, "pane_2");
    let source = split_surface(true).frame;

    let selected: Vec<_> = (inactive.top()..inactive.bottom())
        .flat_map(|y| (inactive.left()..inactive.right()).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let surface_cell = cell_at(&source, x - inactive.x + 11, y - inactive.y);
            surface_cell.bg == packed(Color::Reset)
                && cell_at(&baseline, x, y).bg != packed(Color::Reset)
        })
        .collect();
    assert!(!selected.is_empty(), "selection paints the inactive pane");
    for (x, y) in &selected {
        assert_eq!(
            cell_at(&dimmed, *x, *y),
            cell_at(&baseline, *x, *y),
            "({x}, {y})"
        );
    }
    // Every other cell follows the ordinary rule (selected cells are non-Reset in the baseline).
    assert_only_eligible_fg_dimmed(&baseline, &dimmed, &[inactive], host, 50);
}

#[test]
fn inactive_pane_dim_never_mutates_retained_surface() {
    let host = (DARK_BG, DARK_FG);
    let mut state = state_with("[ui]\ninactive_pane_dim_percent = 100\n", Some(host));
    let dimmed = compose(&mut state, split_surface(true));
    assert_eq!(
        state.pane_surface.as_ref().expect("retained surface"),
        &split_surface(true),
        "copy and selection read the untouched surface"
    );
    let inactive = inner_rect(&state, "pane_2");
    let text = cell_at(&dimmed, inactive.x, inactive.y);
    assert_eq!(text.symbol, "a");
    assert_eq!(
        text.fg,
        packed(Color::Rgb(0, 0, 0)),
        "100 matches the background"
    );
    assert_eq!(text.bg, packed(Color::Reset), "background never written");
}

#[test]
fn inactive_pane_dim_applies_to_retained_surface_patch_fast_path() {
    let host = (DARK_BG, DARK_FG);
    let mut state = state_with("[ui]\ninactive_pane_dim_percent = 50\n", Some(host));
    let composed = compose(&mut state, split_surface(true));
    let row = |x: u16, y: u16| crate::protocol::PaneSurfacePatchRow {
        x,
        y,
        cells: vec![
            CellData {
                symbol: "N".into(),
                fg: packed(Color::Reset),
                bg: packed(Color::Reset),
                modifier: 0,
                skip: false,
                hyperlink: None,
            };
            8
        ],
    };
    let mut panes = split_surface(true).panes;
    for pane in &mut panes {
        pane.content_revision = 2;
    }
    let patch = crate::protocol::PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        base_surface_revision: 1,
        surface_revision: 2,
        // Row 1 of the focused bordered pane and row 2 of the unfocused borderless pane.
        rows: vec![row(1, 1), row(11, 2)],
        panes,
        cursor: None,
    };

    let ClientPaneSurfacePatchOutcome::Applied(Some(composed_patch)) =
        state.apply_pane_surface_patch(patch)
    else {
        panic!("expected the fast retained patch path");
    };
    let presented =
        apply_composed_surface_patch(&composed, composed_patch).expect("apply composed patch");
    let full = state.compose(106, 20).expect("full compose").frame.clone();
    assert_eq!(
        presented, full,
        "fast patch presents what a full compose shows"
    );

    let focused = inner_rect(&state, "pane_1");
    let inactive = inner_rect(&state, "pane_2");
    for offset in 0..8 {
        let focused_cell = cell_at(&presented, focused.x + offset, focused.y);
        assert_eq!(
            (focused_cell.symbol.as_str(), focused_cell.fg),
            ("N", packed(Color::Reset))
        );
        let inactive_cell = cell_at(&presented, inactive.x + offset, inactive.y + 2);
        assert_eq!(
            (
                inactive_cell.symbol.as_str(),
                inactive_cell.fg,
                inactive_cell.bg
            ),
            ("N", packed(Color::Rgb(100, 100, 100)), packed(Color::Reset))
        );
    }
    let stored = &state.pane_surface.as_ref().expect("surface").frame;
    assert_eq!(
        cell_at(stored, 11, 2).fg,
        packed(Color::Reset),
        "stored surface untouched"
    );
}

#[test]
fn inactive_pane_dim_records_host_foreground_and_palette_replies() {
    let mut state = state_with("[ui]\ninactive_pane_dim_percent = 40\n", None);
    let outcome = state.handle_raw_events(vec![
        crate::raw_input::RawInputEvent::HostDefaultColor {
            kind: crate::terminal_theme::DefaultColorKind::Foreground,
            color: DARK_FG,
        },
        crate::raw_input::RawInputEvent::HostPaletteColors {
            colors: vec![(1, RgbColor { r: 255, g: 0, b: 0 })],
        },
    ]);
    assert!(
        outcome.repaint,
        "new host colors repaint while dimming is on"
    );
    assert_eq!(state.host_foreground, Some(DARK_FG));
    assert_eq!(state.host_palette[1], Some(RgbColor { r: 255, g: 0, b: 0 }));
    assert_eq!(state.host_palette[2], None);

    let mut off = state_with("", None);
    let outcome = off.handle_raw_events(vec![crate::raw_input::RawInputEvent::HostDefaultColor {
        kind: crate::terminal_theme::DefaultColorKind::Foreground,
        color: DARK_FG,
    }]);
    assert!(!outcome.repaint, "no extra repaint while dimming is off");
    assert_eq!(off.host_foreground, Some(DARK_FG));
}
