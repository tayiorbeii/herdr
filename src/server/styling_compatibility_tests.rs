use super::*;
use sha2::{Digest, Sha256};

// This fixture runs the actual server surface renderer and client compositor without
// starting a session, opening a PTY, or depending on the checkout's path or branch.
fn fixture(config: &Config, cols: u16, rows: u16) -> serde_json::Value {
    fixture_focused(config, cols, rows, 0)
}

fn fixture_focused(config: &Config, cols: u16, rows: u16, focused: usize) -> serde_json::Value {
    let mut workspaces = active_panes(3);
    let workspace = &mut workspaces[0];
    workspace.id = "w1".into();
    workspace.cached_git_branch = Some("fixture".into());
    workspace.cached_git_ahead_behind = None;
    let mut pipeline = RenderPipeline::with_config(workspaces, config);
    pipeline.app.state.mode = crate::app::Mode::Terminal;
    pipeline.app.state.ensure_test_terminals();
    let pane_ids = pipeline.app.state.workspaces[0].tabs[0].layout.pane_ids();
    for (index, pane_id) in pane_ids.iter().enumerate() {
        let terminal_id = pipeline.app.state.workspaces[0]
            .terminal_id(*pane_id)
            .expect("fixture terminal")
            .clone();
        let terminal = pipeline.app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Idle,
        );
        terminal.agent_name = Some(format!("agent-{}", index + 1));
        terminal.last_agent_state_change_seq = Some(1);
        terminal.metadata_tokens.patch(
            std::collections::HashMap::from([("role".into(), Some("builder".into()))]),
            None,
            Instant::now(),
        );
    }
    pipeline.app.state.workspaces[0].tabs[0]
        .layout
        .focus_pane(pane_ids[focused]);
    pipeline
        .client
        .set_snapshot(Box::new(super::super::client_shell::snapshot(
            &pipeline.app,
            "styling-fixture",
            1,
            None,
            None,
        )));
    pipeline.client.handle_raw_events(vec![
        crate::raw_input::RawInputEvent::HostDefaultColor {
            kind: crate::terminal_theme::DefaultColorKind::Background,
            color: crate::terminal_theme::RgbColor { r: 0, g: 0, b: 0 },
        },
        crate::raw_input::RawInputEvent::HostDefaultColor {
            kind: crate::terminal_theme::DefaultColorKind::Foreground,
            color: crate::terminal_theme::RgbColor {
                r: 220,
                g: 220,
                b: 220,
            },
        },
    ]);
    let size = pipeline.client.surface_size(cols, rows);
    let rendered = super::super::client_shell::render_pane_surface(
        &mut pipeline.app,
        Some(crate::ui::TabSurfaceTarget {
            workspace_index: 0,
            tab_index: 0,
        }),
        Rect::new(0, 0, size.cols, size.rows),
        true,
        true,
        HostCellSize {
            width_px: 10,
            height_px: 20,
        },
        &pipeline.graphics_delivery,
        1,
    )
    .expect("fixture surface");
    let geometry: Vec<_> = rendered
        .panes
        .iter()
        .map(|pane| {
            serde_json::json!({
                "rect": pane.rect,
                "inner_rect": pane.inner_rect,
                "scrollbar_rect": pane.scrollbar_rect,
                "focused": pane.focused,
            })
        })
        .collect();
    pipeline.client.set_pane_surface(PaneSurfaceFrame {
        boot_id: "styling-fixture".into(),
        projection_revision: 1,
        surface_revision: 0,
        frame: rendered.frame,
        panes: rendered.panes,
        splits: rendered.splits,
        popup: rendered.popup,
        graphics: rendered.graphics,
    });
    let frame = pipeline
        .client
        .compose(cols, rows)
        .expect("composed fixture")
        .frame
        .clone();
    serde_json::json!({ "frame": frame, "panes": geometry })
}

fn digest(snapshot: &serde_json::Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(snapshot).unwrap())
    )
}

