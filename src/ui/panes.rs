use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::scrollbar::{render_pane_scrollbar, should_show_scrollbar};
#[cfg(test)]
use super::text::display_width;
use super::text::truncate_end;
use super::widgets::panel_contrast_fg;
use crate::app::state::Palette;
use crate::app::AppState;
use crate::layout::PaneInfo;
use crate::popup_size::resolve_popup_geometry;
use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};

pub(crate) fn pane_is_scrolled_back(rt: &TerminalRuntime) -> bool {
    rt.scroll_metrics()
        .is_some_and(|metrics| metrics.offset_from_bottom > 0)
}

fn pane_border_title(label: &str, pane_width: u16, _focused: bool) -> Option<String> {
    let label = label.trim();
    if label.is_empty() || pane_width <= 4 {
        return None;
    }
    let max_label_width = pane_width.saturating_sub(4) as usize;
    Some(format!(" {} ", truncate_end(label, max_label_width)))
}

// Full view computation reaches this helper for active and background panes.
// Keep terminal queries narrow, allocation-free, and short under the core lock.
fn terminal_inner_rect(rt: &TerminalRuntime, pane_inner: Rect, pane_scrollbars: bool) -> Rect {
    if !pane_scrollbars || pane_inner.width <= 4 || rt.alternate_screen_active() {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

pub(crate) fn pane_inner_rect(area: Rect, borders: Borders) -> Rect {
    if borders.is_empty() {
        area
    } else {
        Block::default().borders(borders).inner(area)
    }
}

fn ranges_overlap(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    a_start < b_start.saturating_add(b_len) && b_start < a_start.saturating_add(a_len)
}

fn pane_to_right<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let right = info.rect.x.saturating_add(info.rect.width);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.x == right
            && ranges_overlap(
                info.rect.y,
                info.rect.height,
                other.rect.y,
                other.rect.height,
            )
    })
}

fn pane_below<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let bottom = info.rect.y.saturating_add(info.rect.height);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.y == bottom
            && ranges_overlap(info.rect.x, info.rect.width, other.rect.x, other.rect.width)
    })
}

fn shrink_for_gap(size: u16, gap: u16) -> u16 {
    if size > gap {
        size - gap
    } else {
        size
    }
}

/// Configured inter-pane spacing: the legacy `ui.pane_gaps` boolean, or the
/// `ui.pane_gap_cells` override when it is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneSpacing {
    Legacy(bool),
    Cells(u16),
}

/// Inter-pane spacing resolved for one tab layout. `separate` keeps each pane's
/// own frame on edges shared with a neighbour; `blank` is the number of empty
/// cells taken from the leading (left/top) pane at every internal boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PaneGaps {
    pub(crate) separate: bool,
    pub(crate) blank: u16,
}

impl PaneGaps {
    pub(crate) fn legacy(pane_gaps: bool, draws_borders: bool) -> Self {
        Self {
            separate: pane_gaps,
            blank: u16::from(pane_gaps && !draws_borders),
        }
    }

    fn cells(cells: u16) -> Self {
        Self {
            separate: cells > 0,
            blank: cells,
        }
    }
}

#[derive(Clone, Copy)]
struct PaneArea {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl PaneArea {
    fn of(panes: &[PaneInfo]) -> Self {
        Self {
            left: panes.iter().map(|info| info.rect.x).min().unwrap_or(0),
            top: panes.iter().map(|info| info.rect.y).min().unwrap_or(0),
            right: panes
                .iter()
                .map(|info| info.rect.x.saturating_add(info.rect.width))
                .max()
                .unwrap_or(0),
            bottom: panes
                .iter()
                .map(|info| info.rect.y.saturating_add(info.rect.height))
                .max()
                .unwrap_or(0),
        }
    }
}

fn chrome_borders(
    rect: Rect,
    right_neighbor: bool,
    below_neighbor: bool,
    separate: bool,
    pane_outer_borders: bool,
    area: PaneArea,
) -> Borders {
    let mut borders = Borders::ALL;
    if !separate {
        if right_neighbor {
            borders.remove(Borders::RIGHT);
        }
        if below_neighbor {
            borders.remove(Borders::BOTTOM);
        }
    }
    if !pane_outer_borders {
        if rect.x == area.left {
            borders.remove(Borders::LEFT);
        }
        if rect.y == area.top {
            borders.remove(Borders::TOP);
        }
        if rect.x.saturating_add(rect.width) == area.right {
            borders.remove(Borders::RIGHT);
        }
        if rect.y.saturating_add(rect.height) == area.bottom {
            borders.remove(Borders::BOTTOM);
        }
    }
    borders
}

pub(crate) fn apply_pane_chrome(
    panes: Vec<PaneInfo>,
    pane_borders: crate::config::PaneBordersConfig,
    pane_gaps: bool,
    pane_outer_borders: bool,
) -> Vec<PaneInfo> {
    let gaps = PaneGaps::legacy(pane_gaps, pane_borders.draws_borders());
    apply_resolved_pane_chrome(panes, pane_borders, gaps, pane_outer_borders)
}

/// Applies pane frames and inter-pane spacing. Absent `pane_gap_cells` takes the
/// legacy `pane_gaps` path; the returned gaps drive border merging and split hits.
pub(crate) fn apply_pane_spacing(
    panes: Vec<PaneInfo>,
    pane_borders: crate::config::PaneBordersConfig,
    spacing: PaneSpacing,
    pane_outer_borders: bool,
) -> (Vec<PaneInfo>, PaneGaps) {
    let gaps = match spacing {
        PaneSpacing::Legacy(pane_gaps) => {
            return (
                apply_pane_chrome(panes, pane_borders, pane_gaps, pane_outer_borders),
                PaneGaps::legacy(pane_gaps, pane_borders.draws_borders()),
            );
        }
        PaneSpacing::Cells(cells) => PaneGaps::cells(effective_gap_cells(
            &panes,
            pane_borders,
            cells,
            pane_outer_borders,
        )),
    };
    (
        apply_resolved_pane_chrome(panes, pane_borders, gaps, pane_outer_borders),
        gaps,
    )
}

/// Largest gap up to `cells` that still leaves one usable column (row) in every
/// pane that gives up cells on that axis and has one in the shared-divider
/// layout. Zero falls back to shared dividers, the baseline tiny layout.
fn effective_gap_cells(
    panes: &[PaneInfo],
    pane_borders: crate::config::PaneBordersConfig,
    cells: u16,
    pane_outer_borders: bool,
) -> u16 {
    if cells == 0 || panes.len() < 2 {
        return 0;
    }
    let bordered = pane_borders.shows_borders(true);
    let area = PaneArea::of(panes);
    let inner = |info: &PaneInfo, right: bool, below: bool, separate: bool| {
        let borders = if bordered {
            chrome_borders(info.rect, right, below, separate, pane_outer_borders, area)
        } else {
            Borders::NONE
        };
        pane_inner_rect(info.rect, borders)
    };
    let mut gap = cells;
    for info in panes {
        let right = pane_to_right(info, panes).is_some();
        let below = pane_below(info, panes).is_some();
        if !right && !below {
            continue;
        }
        let shared = inner(info, right, below, false);
        let separated = inner(info, right, below, true);
        if right && shared.width > 0 {
            gap = gap.min(separated.width.saturating_sub(1));
        }
        if below && shared.height > 0 {
            gap = gap.min(separated.height.saturating_sub(1));
        }
    }
    gap
}

fn apply_resolved_pane_chrome(
    panes: Vec<PaneInfo>,
    pane_borders: crate::config::PaneBordersConfig,
    gaps: PaneGaps,
    pane_outer_borders: bool,
) -> Vec<PaneInfo> {
    let multi_pane = panes.len() > 1;
    let bordered = pane_borders.shows_borders(multi_pane);
    let area = PaneArea::of(&panes);
    panes
        .iter()
        .cloned()
        .map(|mut info| {
            let right_neighbor = multi_pane.then(|| pane_to_right(&info, &panes)).flatten();
            let below_neighbor = multi_pane.then(|| pane_below(&info, &panes)).flatten();

            if multi_pane && gaps.blank > 0 {
                if right_neighbor.is_some() {
                    info.rect.width = shrink_for_gap(info.rect.width, gaps.blank);
                }
                if below_neighbor.is_some() {
                    info.rect.height = shrink_for_gap(info.rect.height, gaps.blank);
                }
            }

            info.borders = if !bordered {
                Borders::NONE
            } else {
                chrome_borders(
                    info.rect,
                    right_neighbor.is_some(),
                    below_neighbor.is_some(),
                    gaps.separate,
                    pane_outer_borders,
                    area,
                )
            };
            info
        })
        .collect()
}

fn runtime_for_tab_pane<'a>(
    _app: &'a AppState,
    terminal_runtimes: &'a TerminalRuntimeRegistry,
    _workspace_index: usize,
    tab: &'a crate::workspace::Tab,
    pane_id: crate::layout::PaneId,
) -> Option<(&'a crate::terminal::TerminalId, &'a TerminalRuntime)> {
    let terminal_id = tab.terminal_id(pane_id)?;
    #[cfg(test)]
    if let Some(runtime) = _app
        .workspaces
        .get(_workspace_index)?
        .test_runtimes
        .get(&pane_id)
        .or_else(|| tab.runtimes.get(&pane_id))
    {
        return Some((terminal_id, runtime));
    }
    terminal_runtimes
        .get(terminal_id)
        .map(|runtime| (terminal_id, runtime))
}

