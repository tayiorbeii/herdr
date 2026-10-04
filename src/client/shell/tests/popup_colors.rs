use super::*;
use crate::protocol::{color_to_u32, CellData};
use ratatui::style::Color;

const COLS: u16 = 106;
const ROWS: u16 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PopupCell {
    Border,
    Shell,
    Pty,
}

fn config_from(toml_text: &str) -> Config {
    toml::from_str(toml_text).expect("test config parses")
}

/// PTY frame smaller than the popup inner rect so shell blank cells stay visible next to it.
/// Row 0: `R` explicit indexed bg, `G` explicit RGB bg, `D` default bg; the rest are default blanks.
fn popup_frame() -> FrameData {
    let mut buffer = Buffer::empty(Rect::new(0, 0, 7, 2));
    buffer[(0, 0)].set_symbol("R").set_bg(Color::Indexed(1));
    buffer[(1, 0)].set_symbol("G").set_bg(Color::Rgb(1, 2, 3));
    buffer[(2, 0)].set_symbol("D").set_fg(Color::Green);
    FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[])
}

fn surface_with_popup_frame(title: &str, frame: FrameData) -> PaneSurfaceFrame {
    let mut surface = surface_with_popup();
    let popup = surface.popup.as_mut().expect("popup fixture");
    popup.title = title.into();
    popup.frame = frame;
    surface
}

fn popup_state(config: &Config) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface_with_popup_frame("popup", popup_frame()));
    state
}

fn compose_popup(config: &Config) -> (FrameData, PaneHit) {
    let mut state = popup_state(config);
    let frame = state.compose(COLS, ROWS).expect("popup frame").frame;
    let hit = state.hits.popup.clone().expect("popup hit");
    (frame, hit)
}

fn classify(hit: &PaneHit, frame_size: (u16, u16), x: u16, y: u16) -> Option<PopupCell> {
    let outer = hit.rect;
    if x < outer.x || y < outer.y || x >= outer.right() || y >= outer.bottom() {
        return None;
    }
    if x == outer.x || y == outer.y || x == outer.right() - 1 || y == outer.bottom() - 1 {
        return Some(PopupCell::Border);
    }
    let inner = hit.inner_rect;
    let in_pty = x >= inner.x
        && y >= inner.y
        && x < inner.x + frame_size.0.min(inner.width)
        && y < inner.y + frame_size.1.min(inner.height);
    Some(if in_pty {
        PopupCell::Pty
    } else {
        PopupCell::Shell
    })
}

fn cell(frame: &FrameData, x: u16, y: u16) -> &CellData {
    &frame.cells[y as usize * frame.width as usize + x as usize]
}

fn popup_cells(frame: &FrameData, hit: &PaneHit, kind: PopupCell) -> Vec<(u16, u16)> {
    let mut cells = Vec::new();
    for y in 0..frame.height {
        for x in 0..frame.width {
            if classify(hit, (7, 2), x, y) == Some(kind) {
                cells.push((x, y));
            }
        }
    }
    assert!(!cells.is_empty(), "no {kind:?} cells");
    cells
}

/// Expected colors per consumer: (border fg, shell bg, PTY default bg).
fn assert_popup_colors(
    frame: &FrameData,
    hit: &PaneHit,
    border_fg: Color,
    shell_bg: Color,
    pty_bg: Color,
) {
    for (x, y) in popup_cells(frame, hit, PopupCell::Border) {
        let cell = cell(frame, x, y);
        assert_eq!(cell.fg, color_to_u32(border_fg), "border fg at {x},{y}");
        assert_eq!(cell.bg, color_to_u32(shell_bg), "border bg at {x},{y}");
    }
    for (x, y) in popup_cells(frame, hit, PopupCell::Shell) {
        let cell = cell(frame, x, y);
        assert_eq!(cell.symbol, " ", "shell symbol at {x},{y}");
        assert_eq!(cell.bg, color_to_u32(shell_bg), "shell bg at {x},{y}");
    }
    let inner = hit.inner_rect;
    for (x, y) in popup_cells(frame, hit, PopupCell::Pty) {
        let cell = cell(frame, x, y);
        let expected = match (x - inner.x, y - inner.y) {
            (0, 0) => Color::Indexed(1),
            (1, 0) => Color::Rgb(1, 2, 3),
            _ => pty_bg,
        };
        assert_eq!(cell.bg, color_to_u32(expected), "pty bg at {x},{y}");
    }
    let title_x = (hit.rect.x..hit.rect.right())
        .find(|&x| cell(frame, x, hit.rect.y).symbol == "p")
        .expect("title text");
    assert_eq!(
        cell(frame, title_x, hit.rect.y).fg,
        color_to_u32(border_fg),
        "title inherits the border fg"
    );
    let default_cell = cell(frame, inner.x + 2, inner.y);
    assert_eq!(default_cell.symbol, "D");
    assert_eq!(default_cell.fg, color_to_u32(Color::Green));
}

