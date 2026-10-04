use super::*;

fn numbered_lines(range: std::ops::Range<usize>) -> Vec<u8> {
    range
        .map(|n| format!("line {n:>3}\r\n"))
        .collect::<String>()
        .into_bytes()
}

/// `retained_test_server_with_control` with a scrollback-capable runtime, so
/// the scrollbar lane can appear. Keep the control receiver alive: dropping it
/// stops the test writer thread.
fn scrollback_test_server(
    initial_screen: &[u8],
) -> (
    HeadlessServer,
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
    crate::layout::PaneId,
) {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("test");
    let pane_id = workspace.focused_pane_id().expect("focused pane");
    workspace.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            80,
            24,
            64 * 1024,
            initial_screen,
        ),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    let (client_tx, client_control_rx, client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    assert!(server.claim_unowned_shell_tab_geometry(1, true));
    (server, client_control_rx, client_rx, pane_id)
}

/// Wait for the streamed frame, then empty the single-slot render lane so the
/// next render is not deferred behind it.
#[track_caller]
fn receive(render_rx: &std::sync::mpsc::Receiver<Vec<u8>>) {
    render_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("streamed frame");
    while render_rx.try_recv().is_ok() {}
}

/// Like `receive`, for renders that may legitimately stream nothing.
fn settle(render_rx: &std::sync::mpsc::Receiver<Vec<u8>>) {
    let _ = render_rx.recv_timeout(Duration::from_millis(200));
    while render_rx.try_recv().is_ok() {}
}

fn pty_size(server: &HeadlessServer, pane_id: crate::layout::PaneId) -> (u16, u16) {
    server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("pane runtime")
        .current_size()
}

fn committed_surface(server: &HeadlessServer) -> protocol::PaneSurfaceFrame {
    server.clients[&1]
        .render_state
        .last_pane_surface()
        .expect("committed pane surface")
        .clone()
}

fn surface_rect(x: u16, y: u16, width: u16, height: u16) -> protocol::SurfaceRect {
    protocol::SurfaceRect {
        x,
        y,
        width,
        height,
    }
}

fn cell_symbol(frame: &FrameData, x: u16, y: u16) -> &str {
    frame.cells[usize::from(y) * usize::from(frame.width) + usize::from(x)]
        .symbol
        .as_str()
}

fn set_pane_padding(server: &mut HeadlessServer, padding: u16) {
    server.app.state.pane_padding_cells = padding;
    assert!(server.reapply_controlled_shell_tab_geometry(false));
}

/// Lone 80x24 pane: no frame, stable scrollbar gutter at column 79.
const LANE: protocol::SurfaceRect = protocol::SurfaceRect {
    x: 79,
    y: 0,
    width: 1,
    height: 24,
};