fn stable_scrollbar_gutter(
    rt: &TerminalRuntime,
    pane_inner: Rect,
    pane_scrollbars: bool,
) -> (Rect, Option<Rect>) {
    let inner_rect = terminal_inner_rect(rt, pane_inner, pane_scrollbars);
    if inner_rect == pane_inner {
        return (inner_rect, None);
    }
    let gutter = Rect::new(
        pane_inner.x + pane_inner.width.saturating_sub(1),
        pane_inner.y,
        1,
        pane_inner.height,
    );
    let scrollbar_rect = rt
        .scroll_metrics()
        .filter(|metrics| should_show_scrollbar(*metrics))
        .map(|_| gutter);

    (inner_rect, scrollbar_rect)
}

/// Resize every visible runtime in a tab to the geometry it would receive if the tab were selected.
pub(super) fn resize_tab_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    workspace_index: usize,
    tab: &crate::workspace::Tab,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        if let Some((terminal_id, rt)) =
            runtime_for_tab_pane(app, terminal_runtimes, workspace_index, tab, focused_id)
        {
            let borders = if app.pane_borders.shows_borders(multi_pane) && app.pane_outer_borders {
                Borders::ALL
            } else {
                Borders::NONE
            };
            let pane_inner = pane_inner_rect(area, borders);
            let inner_rect = terminal_inner_rect(rt, pane_inner, app.pane_scrollbars);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return;
    }

    let (pane_infos, _) = apply_pane_spacing(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_spacing(),
        app.pane_outer_borders,
    );
    for info in pane_infos {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        if let Some((terminal_id, rt)) =
            runtime_for_tab_pane(app, terminal_runtimes, workspace_index, tab, info.id)
        {
            let inner_rect = terminal_inner_rect(rt, pane_inner, app.pane_scrollbars);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
    }
}

/// Compute pane layout info and optionally resize pane runtimes to match.
pub(super) fn compute_pane_infos_for_tab(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    ws_idx: usize,
    tab_idx: usize,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> (Vec<PaneInfo>, PaneGaps) {
    let Some(tab) = app
        .workspaces
        .get(ws_idx)
        .and_then(|workspace| workspace.tabs.get(tab_idx))
    else {
        return (Vec::new(), PaneGaps::default());
    };

    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        let borders = if app.pane_borders.shows_borders(multi_pane) && app.pane_outer_borders {
            Borders::ALL
        } else {
            Borders::NONE
        };
        let pane_inner = pane_inner_rect(area, borders);
        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, focused_id) {
            (inner_rect, scrollbar_rect) =
                stable_scrollbar_gutter(rt, pane_inner, app.pane_scrollbars);
            if resize_panes
                && tab.terminal_id(focused_id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        let zoomed = PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect,
            scrollbar_rect,
            borders,
            is_focused: true,
        };
        return (vec![zoomed], PaneGaps::default());
    }

    let (mut pane_infos, pane_gaps) = apply_pane_spacing(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_spacing(),
        app.pane_outer_borders,
    );

    for info in &mut pane_infos {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            (inner_rect, scrollbar_rect) =
                stable_scrollbar_gutter(rt, pane_inner, app.pane_scrollbars);
            if resize_panes
                && tab.terminal_id(info.id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }

        info.inner_rect = inner_rect;
        info.scrollbar_rect = scrollbar_rect;
    }

    (pane_infos, pane_gaps)
}

