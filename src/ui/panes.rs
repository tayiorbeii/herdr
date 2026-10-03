use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::scrollbar::{render_pane_scrollbar, should_show_scrollbar};
use super::sidebar::{
    apply_token_style, sidebar_agent_row, sidebar_token_separator, AgentTokenContext, ResolvedToken,
};
#[cfg(test)]
use super::text::display_width;
use super::text::truncate_end;
use super::widgets::panel_contrast_fg;
use crate::app::state::Palette;
use crate::app::AppState;
use crate::config::AgentSidebarToken;
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

enum PaneBorderTitle {
    Plain(String),
    Composed(Vec<ratatui::text::Span<'static>>),
}

fn pane_border_title_base_style(palette: &Palette, focused: bool) -> Style {
    let color = if focused {
        palette.accent
    } else {
        palette.overlay0
    };
    let mut style = Style::default().fg(color);
    if focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

// Title source order: opt-in manual label, then the configured token row, then the
// baseline effective title -> manual label -> gated identity chain.
fn pane_border_title_for(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    info: &PaneInfo,
) -> Option<PaneBorderTitle> {
    let terminal = ws
        .pane_state(info.id)
        .and_then(|pane| app.terminals.get(&pane.attached_terminal_id))?;
    if app.pane_manual_label_first {
        if let Some(label) = terminal
            .manual_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
        {
            return pane_border_title(label, info.rect.width, info.is_focused)
                .map(PaneBorderTitle::Plain);
        }
    }
    if let Some(row) = app.pane_title_tokens.as_deref() {
        if let Some(spans) = composed_pane_border_title(app, ws, info, terminal, row) {
            return Some(PaneBorderTitle::Composed(spans));
        }
    }
    terminal
        .border_label(app.show_agent_labels_on_pane_borders)
        .and_then(|label| pane_border_title(&label, info.rect.width, info.is_focused))
        .map(PaneBorderTitle::Plain)
}

fn row_uses(row: &[AgentSidebarToken], token: AgentSidebarToken) -> bool {
    row.iter().any(|configured| *configured.parts().0 == token)
}

fn composed_pane_border_title(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    info: &PaneInfo,
    terminal: &crate::terminal::TerminalState,
    row: &[AgentSidebarToken],
) -> Option<Vec<ratatui::text::Span<'static>>> {
    if info.rect.width <= 4 {
        return None;
    }
    let resolved = resolve_pane_tokens(app, ws, info, terminal, row);
    compose_title_spans(
        &resolved,
        pane_border_title_base_style(&app.palette, info.is_focused),
        info.is_focused,
        info.rect.width.saturating_sub(4) as usize,
    )
}

// Resolves a configured row with the sidebar resolver against server-side pane data and keeps
// tokens with visible text. Status and machine tokens are client presentation, so they resolve
// as missing here.
fn resolve_pane_tokens(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    info: &PaneInfo,
    terminal: &crate::terminal::TerminalState,
    row: &[AgentSidebarToken],
) -> Vec<ResolvedToken> {
    let workspace = if row_uses(row, AgentSidebarToken::Workspace) {
        ws.display_name_from_terminals(&app.terminals)
    } else {
        String::new()
    };
    let tab = row_uses(row, AgentSidebarToken::Tab)
        .then(|| ws.find_tab_index_for_pane(info.id))
        .flatten()
        .filter(|tab_idx| ws.tabs.len() > 1 || !ws.tabs[*tab_idx].is_auto_named())
        .and_then(|tab_idx| ws.tab_display_name(tab_idx));
    let title = terminal.effective_title();
    let pane = row_uses(row, AgentSidebarToken::Pane)
        .then(|| title.clone().or_else(|| terminal.manual_label.clone()))
        .flatten();
    let agent = row_uses(row, AgentSidebarToken::Agent)
        .then(|| {
            terminal
                .effective_display_agent()
                .or_else(|| terminal.agent_name.clone())
                .or_else(|| terminal.effective_agent_label().map(str::to_string))
                .or_else(|| title.clone())
        })
        .flatten();
    let terminal_title_stripped = row_uses(row, AgentSidebarToken::TerminalTitleStripped)
        .then(|| terminal.terminal_title_stripped())
        .flatten();
    let context = AgentTokenContext {
        machine: None,
        workspace: &workspace,
        tab: tab.as_deref(),
        pane: pane.as_deref(),
        agent_label: agent.as_deref(),
        terminal_title: terminal.terminal_title.as_deref(),
        terminal_title_stripped: terminal_title_stripped.as_deref(),
        canonical_agent: None,
        tokens: &terminal.metadata_tokens,
    };
    sidebar_agent_row(row, &context, "")
        .into_iter()
        .filter(|token| {
            token
                .kind
                .text_value()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .collect()
}

// Unfocused pane frame colour. Every inactive line fallback goes through here.
fn pane_border_inactive_color(palette: &Palette) -> Color {
    palette.overlay0
}

// An unfocused pane's identity line colour: the configured token's explicit fg when the token
// resolves to visible text. Focused panes skip resolution because focus owns their cells.
fn pane_border_identity_color(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    info: &PaneInfo,
    token: &AgentSidebarToken,
) -> Option<Color> {
    if info.is_focused {
        return None;
    }
    let terminal = ws
        .pane_state(info.id)
        .and_then(|pane| app.terminals.get(&pane.attached_terminal_id))?;
    resolve_pane_tokens(app, ws, info, terminal, std::slice::from_ref(token))
        .first()?
        .style
        .fg
        .map(|fg| fg.ratatui())
}

// Line colour from one walk over the cell's owners (the panes `line_touches_pane` accepts):
// any focused owner wins with accent; otherwise the cell takes an identity colour only when
// every owner has one and they agree, else the inactive fallback.
fn identity_line_color(
    palette: &Palette,
    pane_infos: &[PaneInfo],
    identities: &[Option<Color>],
    x: u16,
    y: u16,
    pane_gaps: bool,
) -> Color {
    let mut agreed = None;
    let mut neutral = false;
    for (info, identity) in pane_infos.iter().zip(identities) {
        if !line_touches_pane(x, y, info, pane_gaps) {
            continue;
        }
        if info.is_focused {
            return palette.accent;
        }
        // Keep walking after a disagreement: a later owner may still be focused.
        match (*identity, agreed) {
            (Some(color), Some(agreed)) if color != agreed => neutral = true,
            (Some(color), _) => agreed = Some(color),
            (None, _) => neutral = true,
        }
    }
    match agreed {
        Some(color) if !neutral => color,
        _ => pane_border_inactive_color(palette),
    }
}

fn compose_title_spans(
    resolved: &[ResolvedToken],
    base: Style,
    focused: bool,
    max_width: usize,
) -> Option<Vec<ratatui::text::Span<'static>>> {
    use ratatui::text::Span;
    let mut content = Vec::new();
    for (index, token) in resolved.iter().enumerate() {
        let Some(text) = token.kind.text_value().map(str::trim) else {
            continue;
        };
        if index > 0 {
            content.push(Span::styled(
                sidebar_token_separator(&resolved[index - 1], token),
                base,
            ));
        }
        // Focus adds BOLD on top of token styles; token fg replaces the fallback fg.
        let mut style = apply_token_style(base.remove_modifier(Modifier::BOLD), token.style);
        if focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        content.push(Span::styled(text.to_string(), style));
    }
    if content.is_empty() || max_width == 0 {
        return None;
    }
    let mut spans = vec![Span::styled(" ", base)];
    spans.extend(clip_title_spans(content, max_width));
    spans.push(Span::styled(" ", base));
    Some(spans)
}

fn grapheme_width(grapheme: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(grapheme)
}

// End-clips styled spans by display width with the existing "…" marker, cutting only at
// grapheme-cluster boundaries.
fn clip_title_spans(
    spans: Vec<ratatui::text::Span<'static>>,
    max_width: usize,
) -> Vec<ratatui::text::Span<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    let total = spans
        .iter()
        .flat_map(|span| span.content.graphemes(true))
        .map(grapheme_width)
        .sum::<usize>();
    if total <= max_width {
        return spans;
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0usize;
    let mut clipped = Vec::new();
    for span in spans {
        let mut kept = String::new();
        for grapheme in span.content.graphemes(true) {
            let width = grapheme_width(grapheme);
            if used + width > budget {
                kept.push('…');
                clipped.push(ratatui::text::Span::styled(kept, span.style));
                return clipped;
            }
            kept.push_str(grapheme);
            used += width;
        }
        clipped.push(ratatui::text::Span::styled(kept, span.style));
    }
    clipped
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

fn shrink_for_one_cell_gap(size: u16) -> u16 {
    if size > 1 {
        size - 1
    } else {
        size
    }
}

pub(crate) fn apply_pane_chrome(
    panes: Vec<PaneInfo>,
    pane_borders: crate::config::PaneBordersConfig,
    pane_gaps: bool,
    pane_outer_borders: bool,
) -> Vec<PaneInfo> {
    let multi_pane = panes.len() > 1;
    let bordered = pane_borders.shows_borders(multi_pane);
    let outer_left = panes.iter().map(|info| info.rect.x).min().unwrap_or(0);
    let outer_top = panes.iter().map(|info| info.rect.y).min().unwrap_or(0);
    let outer_right = panes
        .iter()
        .map(|info| info.rect.x.saturating_add(info.rect.width))
        .max()
        .unwrap_or(0);
    let outer_bottom = panes
        .iter()
        .map(|info| info.rect.y.saturating_add(info.rect.height))
        .max()
        .unwrap_or(0);
    panes
        .iter()
        .cloned()
        .map(|mut info| {
            let right_neighbor = multi_pane.then(|| pane_to_right(&info, &panes)).flatten();
            let below_neighbor = multi_pane.then(|| pane_below(&info, &panes)).flatten();

            if multi_pane && pane_gaps && !pane_borders.draws_borders() {
                if right_neighbor.is_some() {
                    info.rect.width = shrink_for_one_cell_gap(info.rect.width);
                }
                if below_neighbor.is_some() {
                    info.rect.height = shrink_for_one_cell_gap(info.rect.height);
                }
            }

            info.borders = if !bordered {
                Borders::NONE
            } else {
                let mut borders = Borders::ALL;
                if !pane_gaps {
                    if right_neighbor.is_some() {
                        borders.remove(Borders::RIGHT);
                    }
                    if below_neighbor.is_some() {
                        borders.remove(Borders::BOTTOM);
                    }
                }
                if !pane_outer_borders {
                    if info.rect.x == outer_left {
                        borders.remove(Borders::LEFT);
                    }
                    if info.rect.y == outer_top {
                        borders.remove(Borders::TOP);
                    }
                    if info.rect.x.saturating_add(info.rect.width) == outer_right {
                        borders.remove(Borders::RIGHT);
                    }
                    if info.rect.y.saturating_add(info.rect.height) == outer_bottom {
                        borders.remove(Borders::BOTTOM);
                    }
                }
                borders
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

    for info in apply_pane_chrome(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_gaps,
        app.pane_outer_borders,
    ) {
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
) -> Vec<PaneInfo> {
    let Some(tab) = app
        .workspaces
        .get(ws_idx)
        .and_then(|workspace| workspace.tabs.get(tab_idx))
    else {
        return Vec::new();
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
        return vec![PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect,
            scrollbar_rect,
            borders,
            is_focused: true,
        }];
    }

    let mut pane_infos = apply_pane_chrome(
        tab.layout.panes(area),
        app.pane_borders,
        app.pane_gaps,
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

    pane_infos
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
}

pub(super) fn render_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    target: Option<super::tab_surface::TabSurfaceTarget>,
    pane_infos: &[PaneInfo],
    split_borders: &[crate::layout::SplitBorder],
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

    render_pane_borders(app, ws, pane_infos, split_borders, frame);
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
    frame: &mut Frame,
) {
    if !app.pane_borders.draws_borders() || pane_infos.iter().all(|info| info.borders.is_empty()) {
        return;
    }

    let mut cells = std::collections::HashMap::<(u16, u16), LineCell>::new();
    for info in pane_infos {
        add_pane_border_cells(&mut cells, info);
    }
    add_split_border_cells(app.pane_gaps, split_borders, &mut cells);

    // Resolved once per pane per render; absent config keeps the plain focus/inactive choice.
    let identities = app.pane_border_identity_token.as_ref().map(|token| {
        pane_infos
            .iter()
            .map(|info| pane_border_identity_color(app, ws, info, token))
            .collect::<Vec<_>>()
    });
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
        let symbol = line_cell_symbol(line);
        if symbol.is_empty() {
            continue;
        }
        let color = match identities.as_deref() {
            Some(identities) => {
                identity_line_color(&app.palette, pane_infos, identities, x, y, app.pane_gaps)
            }
            None => {
                let focused = pane_infos
                    .iter()
                    .any(|info| info.is_focused && line_touches_pane(x, y, info, app.pane_gaps));
                if focused {
                    app.palette.accent
                } else {
                    pane_border_inactive_color(&app.palette)
                }
            }
        };
        let cell = &mut buf[(x, y)];
        cell.set_symbol(symbol);
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
        let Some(title) = pane_border_title_for(app, ws, info) else {
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
        let style = pane_border_title_base_style(&app.palette, info.is_focused);
        match title {
            PaneBorderTitle::Plain(title) => {
                buf.set_stringn(
                    start_x,
                    y,
                    title,
                    end_x.saturating_sub(start_x) as usize,
                    style,
                );
            }
            PaneBorderTitle::Composed(spans) => {
                let mut x = start_x;
                for span in spans {
                    if x >= end_x {
                        break;
                    }
                    (x, _) = buf.set_stringn(
                        x,
                        y,
                        &span.content,
                        end_x.saturating_sub(x) as usize,
                        span.style,
                    );
                }
            }
        }
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
        render_pane_borders(app, ws, &app.view.pane_infos, split_borders, frame);
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

    fn title_row(row: &str) -> Vec<AgentSidebarToken> {
        #[derive(serde::Deserialize)]
        struct Row {
            row: Vec<AgentSidebarToken>,
        }
        toml::from_str::<Row>(&format!("row = {row}")).unwrap().row
    }

    fn title_test_app(split: bool) -> (AppState, Vec<PaneId>) {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("repo");
        let root = workspace.tabs[0].root_pane;
        let mut panes = vec![root];
        if split {
            panes.push(workspace.test_split(ratatui::layout::Direction::Horizontal));
            workspace.tabs[0].layout.focus_pane(root);
        }
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.ensure_test_terminals();
        (app, panes)
    }

    fn title_terminal(app: &mut AppState, pane: PaneId) -> &mut TerminalState {
        let terminal_id = app.workspaces[0].terminal_id(pane).unwrap().clone();
        app.terminals.get_mut(&terminal_id).unwrap()
    }

    fn report_title(terminal: &mut TerminalState, title: &str) {
        terminal.set_detected_state(
            Some(crate::detect::Agent::Claude),
            crate::detect::AgentState::Idle,
        );
        terminal.set_agent_metadata(crate::terminal::AgentMetadataReport {
            source: "user:presentation".into(),
            agent_label: Some("claude".into()),
            applies_to_source: None,
            title: Some(title.into()),
            display_agent: None,
            state_labels: std::collections::HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            ttl: None,
            seq: None,
        });
    }

    fn set_tokens(terminal: &mut TerminalState, tokens: &[(&str, &str)]) {
        terminal.metadata_tokens.patch(
            tokens
                .iter()
                .map(|(key, value)| ((*key).to_string(), Some((*value).to_string())))
                .collect(),
            None,
            std::time::Instant::now(),
        );
    }

    fn render_title_tab(app: &AppState, area: Rect) -> Buffer {
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
        crate::server::render_stream::render_tab_surface_virtual(app, &runtimes, layout, area).0
    }

    fn title_pane_rect(app: &AppState, area: Rect, pane: PaneId) -> Rect {
        let runtimes = TerminalRuntimeRegistry::new();
        crate::ui::compute_tab_surface_for(
            app,
            &runtimes,
            Some(crate::ui::TabSurfaceTarget {
                workspace_index: 0,
                tab_index: 0,
            }),
            area,
            false,
            Default::default(),
        )
        .pane_infos
        .into_iter()
        .find(|info| info.id == pane)
        .unwrap()
        .rect
    }

    fn row_text(buffer: &Buffer, y: u16, x: std::ops::Range<u16>) -> String {
        x.map(|x| buffer[(x, y)].symbol()).collect()
    }

    fn assert_cells_style(buffer: &Buffer, y: u16, x: std::ops::Range<u16>, style: Style) {
        for x in x {
            let cell = &buffer[(x, y)];
            assert_eq!(cell.fg, style.fg.unwrap(), "fg at ({x}, {y})");
            assert_eq!(cell.modifier, style.add_modifier, "modifier at ({x}, {y})");
        }
    }

    #[test]
    fn pane_title_tokens_absent_renders_baseline_buffer() {
        let (mut app, panes) = title_test_app(true);
        report_title(title_terminal(&mut app, panes[0]), "Prompt title");
        title_terminal(&mut app, panes[0]).set_manual_label("manual".into());
        set_tokens(title_terminal(&mut app, panes[0]), &[("task", "deploy")]);
        let area = Rect::new(0, 0, 60, 6);
        let baseline = render_title_tab(&app, area);

        // Characterize the protected plain path: effective title first, accent + BOLD on focus.
        assert_eq!(row_text(&baseline, 0, 1..15), " Prompt title ");
        assert_cells_style(
            &baseline,
            0,
            1..15,
            Style::default()
                .fg(app.palette.accent)
                .add_modifier(Modifier::BOLD),
        );

        // A row whose tokens are all missing falls back to the identical plain buffer.
        app.pane_title_tokens = Some(title_row(r#"["$missing", "state_icon", "machine"]"#));
        assert_eq!(render_title_tab(&app, area), baseline);
    }

    #[test]
    fn pane_title_default_precedence_keeps_reported_title_over_manual_label() {
        let (mut app, panes) = title_test_app(true);
        report_title(title_terminal(&mut app, panes[0]), "Prompt title");
        title_terminal(&mut app, panes[0]).set_manual_label("manual".into());
        let area = Rect::new(0, 0, 60, 6);

        assert_eq!(
            row_text(&render_title_tab(&app, area), 0, 1..15),
            " Prompt title "
        );
    }

    #[test]
    fn pane_manual_label_first_prefers_manual_label_over_tokens_and_title() {
        let (mut app, panes) = title_test_app(true);
        report_title(title_terminal(&mut app, panes[0]), "Prompt title");
        title_terminal(&mut app, panes[0]).set_manual_label("manual".into());
        set_tokens(title_terminal(&mut app, panes[0]), &[("task", "deploy")]);
        app.pane_title_tokens = Some(title_row(r#"["$task"]"#));
        let area = Rect::new(0, 0, 60, 6);

        assert_eq!(row_text(&render_title_tab(&app, area), 0, 1..9), " deploy ");

        app.pane_manual_label_first = true;
        let buffer = render_title_tab(&app, area);
        assert_eq!(row_text(&buffer, 0, 1..9), " manual ");

        app.pane_title_tokens = None;
        assert_eq!(render_title_tab(&app, area), buffer);

        // A blank manual label does not win.
        title_terminal(&mut app, panes[0]).set_manual_label("   ".into());
        app.pane_title_tokens = Some(title_row(r#"["$task"]"#));
        assert_eq!(row_text(&render_title_tab(&app, area), 0, 1..9), " deploy ");
    }

    #[test]
    fn pane_title_tokens_missing_fall_back_to_gated_identity() {
        let (mut app, panes) = title_test_app(true);
        title_terminal(&mut app, panes[0]).set_detected_state(
            Some(crate::detect::Agent::Claude),
            crate::detect::AgentState::Idle,
        );
        let area = Rect::new(0, 0, 60, 6);

        for gate in [false, true] {
            app.show_agent_labels_on_pane_borders = gate;
            app.pane_title_tokens = None;
            let baseline = render_title_tab(&app, area);
            assert_eq!(row_text(&baseline, 0, 1..9) == " claude ", gate);

            app.pane_title_tokens = Some(title_row(r#"["$missing"]"#));
            assert_eq!(render_title_tab(&app, area), baseline);

            // An explicit agent token is not gated.
            app.pane_title_tokens = Some(title_row(r#"["agent"]"#));
            assert_eq!(row_text(&render_title_tab(&app, area), 0, 1..9), " claude ");
        }
    }

    #[test]
    fn pane_title_tokens_compose_builtin_and_custom_with_token_styles() {
        let (mut app, panes) = title_test_app(true);
        for pane in &panes {
            let terminal = title_terminal(&mut app, *pane);
            set_tokens(terminal, &[("task", "fix")]);
            terminal.set_manual_label("api".into());
        }
        app.pane_title_tokens = Some(title_row(
            r##"["workspace", "$absent", { token = "$task", fg = "#f38ba8", dim = true }, { token = "pane", bold = false }]"##,
        ));
        let area = Rect::new(0, 0, 60, 6);
        let buffer = render_title_tab(&app, area);
        let right = title_pane_rect(&app, area, panes[1]);

        let expected = " repo · fix · api ";
        let width = expected.chars().count() as u16;
        assert_eq!(row_text(&buffer, 0, 1..1 + width), expected);
        let rx = right.x + 1;
        assert_eq!(row_text(&buffer, 0, rx..rx + width), expected);

        let token_fg = Color::Rgb(0xf3, 0x8b, 0xa8);
        for (x0, focused, fallback) in [
            (1, true, app.palette.accent),
            (rx, false, app.palette.overlay0),
        ] {
            let bold = if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            };
            // padding + workspace + separator use the fallback colour.
            assert_cells_style(
                &buffer,
                0,
                x0..x0 + 8,
                Style::default().fg(fallback).add_modifier(bold),
            );
            // token fg wins and keeps dim; focus adds BOLD on top.
            assert_cells_style(
                &buffer,
                0,
                x0 + 8..x0 + 11,
                Style::default()
                    .fg(token_fg)
                    .add_modifier(Modifier::DIM | bold),
            );
            assert_cells_style(
                &buffer,
                0,
                x0 + 11..x0 + 14,
                Style::default().fg(fallback).add_modifier(bold),
            );
            // bold = false holds only while unfocused.
            assert_cells_style(
                &buffer,
                0,
                x0 + 14..x0 + 18,
                Style::default().fg(fallback).add_modifier(bold),
            );
        }
    }

    #[test]
    fn pane_title_tokens_clip_by_display_width_without_splitting_graphemes() {
        let (mut app, panes) = title_test_app(true);
        set_tokens(
            title_terminal(&mut app, panes[0]),
            &[("a", "abcdef"), ("b", "XYZ")],
        );
        set_tokens(title_terminal(&mut app, panes[1]), &[("a", "模块组织x")]);
        app.pane_title_tokens = Some(title_row(r#"["$a", "$b"]"#));
        // Two 12-column panes: an 8-column title budget each.
        let area = Rect::new(0, 0, 23, 4);
        let buffer = render_title_tab(&app, area);

        assert_eq!(row_text(&buffer, 0, 1..11), " abcdef … ");
        let right = title_pane_rect(&app, area, panes[1]);
        let x = right.x + 1;
        assert_eq!(buffer[(x + 1, 0)].symbol(), "模");
        assert_eq!(buffer[(x + 3, 0)].symbol(), "块");
        assert_eq!(buffer[(x + 5, 0)].symbol(), "组");
        assert_eq!(buffer[(x + 7, 0)].symbol(), "…");
        assert_eq!(buffer[(x + 8, 0)].symbol(), " ");
    }

    #[test]
    fn compose_title_spans_clip_grapheme_clusters() {
        use ratatui::text::Span;
        let family = "👨\u{200d}👩\u{200d}👧";
        let clipped = clip_title_spans(vec![Span::raw(format!("ab{family}cd"))], 4);
        assert_eq!(
            clipped
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "ab…"
        );

        let clipped = clip_title_spans(vec![Span::raw("a🇺🇸b")], 3);
        assert_eq!(
            clipped
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "a…"
        );

        let clipped = clip_title_spans(vec![Span::raw("e\u{301}xyz")], 3);
        assert_eq!(
            clipped
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "e\u{301}x…"
        );

        let fits = vec![Span::raw("ab"), Span::raw("cd")];
        assert_eq!(clip_title_spans(fits.clone(), 4), fits);
        let clipped = clip_title_spans(vec![Span::raw("abcdef")], 1);
        assert_eq!(
            clipped
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "…"
        );
    }

    #[test]
    fn pane_title_tokens_render_on_single_pane_with_always_borders() {
        let (mut app, panes) = title_test_app(false);
        set_tokens(title_terminal(&mut app, panes[0]), &[("task", "solo")]);
        app.pane_title_tokens = Some(title_row(r#"["$task"]"#));
        let area = Rect::new(0, 0, 30, 5);

        // auto visibility is unchanged: no frame and no title for a lone pane.
        let auto = render_title_tab(&app, area);
        assert!(!row_text(&auto, 0, 0..30).contains("solo"));

        app.pane_borders = PaneBordersConfig::Always;
        let buffer = render_title_tab(&app, area);
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(row_text(&buffer, 0, 1..7), " solo ");
    }

    #[test]
    fn pane_title_tokens_render_on_zoomed_pane() {
        let (mut app, panes) = title_test_app(true);
        set_tokens(title_terminal(&mut app, panes[0]), &[("task", "zoomed")]);
        app.pane_title_tokens = Some(title_row(r#"["$task"]"#));
        app.workspaces[0].zoomed = true;
        let area = Rect::new(0, 0, 40, 6);

        let buffer = render_title_tab(&app, area);
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        assert_eq!(row_text(&buffer, 0, 1..9), " zoomed ");
    }

    const BUILD_FG: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
    const REVIEW_FG: Color = Color::Rgb(0xf9, 0xe2, 0xaf);

    fn identity_token(token: &str) -> AgentSidebarToken {
        title_row(&format!("[{token}]")).remove(0)
    }

    fn role_identity_token() -> AgentSidebarToken {
        identity_token(
            r##"{ token = "$role", rules = [{ equals = "build", fg = "#a6e3a1" }, { equals = "review", fg = "#f9e2af" }] }"##,
        )
    }

    // Left pane `root` (focused) | right column split into `top` over `bottom`.
    fn identity_test_app() -> (AppState, [PaneId; 3]) {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("repo");
        let root = workspace.tabs[0].root_pane;
        let top = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(root);
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.ensure_test_terminals();
        (app, [root, top, bottom])
    }

    const IDENTITY_AREA: Rect = Rect::new(0, 0, 30, 12);

    fn fg_at(buffer: &Buffer, x: u16, y: u16) -> Color {
        buffer[(x, y)].fg
    }

    // Cells whose rendering differs between two buffers of the same area.
    fn changed_cells(a: &Buffer, b: &Buffer) -> Vec<(u16, u16)> {
        let area = a.area;
        let mut changed = Vec::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if a[(x, y)] != b[(x, y)] {
                    changed.push((x, y));
                }
            }
        }
        changed
    }

    fn on_perimeter(rect: Rect, x: u16, y: u16) -> bool {
        let right = rect.x + rect.width - 1;
        let bottom = rect.y + rect.height - 1;
        let in_cols = (rect.x..=right).contains(&x);
        let in_rows = (rect.y..=bottom).contains(&y);
        (in_rows && (x == rect.x || x == right)) || (in_cols && (y == rect.y || y == bottom))
    }

    #[test]
    fn pane_border_shared_cells_follow_focus_ownership() {
        let (mut app, [_, top, bottom]) = identity_test_app();
        let area = IDENTITY_AREA;
        let top_rect = title_pane_rect(&app, area, top);
        let bottom_rect = title_pane_rect(&app, area, bottom);
        let divider_x = top_rect.x;
        let shared_y = bottom_rect.y;
        let mid_x = top_rect.x + top_rect.width / 2;
        let accent = app.palette.accent;
        let inactive = app.palette.overlay0;

        // Shared dividers: one column owned by root and the right panes, one row owned by
        // top and bottom; junctions are owned by every adjacent pane.
        let buffer = render_title_tab(&app, area);
        assert_eq!(buffer[(divider_x, shared_y)].symbol(), "├");
        assert_eq!(fg_at(&buffer, divider_x, shared_y), accent);
        assert_eq!(fg_at(&buffer, divider_x, 2), accent);
        assert_eq!(fg_at(&buffer, mid_x, shared_y), inactive);
        assert_eq!(fg_at(&buffer, mid_x, 0), inactive);

        // Any focused owner wins the shared cell; neighbours keep the inactive colour elsewhere.
        app.workspaces[0].tabs[0].layout.focus_pane(top);
        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, mid_x, shared_y), accent);
        assert_eq!(fg_at(&buffer, divider_x, 2), accent);
        assert_eq!(
            fg_at(&buffer, mid_x, bottom_rect.y + bottom_rect.height - 1),
            inactive
        );
        assert_eq!(fg_at(&buffer, 0, 2), inactive);
    }

    #[test]
    fn pane_border_identity_absent_renders_baseline_buffer() {
        let (mut app, panes) = identity_test_app();
        for pane in panes {
            set_tokens(title_terminal(&mut app, pane), &[("role", "build")]);
        }
        let baseline = render_title_tab(&app, IDENTITY_AREA);

        // A configured token that is missing on every pane changes nothing.
        app.pane_border_identity_token =
            Some(identity_token(r##"{ token = "$absent", fg = "#a6e3a1" }"##));
        assert_eq!(render_title_tab(&app, IDENTITY_AREA), baseline);
    }

    #[test]
    fn pane_border_identity_colors_private_segments_and_neutral_shared_dividers() {
        let (mut app, [root, top, bottom]) = identity_test_app();
        set_tokens(title_terminal(&mut app, root), &[("role", "review")]);
        set_tokens(title_terminal(&mut app, top), &[("role", "build")]);
        set_tokens(title_terminal(&mut app, bottom), &[("role", "review")]);
        title_terminal(&mut app, top).set_manual_label("api".into());
        let area = IDENTITY_AREA;
        let baseline = render_title_tab(&app, area);
        let top_rect = title_pane_rect(&app, area, top);
        let bottom_rect = title_pane_rect(&app, area, bottom);
        let root_rect = title_pane_rect(&app, area, root);
        let mid_x = top_rect.x + top_rect.width / 2;
        let right_x = area.width - 1;
        let shared_y = bottom_rect.y;
        let bottom_y = area.height - 1;

        app.pane_border_identity_token = Some(role_identity_token());
        let buffer = render_title_tab(&app, area);

        // Neighbours differ on their private segments.
        assert_eq!(fg_at(&buffer, right_x, 2), BUILD_FG);
        assert_eq!(fg_at(&buffer, right_x, shared_y + 2), REVIEW_FG);
        assert_eq!(fg_at(&buffer, mid_x, bottom_y), REVIEW_FG);
        // The divider and junctions they share stay neutral; cells the focused pane owns stay accent.
        assert_eq!(fg_at(&buffer, mid_x, shared_y), app.palette.overlay0);
        assert_eq!(fg_at(&buffer, right_x, shared_y), app.palette.overlay0);
        assert_eq!(fg_at(&buffer, top_rect.x, 2), app.palette.accent);
        assert_eq!(fg_at(&buffer, 0, 2), app.palette.accent);
        // Only frame colours of the unfocused panes changed: no glyph, title or terminal cell.
        for (x, y) in changed_cells(&baseline, &buffer) {
            assert_eq!(buffer[(x, y)].symbol(), baseline[(x, y)].symbol());
            assert!(
                !on_perimeter(root_rect, x, y) || x >= top_rect.x,
                "({x}, {y})"
            );
            assert!(
                on_perimeter(top_rect, x, y) || on_perimeter(bottom_rect, x, y),
                "({x}, {y})"
            );
            assert!(
                !(y == 0 && (top_rect.x + 1..top_rect.x + 6).contains(&x)),
                "title ({x}, {y})"
            );
        }
        assert_eq!(
            row_text(&buffer, 0, top_rect.x + 1..top_rect.x + 6),
            " api "
        );

        // Owners that agree colour the shared divider too.
        set_tokens(title_terminal(&mut app, top), &[("role", "review")]);
        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, mid_x, shared_y), REVIEW_FG);
        assert_eq!(fg_at(&buffer, top_rect.x, shared_y), app.palette.accent);

        // With gaps every pane owns its whole frame.
        set_tokens(title_terminal(&mut app, top), &[("role", "build")]);
        app.pane_gaps = true;
        let buffer = render_title_tab(&app, area);
        let top_rect = title_pane_rect(&app, area, top);
        let bottom_rect = title_pane_rect(&app, area, bottom);
        let mid_x = top_rect.x + top_rect.width / 2;
        assert_eq!(
            fg_at(&buffer, mid_x, top_rect.y + top_rect.height - 1),
            BUILD_FG
        );
        assert_eq!(fg_at(&buffer, mid_x, bottom_rect.y), REVIEW_FG);
        assert_eq!(fg_at(&buffer, top_rect.x, 2), BUILD_FG);
        assert_eq!(fg_at(&buffer, root_rect.x, 2), app.palette.accent);
    }

    #[test]
    fn pane_border_identity_four_way_junction_follows_every_owner() {
        // 2x2 grid: the centre junction is owned by all four panes.
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("repo");
        let top_left = workspace.tabs[0].root_pane;
        let top_right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        let bottom_right = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(top_left);
        let bottom_left = workspace.test_split(ratatui::layout::Direction::Vertical);
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.ensure_test_terminals();
        let panes = [top_left, top_right, bottom_left, bottom_right];
        let area = IDENTITY_AREA;
        let centre = {
            let rect = title_pane_rect(&app, area, bottom_right);
            (rect.x, rect.y)
        };
        app.pane_border_identity_token = Some(role_identity_token());

        let render = |app: &mut AppState, roles: [&str; 4], focus: PaneId| {
            for (pane, role) in panes.iter().zip(roles) {
                set_tokens(title_terminal(app, *pane), &[("role", role)]);
            }
            app.workspaces[0].tabs[0].layout.focus_pane(focus);
            let buffer = render_title_tab(app, area);
            assert_eq!(buffer[centre].symbol(), "┼");
            fg_at(&buffer, centre.0, centre.1)
        };

        // Any focused owner wins, wherever it sits among disagreeing or unset owners.
        for focus in panes {
            for roles in [
                ["build", "review", "none", "build"],
                ["review", "build", "build", "none"],
            ] {
                assert_eq!(render(&mut app, roles, focus), app.palette.accent);
            }
        }
        // The other three agreeing is not enough while the focused pane owns the cell.
        assert_eq!(render(&mut app, ["build"; 4], top_left), app.palette.accent);
        // Unfocused owners elsewhere: the right-edge junction between top_right and bottom_right.
        let right_x = area.width - 1;
        render(&mut app, ["build"; 4], top_left);
        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, right_x, centre.1), BUILD_FG);
        render(&mut app, ["build", "review", "build", "build"], top_left);
        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, right_x, centre.1), app.palette.overlay0);
    }

    #[test]
    fn pane_border_identity_focus_wins() {
        let (mut app, [root, top, bottom]) = identity_test_app();
        for pane in [root, top, bottom] {
            set_tokens(title_terminal(&mut app, pane), &[("role", "build")]);
        }
        app.pane_border_identity_token = Some(role_identity_token());
        let area = IDENTITY_AREA;
        let right_x = area.width - 1;

        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, 0, 2), app.palette.accent);
        assert_eq!(fg_at(&buffer, right_x, 2), BUILD_FG);

        app.workspaces[0].tabs[0].layout.focus_pane(top);
        let buffer = render_title_tab(&app, area);
        assert_eq!(fg_at(&buffer, 0, 2), BUILD_FG);
        assert_eq!(fg_at(&buffer, right_x, 2), app.palette.accent);
        let shared_y = title_pane_rect(&app, area, bottom).y;
        assert_eq!(fg_at(&buffer, right_x, shared_y), app.palette.accent);
        assert_eq!(fg_at(&buffer, right_x, shared_y + 2), BUILD_FG);
    }

    #[test]
    fn pane_border_identity_without_fg_keeps_inactive_color() {
        let (mut app, [root, top, bottom]) = identity_test_app();
        set_tokens(title_terminal(&mut app, root), &[("role", "build")]);
        set_tokens(title_terminal(&mut app, top), &[("role", "other")]);
        set_tokens(title_terminal(&mut app, bottom), &[("role", "   ")]);
        let baseline = render_title_tab(&app, IDENTITY_AREA);

        // No fg anywhere, a rule that does not match, and a blank value invent no colour.
        for token in [
            r#""$role""#,
            r#"{ token = "$role", bold = true }"#,
            r##"{ token = "$role", rules = [{ equals = "build", fg = "#a6e3a1" }] }"##,
        ] {
            app.pane_border_identity_token = Some(identity_token(token));
            assert_eq!(render_title_tab(&app, IDENTITY_AREA), baseline, "{token}");
        }
    }

    #[test]
    fn pane_border_identity_fallback_keeps_explicit_reset() {
        let (mut app, [_, top, bottom]) = identity_test_app();
        set_tokens(title_terminal(&mut app, top), &[("role", "build")]);
        app.palette.overlay0 = Color::Reset;
        app.pane_border_identity_token = Some(role_identity_token());
        let area = IDENTITY_AREA;
        let buffer = render_title_tab(&app, area);
        let shared_y = title_pane_rect(&app, area, bottom).y;
        let right_x = area.width - 1;

        assert_eq!(fg_at(&buffer, right_x, 2), BUILD_FG);
        assert_eq!(fg_at(&buffer, right_x, shared_y), Color::Reset);
        assert_eq!(fg_at(&buffer, right_x, shared_y + 2), Color::Reset);
    }

    #[test]
    fn pane_border_identity_single_and_zoomed_panes_stay_focused() {
        let (mut app, panes) = title_test_app(false);
        set_tokens(title_terminal(&mut app, panes[0]), &[("role", "build")]);
        app.pane_borders = PaneBordersConfig::Always;
        app.pane_border_identity_token = Some(role_identity_token());
        let area = Rect::new(0, 0, 30, 5);

        let buffer = render_title_tab(&app, area);
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        for (x, y) in [(0, 2), (29, 2), (10, 4), (29, 4)] {
            assert_eq!(fg_at(&buffer, x, y), app.palette.accent, "({x}, {y})");
        }

        let (mut app, panes) = title_test_app(true);
        for pane in &panes {
            set_tokens(title_terminal(&mut app, *pane), &[("role", "build")]);
        }
        app.pane_border_identity_token = Some(role_identity_token());
        app.workspaces[0].zoomed = true;
        let buffer = render_title_tab(&app, area);
        assert_eq!(buffer[(0, 0)].symbol(), "┌");
        for (x, y) in [(0, 2), (29, 2), (10, 4)] {
            assert_eq!(fg_at(&buffer, x, y), app.palette.accent, "({x}, {y})");
        }
    }
}