#[tokio::test]
async fn reload_pane_padding_resizes_shell_pty_and_published_geometry() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "hpad-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let (mut server, _control_rx, render_rx, pane_id) =
        scrollback_test_server(&numbered_lines(0..40));
    server.render_and_stream();
    receive(&render_rx);
    assert_eq!(pty_size(&server, pane_id), (24, 79));

    fs::write(&path, "[ui]\npane_padding_cells = 2\n").unwrap();
    server.reload_server_config(false);
    assert_eq!(server.app.state.pane_padding_cells, 2);
    assert_eq!(
        pty_size(&server, pane_id),
        (20, 75),
        "reload must resize the PTY"
    );

    server.render_and_stream();
    receive(&render_rx);
    let surface = committed_surface(&server);
    let pane = &surface.panes[0];
    assert_eq!(pane.rect, surface_rect(0, 0, 80, 24));
    // The client maps mouse input, selection and copy mode from this origin.
    assert_eq!(pane.inner_rect, surface_rect(2, 2, 75, 20));
    assert_eq!(pane.scrollbar_rect, Some(LANE));
    for y in 0..24 {
        for x in [0, 1, 77, 78] {
            assert_eq!(cell_symbol(&surface.frame, x, y), " ", "padding {x},{y}");
        }
    }
    for y in [0, 1, 22, 23] {
        for x in 0..79 {
            assert_eq!(cell_symbol(&surface.frame, x, y), " ", "padding {x},{y}");
        }
    }
    assert!(
        (2..22).any(|y| cell_symbol(&surface.frame, 2, y) == "l"),
        "terminal text starts at the padded origin column"
    );
    assert_ne!(
        cell_symbol(&surface.frame, 79, 0),
        " ",
        "scrollbar lane drawn"
    );

    fs::write(&path, "").unwrap();
    server.reload_server_config(false);
    assert_eq!(pty_size(&server, pane_id), (24, 79));
    server.render_and_stream();
    receive(&render_rx);
    let pane = committed_surface(&server).panes[0].clone();
    assert_eq!(pane.inner_rect, surface_rect(0, 0, 79, 24));
    assert_eq!(pane.scrollbar_rect, Some(LANE));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = fs::remove_dir_all(&dir);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn retained_scrollbar_patch_keeps_lane_outside_padding() {
    let (mut server, _control_rx, render_rx, pane_id) =
        scrollback_test_server(&numbered_lines(0..40));
    set_pane_padding(&mut server, 2);
    server.render_and_stream();
    receive(&render_rx);
    let before = committed_surface(&server);
    assert_eq!(before.panes[0].inner_rect, surface_rect(2, 2, 75, 20));
    assert_eq!(before.panes[0].scrollbar_rect, Some(LANE));

    write_shared_test_pane(&mut server, pane_id, &numbered_lines(40..42));
    assert!(server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id])));
    receive(&render_rx);
    let retained = committed_surface(&server);
    assert_eq!(retained.panes[0].inner_rect, before.panes[0].inner_rect);
    assert_eq!(retained.panes[0].scrollbar_rect, Some(LANE));

    // The retained frame must equal what the complete renderer draws.
    server.render_and_stream();
    settle(&render_rx);
    let complete = committed_surface(&server);
    assert_eq!(complete.panes[0].scrollbar_rect, Some(LANE));
    assert_eq!(retained.frame.cells, complete.frame.cells);
    shutdown_test_runtimes(&mut server);
}

#[tokio::test]
async fn retained_scrollbar_first_appearance_with_padding_uses_complete_renderer() {
    for padding in [0u16, 2] {
        // Fill all but the last content row, so two more lines start scrollback.
        let rows = usize::from(24 - 2 * padding);
        let (mut server, _control_rx, render_rx, pane_id) =
            scrollback_test_server(&numbered_lines(0..rows - 1));
        set_pane_padding(&mut server, padding);
        server.render_and_stream();
        receive(&render_rx);
        assert_eq!(committed_surface(&server).panes[0].scrollbar_rect, None);

        write_shared_test_pane(&mut server, pane_id, &numbered_lines(rows - 1..rows + 1));
        let retained = server.render_retained_pane_surface_and_stream(&HashSet::from([pane_id]));
        settle(&render_rx);
        if padding == 0 {
            assert!(retained, "control: the unpadded lane is derived in place");
            assert_eq!(
                committed_surface(&server).panes[0].scrollbar_rect,
                Some(LANE)
            );
        } else {
            assert!(
                !retained,
                "padded lane must come from the complete renderer"
            );
            server.render_and_stream();
            receive(&render_rx);
            let pane = committed_surface(&server).panes[0].clone();
            assert_eq!(pane.scrollbar_rect, Some(LANE));
            assert_eq!(pane.inner_rect, surface_rect(2, 2, 75, 20));
        }
        shutdown_test_runtimes(&mut server);
    }
}