#[cfg(test)]
fn compute_pane_infos(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Vec<PaneInfo> {
    let Some(workspace_index) = app.active else {
        return Vec::new();
    };
    let Some(tab_index) = app
        .workspaces
        .get(workspace_index)
        .map(crate::workspace::Workspace::active_tab_index)
    else {
        return Vec::new();
    };
    compute_pane_infos_for_tab(
        app,
        terminal_runtimes,
        workspace_index,
        tab_index,
        area,
        resize_panes,
        cell_size,
    )
    .0
}

pub(super) fn render_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    target: Option<super::tab_surface::TabSurfaceTarget>,
    pane_infos: &[PaneInfo],
    split_borders: &[crate::layout::SplitBorder],
    pane_gaps: PaneGaps,
) {
    let Some(target) = target else {
        return;
    };
    let ws_idx = target.workspace_index;
    let Some(ws) = app.workspaces.get(ws_idx) else {
        return;
    };

    for info in pane_infos {
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            let show_cursor = info.is_focused
                && !pane_is_scrolled_back(rt)
                && app.pane_exposes_host_cursor(ws_idx, info.id);
            rt.render(frame, info.inner_rect, show_cursor);
            render_pane_scrollbar(app, frame, info, rt);
        } else if let Some(reason) = ws
            .tabs
            .get(target.tab_index)
            .and_then(|tab| tab.terminal_id(info.id))
            .and_then(|id| app.terminals.get(id))
            .and_then(|terminal| terminal.restore_error.as_deref())
        {
            frame.render_widget(
                Paragraph::new(reason).wrap(Wrap { trim: false }),
                info.inner_rect,
            );
        }
    }

    render_pane_borders(app, ws, pane_infos, split_borders, pane_gaps, frame);
}

pub(crate) fn popup_pane_rects(app: &AppState, area: Rect) -> Option<(Rect, Rect)> {
    let popup = app.popup_pane.as_ref()?;
    resolve_popup_geometry(popup.width, popup.height, area)
        .map(|geometry| (geometry.outer, geometry.inner))
}

pub(super) fn resize_popup_pane(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let Some(popup) = app.popup_pane.as_ref() else {
        return;
    };
    let Some((_outer, inner)) = popup_pane_rects(app, area) else {
        return;
    };
    if app.direct_attach_resize_locks.contains(&popup.terminal_id) {
        return;
    }
    if let Some(rt) = terminal_runtimes.get(&popup.terminal_id) {
        rt.resize(
            inner.height,
            inner.width,
            cell_size.width_px,
            cell_size.height_px,
        );
    }
}

#[derive(Clone, Copy, Default)]
struct LineCell {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

fn render_pane_borders(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    pane_infos: &[PaneInfo],
    split_borders: &[crate::layout::SplitBorder],
    pane_gaps: PaneGaps,
    frame: &mut Frame,
) {
    if !app.pane_borders.draws_borders() || pane_infos.iter().all(|info| info.borders.is_empty()) {
        return;
    }

    let mut cells = std::collections::HashMap::<(u16, u16), LineCell>::new();
    for info in pane_infos {
        add_pane_border_cells(&mut cells, info);
    }
    add_split_border_cells(pane_gaps.separate, split_borders, &mut cells);

    let buf = frame.buffer_mut();
    let area = buf.area;
    for ((x, y), line) in cells {
        if x < area.x
            || x >= area.x.saturating_add(area.width)
            || y < area.y
            || y >= area.y.saturating_add(area.height)
        {
            continue;
        }
        let focused = pane_infos
            .iter()
            .any(|info| info.is_focused && line_touches_pane(x, y, info, pane_gaps.separate));
        let symbol = line_cell_symbol(line);
        if symbol.is_empty() {
            continue;
        }
        let cell = &mut buf[(x, y)];
        cell.set_symbol(symbol);
        let color = if focused {
            app.palette.accent
        } else {
            app.palette.overlay0
        };
        cell.set_style(Style::default().fg(color));
    }

    render_pane_border_titles(app, ws, pane_infos, frame);
}

fn add_split_border_cells(
    pane_gaps: bool,
    split_borders: &[crate::layout::SplitBorder],
    cells: &mut std::collections::HashMap<(u16, u16), LineCell>,
) {
    if pane_gaps {
        return;
    }

    for split in split_borders {
        match split.direction {
            ratatui::layout::Direction::Horizontal => {
                let x = split.pos;
                let end = split.area.y.saturating_add(split.area.height);
                for y in split.area.y..=end {
                    if !cells.contains_key(&(x, y)) {
                        continue;
                    }
                    let left = x
                        .checked_sub(1)
                        .and_then(|left_x| cells.get(&(left_x, y)))
                        .is_some_and(|cell| cell.left || cell.right);
                    let right = cells
                        .get(&(x.saturating_add(1), y))
                        .is_some_and(|cell| cell.left || cell.right);
                    let cell = cells.entry((x, y)).or_default();
                    cell.up |= y > split.area.y;
                    cell.down |= y + 1 < end;
                    cell.left |= left;
                    cell.right |= right;
                }
            }
            ratatui::layout::Direction::Vertical => {
                let y = split.pos;
                let end = split.area.x.saturating_add(split.area.width);
                for x in split.area.x..=end {
                    if !cells.contains_key(&(x, y)) {
                        continue;
                    }
                    let up = y
                        .checked_sub(1)
                        .and_then(|up_y| cells.get(&(x, up_y)))
                        .is_some_and(|cell| cell.up || cell.down);
                    let down = cells
                        .get(&(x, y.saturating_add(1)))
                        .is_some_and(|cell| cell.up || cell.down);
                    let cell = cells.entry((x, y)).or_default();
                    cell.left |= x > split.area.x;
                    cell.right |= x + 1 < end;
                    cell.up |= up;
                    cell.down |= down;
                }
            }
        }
    }
}

fn add_pane_border_cells(
    cells: &mut std::collections::HashMap<(u16, u16), LineCell>,
    info: &PaneInfo,
) {
    let rect = info.rect;
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let right = rect.x.saturating_add(rect.width).saturating_sub(1);
    let bottom = rect.y.saturating_add(rect.height).saturating_sub(1);

    if info.borders.contains(Borders::TOP) {
        for x in rect.x..=right {
            let cell = cells.entry((x, rect.y)).or_default();
            cell.left |= x > rect.x;
            cell.right |= x < right;
        }
    }
    if info.borders.contains(Borders::BOTTOM) {
        for x in rect.x..=right {
            let cell = cells.entry((x, bottom)).or_default();
            cell.left |= x > rect.x;
            cell.right |= x < right;
        }
    }
    if info.borders.contains(Borders::LEFT) {
        for y in rect.y..=bottom {
            let cell = cells.entry((rect.x, y)).or_default();
            cell.up |= y > rect.y;
            cell.down |= y < bottom;
        }
    }
    if info.borders.contains(Borders::RIGHT) {
        for y in rect.y..=bottom {
            let cell = cells.entry((right, y)).or_default();
            cell.up |= y > rect.y;
            cell.down |= y < bottom;
        }
    }
}

fn line_touches_pane(x: u16, y: u16, info: &PaneInfo, pane_gaps: bool) -> bool {
    let rect = info.rect;
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let right = rect.x.saturating_add(rect.width).saturating_sub(1);
    let bottom = rect.y.saturating_add(rect.height).saturating_sub(1);
    let in_rows = y >= rect.y && y <= bottom;
    let in_cols = x >= rect.x && x <= right;
    let own_border =
        (in_rows && (x == rect.x || x == right)) || (in_cols && (y == rect.y || y == bottom));

    if pane_gaps {
        return own_border;
    }

    let shared_right = rect.x.saturating_add(rect.width);
    let shared_bottom = rect.y.saturating_add(rect.height);
    own_border
        || (in_rows && x == shared_right)
        || (in_cols && y == shared_bottom)
        || (x == shared_right && y == shared_bottom)
}

fn render_pane_border_titles(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    pane_infos: &[PaneInfo],
    frame: &mut Frame,
) {
    let buf = frame.buffer_mut();
    let area = buf.area;
    for info in pane_infos {
        if !info.borders.contains(Borders::TOP) || info.rect.width <= 4 {
            continue;
        }
        let Some(title) = ws
            .pane_state(info.id)
            .and_then(|pane| app.terminals.get(&pane.attached_terminal_id))
            .and_then(|terminal| terminal.border_label(app.show_agent_labels_on_pane_borders))
            .and_then(|label| pane_border_title(&label, info.rect.width, info.is_focused))
        else {
            continue;
        };
        let y = info.rect.y;
        if y < area.y || y >= area.y.saturating_add(area.height) {
            continue;
        }
        let start_x = info.rect.x.saturating_add(1);
        let end_x = info
            .rect
            .x
            .saturating_add(info.rect.width)
            .saturating_sub(1)
            .min(area.x.saturating_add(area.width));
        if start_x >= end_x {
            continue;
        }
        let color = if info.is_focused {
            app.palette.accent
        } else {
            app.palette.overlay0
        };
        let mut style = Style::default().fg(color);
        if info.is_focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        buf.set_stringn(
            start_x,
            y,
            title,
            end_x.saturating_sub(start_x) as usize,
            style,
        );
    }
}

fn line_cell_symbol(line: LineCell) -> &'static str {
    match (line.up, line.down, line.left, line.right) {
        (true, true, true, true) => "┼",
        (true, true, true, false) => "┤",
        (true, true, false, true) => "├",
        (true, false, true, true) => "┴",
        (false, true, true, true) => "┬",
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => {
            "│"
        }
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            "─"
        }
        (false, true, false, true) => "┌",
        (false, true, true, false) => "┐",
        (true, false, false, true) => "└",
        (true, false, true, false) => "┘",
        _ => "",
    }
}