/// Every cell outside the popup outer rect must equal the reference composition.
fn assert_outside_popup_equal(left: &FrameData, right: &FrameData, hit: &PaneHit) {
    assert_eq!((left.width, left.height), (right.width, right.height));
    for y in 0..left.height {
        for x in 0..left.width {
            if classify(hit, (7, 2), x, y).is_none() {
                assert_eq!(
                    cell(left, x, y),
                    cell(right, x, y),
                    "outside popup at {x},{y}"
                );
            }
        }
    }
}

#[test]
fn popup_chrome_absent_keys_match_baseline_per_consumer() {
    for theme in ["catppuccin", "terminal"] {
        let config = config_from(&format!("[theme]\nname = \"{theme}\"\n"));
        let palette = crate::app::client_palette_from_config(&config);
        let (frame, hit) = compose_popup(&config);
        // Baseline: border/title fg = accent, shell cells bg = panel_bg, PTY default cells verbatim Reset.
        assert_popup_colors(&frame, &hit, palette.accent, palette.panel_bg, Color::Reset);
    }

    // Mode-only popup keys without auto_switch must not leak into the manual palette.
    let reference = compose_popup(&Config::default());
    let ignored = compose_popup(&config_from(
        r##"
[theme.custom.light]
popup_bg = "red"
popup_border = "blue"
[theme.custom.dark]
popup_bg = "green"
popup_border = "yellow"
"##,
    ));
    assert_eq!(reference.0, ignored.0);
}

#[test]
fn popup_bg_only_paints_shell_and_default_pty_cells() {
    let config = config_from("[theme.custom]\npopup_bg = \"#102030\"\n");
    let palette = crate::app::client_palette_from_config(&config);
    let (frame, hit) = compose_popup(&config);
    let bg = Color::Rgb(16, 32, 48);
    assert_popup_colors(&frame, &hit, palette.accent, bg, bg);
}

#[test]
fn popup_border_only_paints_border_and_title_foreground() {
    let config = config_from("[theme.custom]\npopup_border = \"magenta\"\n");
    let palette = crate::app::client_palette_from_config(&config);
    let (frame, hit) = compose_popup(&config);
    assert_popup_colors(&frame, &hit, Color::Magenta, palette.panel_bg, Color::Reset);
}

#[test]
fn popup_bg_and_border_leave_everything_outside_the_popup_unchanged() {
    let config =
        config_from("[theme.custom]\npopup_bg = \"#102030\"\npopup_border = \"#a0b0c0\"\n");
    let (frame, hit) = compose_popup(&config);
    let bg = Color::Rgb(16, 32, 48);
    assert_popup_colors(&frame, &hit, Color::Rgb(160, 176, 192), bg, bg);

    let (reference, reference_hit) = compose_popup(&Config::default());
    assert_eq!(hit.rect, reference_hit.rect);
    assert_eq!(hit.inner_rect, reference_hit.inner_rect);
    assert_eq!(frame.cursor, reference.cursor);
    assert_outside_popup_equal(&frame, &reference, &hit);
    assert_eq!(
        crate::app::client_palette_from_config(&config).accent,
        crate::app::client_palette_from_config(&Config::default()).accent
    );
}

#[test]
fn popup_reset_values_are_terminal_defaults() {
    let config = config_from(
        "[theme]\nname = \"catppuccin\"\n[theme.custom]\npopup_bg = \"reset\"\npopup_border = \"default\"\n",
    );
    let (frame, hit) = compose_popup(&config);
    assert_popup_colors(&frame, &hit, Color::Reset, Color::Reset, Color::Reset);
}

#[test]
fn popup_invalid_colors_fall_back_to_cyan() {
    let config =
        config_from("[theme.custom]\npopup_bg = \"not-a-color\"\npopup_border = \"#zzzzzz\"\n");
    let (frame, hit) = compose_popup(&config);
    assert_popup_colors(&frame, &hit, Color::Cyan, Color::Cyan, Color::Cyan);
}