/// Two side-by-side panes with real runtimes at an arbitrary client size.
fn split_spacing_server(
    cols: u16,
    rows: u16,
    gap_cells: Option<u16>,
    padding: u16,
) -> (
    HeadlessServer,
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("test");
    let left = workspace.focused_pane_id().expect("focused pane");
    let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
    for pane in [left, right] {
        workspace.insert_test_runtime(
            pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(cols, rows, b"x"),
        );
    }
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.app.state.pane_gap_cells = gap_cells;
    server.app.state.pane_padding_cells = padding;

    let (client_tx, client_control_rx, client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (cols, rows),
            crate::kitty_graphics::HostCellSize::default(),
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    assert!(server.claim_unowned_shell_tab_geometry(1, true));
    (server, client_control_rx, client_rx)
}

/// S2-pane x S1: padding is applied inside each pane's allocation after the
/// configured gap. Every PTY equals its published content rect, the published
/// content rect stays inside the pane rect (the client derives pane hits and
/// mouse origins from these two rects), and gap and padding cells stay blank.
#[tokio::test]
async fn pane_padding_applies_inside_pane_gap_cells_allocation() {
    for (cols, rows) in [(80u16, 24u16), (24, 8), (16, 6), (12, 4)] {
        let (mut unpadded_server, _unpadded_control_rx, unpadded_rx) =
            split_spacing_server(cols, rows, Some(2), 0);
        unpadded_server.render_and_stream();
        receive(&unpadded_rx);
        let baseline = unpadded_server.app.state.view.pane_infos.clone();
        shutdown_test_runtimes(&mut unpadded_server);
        let (mut server, _control_rx, render_rx) = split_spacing_server(cols, rows, Some(2), 1);
        server.render_and_stream();
        receive(&render_rx);
        let surface = committed_surface(&server);
        let infos = server.app.state.view.pane_infos.clone();
        assert_eq!(surface.panes.len(), 2, "{cols}x{rows}");
        assert_eq!(infos.len(), 2, "{cols}x{rows}");
        for info in &infos {
            let published = surface
                .panes
                .iter()
                .find(|pane| pane.rect == info.rect.into())
                .expect("published pane for view pane");
            assert_eq!(
                published.inner_rect,
                info.inner_rect.into(),
                "{cols}x{rows}"
            );
            // The PTY size floor applies only where the unpadded allocation is
            // already below it (baseline tiny behavior); padding never causes it.
            assert_eq!(
                pty_size(&server, info.id),
                (
                    info.inner_rect.height.max(crate::pane::MIN_PANE_ROWS),
                    info.inner_rect.width.max(crate::pane::MIN_PANE_COLS)
                ),
                "{cols}x{rows}: PTY must equal the published content rect"
            );
            let unpadded = baseline
                .iter()
                .find(|base| base.rect == info.rect)
                .expect("padding never moves the pane allocation");
            assert_eq!(
                info.inner_rect,
                crate::ui::pad_pane_content(unpadded.inner_rect, 1),
                "{cols}x{rows}: padding insets the gap-adjusted content rect"
            );
            if unpadded.inner_rect.width >= crate::pane::MIN_PANE_COLS {
                assert!(info.inner_rect.width >= crate::pane::MIN_PANE_COLS);
            }
            if unpadded.inner_rect.height >= crate::pane::MIN_PANE_ROWS {
                assert!(info.inner_rect.height >= crate::pane::MIN_PANE_ROWS);
            }
            let inner = info.inner_rect;
            let rect = info.rect;
            assert!(inner.x >= rect.x && inner.y >= rect.y, "{cols}x{rows}");
            assert!(inner.right() <= rect.right(), "{cols}x{rows}");
            assert!(inner.bottom() <= rect.bottom(), "{cols}x{rows}");
            assert!(inner.width >= 1 && inner.height >= 1, "{cols}x{rows}");
        }
        let mut rects: Vec<Rect> = infos.iter().map(|info| info.rect).collect();
        rects.sort_by_key(|rect| rect.x);
        let gap = rects[1].x - rects[0].right();
        if (cols, rows) == (80, 24) {
            assert_eq!(gap, 2, "full-size layout keeps the configured gap");
            for info in &infos {
                // One frame cell, then one padding cell, on every side.
                assert_eq!(info.inner_rect.x, info.rect.x + 2);
                assert_eq!(info.inner_rect.y, info.rect.y + 2);
                assert!(info.inner_rect.bottom() + 2 <= info.rect.bottom());
            }
            for y in 0..rows {
                for x in rects[0].right()..rects[1].x {
                    assert_eq!(cell_symbol(&surface.frame, x, y), " ", "gap {x},{y}");
                }
            }
            for info in &infos {
                let pad_x = info.inner_rect.x - 1;
                for y in info.inner_rect.y..info.inner_rect.bottom() {
                    assert_eq!(
                        cell_symbol(&surface.frame, pad_x, y),
                        " ",
                        "pad {pad_x},{y}"
                    );
                }
            }
        } else {
            assert!(gap <= 2, "{cols}x{rows}: the gap only shrinks");
        }
        shutdown_test_runtimes(&mut server);
    }
}