pub(crate) fn render_selection_highlight<P: PartialEq>(
    selection: Option<&crate::selection::Selection<P>>,
    buffer: &mut Buffer,
    pane_id: &P,
    inner: Rect,
    scroll_metrics: Option<crate::pane::ScrollMetrics>,
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) {
    let Some(selection) =
        selection.filter(|selection| selection.is_visible() && &selection.pane_id == pane_id)
    else {
        return;
    };
    let style = automatic_selection_style(p, host_theme);
    for y in 0..inner.height {
        for x in 0..inner.width {
            if selection.contains(y, x, scroll_metrics) {
                buffer[(inner.x + x, inner.y + y)].set_style(style);
            }
        }
    }
}

type Rgb = (u8, u8, u8);

fn automatic_selection_style(
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) -> Style {
    let bg = automatic_selection_bg(p, host_theme);
    Style::reset().fg(selection_fg_for_bg(bg, p)).bg(bg)
}

fn automatic_selection_bg(p: &Palette, host_theme: crate::terminal_theme::TerminalTheme) -> Color {
    let fallback = selection_palette_background(p);
    let Some(background) = host_theme
        .background
        .map(|color| (color.r, color.g, color.b))
        .or(match fallback {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        })
    else {
        return fallback;
    };

    let target = if relative_luminance(background) < 0.5 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    let selected = mix_rgb(background, target, 0.28);
    Color::Rgb(selected.0, selected.1, selected.2)
}

fn selection_palette_background(p: &Palette) -> Color {
    if p.panel_bg == Color::Reset {
        p.surface_dim
    } else {
        p.panel_bg
    }
}

fn selection_fg_for_bg(bg: Color, p: &Palette) -> Color {
    if let Color::Rgb(r, g, b) = bg {
        let luminance = relative_luminance((r, g, b));
        let black_contrast = (luminance + 0.05) / 0.05;
        let white_contrast = 1.05 / (luminance + 0.05);
        return if black_contrast > white_contrast {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        };
    }

    color_to_rgb(bg)
        .map(|bg| {
            if relative_luminance(bg) < 0.5 {
                Color::White
            } else {
                Color::Black
            }
        })
        .unwrap_or_else(|| panel_contrast_fg(p))
}

fn mix_rgb(base: Rgb, target: Rgb, amount: f32) -> Rgb {
    fn channel(base: u8, target: u8, amount: f32) -> u8 {
        (f32::from(base) + (f32::from(target) - f32::from(base)) * amount).round() as u8
    }
    (
        channel(base.0, target.0, amount),
        channel(base.1, target.1, amount),
        channel(base.2, target.2, amount),
    )
}