fn export(name: &str, snapshot: &serde_json::Value) {
    if let Some(directory) = std::env::var_os("HERDR_STYLING_SNAPSHOT_DIR") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(snapshot).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn styling_defaults_preserve_upstream_cells_and_geometry() {
    // Recorded on upstream master 5bd900d8 before importing any styling layer.
    // IDs are excluded; symbols, colors, modifiers, cursor and geometry are retained.
    let actual = [(120, 40), (40, 12)].map(|(cols, rows)| {
        let snapshot = fixture(&Config::default(), cols, rows);
        export(&format!("default-{cols}x{rows}"), &snapshot);
        digest(&snapshot)
    });
    assert_eq!(
        actual,
        [
            "5e0f8cbd58a5a4640a6d069a2c5daf3e54e1df1806168775836d98bfa036dcdf",
            "5df750d70281dc09187fa02b13014b1a061b151833d1a238d297346c0e7df242",
        ]
    );
}

const STYLING_OFF: &str = r##"[experimental]
allow_nested = true
[ui]
rounded_borders = false
pane_padding_cells = 0
sidebar_padding_cells = 0
pane_border_style = "light"
pane_border_style_active = "light"
pane_border_style_inactive = "light"
pane_manual_label_first = false
inactive_pane_dim_percent = 0
[theme.custom]
pane_inactive_bg = "reset"
"##;
const STYLING_ON: &str = r##"[experimental]
allow_nested = true
[ui]
pane_borders = "always"
rounded_borders = true
pane_gap_cells = 2
pane_padding_cells = 1
sidebar_padding_cells = 1
pane_border_style_active = "heavy"
pane_border_style_inactive = "rounded"
pane_title_tokens = [{ token = "$role", fg = "#abcdef", bold = true }]
pane_border_identity_token = { token = "$role", fg = "#335577" }
inactive_pane_dim_percent = 50
[theme.custom]
active_tab_fg = "#112233"
active_tab_bg = "#fab387"
pane_border_active = "#fab387"
pane_border_inactive = "#8899aa"
popup_bg = "#101820"
popup_border = "#aabbcc"
pane_inactive_bg = "#203040"
"##;

#[tokio::test(flavor = "current_thread")]
async fn styling_explicit_off_matches_refreshed_upstream() {
    let off: Config = toml::from_str(STYLING_OFF).expect("off control config");
    for (cols, rows) in [(120, 40), (40, 12)] {
        let baseline = fixture(&Config::default(), cols, rows);
        let disabled = fixture(&off, cols, rows);
        export(&format!("off-{cols}x{rows}"), &disabled);
        assert_eq!(disabled, baseline, "explicit off at {cols}x{rows}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn styling_combined_controls_keep_geometry_and_foreground_transforms_separate() {
    let on: Config = toml::from_str(STYLING_ON).expect("combined config");
    let without_dim: Config = toml::from_str(&STYLING_ON.replace(
        "inactive_pane_dim_percent = 50",
        "inactive_pane_dim_percent = 0",
    ))
    .expect("dimming control config");
    for (cols, rows) in [(120, 40), (40, 12)] {
        let mut previous_geometry = None;
        let mut previous_cells = None;
        for focused in [0, 1] {
            let styled = fixture_focused(&on, cols, rows, focused);
            let undimmed = fixture_focused(&without_dim, cols, rows, focused);
            export(&format!("enabled-{cols}x{rows}-focus-{focused}"), &styled);
            let frame: crate::protocol::FrameData =
                serde_json::from_value(styled["frame"].clone()).unwrap();
            let control: crate::protocol::FrameData =
                serde_json::from_value(undimmed["frame"].clone()).unwrap();
            assert_eq!(frame.cursor, control.cursor);
            assert_eq!(frame.hyperlinks, control.hyperlinks);
            assert_eq!(frame.graphics, control.graphics);
            assert_eq!((frame.width, frame.height), (control.width, control.height));
            assert_eq!(frame.cells.len(), control.cells.len());
            let mut changed_foregrounds = 0;
            for (cell, original) in frame.cells.iter().zip(&control.cells) {
                assert_eq!(cell.symbol, original.symbol, "dimming must preserve glyphs");
                assert_eq!(cell.bg, original.bg, "dimming must preserve backgrounds");
                assert_eq!(
                    cell.modifier, original.modifier,
                    "dimming must preserve modifiers"
                );
                changed_foregrounds += usize::from(cell.fg != original.fg);
            }
            assert!(
                changed_foregrounds > 0,
                "enabled dimming must affect real terminal text"
            );
            assert!(
                frame.cells.iter().any(|cell| cell.bg == 0x02203040),
                "inactive tint"
            );
            assert!(
                frame.cells.iter().any(|cell| cell.fg == 0x02abcdef),
                "styled title token"
            );
            assert!(
                frame.cells.iter().any(|cell| cell.fg == 0x02335577),
                "inactive identity color"
            );
            assert!(
                frame.cells.iter().any(|cell| cell.fg == 0x02fab387),
                "active border override"
            );
            assert!(
                frame
                    .cells
                    .iter()
                    .any(|cell| cell.symbol == "╭" || cell.symbol == "╰"),
                "rounded inactive frames"
            );
            assert!(
                frame
                    .cells
                    .iter()
                    .any(|cell| cell.symbol == "━" || cell.symbol == "┃"),
                "heavy focused frame"
            );
            let geometry: Vec<_> = styled["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pane| {
                    let rect = &pane["rect"];
                    let inner = &pane["inner_rect"];
                    for (axis, extent) in [("x", "width"), ("y", "height")] {
                        let outer_start = rect[axis].as_u64().unwrap();
                        let inner_start = inner[axis].as_u64().unwrap();
                        assert!(inner_start >= outer_start);
                        assert!(
                            inner_start + inner[extent].as_u64().unwrap()
                                <= outer_start + rect[extent].as_u64().unwrap()
                        );
                    }
                    (rect.clone(), inner.clone(), pane["scrollbar_rect"].clone())
                })
                .collect();
            if let Some(previous) = &previous_geometry {
                assert_eq!(
                    &geometry, previous,
                    "focus must not change gaps, padding or scrollbar geometry"
                );
            }
            previous_geometry = Some(geometry);
            if let Some(previous) = &previous_cells {
                assert_ne!(
                    &styled["frame"]["cells"], previous,
                    "focus must move visible border and content treatment"
                );
            }
            previous_cells = Some(styled["frame"]["cells"].clone());
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "manual combined styling pipeline scaling profile"]
async fn styling_render_scale_profile() {
    let enabled: Config = toml::from_str(STYLING_ON).expect("combined config");
    for (scenario, build) in [
        ("active", active_panes as fn(usize) -> Vec<Workspace>),
        ("background", workspaces),
    ] {
        let stock: Config = toml::from_str(
            r#"[ui]
pane_borders = "always"
"#,
        )
        .unwrap();
        let same_geometry: Config = toml::from_str(
            &STYLING_ON
                .replace("pane_gap_cells = 2\n", "")
                .replace("pane_padding_cells = 1", "pane_padding_cells = 0")
                .replace("sidebar_padding_cells = 1", "sidebar_padding_cells = 0"),
        )
        .unwrap();
        for (mode, config) in [
            ("stock_framed", &stock),
            ("styled_same_geometry", &same_geometry),
            ("all_enabled", &enabled),
        ] {
            let rows = [1, 15].map(|count| {
                let mut pipeline = RenderPipeline::with_config(build(count), config);
                pipeline.app.state.ensure_test_terminals();
                for terminal in pipeline.app.state.terminals.values_mut() {
                    terminal.set_detected_state(
                        Some(crate::detect::Agent::Pi),
                        crate::detect::AgentState::Idle,
                    );
                    terminal.metadata_tokens.patch(
                        std::collections::HashMap::from([("role".into(), Some("builder".into()))]),
                        None,
                        Instant::now(),
                    );
                }
                pipeline.client.handle_raw_events(vec![
                    crate::raw_input::RawInputEvent::HostDefaultColor {
                        kind: crate::terminal_theme::DefaultColorKind::Background,
                        color: crate::terminal_theme::RgbColor { r: 0, g: 0, b: 0 },
                    },
                    crate::raw_input::RawInputEvent::HostDefaultColor {
                        kind: crate::terminal_theme::DefaultColorKind::Foreground,
                        color: crate::terminal_theme::RgbColor {
                            r: 220,
                            g: 220,
                            b: 220,
                        },
                    },
                ]);
                pipeline
                    .client
                    .set_snapshot(Box::new(super::super::client_shell::snapshot(
                        &pipeline.app,
                        "bench-boot",
                        1,
                        None,
                        None,
                    )));
                (count, profile_pipeline(pipeline))
            });
            println!("combined styling {scenario} {mode}: {COLS}x{ROWS}");
            print_stage("server", &rows, |stats| stats.server);
            print_stage("client", &rows, |stats| stats.client);
            print_stage("total", &rows, |stats| stats.total);
        }
    }
}