#[test]
fn popup_colors_follow_host_appearance() {
    use crate::terminal_theme::HostAppearance;

    let config = config_from(
        r##"
[theme]
name = "terminal"
auto_switch = true
[theme.custom]
popup_bg = "blue"
[theme.custom.light]
popup_border = "red"
[theme.custom.dark]
popup_bg = "reset"
"##,
    );
    let mut state = popup_state(&config);
    for (appearance, border, bg) in [
        (HostAppearance::Light, Some(Color::Red), Color::Blue),
        (HostAppearance::Dark, None, Color::Reset),
    ] {
        state.handle_raw_events(vec![RawInputEvent::HostColorSchemeChanged(appearance)]);
        let border = border.unwrap_or(state.config.palette.accent);
        let frame = state.compose(COLS, ROWS).expect("popup frame");
        let hit = state.hits.popup.clone().expect("popup hit");
        assert_popup_colors(&frame, &hit, border, bg, bg);
    }
}

#[test]
fn popup_colors_apply_to_command_and_plugin_popup_titles() {
    let config = config_from("[theme.custom]\npopup_bg = \"#102030\"\npopup_border = \"yellow\"\n");
    let bg = Color::Rgb(16, 32, 48);
    // Command popups are titled "popup"; plugin popups use the manifest pane title.
    for title in ["popup", "plugin pane"] {
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        state.set_snapshot(Box::new(snapshot()));
        state.set_pane_surface(surface_with_popup_frame(title, popup_frame()));
        let frame = state.compose(COLS, ROWS).expect("popup frame");
        let hit = state.hits.popup.clone().expect("popup hit");
        assert_popup_colors(&frame, &hit, Color::Yellow, bg, bg);
    }
}

#[test]
fn popup_colors_follow_resize_and_close_without_residue() {
    let config = config_from("[theme.custom]\npopup_bg = \"#102030\"\npopup_border = \"yellow\"\n");
    let bg = Color::Rgb(16, 32, 48);
    let mut state = popup_state(&config);
    for (cols, rows) in [(COLS, ROWS), (60, 12), (COLS, ROWS)] {
        let frame = state.compose(cols, rows).expect("popup frame");
        let hit = state.hits.popup.clone().expect("popup hit");
        assert_popup_colors(&frame, &hit, Color::Yellow, bg, bg);
    }

    state.set_pane_surface(surface());
    let closed = state.compose(COLS, ROWS).expect("closed frame");
    assert!(state.hits.popup.is_none());
    let mut reference = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    reference.set_snapshot(Box::new(snapshot()));
    reference.set_pane_surface(surface());
    assert_eq!(
        closed.frame,
        reference
            .compose(COLS, ROWS)
            .expect("reference frame")
            .frame
    );
}

#[tokio::test]
async fn popup_bg_respects_real_terminal_background_provenance() {
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(
        7,
        2,
        b"\x1b[41mR\x1b[48;2;1;2;3mG\x1b[49mD\x1b[0m",
    );
    let (buffer, _) =
        crate::server::render_stream::render_terminal_virtual(&runtime, Rect::new(0, 0, 7, 2));
    let pty = FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[]);
    // Server-side provenance: explicit SGR backgrounds are non-Reset, default/SGR 49 cells are Reset.
    assert_eq!(pty.cells[0].bg, color_to_u32(Color::Indexed(1)));
    assert_eq!(pty.cells[1].bg, color_to_u32(Color::Rgb(1, 2, 3)));
    for (index, cell) in pty.cells.iter().enumerate().skip(2) {
        assert_eq!(cell.bg, color_to_u32(Color::Reset), "pty cell {index}");
    }

    let config = config_from("[theme.custom]\npopup_bg = \"#102030\"\n");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface_with_popup_frame("popup", pty));
    let frame = state.compose(COLS, ROWS).expect("popup frame");
    let inner = state.hits.popup.as_ref().expect("popup hit").inner_rect;
    let bg = |x: u16, y: u16| cell(&frame, inner.x + x, inner.y + y).bg;
    assert_eq!(bg(0, 0), color_to_u32(Color::Indexed(1)));
    assert_eq!(bg(1, 0), color_to_u32(Color::Rgb(1, 2, 3)));
    assert_eq!(cell(&frame, inner.x + 2, inner.y).symbol, "D");
    for (x, y) in [(2, 0), (3, 0), (6, 0), (0, 1), (6, 1)] {
        assert_eq!(
            bg(x, y),
            color_to_u32(Color::Rgb(16, 32, 48)),
            "pty {x},{y}"
        );
    }
}