fn relative_luminance(color: Rgb) -> f32 {
    fn channel(value: u8) -> f32 {
        let value = f32::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
}

fn color_to_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Reset => None,
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((255, 255, 255)),
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PaneBordersConfig;
    use crate::layout::PaneId;
    use crate::selection::Selection;
    use crate::terminal::TerminalRuntime;
    use crate::terminal::TerminalState;
    use crate::workspace::Workspace;

    fn render_view_pane_borders(
        app: &AppState,
        ws: &Workspace,
        split_borders: &[crate::layout::SplitBorder],
        frame: &mut Frame,
    ) {
        let pane_gaps = PaneGaps::legacy(app.pane_gaps, app.pane_borders.draws_borders());
        render_pane_borders(
            app,
            ws,
            &app.view.pane_infos,
            split_borders,
            pane_gaps,
            frame,
        );
    }

    #[test]
    fn unavailable_pane_renders_restore_failure_without_a_runtime() {
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("unavailable")];
        app.active = Some(0);
        app.ensure_test_terminals();
        let pane_id = app.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.workspaces[0].terminal_id(pane_id).unwrap().clone();
        app.terminals.get_mut(&terminal_id).unwrap().restore_error =
            Some("Saved directory is unavailable. Restart to retry.".into());
        let runtimes = TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 80, 24);
        let layout = crate::ui::compute_tab_surface_for(
            &app,
            &runtimes,
            Some(crate::ui::TabSurfaceTarget {
                workspace_index: 0,
                tab_index: 0,
            }),
            area,
            false,
            Default::default(),
        );
        let (buffer, cursor, _, _) =
            crate::server::render_stream::render_tab_surface_virtual(&app, &runtimes, layout, area);
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Saved directory is unavailable."));
        assert!(cursor.is_none_or(|cursor| !cursor.visible));
    }

    #[test]
    fn pane_border_title_trims_and_truncates() {
        assert_eq!(
            pane_border_title(" claude ", 20, false).as_deref(),
            Some(" claude ")
        );
        assert_eq!(
            pane_border_title(" claude ", 20, true).as_deref(),
            Some(" claude ")
        );
        assert_eq!(pane_border_title("", 20, false), None);
        assert_eq!(
            pane_border_title("abcdef", 8, false).as_deref(),
            Some(" abc… ")
        );
        assert_eq!(
            pane_border_title("abcdef", 8, true).as_deref(),
            Some(" abc… ")
        );
        assert_eq!(pane_border_title("abcdef", 4, false), None);
    }

    #[test]
    fn pane_border_title_truncates_cjk_by_display_width() {
        let title = pane_border_title("1 模块组织（已定）", 12, false).unwrap();

        assert_eq!(title, " 1 模块… ");
        assert!(display_width(title.as_str()) <= 10);
    }

    #[test]
    fn pane_border_renderer_places_adjacent_cjk_by_display_width() {
        let mut app = AppState::test_new();
        app.view.terminal_area = Rect::new(0, 0, 12, 3);
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        app.view.pane_infos = vec![PaneInfo {
            id: pane_id,
            rect: Rect::new(0, 0, 12, 3),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: false,
        }];

        let terminal_id = ws.tabs[0].panes[&pane_id].attached_terminal_id.clone();
        let mut terminal_state = TerminalState::new(terminal_id.clone(), "/tmp".into());
        terminal_state.set_manual_label("1 模块组织（已定）".into());
        app.terminals.insert(terminal_id, terminal_state);

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(12, 3)).unwrap();
        terminal
            .draw(|frame| render_view_pane_borders(&app, &ws, &[], frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(4, 0)].symbol(), "模");
        assert_eq!(buffer[(5, 0)].symbol(), " ");
        assert_eq!(buffer[(6, 0)].symbol(), "块");
    }

    #[test]
    fn default_horizontal_split_uses_one_shared_divider_column() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Auto,
            false,
            true,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect.x + left.rect.width, right.rect.x);
        assert!(!left.borders.contains(Borders::RIGHT));
        assert!(right.borders.contains(Borders::LEFT));
    }

    #[test]
    fn default_vertical_split_uses_one_shared_divider_row() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Auto,
            false,
            true,
        );
        let top = infos.iter().find(|info| info.id == root).unwrap();
        let bottom = infos.iter().find(|info| info.id == bottom).unwrap();

        assert_eq!(top.rect.y + top.rect.height, bottom.rect.y);
        assert!(!top.borders.contains(Borders::BOTTOM));
        assert!(bottom.borders.contains(Borders::TOP));
    }

    #[test]
    fn disabled_outer_borders_keep_only_shared_pane_dividers() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Auto,
            false,
            false,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.borders, Borders::NONE);
        assert_eq!(right.borders, Borders::LEFT);
    }

    #[test]
    fn pane_gaps_keep_independent_bordered_panes() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Auto,
            true,
            true,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect.x + left.rect.width, right.rect.x);
        assert_eq!(left.borders, Borders::ALL);
        assert_eq!(right.borders, Borders::ALL);
    }

    #[test]
    fn borderless_pane_gaps_add_one_empty_cell_between_panes() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Off,
            true,
            true,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect, Rect::new(0, 0, 49, 20));
        assert_eq!(right.rect, Rect::new(50, 0, 50, 20));
        assert!(left.borders.is_empty());
        assert!(right.borders.is_empty());
    }

    #[test]
    fn disabled_pane_borders_make_inner_rect_equal_visual_rect() {
        let mut workspace = Workspace::test_new("test");
        workspace.test_split(ratatui::layout::Direction::Horizontal);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            PaneBordersConfig::Off,
            false,
            true,
        );

        for info in infos {
            assert!(info.borders.is_empty());
            assert_eq!(pane_inner_rect(info.rect, info.borders), info.rect);
        }
    }

    #[test]
    fn always_pane_borders_frame_lone_pane() {
        let workspace = Workspace::test_new("test");
        let area = Rect::new(0, 0, 100, 20);

        let default_infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(area),
            PaneBordersConfig::Auto,
            false,
            true,
        );
        assert_eq!(default_infos[0].borders, Borders::NONE);

        let framed_infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(area),
            PaneBordersConfig::Always,
            false,
            true,
        );
        assert_eq!(framed_infos[0].borders, Borders::ALL);

        let no_outer_infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(area),
            PaneBordersConfig::Always,
            false,
            false,
        );
        assert_eq!(no_outer_infos[0].borders, Borders::NONE);
    }

    #[test]
    fn global_pane_border_renderer_composes_junctions_and_focus_style() {
        let mut app = AppState::test_new();
        app.view.terminal_area = Rect::new(0, 0, 4, 4);
        app.view.pane_infos = vec![
            PaneInfo {
                id: PaneId::from_raw(1),
                rect: Rect::new(0, 0, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT,
                is_focused: true,
            },
            PaneInfo {
                id: PaneId::from_raw(2),
                rect: Rect::new(2, 0, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT | Borders::RIGHT,
                is_focused: false,
            },
            PaneInfo {
                id: PaneId::from_raw(3),
                rect: Rect::new(0, 2, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT | Borders::BOTTOM,
                is_focused: false,
            },
            PaneInfo {
                id: PaneId::from_raw(4),
                rect: Rect::new(2, 2, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: false,
            },
        ];
        let split_borders = vec![
            crate::layout::SplitBorder {
                pos: 2,
                direction: ratatui::layout::Direction::Horizontal,
                ratio: 0.5,
                area: Rect::new(0, 0, 4, 4),
                path: vec![],
            },
            crate::layout::SplitBorder {
                pos: 2,
                direction: ratatui::layout::Direction::Vertical,
                ratio: 0.5,
                area: Rect::new(0, 0, 4, 4),
                path: vec![false],
            },
        ];
        let ws = Workspace::test_new("test");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 4)).unwrap();

        terminal
            .draw(|frame| render_view_pane_borders(&app, &ws, &split_borders, frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 2)].symbol(), "┼");
        assert_eq!(buffer[(2, 2)].style().fg, Some(app.palette.accent));
        assert_eq!(buffer[(2, 1)].symbol(), "│");
        assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.accent));
    }

    #[test]
    fn gapped_pane_focus_does_not_color_neighbor_border() {
        let mut app = AppState::test_new();
        app.pane_gaps = true;
        app.view.terminal_area = Rect::new(0, 0, 4, 3);
        app.view.pane_infos = vec![
            PaneInfo {
                id: PaneId::from_raw(1),
                rect: Rect::new(0, 0, 2, 3),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: true,
            },
            PaneInfo {
                id: PaneId::from_raw(2),
                rect: Rect::new(2, 0, 2, 3),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: false,
            },
        ];
        let ws = Workspace::test_new("test");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 3)).unwrap();

        terminal
            .draw(|frame| render_view_pane_borders(&app, &ws, &[], frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 1)].style().fg, Some(app.palette.accent));
        assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.overlay0));
    }

    #[tokio::test]
    async fn pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[tokio::test]
    async fn alternate_screen_reclaims_scrollbar_gutter_and_restores_it_on_exit() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(
                40,
                8,
                1024,
                b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
            ),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let assert_geometry = |expected_width, has_scrollbar| {
            let infos = compute_pane_infos(
                &app,
                &terminal_runtimes,
                area,
                true,
                crate::kitty_graphics::HostCellSize::default(),
            );
            assert_eq!(
                infos[0].inner_rect,
                Rect::new(area.x, area.y, expected_width, area.height)
            );
            assert_eq!(infos[0].scrollbar_rect.is_some(), has_scrollbar);
            assert_eq!(
                app.workspaces[0].tabs[0].runtimes[&root_pane].current_size(),
                (area.height, expected_width)
            );
        };

        assert_geometry(39, true);
        app.workspaces[0].tabs[0].runtimes[&root_pane].test_process_pty_bytes(b"\x1b[?1049h");
        assert_geometry(40, false);
        app.workspaces[0].tabs[0].runtimes[&root_pane].test_process_pty_bytes(b"\x1b[?1049l");
        assert_geometry(39, true);
    }

    #[tokio::test]
    async fn zoomed_pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        workspace.zoomed = true;
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[tokio::test]
    async fn zoomed_multi_pane_keeps_border_space() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let focused_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.zoomed = true;
        workspace.tabs[0].runtimes.insert(
            focused_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.id, focused_pane);
        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(11, 4, 37, 6));
    }

    #[tokio::test]
    async fn tiny_pane_does_not_reserve_scrollbar_gutter() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(4, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 4, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, area);
    }

    #[tokio::test]
    async fn pane_scrollbar_setting_controls_reserved_column() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(
                40,
                8,
                1024,
                b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
            ),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, Some(Rect::new(49, 3, 1, 8)));
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));

        app.pane_scrollbars = false;
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, area);
    }

    #[test]
    fn selection_highlight_uses_one_uniform_style() {
        let palette = Palette::catppuccin();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: None,
            background: Some(crate::terminal_theme::RgbColor {
                r: 12,
                g: 14,
                b: 16,
            }),
            ..Default::default()
        };
        let expected_style = automatic_selection_style(&palette, host_theme);
        let selection = Some(Selection::absolute_range(
            PaneId::from_raw(1),
            (0, 0),
            (0, 2),
        ));
        let backend = ratatui::backend::TestBackend::new(4, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                buf[(0, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(10, 220, 120))
                        .bg(Color::Black),
                );
                buf[(1, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(220, 180, 40))
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
                buf[(2, 0)].set_style(Style::default().fg(Color::Blue).bg(Color::Reset));
                render_selection_highlight(
                    selection.as_ref(),
                    frame.buffer_mut(),
                    &PaneId::from_raw(1),
                    Rect::new(0, 0, 4, 1),
                    None,
                    &palette,
                    host_theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let first = buffer[(0, 0)].style();
        let second = buffer[(1, 0)].style();
        let third = buffer[(2, 0)].style();

        assert_eq!(first.fg, expected_style.fg);
        assert_eq!(second.fg, expected_style.fg);
        assert_eq!(third.fg, expected_style.fg);
        assert_eq!(first.bg, expected_style.bg);
        assert_eq!(second.bg, expected_style.bg);
        assert_eq!(third.bg, expected_style.bg);
        assert_eq!(first.add_modifier, expected_style.add_modifier);
        assert_eq!(second.add_modifier, expected_style.add_modifier);
        assert_eq!(third.add_modifier, expected_style.add_modifier);
        assert!(!second.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn automatic_selection_background_uses_host_background() {
        let bg = automatic_selection_bg(
            &Palette::terminal(),
            crate::terminal_theme::TerminalTheme {
                foreground: Some(crate::terminal_theme::RgbColor {
                    r: 230,
                    g: 230,
                    b: 230,
                }),
                background: Some(crate::terminal_theme::RgbColor {
                    r: 12,
                    g: 14,
                    b: 16,
                }),
                ..Default::default()
            },
        );

        let Color::Rgb(r, g, b) = bg else {
            panic!("selection background should resolve to rgb");
        };
        assert!(relative_luminance((r, g, b)) > relative_luminance((12, 14, 16)));
    }

    #[test]
    fn automatic_selection_rgb_style_is_readable_with_or_without_host_background() {
        for (background, selected_bg, selected_fg) in [
            ((239, 241, 245), (172, 174, 176), (0, 0, 0)),
            ((26, 27, 38), (90, 91, 99), (255, 255, 255)),
            ((45, 53, 59), (104, 110, 114), (255, 255, 255)),
        ] {
            let mut palette = Palette::catppuccin();
            let (r, g, b) = background;
            palette.panel_bg = Color::Rgb(r, g, b);
            let expected = Style::reset()
                .bg(Color::Rgb(selected_bg.0, selected_bg.1, selected_bg.2))
                .fg(Color::Rgb(selected_fg.0, selected_fg.1, selected_fg.2));

            assert_eq!(
                automatic_selection_style(&palette, Default::default()),
                expected
            );
            assert_eq!(
                automatic_selection_style(
                    &Palette::terminal(),
                    crate::terminal_theme::TerminalTheme {
                        background: Some(crate::terminal_theme::RgbColor { r, g, b }),
                        ..Default::default()
                    },
                ),
                expected
            );
        }
    }

    #[test]
    fn automatic_selection_preserves_symbolic_palette_fallbacks() {
        let mut palette = Palette::terminal();
        assert_eq!(
            automatic_selection_style(&palette, Default::default()),
            Style::reset().fg(Color::White).bg(Color::DarkGray)
        );
        for fallback in [Color::Blue, Color::White, Color::Indexed(42), Color::Reset] {
            palette.surface_dim = fallback;
            assert_eq!(
                automatic_selection_bg(&palette, Default::default()),
                fallback
            );
        }
    }

    /// `(A / D) | (B / C)`: two columns, each split into two rows.
    fn grid_workspace() -> (Workspace, [PaneId; 4]) {
        let mut workspace = Workspace::test_new("grid");
        let a = workspace.tabs[0].root_pane;
        let b = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let c = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(a);
        let d = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(a);
        (workspace, [a, b, c, d])
    }

    fn spacing_app(
        workspace: Workspace,
        pane_borders: PaneBordersConfig,
        pane_gaps: bool,
        pane_gap_cells: Option<u16>,
        pane_outer_borders: bool,
    ) -> AppState {
        let mut app = AppState::test_new();
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.pane_borders = pane_borders;
        app.pane_gaps = pane_gaps;
        app.pane_gap_cells = pane_gap_cells;
        app.pane_outer_borders = pane_outer_borders;
        app
    }

    type PaneGeometry = Vec<(PaneId, Rect, Rect, Borders)>;

    fn render_spacing(app: &AppState, area: Rect) -> (Buffer, PaneGeometry, PaneGaps) {
        let runtimes = TerminalRuntimeRegistry::new();
        let layout = crate::ui::compute_tab_surface_for(
            app,
            &runtimes,
            Some(crate::ui::TabSurfaceTarget {
                workspace_index: 0,
                tab_index: 0,
            }),
            area,
            false,
            Default::default(),
        );
        let (buffer, _, _, layout) =
            crate::server::render_stream::render_tab_surface_virtual(app, &runtimes, layout, area);
        let geometry = layout
            .pane_infos
            .iter()
            .map(|info| (info.id, info.rect, info.inner_rect, info.borders))
            .collect();
        (buffer, geometry, layout.pane_gaps)
    }

    fn rect_of(geometry: &PaneGeometry, id: PaneId) -> Rect {
        geometry.iter().find(|pane| pane.0 == id).unwrap().1
    }

    /// Geometry without pane ids, for comparing two separately built workspaces.
    fn strip(geometry: PaneGeometry) -> Vec<(Rect, Rect, Borders)> {
        geometry
            .into_iter()
            .map(|(_, rect, inner, borders)| (rect, inner, borders))
            .collect()
    }

    fn row_text(buffer: &Buffer, y: u16, xs: std::ops::Range<u16>) -> String {
        xs.map(|x| buffer[(x, y)].symbol()).collect()
    }

    fn column_text(buffer: &Buffer, x: u16, ys: std::ops::Range<u16>) -> String {
        ys.map(|y| buffer[(x, y)].symbol()).collect()
    }

    #[test]
    fn absent_pane_gap_cells_runs_the_legacy_path() {
        let area = Rect::new(0, 0, 80, 24);
        for pane_borders in [PaneBordersConfig::Auto, PaneBordersConfig::Off] {
            for pane_gaps in [true, false] {
                for outer in [true, false] {
                    let (workspace, _) = grid_workspace();
                    let app = spacing_app(workspace, pane_borders, pane_gaps, None, outer);
                    assert_eq!(app.pane_spacing(), PaneSpacing::Legacy(pane_gaps));
                    let panes = app.workspaces[0].tabs[0].layout.panes(area);
                    let legacy = apply_pane_chrome(panes.clone(), pane_borders, pane_gaps, outer);
                    let (spaced, gaps) =
                        apply_pane_spacing(panes, pane_borders, app.pane_spacing(), outer);
                    assert_eq!(
                        gaps,
                        PaneGaps::legacy(pane_gaps, pane_borders.draws_borders())
                    );
                    let shape = |infos: &[PaneInfo]| {
                        infos
                            .iter()
                            .map(|info| (info.id, info.rect, info.borders))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(shape(&spaced), shape(&legacy));
                }
            }
        }

        // Legacy separated frames touch: no blank cell between them.
        let (workspace, [a, b, ..]) = grid_workspace();
        let app = spacing_app(workspace, PaneBordersConfig::Auto, true, None, true);
        let (buffer, geometry, _) = render_spacing(&app, area);
        let a_rect = rect_of(&geometry, a);
        assert_eq!(a_rect.right(), rect_of(&geometry, b).x);
        assert_eq!(
            row_text(&buffer, 4, a_rect.right() - 1..a_rect.right() + 1),
            "││"
        );
    }

    #[test]
    fn pane_gap_cells_zero_matches_legacy_shared_dividers() {
        let area = Rect::new(0, 0, 80, 24);
        for pane_borders in [PaneBordersConfig::Auto, PaneBordersConfig::Off] {
            for outer in [true, false] {
                let legacy = spacing_app(grid_workspace().0, pane_borders, false, None, outer);
                // `pane_gaps = true` beside the override proves the override wins.
                let zero = spacing_app(grid_workspace().0, pane_borders, true, Some(0), outer);
                let (legacy_buffer, legacy_geometry, legacy_gaps) = render_spacing(&legacy, area);
                let (zero_buffer, zero_geometry, zero_gaps) = render_spacing(&zero, area);
                assert_eq!(zero_buffer, legacy_buffer, "{pane_borders:?} outer={outer}");
                assert_eq!(zero_gaps, legacy_gaps);
                assert_eq!(strip(zero_geometry), strip(legacy_geometry));
            }
        }
    }

    #[test]
    fn pane_gap_cells_one_against_legacy_pane_gaps() {
        let area = Rect::new(0, 0, 80, 24);

        // Frames off: one blank cell taken from the leading pane in both forms.
        let legacy = spacing_app(grid_workspace().0, PaneBordersConfig::Off, true, None, true);
        let one = spacing_app(
            grid_workspace().0,
            PaneBordersConfig::Off,
            false,
            Some(1),
            true,
        );
        let (legacy_buffer, legacy_geometry, legacy_gaps) = render_spacing(&legacy, area);
        let (one_buffer, one_geometry, one_gaps) = render_spacing(&one, area);
        assert_eq!(one_buffer, legacy_buffer);
        assert_eq!(one_gaps, legacy_gaps);
        assert_eq!(strip(one_geometry), strip(legacy_geometry));

        // Frames on: legacy frames touch, one cell leaves a blank column/row between them.
        let (workspace, [legacy_a, ..]) = grid_workspace();
        let legacy = spacing_app(workspace, PaneBordersConfig::Auto, true, None, true);
        let (workspace, [a, b, c, d]) = grid_workspace();
        let one = spacing_app(workspace, PaneBordersConfig::Auto, false, Some(1), true);
        let (legacy_buffer, legacy_geometry, _) = render_spacing(&legacy, area);
        let (one_buffer, one_geometry, one_gaps) = render_spacing(&one, area);
        assert_eq!(
            one_gaps,
            PaneGaps {
                separate: true,
                blank: 1
            }
        );
        assert_eq!(rect_of(&legacy_geometry, legacy_a), Rect::new(0, 0, 40, 12));
        assert_eq!(rect_of(&one_geometry, a), Rect::new(0, 0, 39, 11));
        assert_eq!(rect_of(&one_geometry, b), Rect::new(40, 0, 40, 11));
        assert_eq!(rect_of(&one_geometry, d), Rect::new(0, 12, 39, 12));
        // The trailing pane of both boundaries keeps its baseline rect.
        assert_eq!(rect_of(&one_geometry, c), Rect::new(40, 12, 40, 12));
        assert_eq!(row_text(&legacy_buffer, 4, 39..41), "││");
        assert_eq!(row_text(&one_buffer, 4, 38..41), "│ │");
        assert_eq!(column_text(&legacy_buffer, 4, 11..13), "──");
        assert_eq!(column_text(&one_buffer, 4, 10..13), "─ ─");
        assert_ne!(one_buffer, legacy_buffer);
    }

    #[test]
    fn pane_gap_cells_leave_n_blank_cells_between_frames() {
        let area = Rect::new(0, 0, 80, 24);
        let (workspace, [a, b, c, d]) = grid_workspace();
        let app = spacing_app(workspace, PaneBordersConfig::Auto, true, Some(3), true);
        let (buffer, geometry, gaps) = render_spacing(&app, area);

        assert_eq!(
            gaps,
            PaneGaps {
                separate: true,
                blank: 3
            }
        );
        // Baseline split rounding is kept; the leading pane of each boundary gives up N cells.
        assert_eq!(rect_of(&geometry, a), Rect::new(0, 0, 37, 9));
        assert_eq!(rect_of(&geometry, d), Rect::new(0, 12, 37, 12));
        assert_eq!(rect_of(&geometry, b), Rect::new(40, 0, 40, 9));
        assert_eq!(rect_of(&geometry, c), Rect::new(40, 12, 40, 12));
        for (_, rect, inner, borders) in &geometry {
            assert_eq!(*borders, Borders::ALL);
            assert_eq!(*inner, pane_inner_rect(*rect, Borders::ALL));
        }

        // N blank cells between opposing frame edges, both frames kept.
        assert_eq!(row_text(&buffer, 4, 36..41), "│   │");
        assert_eq!(row_text(&buffer, 15, 36..41), "│   │");
        assert_eq!(column_text(&buffer, 4, 8..13), "─   ─");
        assert_eq!(column_text(&buffer, 60, 8..13), "─   ─");
        assert_eq!(row_text(&buffer, 10, 0..80).trim(), "");
        // No outer margin: the outer frame still sits on the area edge.
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(buffer[(79, 0)].symbol(), "┐");
        assert_eq!(buffer[(0, 23)].symbol(), "└");
        assert_eq!(buffer[(79, 23)].symbol(), "┘");
    }

    #[test]
    fn pane_gap_cells_without_frames_leave_n_blank_cells() {
        let area = Rect::new(0, 0, 80, 24);
        let (workspace, [a, b, c, d]) = grid_workspace();
        let app = spacing_app(workspace, PaneBordersConfig::Off, false, Some(3), true);
        let (buffer, geometry, gaps) = render_spacing(&app, area);

        assert_eq!(gaps.blank, 3);
        assert_eq!(rect_of(&geometry, a), Rect::new(0, 0, 37, 9));
        assert_eq!(rect_of(&geometry, d), Rect::new(0, 12, 37, 12));
        assert_eq!(rect_of(&geometry, b), Rect::new(40, 0, 40, 9));
        assert_eq!(rect_of(&geometry, c), Rect::new(40, 12, 40, 12));
        for (_, rect, inner, borders) in &geometry {
            assert!(borders.is_empty());
            assert_eq!(inner, rect);
        }
        assert!(buffer.content.iter().all(|cell| cell.symbol() == " "));
    }

    #[test]
    fn pane_gap_cells_shrink_only_as_needed_in_small_areas() {
        let two_columns = || {
            let mut workspace = Workspace::test_new("small");
            let left = workspace.tabs[0].root_pane;
            workspace.test_split(ratatui::layout::Direction::Horizontal);
            workspace.tabs[0].layout.focus_pane(left);
            (workspace, left)
        };
        let (workspace, left) = two_columns();
        let app = spacing_app(workspace, PaneBordersConfig::Auto, true, Some(10), true);
        let shared = spacing_app(two_columns().0, PaneBordersConfig::Auto, false, None, true);

        // 20 columns: the leading pane keeps exactly one usable column.
        let (_, geometry, gaps) = render_spacing(&app, Rect::new(0, 0, 20, 8));
        assert_eq!(gaps.blank, 7);
        let inner = geometry.iter().find(|pane| pane.0 == left).unwrap().2;
        assert_eq!(inner.width, 1);

        // 4 columns: no gap fits, so the shared-divider baseline is used unchanged.
        let tiny = Rect::new(0, 0, 4, 8);
        let (tiny_buffer, tiny_geometry, tiny_gaps) = render_spacing(&app, tiny);
        let (shared_buffer, shared_geometry, shared_gaps) = render_spacing(&shared, tiny);
        assert_eq!(tiny_gaps, shared_gaps);
        assert_eq!(tiny_buffer, shared_buffer);
        assert_eq!(strip(tiny_geometry), strip(shared_geometry));

        // 2 columns: panes already unusable at baseline do not force the fallback,
        // and no rect leaves the area or wraps.
        let degenerate = Rect::new(0, 0, 2, 8);
        let (_, geometry, _) = render_spacing(&app, degenerate);
        for (_, rect, inner, _) in &geometry {
            assert!(rect.right() <= degenerate.right() && rect.bottom() <= degenerate.bottom());
            assert!(inner.width <= rect.width && inner.height <= rect.height);
        }

        // Re-expanding restores the original layout; N stays configured throughout.
        let area = Rect::new(0, 0, 80, 24);
        let (before_buffer, before, _) = render_spacing(&app, area);
        render_spacing(&app, tiny);
        let (after_buffer, after, after_gaps) = render_spacing(&app, area);
        assert_eq!(after, before);
        assert_eq!(after_buffer, before_buffer);
        assert_eq!(after_gaps.blank, 10);
        assert_eq!(app.pane_gap_cells, Some(10));
    }

    #[test]
    fn pane_gap_cells_do_not_apply_to_a_zoomed_tab() {
        let area = Rect::new(0, 0, 80, 24);
        let zoomed = || {
            let (mut workspace, [a, ..]) = grid_workspace();
            workspace.tabs[0].zoomed = true;
            (workspace, a)
        };
        let (workspace, a) = zoomed();
        let spaced = spacing_app(workspace, PaneBordersConfig::Auto, true, Some(5), true);
        let legacy = spacing_app(zoomed().0, PaneBordersConfig::Auto, true, None, true);
        let (spaced_buffer, spaced_geometry, _) = render_spacing(&spaced, area);
        let (legacy_buffer, legacy_geometry, _) = render_spacing(&legacy, area);
        assert_eq!(spaced_buffer, legacy_buffer);
        assert_eq!(spaced_geometry.len(), 1);
        assert_eq!(rect_of(&spaced_geometry, a), area);
        assert_eq!(strip(spaced_geometry), strip(legacy_geometry));
    }
}
