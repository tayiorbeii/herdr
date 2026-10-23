use ratatui::{
    buffer::Buffer,
    layout::{Direction, Rect},
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
use crate::config::{AgentSidebarToken, BorderDash, PaneBorderStyle};
use crate::layout::{PaneId, PaneInfo};
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
    let mut style = Style::default().fg(pane_border_color(palette, focused));
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

// Resolves the configured row with the sidebar resolver against server-side pane data.
// Status and machine tokens are client presentation, so they resolve as missing here.
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
    let resolved = sidebar_agent_row(row, &context, "")
        .into_iter()
        .filter(|token| {
            token
                .kind
                .text_value()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .collect::<Vec<_>>();
    compose_title_spans(
        &resolved,
        pane_border_title_base_style(&app.palette, info.is_focused),
        info.is_focused,
        info.rect.width.saturating_sub(4) as usize,
    )
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
    terminal_inner_rect_for(pane_inner, pane_scrollbars && !rt.alternate_screen_active())
}

fn terminal_inner_rect_for(pane_inner: Rect, scrollbar_gutter: bool) -> Rect {
    if !scrollbar_gutter || pane_inner.width <= 4 {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

fn zoomed_pane_borders(app: &AppState, multi_pane: bool) -> Borders {
    if app.pane_borders.shows_borders(multi_pane) && app.pane_outer_borders {
        Borders::ALL
    } else {
        Borders::NONE
    }
}

/// Where a pane that is about to be created will sit in its tab.
#[derive(Debug, Clone, Copy)]
pub(crate) enum NewPanePlacement {
    /// The only pane of a new tab or workspace.
    Alone,
    /// The pane created by splitting `target` in workspace `ws_idx`.
    Split {
        ws_idx: usize,
        target: PaneId,
        direction: Direction,
        ratio: f32,
    },
    /// A pane split off and immediately zoomed over its tab.
    ZoomedOverlay,
    /// An existing pane in workspace `ws_idx` getting a new terminal.
    Existing { ws_idx: usize, pane: PaneId },
}

/// Terminal rows and columns a new pane gets once its tab is laid out in
/// `area`, so its program starts at that size instead of being resized.
pub(crate) fn new_pane_terminal_size(
    app: &AppState,
    area: Rect,
    placement: NewPanePlacement,
) -> (u16, u16) {
    let laid_out = |panes: Vec<PaneInfo>, index: usize| {
        let (infos, _) = apply_pane_spacing(
            panes,
            app.pane_borders,
            app.pane_spacing(),
            app.pane_outer_borders,
        );
        pane_inner_rect(infos[index].rect, infos[index].borders)
    };
    // A lone pane has no neighbors, so its chrome matches a zoomed single pane.
    let alone = || pane_inner_rect(area, zoomed_pane_borders(app, false));
    let tab_for = |ws_idx: usize, pane: PaneId| {
        let ws = app.workspaces.get(ws_idx)?;
        ws.tabs.get(ws.find_tab_index_for_pane(pane)?)
    };
    let pane_inner = match placement {
        NewPanePlacement::Alone => alone(),
        NewPanePlacement::Existing { ws_idx, pane } => tab_for(ws_idx, pane)
            .and_then(|tab| {
                if tab.zoomed && tab.layout.focused() == pane {
                    let multi_pane = tab.layout.pane_count() > 1;
                    return Some(pane_inner_rect(area, zoomed_pane_borders(app, multi_pane)));
                }
                let panes = tab.layout.panes(area);
                let index = panes.iter().position(|info| info.id == pane)?;
                Some(laid_out(panes, index))
            })
            .unwrap_or_else(alone),
        NewPanePlacement::ZoomedOverlay => pane_inner_rect(area, zoomed_pane_borders(app, true)),
        NewPanePlacement::Split {
            ws_idx,
            target,
            direction,
            ratio,
        } => tab_for(ws_idx, target)
            .and_then(|tab| tab.layout.panes_after_split(area, target, direction, ratio))
            .map(|(panes, new_index)| laid_out(panes, new_index))
            .unwrap_or_else(alone),
    };
    new_terminal_size(app, pane_inner)
}

/// Terminal rows and columns for every pane of `layout` laid out in `area`, in
/// pane order, so a multi-pane layout can start each program at its final size.
pub(crate) fn new_layout_terminal_sizes(
    app: &AppState,
    area: Rect,
    layout: &crate::layout::TileLayout,
) -> Vec<(u16, u16)> {
    apply_pane_spacing(
        layout.panes(area),
        app.pane_borders,
        app.pane_spacing(),
        app.pane_outer_borders,
    )
    .0
    .into_iter()
    .map(|info| new_terminal_size(app, pane_inner_rect(info.rect, info.borders)))
    .collect()
}

fn new_terminal_size(app: &AppState, pane_inner: Rect) -> (u16, u16) {
    // A new program starts on the primary screen, which reserves the gutter;
    // padding then insets it exactly as the first layout will.
    let inner = pad_pane_content(
        terminal_inner_rect_for(pane_inner, app.pane_scrollbars),
        app.pane_padding_cells,
    );
    (
        inner.height.max(crate::pane::MIN_PANE_ROWS),
        inner.width.max(crate::pane::MIN_PANE_COLS),
    )
}

/// Inset terminal content by `padding` empty cells per side, after the frame and
/// scrollbar gutter are allocated. Padding never shrinks an axis below the PTY
/// size floor, so the PTY always matches the padded rect; an axis already below
/// the floor keeps its baseline size, and a rect with no area is unchanged.
pub(crate) fn pad_pane_content(content: Rect, padding: u16) -> Rect {
    if padding == 0 || content.is_empty() {
        return content;
    }
    let pad_x = padding.min(content.width.saturating_sub(crate::pane::MIN_PANE_COLS) / 2);
    let pad_y = padding.min(content.height.saturating_sub(crate::pane::MIN_PANE_ROWS) / 2);
    Rect::new(
        content.x.saturating_add(pad_x),
        content.y.saturating_add(pad_y),
        content.width - 2 * pad_x,
        content.height - 2 * pad_y,
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
    pane_padding: u16,
) -> (Rect, Option<Rect>) {
    let inner_rect = terminal_inner_rect(rt, pane_inner, pane_scrollbars);
    if inner_rect == pane_inner {
        return (pad_pane_content(inner_rect, pane_padding), None);
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

    (pad_pane_content(inner_rect, pane_padding), scrollbar_rect)
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
            let pane_inner = pane_inner_rect(area, zoomed_pane_borders(app, multi_pane));
            let inner_rect = pad_pane_content(
                terminal_inner_rect(rt, pane_inner, app.pane_scrollbars),
                app.pane_padding_cells,
            );
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
            let inner_rect = pad_pane_content(
                terminal_inner_rect(rt, pane_inner, app.pane_scrollbars),
                app.pane_padding_cells,
            );
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
        let borders = zoomed_pane_borders(app, multi_pane);
        let pane_inner = pane_inner_rect(area, borders);
        let mut inner_rect = pad_pane_content(pane_inner, app.pane_padding_cells);
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, focused_id) {
            (inner_rect, scrollbar_rect) = stable_scrollbar_gutter(
                rt,
                pane_inner,
                app.pane_scrollbars,
                app.pane_padding_cells,
            );
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

        let mut inner_rect = pad_pane_content(pane_inner, app.pane_padding_cells);
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            (inner_rect, scrollbar_rect) = stable_scrollbar_gutter(
                rt,
                pane_inner,
                app.pane_scrollbars,
                app.pane_padding_cells,
            );
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
    // All style keys unset keeps the stock glyph path below.
    let styles = app
        .pane_border_styles
        .resolved()
        .map(|(active, inactive)| (LineStyle::of(active), LineStyle::of(inactive)));
    let focus_frame = styles.and_then(|_| {
        pane_infos
            .iter()
            .find(|info| info.is_focused)
            .and_then(|info| FocusFrame::of(info, pane_gaps.separate))
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
        let focused = pane_infos
            .iter()
            .any(|info| info.is_focused && line_touches_pane(x, y, info, pane_gaps.separate));
        let symbol = match styles {
            Some((active, inactive)) => {
                let owned =
                    focus_frame.map_or_else(LineCell::default, |frame| frame.owned_arms(x, y));
                styled_line_cell_symbol(line, owned, active, inactive)
            }
            None => line_cell_symbol(line),
        };
        if symbol.is_empty() {
            continue;
        }
        let cell = &mut buf[(x, y)];
        cell.set_symbol(symbol);
        cell.set_style(Style::default().fg(pane_border_color(&app.palette, focused)));
    }

    render_pane_border_titles(app, ws, pane_infos, frame);
}

/// Pane frame and title focus color; absent overrides follow accent / overlay0.
fn pane_border_color(palette: &Palette, focused: bool) -> Color {
    if focused {
        palette.pane_border_active.unwrap_or(palette.accent)
    } else {
        palette.pane_border_inactive.unwrap_or(palette.overlay0)
    }
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

/// Glyph family of one border arm. Arms of a cell may differ; Unicode box
/// drawing has a glyph for every light/heavy combination and for light/double
/// mixes across the two axes. Double has no heavy mixes, so a cell with a
/// double arm and a heavy arm is drawn entirely double.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum LineWeight {
    #[default]
    Light,
    Heavy,
    Double,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LineWeights {
    up: LineWeight,
    down: LineWeight,
    left: LineWeight,
    right: LineWeight,
}

/// Glyph traits of one `PaneBorderStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LineStyle {
    weight: LineWeight,
    dash: Option<BorderDash>,
    rounded: bool,
}

impl LineStyle {
    fn of(style: PaneBorderStyle) -> Self {
        let (weight, dash, rounded) = match style {
            PaneBorderStyle::Light => (LineWeight::Light, None, false),
            PaneBorderStyle::Rounded => (LineWeight::Light, None, true),
            PaneBorderStyle::Heavy => (LineWeight::Heavy, None, false),
            PaneBorderStyle::Double => (LineWeight::Double, None, false),
            PaneBorderStyle::LightDashed(dash) => (LineWeight::Light, Some(dash), false),
            PaneBorderStyle::HeavyDashed(dash) => (LineWeight::Heavy, Some(dash), false),
            PaneBorderStyle::RoundedDashed(dash) => (LineWeight::Light, Some(dash), true),
        };
        Self {
            weight,
            dash,
            rounded,
        }
    }
}

/// Border rectangle owned by the focused pane: its own frame plus, without
/// pane gaps, the shared divider column/row it touches to the right and below
/// (the same cells `line_touches_pane` accents).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusFrame {
    left: u16,
    top: u16,
    right: u16,
    bottom: u16,
}

impl FocusFrame {
    fn of(info: &PaneInfo, pane_gaps: bool) -> Option<Self> {
        let rect = info.rect;
        if rect.width == 0 || rect.height == 0 {
            return None;
        }
        let shared_right = rect.x.saturating_add(rect.width);
        let shared_bottom = rect.y.saturating_add(rect.height);
        let own_right = pane_gaps || info.borders.contains(Borders::RIGHT);
        let own_bottom = pane_gaps || info.borders.contains(Borders::BOTTOM);
        Some(Self {
            left: rect.x,
            top: rect.y,
            right: if own_right {
                shared_right.saturating_sub(1)
            } else {
                shared_right
            },
            bottom: if own_bottom {
                shared_bottom.saturating_sub(1)
            } else {
                shared_bottom
            },
        })
    }

    /// Whether the segment from `(x, y)` to `(x + 1, y)` lies on this frame.
    fn owns_horizontal(&self, x: u16, y: u16) -> bool {
        (y == self.top || y == self.bottom) && x >= self.left && x < self.right
    }

    /// Whether the segment from `(x, y)` to `(x, y + 1)` lies on this frame.
    fn owns_vertical(&self, x: u16, y: u16) -> bool {
        (x == self.left || x == self.right) && y >= self.top && y < self.bottom
    }

    /// Which arms of the cell at `(x, y)` lie on this frame. Both halves of a
    /// segment agree, so merged junctions stay connected.
    fn owned_arms(&self, x: u16, y: u16) -> LineCell {
        LineCell {
            up: y.checked_sub(1).is_some_and(|y| self.owns_vertical(x, y)),
            down: self.owns_vertical(x, y),
            left: x.checked_sub(1).is_some_and(|x| self.owns_horizontal(x, y)),
            right: self.owns_horizontal(x, y),
        }
    }
}

/// Glyph of one pane border cell. Each arm takes the weight of the style that
/// owns it (`active` on the focused frame, `inactive` elsewhere). The cell's
/// winning style, active when any present arm is on the focused frame (the
/// same rule as the accent color), decides dashes and arcs.
fn styled_line_cell_symbol(
    line: LineCell,
    owned: LineCell,
    active: LineStyle,
    inactive: LineStyle,
) -> &'static str {
    let style = |owned: bool| if owned { active } else { inactive };
    let weights = LineWeights {
        up: style(owned.up).weight,
        down: style(owned.down).weight,
        left: style(owned.left).weight,
        right: style(owned.right).weight,
    };
    let winner = style(
        (line.up && owned.up)
            || (line.down && owned.down)
            || (line.left && owned.left)
            || (line.right && owned.right),
    );
    if let Some(symbol) = winner
        .dash
        .and_then(|dash| dashed_line_cell_symbol(line, weights, dash))
    {
        return symbol;
    }
    if winner.rounded {
        if let Some(symbol) = arc_line_cell_symbol(line, weights) {
            return symbol;
        }
    }
    weighted_line_cell_symbol(line, weights)
}

/// The shared weight of an axis's present arms, or `None` when the axis has
/// no arm or its two arms differ.
fn axis_weight(first: Option<LineWeight>, second: Option<LineWeight>) -> Option<LineWeight> {
    match (first, second) {
        (Some(first), Some(second)) => (first == second).then_some(first),
        (Some(weight), None) | (None, Some(weight)) => Some(weight),
        (None, None) => None,
    }
}

/// Dashed glyph for a straight run of one light or heavy weight. A lone arm
/// draws a full straight line (as in `line_cell_symbol`), so it counts as
/// straight. Corners, T-junctions, crosses and double lines have no dashed
/// glyphs and stay solid.
fn dashed_line_cell_symbol(
    line: LineCell,
    weights: LineWeights,
    dash: BorderDash,
) -> Option<&'static str> {
    use ratatui::symbols::line;
    let vertical = match (line.up, line.down, line.left, line.right) {
        (_, _, false, false) => axis_weight(
            line.up.then_some(weights.up),
            line.down.then_some(weights.down),
        )
        .map(|weight| (true, weight)),
        (false, false, _, _) => axis_weight(
            line.left.then_some(weights.left),
            line.right.then_some(weights.right),
        )
        .map(|weight| (false, weight)),
        _ => None,
    };
    Some(match (vertical?, dash) {
        ((true, LineWeight::Light), BorderDash::Two) => line::LIGHT_DOUBLE_DASH_VERTICAL,
        ((true, LineWeight::Light), BorderDash::Three) => line::LIGHT_TRIPLE_DASH_VERTICAL,
        ((true, LineWeight::Light), BorderDash::Four) => line::LIGHT_QUADRUPLE_DASH_VERTICAL,
        ((true, LineWeight::Heavy), BorderDash::Two) => line::HEAVY_DOUBLE_DASH_VERTICAL,
        ((true, LineWeight::Heavy), BorderDash::Three) => line::HEAVY_TRIPLE_DASH_VERTICAL,
        ((true, LineWeight::Heavy), BorderDash::Four) => line::HEAVY_QUADRUPLE_DASH_VERTICAL,
        ((false, LineWeight::Light), BorderDash::Two) => line::LIGHT_DOUBLE_DASH_HORIZONTAL,
        ((false, LineWeight::Light), BorderDash::Three) => line::LIGHT_TRIPLE_DASH_HORIZONTAL,
        ((false, LineWeight::Light), BorderDash::Four) => line::LIGHT_QUADRUPLE_DASH_HORIZONTAL,
        ((false, LineWeight::Heavy), BorderDash::Two) => line::HEAVY_DOUBLE_DASH_HORIZONTAL,
        ((false, LineWeight::Heavy), BorderDash::Three) => line::HEAVY_TRIPLE_DASH_HORIZONTAL,
        ((false, LineWeight::Heavy), BorderDash::Four) => line::HEAVY_QUADRUPLE_DASH_HORIZONTAL,
        ((_, LineWeight::Double), _) => return None,
    })
}

/// Arc for a right-angle corner whose two arms are light. Unicode has no
/// heavy, double or mixed arcs, so those corners stay square.
fn arc_line_cell_symbol(line: LineCell, weights: LineWeights) -> Option<&'static str> {
    use ratatui::symbols::line;
    let light = |present: bool, weight: LineWeight| !present || weight == LineWeight::Light;
    if !(light(line.up, weights.up)
        && light(line.down, weights.down)
        && light(line.left, weights.left)
        && light(line.right, weights.right))
    {
        return None;
    }
    match (line.up, line.down, line.left, line.right) {
        (false, true, false, true) => Some(line::ROUNDED_TOP_LEFT),
        (false, true, true, false) => Some(line::ROUNDED_TOP_RIGHT),
        (true, false, false, true) => Some(line::ROUNDED_BOTTOM_LEFT),
        (true, false, true, false) => Some(line::ROUNDED_BOTTOM_RIGHT),
        _ => None,
    }
}

/// Indexed by `[up * 3 + down][left * 3 + right]`, where each arm is
/// 0 = absent, 1 = light, 2 = heavy.
const WEIGHTED_LINE_SYMBOLS: [[&str; 9]; 9] = [
    ["", "╶", "╺", "╴", "─", "╼", "╸", "╾", "━"],
    ["╷", "┌", "┍", "┐", "┬", "┮", "┑", "┭", "┯"],
    ["╻", "┎", "┏", "┒", "┰", "┲", "┓", "┱", "┳"],
    ["╵", "└", "┕", "┘", "┴", "┶", "┙", "┵", "┷"],
    ["│", "├", "┝", "┤", "┼", "┾", "┥", "┽", "┿"],
    ["╽", "┟", "┢", "┧", "╁", "╆", "┪", "╅", "╈"],
    ["╹", "┖", "┗", "┚", "┸", "┺", "┛", "┹", "┻"],
    ["╿", "┞", "┡", "┦", "╀", "╄", "┩", "╃", "╇"],
    ["┃", "┠", "┣", "┨", "╂", "╊", "┫", "╉", "╋"],
];

fn weighted_line_cell_symbol(line: LineCell, weights: LineWeights) -> &'static str {
    let present = |weight: LineWeight| {
        (line.up && weights.up == weight)
            || (line.down && weights.down == weight)
            || (line.left && weights.left == weight)
            || (line.right && weights.right == weight)
    };
    if present(LineWeight::Double) {
        // Light/double mixes exist across the two axes. Otherwise, and for any
        // heavy/double mix, the whole cell is double: every arm stays drawn
        // and a double frame stays closed through its junctions.
        if !present(LineWeight::Heavy) {
            if let Some(symbol) = light_double_line_cell_symbol(line, weights) {
                return symbol;
            }
        }
        return double_line_cell_symbol(line);
    }
    let arm = |present: bool, weight: LineWeight| -> u8 {
        match (present, weight) {
            (false, _) => 0,
            (true, LineWeight::Light) => 1,
            (true, LineWeight::Heavy | LineWeight::Double) => 2,
        }
    };
    let mut up = arm(line.up, weights.up);
    let mut down = arm(line.down, weights.down);
    let mut left = arm(line.left, weights.left);
    let mut right = arm(line.right, weights.right);
    // Like `line_cell_symbol`, a lone arm draws a full straight line.
    match (up, down, left, right) {
        (lone, 0, 0, 0) | (0, lone, 0, 0) => (up, down) = (lone, lone),
        (0, 0, lone, 0) | (0, 0, 0, lone) => (left, right) = (lone, lone),
        _ => {}
    }
    WEIGHTED_LINE_SYMBOLS[usize::from(up * 3 + down)][usize::from(left * 3 + right)]
}

/// Exact light/double glyph: one weight on the vertical arms, the other on
/// the horizontal arms. `None` when an axis is missing or mixed.
fn light_double_line_cell_symbol(line: LineCell, weights: LineWeights) -> Option<&'static str> {
    let vertical = axis_weight(
        line.up.then_some(weights.up),
        line.down.then_some(weights.down),
    )?;
    let horizontal = axis_weight(
        line.left.then_some(weights.left),
        line.right.then_some(weights.right),
    )?;
    if vertical == horizontal {
        return None;
    }
    let vertical_double = vertical == LineWeight::Double;
    Some(
        match (line.up, line.down, line.left, line.right, vertical_double) {
            (false, true, false, true, false) => "╒",
            (false, true, false, true, true) => "╓",
            (false, true, true, false, false) => "╕",
            (false, true, true, false, true) => "╖",
            (true, false, false, true, false) => "╘",
            (true, false, false, true, true) => "╙",
            (true, false, true, false, false) => "╛",
            (true, false, true, false, true) => "╜",
            (true, true, false, true, false) => "╞",
            (true, true, false, true, true) => "╟",
            (true, true, true, false, false) => "╡",
            (true, true, true, false, true) => "╢",
            (false, true, true, true, false) => "╤",
            (false, true, true, true, true) => "╥",
            (true, false, true, true, false) => "╧",
            (true, false, true, true, true) => "╨",
            (true, true, true, true, false) => "╪",
            (true, true, true, true, true) => "╫",
            _ => return None,
        },
    )
}

/// Double counterpart of `line_cell_symbol`, including its lone-arm lines.
fn double_line_cell_symbol(line: LineCell) -> &'static str {
    use ratatui::symbols::line;
    match (line.up, line.down, line.left, line.right) {
        (true, true, true, true) => line::DOUBLE_CROSS,
        (true, true, true, false) => line::DOUBLE_VERTICAL_LEFT,
        (true, true, false, true) => line::DOUBLE_VERTICAL_RIGHT,
        (true, false, true, true) => line::DOUBLE_HORIZONTAL_UP,
        (false, true, true, true) => line::DOUBLE_HORIZONTAL_DOWN,
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => {
            line::DOUBLE_VERTICAL
        }
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            line::DOUBLE_HORIZONTAL
        }
        (false, true, false, true) => line::DOUBLE_TOP_LEFT,
        (false, true, true, false) => line::DOUBLE_TOP_RIGHT,
        (true, false, false, true) => line::DOUBLE_BOTTOM_LEFT,
        (true, false, true, false) => line::DOUBLE_BOTTOM_RIGHT,
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
    use crate::config::{PaneBorderStyles, PaneBordersConfig};
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

    /// Renders a labeled two-pane split through the server surface path and returns the
    /// buffer plus every pane frame/title position.
    fn render_two_pane_chrome(palette: Palette) -> (Buffer, std::collections::HashSet<(u16, u16)>) {
        let mut app = AppState::test_new();
        app.palette = palette;
        let mut workspace = Workspace::test_new("chrome");
        workspace.test_split(ratatui::layout::Direction::Horizontal);
        let panes: Vec<_> = workspace.tabs[0].panes.keys().copied().collect();
        for pane in &panes {
            workspace.tabs[0].runtimes.insert(
                *pane,
                TerminalRuntime::test_with_scrollback_bytes(18, 4, 1024, b"content\n"),
            );
        }
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app.ensure_test_terminals();
        for pane in &panes {
            let terminal_id = app.workspaces[0].tabs[0].panes[pane]
                .attached_terminal_id
                .clone();
            app.terminals
                .get_mut(&terminal_id)
                .unwrap()
                .set_manual_label("agent".into());
        }
        let runtimes = TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 40, 6);
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
        let mut chrome = std::collections::HashSet::new();
        for info in &layout.pane_infos {
            let rect = info.rect;
            for y in rect.y..rect.bottom() {
                for x in rect.x..rect.right() {
                    if x == rect.x || x + 1 == rect.right() || y == rect.y || y + 1 == rect.bottom()
                    {
                        chrome.insert((x, y));
                    }
                }
            }
        }
        let (buffer, _, _, _) =
            crate::server::render_stream::render_tab_surface_virtual(&app, &runtimes, layout, area);
        (buffer, chrome)
    }

    #[tokio::test]
    async fn pane_border_colors_absent_match_accent_and_overlay0() {
        let palette = Palette::catppuccin();
        assert_ne!(palette.accent, palette.overlay0);
        let (buffer, chrome) = render_two_pane_chrome(palette.clone());
        let mut focused_bold_title = false;
        let mut seen = (false, false);
        for &(x, y) in &chrome {
            let cell = &buffer[(x, y)];
            if cell.symbol().trim().is_empty() {
                continue;
            }
            if cell.fg == palette.accent {
                seen.0 = true;
                focused_bold_title |=
                    cell.symbol() == "a" && cell.modifier.contains(Modifier::BOLD);
            } else {
                assert_eq!(cell.fg, palette.overlay0, "cell: {x},{y}");
                assert!(!cell.modifier.contains(Modifier::BOLD), "cell: {x},{y}");
                seen.1 = true;
            }
        }
        assert_eq!(seen, (true, true));
        assert!(focused_bold_title);
        // Pane content is rendered so the override test's non-chrome comparison is not vacuous.
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert_eq!(text.matches("content").count(), 2);
    }

    #[tokio::test]
    async fn pane_border_color_overrides_change_only_pane_chrome() {
        use ratatui::style::Color;

        let base = Palette::catppuccin();
        let (baseline, chrome) = render_two_pane_chrome(base.clone());
        for (active, inactive, expected_active, expected_inactive) in [
            (Some("red"), None, Color::Red, base.overlay0),
            (None, Some("green"), base.accent, Color::Green),
            (Some("red"), Some("green"), Color::Red, Color::Green),
            (Some("reset"), Some("default"), Color::Reset, Color::Reset),
            (
                Some("#123456"),
                Some("#123456"),
                Color::Rgb(18, 52, 86),
                Color::Rgb(18, 52, 86),
            ),
            (Some("not-a-color"), Some("bogus"), Color::Cyan, Color::Cyan),
        ] {
            let custom = crate::config::CustomThemeColors {
                pane_border_active: active.map(Into::into),
                pane_border_inactive: inactive.map(Into::into),
                ..Default::default()
            };
            let palette = base.clone().with_overrides(&custom);
            assert_eq!(palette.accent, base.accent);
            assert_eq!(palette.overlay0, base.overlay0);
            let (buffer, _) = render_two_pane_chrome(palette);
            for (index, before) in baseline.content.iter().enumerate() {
                let (x, y) = baseline.pos_of(index);
                let mut expected = before.clone();
                if chrome.contains(&(x, y)) {
                    if before.fg == base.accent {
                        expected.fg = expected_active;
                    } else if before.fg == base.overlay0 {
                        expected.fg = expected_inactive;
                    }
                }
                assert_eq!(
                    buffer[(x, y)],
                    expected,
                    "active: {active:?}, inactive: {inactive:?}, cell: {x},{y}"
                );
            }
        }
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

    fn weighted_arms(symbol: &str) -> Option<[u8; 4]> {
        let index = WEIGHTED_LINE_SYMBOLS
            .iter()
            .flatten()
            .position(|candidate| !candidate.is_empty() && *candidate == symbol)?;
        let index = u8::try_from(index).ok()?;
        Some([index / 27, index / 9 % 3, index / 3 % 3, index % 3])
    }

    fn has_heavy_arm(symbol: &str) -> bool {
        weighted_arms(symbol).is_some_and(|arms| arms.contains(&2))
    }

    fn buffer_rows(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// Top-left, top-right, bottom-left, bottom-right.
    fn border_grid() -> (Workspace, [PaneId; 4]) {
        let mut workspace = Workspace::test_new("test");
        let top_left = workspace.tabs[0].root_pane;
        let top_right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(top_left);
        let bottom_left = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(top_right);
        let bottom_right = workspace.test_split(ratatui::layout::Direction::Vertical);
        (workspace, [top_left, top_right, bottom_left, bottom_right])
    }

    /// A full-height left pane beside a stacked right pair: left, top-right,
    /// bottom-right. The divider meets the right pair's split in a T.
    fn border_stack() -> (Workspace, [PaneId; 3]) {
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        let top_right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(top_right);
        let bottom_right = workspace.test_split(ratatui::layout::Direction::Vertical);
        (workspace, [left, top_right, bottom_right])
    }

    fn border_app(
        pane_borders: PaneBordersConfig,
        pane_gaps: bool,
        pane_outer_borders: bool,
    ) -> AppState {
        let mut app = AppState::test_new();
        app.pane_borders = pane_borders;
        app.pane_gaps = pane_gaps;
        app.pane_outer_borders = pane_outer_borders;
        app
    }

    const GAPS_AND_OUTER: [(bool, bool); 4] =
        [(false, true), (false, false), (true, true), (true, false)];

    fn style(name: &str) -> PaneBorderStyle {
        PaneBorderStyle::parse(name).unwrap_or_else(|| panic!("style {name}"))
    }

    fn styles(all: Option<&str>, active: Option<&str>, inactive: Option<&str>) -> PaneBorderStyles {
        PaneBorderStyles {
            all: all.map(style),
            active: active.map(style),
            inactive: inactive.map(style),
        }
    }

    fn render_layout_borders(app: &AppState, workspace: &Workspace, area: Rect) -> Buffer {
        let layout = &workspace.tabs[0].layout;
        let (infos, gaps) = apply_pane_spacing(
            layout.panes(area),
            app.pane_borders,
            app.pane_spacing(),
            app.pane_outer_borders,
        );
        let splits = layout.splits(area);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .unwrap();
        terminal
            .draw(|frame| render_pane_borders(app, workspace, &infos, &splits, gaps, frame))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn render_styled(
        app: &mut AppState,
        workspace: &Workspace,
        area: Rect,
        styles: PaneBorderStyles,
    ) -> Buffer {
        app.pane_border_styles = styles;
        render_layout_borders(app, workspace, area)
    }

    fn accented_line_cells(app: &AppState, buffer: &Buffer) -> Vec<(u16, u16)> {
        cells_where(buffer, |cell| {
            decode_glyph(cell.symbol()).is_some() && cell.style().fg == Some(app.palette.accent)
        })
    }

    fn cells_where(
        buffer: &Buffer,
        keep: impl Fn(&ratatui::buffer::Cell) -> bool,
    ) -> Vec<(u16, u16)> {
        let mut cells = Vec::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                if keep(&buffer[(x, y)]) {
                    cells.push((x, y));
                }
            }
        }
        cells
    }

    fn line_cell_from_bits(bits: u8) -> LineCell {
        LineCell {
            up: bits & 8 != 0,
            down: bits & 4 != 0,
            left: bits & 2 != 0,
            right: bits & 1 != 0,
        }
    }

    fn uniform_weights(weight: LineWeight) -> LineWeights {
        LineWeights {
            up: weight,
            down: weight,
            left: weight,
            right: weight,
        }
    }

    const ALL_DASHES: [BorderDash; 3] = [BorderDash::Two, BorderDash::Three, BorderDash::Four];

    /// A decoded pane glyph: arm weights (0 absent, 1 light, 2 heavy, 3 double;
    /// up, down, left, right), dash count (0 solid) and whether it is an arc.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Glyph {
        arms: [u8; 4],
        dash: u8,
        arc: bool,
    }

    impl Glyph {
        fn present(self) -> [bool; 4] {
            self.arms.map(|weight| weight > 0)
        }

        /// The single weight of every present arm, or `None` when mixed.
        fn uniform_weight(self) -> Option<u8> {
            let mut present = self.arms.into_iter().filter(|&weight| weight > 0);
            let first = present.next()?;
            present.all(|weight| weight == first).then_some(first)
        }

        fn is_straight(self) -> bool {
            matches!(
                self.present(),
                [true, true, false, false] | [false, false, true, true]
            )
        }

        fn is_corner(self) -> bool {
            matches!(
                self.present(),
                [false, true, false, true]
                    | [false, true, true, false]
                    | [true, false, false, true]
                    | [true, false, true, false]
            )
        }
    }

    fn dash_count(dash: BorderDash) -> u8 {
        match dash {
            BorderDash::Two => 2,
            BorderDash::Three => 3,
            BorderDash::Four => 4,
        }
    }

    fn weight_code(weight: LineWeight) -> u8 {
        match weight {
            LineWeight::Light => 1,
            LineWeight::Heavy => 2,
            LineWeight::Double => 3,
        }
    }

    /// Decodes every glyph the pane border renderer can draw. Straight lines
    /// read as two-arm lines.
    fn decode_glyph(symbol: &str) -> Option<Glyph> {
        if symbol.is_empty() {
            return None;
        }
        let solid = |arms| {
            Some(Glyph {
                arms,
                dash: 0,
                arc: false,
            })
        };
        if let Some(arms) = weighted_arms(symbol) {
            return solid(arms);
        }
        for bits in (1..16u8).rev() {
            let line = line_cell_from_bits(bits);
            let present = [line.up, line.down, line.left, line.right];
            if double_line_cell_symbol(line) == symbol {
                return solid(present.map(|arm| u8::from(arm) * 3));
            }
            for (vertical, horizontal) in [
                (LineWeight::Light, LineWeight::Double),
                (LineWeight::Double, LineWeight::Light),
            ] {
                let weights = LineWeights {
                    up: vertical,
                    down: vertical,
                    left: horizontal,
                    right: horizontal,
                };
                if light_double_line_cell_symbol(line, weights) == Some(symbol) {
                    let (v, h) = (weight_code(vertical), weight_code(horizontal));
                    return solid([
                        u8::from(line.up) * v,
                        u8::from(line.down) * v,
                        u8::from(line.left) * h,
                        u8::from(line.right) * h,
                    ]);
                }
            }
            if arc_line_cell_symbol(line, LineWeights::default()) == Some(symbol) {
                return Some(Glyph {
                    arms: present.map(u8::from),
                    dash: 0,
                    arc: true,
                });
            }
        }
        for weight in [LineWeight::Light, LineWeight::Heavy] {
            for dash in ALL_DASHES {
                for bits in [12u8, 3] {
                    let line = line_cell_from_bits(bits);
                    if dashed_line_cell_symbol(line, uniform_weights(weight), dash) == Some(symbol)
                    {
                        let w = weight_code(weight);
                        return Some(Glyph {
                            arms: [line.up, line.down, line.left, line.right]
                                .map(|arm| u8::from(arm) * w),
                            dash: dash_count(dash),
                            arc: false,
                        });
                    }
                }
            }
        }
        None
    }

    /// Unicode character names (UnicodeData.txt, checked with Perl
    /// `charnames::viacode`) of every dashed, arc and light/double glyph.
    fn unicode_name(symbol: &str) -> &'static str {
        let mut chars = symbol.chars();
        let ch = chars.next().expect("one char");
        assert!(chars.next().is_none(), "{symbol:?} is one char");
        match ch {
            '\u{2504}' => "BOX DRAWINGS LIGHT TRIPLE DASH HORIZONTAL",
            '\u{2505}' => "BOX DRAWINGS HEAVY TRIPLE DASH HORIZONTAL",
            '\u{2506}' => "BOX DRAWINGS LIGHT TRIPLE DASH VERTICAL",
            '\u{2507}' => "BOX DRAWINGS HEAVY TRIPLE DASH VERTICAL",
            '\u{2508}' => "BOX DRAWINGS LIGHT QUADRUPLE DASH HORIZONTAL",
            '\u{2509}' => "BOX DRAWINGS HEAVY QUADRUPLE DASH HORIZONTAL",
            '\u{250A}' => "BOX DRAWINGS LIGHT QUADRUPLE DASH VERTICAL",
            '\u{250B}' => "BOX DRAWINGS HEAVY QUADRUPLE DASH VERTICAL",
            '\u{254C}' => "BOX DRAWINGS LIGHT DOUBLE DASH HORIZONTAL",
            '\u{254D}' => "BOX DRAWINGS HEAVY DOUBLE DASH HORIZONTAL",
            '\u{254E}' => "BOX DRAWINGS LIGHT DOUBLE DASH VERTICAL",
            '\u{254F}' => "BOX DRAWINGS HEAVY DOUBLE DASH VERTICAL",
            '\u{2552}' => "BOX DRAWINGS DOWN SINGLE AND RIGHT DOUBLE",
            '\u{2553}' => "BOX DRAWINGS DOWN DOUBLE AND RIGHT SINGLE",
            '\u{2555}' => "BOX DRAWINGS DOWN SINGLE AND LEFT DOUBLE",
            '\u{2556}' => "BOX DRAWINGS DOWN DOUBLE AND LEFT SINGLE",
            '\u{2558}' => "BOX DRAWINGS UP SINGLE AND RIGHT DOUBLE",
            '\u{2559}' => "BOX DRAWINGS UP DOUBLE AND RIGHT SINGLE",
            '\u{255B}' => "BOX DRAWINGS UP SINGLE AND LEFT DOUBLE",
            '\u{255C}' => "BOX DRAWINGS UP DOUBLE AND LEFT SINGLE",
            '\u{255E}' => "BOX DRAWINGS VERTICAL SINGLE AND RIGHT DOUBLE",
            '\u{255F}' => "BOX DRAWINGS VERTICAL DOUBLE AND RIGHT SINGLE",
            '\u{2561}' => "BOX DRAWINGS VERTICAL SINGLE AND LEFT DOUBLE",
            '\u{2562}' => "BOX DRAWINGS VERTICAL DOUBLE AND LEFT SINGLE",
            '\u{2564}' => "BOX DRAWINGS DOWN SINGLE AND HORIZONTAL DOUBLE",
            '\u{2565}' => "BOX DRAWINGS DOWN DOUBLE AND HORIZONTAL SINGLE",
            '\u{2567}' => "BOX DRAWINGS UP SINGLE AND HORIZONTAL DOUBLE",
            '\u{2568}' => "BOX DRAWINGS UP DOUBLE AND HORIZONTAL SINGLE",
            '\u{256A}' => "BOX DRAWINGS VERTICAL SINGLE AND HORIZONTAL DOUBLE",
            '\u{256B}' => "BOX DRAWINGS VERTICAL DOUBLE AND HORIZONTAL SINGLE",
            '\u{256D}' => "BOX DRAWINGS LIGHT ARC DOWN AND RIGHT",
            '\u{256E}' => "BOX DRAWINGS LIGHT ARC DOWN AND LEFT",
            '\u{256F}' => "BOX DRAWINGS LIGHT ARC UP AND LEFT",
            '\u{2570}' => "BOX DRAWINGS LIGHT ARC UP AND RIGHT",
            other => panic!(
                "no name recorded for {other:?} (U+{:04X})",
                u32::from(other)
            ),
        }
    }

    #[test]
    fn pane_border_style_glyphs_match_unicode_character_names() {
        // Dashed straight runs, for two-arm and lone-arm cells.
        for (weight, weight_name) in [(LineWeight::Light, "LIGHT"), (LineWeight::Heavy, "HEAVY")] {
            for (dash, dash_name) in [
                (BorderDash::Two, "DOUBLE"),
                (BorderDash::Three, "TRIPLE"),
                (BorderDash::Four, "QUADRUPLE"),
            ] {
                for (bits, axis) in [
                    (12u8, "VERTICAL"),
                    (8, "VERTICAL"),
                    (4, "VERTICAL"),
                    (3, "HORIZONTAL"),
                    (2, "HORIZONTAL"),
                    (1, "HORIZONTAL"),
                ] {
                    let symbol = dashed_line_cell_symbol(
                        line_cell_from_bits(bits),
                        uniform_weights(weight),
                        dash,
                    )
                    .expect("straight runs dash");
                    assert_eq!(
                        unicode_name(symbol),
                        format!("BOX DRAWINGS {weight_name} {dash_name} DASH {axis}"),
                        "bits={bits}"
                    );
                }
            }
        }
        // Arcs: both arms light.
        for (bits, vertical, horizontal) in [
            (5u8, "DOWN", "RIGHT"),
            (6, "DOWN", "LEFT"),
            (9, "UP", "RIGHT"),
            (10, "UP", "LEFT"),
        ] {
            let symbol = arc_line_cell_symbol(line_cell_from_bits(bits), LineWeights::default())
                .expect("light corner arcs");
            assert_eq!(
                unicode_name(symbol),
                format!("BOX DRAWINGS LIGHT ARC {vertical} AND {horizontal}")
            );
        }
        // Light/double mixes: one weight per axis.
        let mut light_double = 0;
        for bits in 1..16u8 {
            let line = line_cell_from_bits(bits);
            let vertical = match (line.up, line.down) {
                (true, true) => "VERTICAL",
                (true, false) => "UP",
                (false, true) => "DOWN",
                (false, false) => continue,
            };
            let horizontal = match (line.left, line.right) {
                (true, true) => "HORIZONTAL",
                (true, false) => "LEFT",
                (false, true) => "RIGHT",
                (false, false) => continue,
            };
            for (v, h, v_name, h_name) in [
                (LineWeight::Light, LineWeight::Double, "SINGLE", "DOUBLE"),
                (LineWeight::Double, LineWeight::Light, "DOUBLE", "SINGLE"),
            ] {
                let symbol = weighted_line_cell_symbol(
                    line,
                    LineWeights {
                        up: v,
                        down: v,
                        left: h,
                        right: h,
                    },
                );
                assert_eq!(
                    unicode_name(symbol),
                    format!("BOX DRAWINGS {vertical} {v_name} AND {horizontal} {h_name}")
                );
                light_double += 1;
            }
        }
        assert_eq!(light_double, 18);
        // The dashed and arc glyphs are ratatui's own constants.
        use ratatui::symbols::line;
        assert_eq!(
            dashed_line_cell_symbol(
                line_cell_from_bits(3),
                uniform_weights(LineWeight::Heavy),
                BorderDash::Two
            ),
            Some(line::HEAVY_DOUBLE_DASH_HORIZONTAL)
        );
        assert_eq!(
            arc_line_cell_symbol(line_cell_from_bits(5), LineWeights::default()),
            Some(line::ROUNDED_TOP_LEFT)
        );
    }

    #[test]
    fn pane_border_style_light_heavy_table_matches_light_family() {
        for bits in 0..16u8 {
            let line = line_cell_from_bits(bits);
            assert_eq!(
                weighted_line_cell_symbol(line, LineWeights::default()),
                line_cell_symbol(line)
            );
            let heavy_symbol = weighted_line_cell_symbol(line, uniform_weights(LineWeight::Heavy));
            let light_arms = weighted_arms(line_cell_symbol(line));
            assert_eq!(
                weighted_arms(heavy_symbol),
                light_arms.map(|arms| arms.map(|weight| weight * 2))
            );
        }
        let mut glyphs: Vec<&str> = WEIGHTED_LINE_SYMBOLS.iter().flatten().copied().collect();
        assert_eq!(glyphs.remove(0), "");
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), 80);
        let weights = LineWeights {
            right: LineWeight::Heavy,
            ..LineWeights::default()
        };
        assert_eq!(
            weighted_line_cell_symbol(line_cell_from_bits(13), weights),
            "┝"
        );
        let weights = LineWeights {
            up: LineWeight::Heavy,
            ..LineWeights::default()
        };
        assert_eq!(
            weighted_line_cell_symbol(line_cell_from_bits(8), weights),
            "┃"
        );
    }

    #[test]
    fn pane_border_style_double_mixes_follow_unicode_coverage() {
        for bits in 1..16u8 {
            let line = line_cell_from_bits(bits);
            let double = double_line_cell_symbol(line);
            let light_arms = decode_glyph(line_cell_symbol(line)).unwrap().present();
            assert_eq!(
                decode_glyph(double).unwrap().present(),
                light_arms,
                "bits={bits}"
            );
            assert_eq!(decode_glyph(double).unwrap().uniform_weight(), Some(3));
            for arm in 0..4 {
                for base in [LineWeight::Heavy, LineWeight::Light] {
                    let mut weights = uniform_weights(base);
                    let present = match arm {
                        0 => {
                            weights.up = LineWeight::Double;
                            line.up
                        }
                        1 => {
                            weights.down = LineWeight::Double;
                            line.down
                        }
                        2 => {
                            weights.left = LineWeight::Double;
                            line.left
                        }
                        _ => {
                            weights.right = LineWeight::Double;
                            line.right
                        }
                    };
                    let symbol = weighted_line_cell_symbol(line, weights);
                    let glyph = decode_glyph(symbol).unwrap();
                    assert_eq!(glyph.present(), light_arms, "bits={bits} {weights:?}");
                    if !present {
                        // A double weight on an absent arm changes nothing.
                        assert_eq!(
                            symbol,
                            weighted_line_cell_symbol(line, uniform_weights(base))
                        );
                    } else if base == LineWeight::Heavy {
                        // GW1 rule: heavy and double never mix, the cell is double.
                        assert_eq!(symbol, double, "bits={bits} {weights:?}");
                    } else {
                        // Light and double: the exact glyph where the double arm
                        // is the only arm on its axis, else the whole cell double.
                        let arms = [line.up, line.down, line.left, line.right];
                        let axis = if arm < 2 {
                            [arms[0], arms[1]]
                        } else {
                            [arms[2], arms[3]]
                        };
                        let other_axis = if arm < 2 {
                            [arms[2], arms[3]]
                        } else {
                            [arms[0], arms[1]]
                        };
                        let alone_on_axis = axis.iter().filter(|&&a| a).count() == 1;
                        if alone_on_axis && other_axis.contains(&true) {
                            let mut expected = arms.map(u8::from);
                            expected[arm] = 3;
                            assert_eq!(glyph.arms, expected, "bits={bits} {symbol}");
                        } else {
                            assert_eq!(symbol, double, "bits={bits} {weights:?}");
                        }
                    }
                }
            }
        }
        // A focused frame corner meeting a heavy neighbor divider: `╦`, not `┳`.
        let mut weights = uniform_weights(LineWeight::Heavy);
        weights.down = LineWeight::Double;
        weights.left = LineWeight::Double;
        assert_eq!(
            weighted_line_cell_symbol(line_cell_from_bits(7), weights),
            "╦"
        );
        // A double focused divider beside a light neighbor: `╟`; reversed: `╞`.
        let mut weights = uniform_weights(LineWeight::Double);
        weights.right = LineWeight::Light;
        assert_eq!(
            weighted_line_cell_symbol(line_cell_from_bits(13), weights),
            "╟"
        );
        let mut weights = uniform_weights(LineWeight::Light);
        weights.right = LineWeight::Double;
        assert_eq!(
            weighted_line_cell_symbol(line_cell_from_bits(13), weights),
            "╞"
        );
    }

    #[test]
    fn pane_border_glyphs_characterize_shared_and_gapped_grids() {
        let (workspace, _) = border_grid();
        let area = Rect::new(0, 0, 12, 6);
        let shared = render_layout_borders(
            &border_app(PaneBordersConfig::Auto, false, true),
            &workspace,
            area,
        );
        assert_eq!(
            buffer_rows(&shared),
            vec![
                "┌─────┬────┐",
                "│     │    │",
                "│     │    │",
                "├─────┼────┤",
                "│     │    │",
                "└─────┴────┘",
            ]
        );
        let gapped = render_layout_borders(
            &border_app(PaneBordersConfig::Auto, true, true),
            &workspace,
            area,
        );
        assert_eq!(
            buffer_rows(&gapped),
            vec![
                "┌────┐┌────┐",
                "│    ││    │",
                "└────┘└────┘",
                "┌────┐┌────┐",
                "│    ││    │",
                "└────┘└────┘",
            ]
        );
    }

    /// Small and large layouts, including frames too narrow to have a segment.
    const MATRIX_AREAS: [Rect; 5] = [
        Rect::new(0, 0, 3, 3),
        Rect::new(0, 0, 4, 4),
        Rect::new(0, 0, 5, 3),
        Rect::new(0, 0, 12, 6),
        Rect::new(0, 0, 40, 12),
    ];

    /// Every layout of the matrix tests: both workspaces, every focus.
    fn matrix_layouts() -> Vec<Workspace> {
        let mut layouts = Vec::new();
        for index in 0..4 {
            let (mut workspace, panes) = border_grid();
            workspace.tabs[0].layout.focus_pane(panes[index]);
            layouts.push(workspace);
        }
        for index in 0..3 {
            let (mut workspace, panes) = border_stack();
            workspace.tabs[0].layout.focus_pane(panes[index]);
            layouts.push(workspace);
        }
        layouts
    }

    /// Pane border styles x S1: style ownership follows the resolved spacing, not the
    /// legacy `pane_gaps` flag, so `pane_gap_cells = 0` beside `pane_gaps = true` draws
    /// exactly the shared-divider glyphs and colors, for every focus position and every
    /// active/inactive style mix (plus every all-panes style).
    #[test]
    fn pane_gap_cells_zero_weights_and_colors_like_shared_dividers() {
        let area = Rect::new(0, 0, 12, 6);
        let mut mixes = Vec::new();
        for (_, all) in PaneBorderStyle::NAMES {
            mixes.push(PaneBorderStyles {
                all: Some(all),
                active: None,
                inactive: None,
            });
        }
        for (_, active) in PaneBorderStyle::NAMES {
            for (_, inactive) in PaneBorderStyle::NAMES {
                mixes.push(PaneBorderStyles {
                    all: None,
                    active: Some(active),
                    inactive: Some(inactive),
                });
            }
        }
        for workspace in matrix_layouts() {
            for mix in &mixes {
                let mut legacy = border_app(PaneBordersConfig::Auto, false, true);
                legacy.pane_border_styles = *mix;
                let mut zero = border_app(PaneBordersConfig::Auto, true, true);
                zero.pane_gap_cells = Some(0);
                zero.pane_border_styles = *mix;
                assert_eq!(
                    render_layout_borders(&zero, &workspace, area),
                    render_layout_borders(&legacy, &workspace, area),
                    "styles={mix:?}"
                );
            }
        }
    }

    #[test]
    fn pane_border_styles_unset_and_explicit_light_match_stock_buffers() {
        for workspace in matrix_layouts() {
            for (gaps, outer) in GAPS_AND_OUTER {
                for area in MATRIX_AREAS {
                    let mut app = border_app(PaneBordersConfig::Auto, gaps, outer);
                    assert_eq!(app.pane_border_styles, PaneBorderStyles::default());
                    let stock = render_layout_borders(&app, &workspace, area);
                    for explicit in [
                        styles(Some("light"), None, None),
                        styles(None, Some("light"), None),
                        styles(None, None, Some("light")),
                        styles(Some("light"), Some("light"), Some("light")),
                    ] {
                        let rendered = render_styled(&mut app, &workspace, area, explicit);
                        assert_eq!(rendered, stock, "{explicit:?} gaps={gaps} outer={outer}");
                    }
                }
            }
        }
    }

    /// Reference weighted glyph rule (per-arm light/heavy/double, any double arm
    /// draws the cell double), kept as an independent oracle for the heavy and double styles.
    fn reference_weighted_line_cell_symbol(line: LineCell, weights: LineWeights) -> &'static str {
        let double = (line.up && weights.up == LineWeight::Double)
            || (line.down && weights.down == LineWeight::Double)
            || (line.left && weights.left == LineWeight::Double)
            || (line.right && weights.right == LineWeight::Double);
        if double {
            return match (line.up, line.down, line.left, line.right) {
                (true, true, true, true) => "╬",
                (true, true, true, false) => "╣",
                (true, true, false, true) => "╠",
                (true, false, true, true) => "╩",
                (false, true, true, true) => "╦",
                (true, true, false, false)
                | (true, false, false, false)
                | (false, true, false, false) => "║",
                (false, false, true, true)
                | (false, false, true, false)
                | (false, false, false, true) => "═",
                (false, true, false, true) => "╔",
                (false, true, true, false) => "╗",
                (true, false, false, true) => "╚",
                (true, false, true, false) => "╝",
                _ => "",
            };
        }
        let arm = |present: bool, weight: LineWeight| -> u8 {
            match (present, weight) {
                (false, _) => 0,
                (true, LineWeight::Light) => 1,
                (true, LineWeight::Heavy | LineWeight::Double) => 2,
            }
        };
        let mut up = arm(line.up, weights.up);
        let mut down = arm(line.down, weights.down);
        let mut left = arm(line.left, weights.left);
        let mut right = arm(line.right, weights.right);
        match (up, down, left, right) {
            (lone, 0, 0, 0) | (0, lone, 0, 0) => (up, down) = (lone, lone),
            (0, 0, lone, 0) | (0, 0, 0, lone) => (left, right) = (lone, lone),
            _ => {}
        }
        WEIGHTED_LINE_SYMBOLS[usize::from(up * 3 + down)][usize::from(left * 3 + right)]
    }

    /// Reference render for the heavy/double style cases:
    /// the stock buffer with each border cell's symbol chosen by the reference rule.
    fn reference_render(
        app: &mut AppState,
        workspace: &Workspace,
        area: Rect,
        focus_weight: bool,
        heavy: bool,
    ) -> Buffer {
        let mut buffer = render_styled(app, workspace, area, PaneBorderStyles::default());
        if !app.pane_borders.draws_borders() {
            return buffer;
        }
        let layout = &workspace.tabs[0].layout;
        let infos = apply_pane_chrome(
            layout.panes(area),
            app.pane_borders,
            app.pane_gaps,
            app.pane_outer_borders,
        );
        let mut cells = std::collections::HashMap::<(u16, u16), LineCell>::new();
        for info in &infos {
            add_pane_border_cells(&mut cells, info);
        }
        add_split_border_cells(app.pane_gaps, &layout.splits(area), &mut cells);
        let frame = if focus_weight {
            infos
                .iter()
                .find(|info| info.is_focused)
                .and_then(|info| FocusFrame::of(info, app.pane_gaps))
        } else {
            None
        };
        let weight = |owned: bool, base: LineWeight, emphasis: LineWeight| {
            if owned {
                emphasis
            } else {
                base
            }
        };
        for ((x, y), line) in cells {
            if x >= area.width
                || y >= area.height
                || buffer[(x, y)].symbol() != line_cell_symbol(line)
            {
                // Out of view, or a title glyph painted over the border.
                continue;
            }
            let (base, emphasis) = if heavy {
                (LineWeight::Heavy, LineWeight::Double)
            } else {
                (LineWeight::Light, LineWeight::Heavy)
            };
            let symbol = match frame {
                Some(frame) => {
                    let owned = frame.owned_arms(x, y);
                    reference_weighted_line_cell_symbol(
                        line,
                        LineWeights {
                            up: weight(owned.up, base, emphasis),
                            down: weight(owned.down, base, emphasis),
                            left: weight(owned.left, base, emphasis),
                            right: weight(owned.right, base, emphasis),
                        },
                    )
                }
                None if heavy => {
                    reference_weighted_line_cell_symbol(line, uniform_weights(LineWeight::Heavy))
                }
                None => line_cell_symbol(line),
            };
            if !symbol.is_empty() {
                buffer[(x, y)].set_symbol(symbol);
            }
        }
        buffer
    }

    #[test]
    fn pane_border_styles_heavy_and_double_match_reference_weighted_rule() {
        // (focused frame heavy, every frame heavy) -> the style keys.
        let mappings = [
            (true, false, styles(None, Some("heavy"), None)),
            (false, true, styles(Some("heavy"), None, None)),
            (true, true, styles(Some("heavy"), Some("double"), None)),
        ];
        let mut compared = 0;
        for palette in [Palette::catppuccin(), Palette::catppuccin_latte()] {
            for pane_borders in [PaneBordersConfig::Auto, PaneBordersConfig::Always] {
                for workspace in matrix_layouts()
                    .into_iter()
                    .chain([Workspace::test_new("lone")])
                {
                    for (gaps, outer) in GAPS_AND_OUTER {
                        for area in MATRIX_AREAS {
                            for (focus_weight, heavy, new_keys) in mappings {
                                let mut app = border_app(pane_borders, gaps, outer);
                                app.palette = palette.clone();
                                let old = reference_render(
                                    &mut app,
                                    &workspace,
                                    area,
                                    focus_weight,
                                    heavy,
                                );
                                let new = render_styled(&mut app, &workspace, area, new_keys);
                                assert_eq!(
                                    new, old,
                                    "focus_weight={focus_weight} heavy={heavy} gaps={gaps} outer={outer} area={area:?}"
                                );
                                compared += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(compared, 2 * 2 * 8 * 4 * 5 * 3);
    }

    #[test]
    fn pane_border_style_active_heavy_matches_old_focus_weight_buffers() {
        let (mut workspace, [top_left, _, _, bottom_right]) = border_grid();
        let area = Rect::new(0, 0, 12, 6);
        let active_heavy = styles(None, Some("heavy"), None);
        let mut app = border_app(PaneBordersConfig::Auto, false, true);

        workspace.tabs[0].layout.focus_pane(top_left);
        let emphasized = render_styled(&mut app, &workspace, area, active_heavy);
        assert_eq!(
            buffer_rows(&emphasized),
            vec![
                "┏━━━━━┱────┐",
                "┃     ┃    │",
                "┃     ┃    │",
                "┡━━━━━╃────┤",
                "│     │    │",
                "└─────┴────┘",
            ]
        );
        assert_eq!(
            cells_where(&emphasized, |c| has_heavy_arm(c.symbol())),
            accented_line_cells(&app, &emphasized)
        );

        // Moving focus moves the cue; the previous frame returns to light.
        workspace.tabs[0].layout.focus_pane(bottom_right);
        let emphasized = render_styled(&mut app, &workspace, area, active_heavy);
        assert_eq!(
            buffer_rows(&emphasized),
            vec![
                "┌─────┬────┐",
                "│     │    │",
                "│     │    │",
                "├─────╆━━━━┪",
                "│     ┃    ┃",
                "└─────┺━━━━┛",
            ]
        );

        // Separate frames, outer edges on and off.
        workspace.tabs[0].layout.focus_pane(top_left);
        let mut app = border_app(PaneBordersConfig::Auto, true, true);
        assert_eq!(
            buffer_rows(&render_styled(&mut app, &workspace, area, active_heavy)),
            vec![
                "┏━━━━┓┌────┐",
                "┃    ┃│    │",
                "┗━━━━┛└────┘",
                "┌────┐┌────┐",
                "│    ││    │",
                "└────┘└────┘",
            ]
        );
        let mut app = border_app(PaneBordersConfig::Auto, true, false);
        assert_eq!(
            buffer_rows(&render_styled(&mut app, &workspace, area, active_heavy)),
            vec![
                "     ┃│     ",
                "     ┃│     ",
                "━━━━━┛└─────",
                "─────┐┌─────",
                "     ││     ",
                "     ││     ",
            ]
        );
        // Shared dividers without outer edges.
        let mut app = border_app(PaneBordersConfig::Auto, false, false);
        assert_eq!(
            buffer_rows(&render_styled(&mut app, &workspace, area, active_heavy)),
            vec![
                "      ┃     ",
                "      ┃     ",
                "      ┃     ",
                "━━━━━━╃─────",
                "      │     ",
                "      │     ",
            ]
        );
        // A lone pane framed by `always`.
        let lone = Workspace::test_new("test");
        let mut app = border_app(PaneBordersConfig::Always, false, true);
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &lone,
                Rect::new(0, 0, 6, 3),
                active_heavy
            )),
            vec!["┏━━━━┓", "┃    ┃", "┗━━━━┛"]
        );
    }

    #[test]
    fn pane_border_style_heavy_matches_old_heavy_borders_buffers() {
        let (mut workspace, [top_left, _, _, bottom_right]) = border_grid();
        workspace.tabs[0].layout.focus_pane(top_left);
        let area = Rect::new(0, 0, 12, 6);
        let all_heavy = styles(Some("heavy"), None, None);
        for (gaps, outer, expected) in [
            (
                false,
                true,
                [
                    "┏━━━━━┳━━━━┓",
                    "┃     ┃    ┃",
                    "┃     ┃    ┃",
                    "┣━━━━━╋━━━━┫",
                    "┃     ┃    ┃",
                    "┗━━━━━┻━━━━┛",
                ],
            ),
            (
                false,
                false,
                [
                    "      ┃     ",
                    "      ┃     ",
                    "      ┃     ",
                    "━━━━━━╋━━━━━",
                    "      ┃     ",
                    "      ┃     ",
                ],
            ),
            (
                true,
                true,
                [
                    "┏━━━━┓┏━━━━┓",
                    "┃    ┃┃    ┃",
                    "┗━━━━┛┗━━━━┛",
                    "┏━━━━┓┏━━━━┓",
                    "┃    ┃┃    ┃",
                    "┗━━━━┛┗━━━━┛",
                ],
            ),
            (
                true,
                false,
                [
                    "     ┃┃     ",
                    "     ┃┃     ",
                    "━━━━━┛┗━━━━━",
                    "━━━━━┓┏━━━━━",
                    "     ┃┃     ",
                    "     ┃┃     ",
                ],
            ),
        ] {
            let mut app = border_app(PaneBordersConfig::Auto, gaps, outer);
            let plain = render_layout_borders(&app, &workspace, area);
            let heavy = render_styled(&mut app, &workspace, area, all_heavy);
            assert_eq!(buffer_rows(&heavy), expected, "gaps={gaps} outer={outer}");
            // The accent color alone still marks focus.
            assert!(!accented_line_cells(&app, &heavy).is_empty());
            assert_eq!(
                accented_line_cells(&app, &heavy),
                accented_line_cells(&app, &plain)
            );
        }

        // Heavy everywhere plus a double focused frame (the old combination).
        let heavy_double = styles(Some("heavy"), Some("double"), None);
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        for (focused, expected) in [
            (
                top_left,
                [
                    "╔═════╦━━━━┓",
                    "║     ║    ┃",
                    "║     ║    ┃",
                    "╠═════╬━━━━┫",
                    "┃     ┃    ┃",
                    "┗━━━━━┻━━━━┛",
                ],
            ),
            (
                bottom_right,
                [
                    "┏━━━━━┳━━━━┓",
                    "┃     ┃    ┃",
                    "┃     ┃    ┃",
                    "┣━━━━━╬════╣",
                    "┃     ║    ║",
                    "┗━━━━━╩════╝",
                ],
            ),
        ] {
            workspace.tabs[0].layout.focus_pane(focused);
            let plain = render_layout_borders(&app, &workspace, area);
            let both = render_styled(&mut app, &workspace, area, heavy_double);
            assert_eq!(buffer_rows(&both), expected);
            let double = cells_where(&both, |c| {
                decode_glyph(c.symbol()).and_then(Glyph::uniform_weight) == Some(3)
            });
            assert_eq!(double, accented_line_cells(&app, &plain));
        }

        let lone = Workspace::test_new("test");
        let mut app = border_app(PaneBordersConfig::Always, false, true);
        let lone_area = Rect::new(0, 0, 6, 3);
        assert_eq!(
            buffer_rows(&render_styled(&mut app, &lone, lone_area, all_heavy)),
            vec!["┏━━━━┓", "┃    ┃", "┗━━━━┛"]
        );
        assert_eq!(
            buffer_rows(&render_styled(&mut app, &lone, lone_area, heavy_double)),
            vec!["╔════╗", "║    ║", "╚════╝"]
        );
        // `auto` leaves a lone pane unframed whatever the style.
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        let plain = render_layout_borders(&app, &lone, lone_area);
        assert_eq!(
            render_styled(&mut app, &lone, lone_area, heavy_double),
            plain
        );
    }

    #[test]
    fn pane_border_styles_keep_focused_title_text_and_bold() {
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(left);
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        let terminal_id = workspace.tabs[0].panes[&left].attached_terminal_id.clone();
        let mut terminal_state = TerminalState::new(terminal_id.clone(), "/tmp".into());
        terminal_state.set_manual_label("api".into());
        app.terminals.insert(terminal_id, terminal_state);
        let area = Rect::new(0, 0, 20, 4);

        for (keys, expected) in [
            (
                styles(None, Some("heavy"), None),
                [
                    "┏ api ━━━━┱────────┐",
                    "┃         ┃        │",
                    "┃         ┃        │",
                    "┗━━━━━━━━━┹────────┘",
                ],
            ),
            (
                styles(Some("heavy"), None, None),
                [
                    "┏ api ━━━━┳━━━━━━━━┓",
                    "┃         ┃        ┃",
                    "┃         ┃        ┃",
                    "┗━━━━━━━━━┻━━━━━━━━┛",
                ],
            ),
            (
                styles(Some("heavy"), Some("double"), None),
                [
                    "╔ api ════╦━━━━━━━━┓",
                    "║         ║        ┃",
                    "║         ║        ┃",
                    "╚═════════╩━━━━━━━━┛",
                ],
            ),
            (
                styles(None, Some("light-dashed-3"), Some("rounded")),
                [
                    "┌ api ┄┄┄┄┬────────╮",
                    "┆         ┆        │",
                    "┆         ┆        │",
                    "└┄┄┄┄┄┄┄┄┄┴────────╯",
                ],
            ),
        ] {
            let buffer = render_styled(&mut app, &workspace, area, keys);
            assert_eq!(buffer_rows(&buffer), expected, "{keys:?}");
            let title = &buffer[(2, 0)];
            assert_eq!(title.symbol(), "a");
            assert_eq!(title.style().fg, Some(app.palette.accent));
            assert!(title.style().add_modifier.contains(Modifier::BOLD));
        }
    }

    /// Asserts the styled buffer keeps the stock render's cells, styles and arm
    /// presence, that every arm meets its neighbor, and that dashes and arcs
    /// sit only where the rules allow. Returns the decoded glyphs.
    fn assert_connected_glyph_only_change(stock: &Buffer, styled: &Buffer, context: &str) {
        assert_eq!(stock.area, styled.area, "{context}");
        let area = stock.area;
        for y in 0..area.height {
            for x in 0..area.width {
                let (before, after) = (&stock[(x, y)], &styled[(x, y)]);
                assert_eq!(
                    before.style(),
                    after.style(),
                    "{context}: style at ({x}, {y})"
                );
                let Some(stock_glyph) = decode_glyph(before.symbol()) else {
                    assert_eq!(
                        before.symbol(),
                        after.symbol(),
                        "{context}: stray cell at ({x}, {y})"
                    );
                    continue;
                };
                let glyph = decode_glyph(after.symbol()).unwrap_or_else(|| {
                    panic!(
                        "{context}: unknown glyph {:?} at ({x}, {y})",
                        after.symbol()
                    )
                });
                assert_eq!(
                    glyph.present(),
                    stock_glyph.present(),
                    "{context}: arms at ({x}, {y})"
                );
                if glyph.dash > 0 {
                    assert!(
                        glyph.is_straight(),
                        "{context}: dashed junction at ({x}, {y})"
                    );
                }
                if glyph.arc {
                    assert!(
                        glyph.is_corner() && glyph.uniform_weight() == Some(1),
                        "{context}: arc at ({x}, {y})"
                    );
                }
                // Every arm meets the matching arm of a neighboring line glyph.
                let neighbors = [
                    (y.checked_sub(1).map(|y| (x, y)), 0, 1),
                    ((y + 1 < area.height).then_some((x, y + 1)), 1, 0),
                    (x.checked_sub(1).map(|x| (x, y)), 2, 3),
                    ((x + 1 < area.width).then_some((x + 1, y)), 3, 2),
                ];
                for (neighbor, arm, opposite) in neighbors {
                    let Some(neighbor) = neighbor else { continue };
                    let Some(other) = decode_glyph(styled[neighbor].symbol()) else {
                        continue;
                    };
                    let other_stock = decode_glyph(stock[neighbor].symbol()).unwrap();
                    if stock_glyph.present()[arm] && other_stock.present()[opposite] {
                        assert!(
                            glyph.present()[arm] && other.present()[opposite],
                            "{context}: broken segment ({x}, {y}) -> {neighbor:?}"
                        );
                    }
                }
            }
        }
    }

    /// The renderer's merged border arms (before glyph selection).
    fn border_line_cells(
        app: &AppState,
        workspace: &Workspace,
        area: Rect,
    ) -> std::collections::HashMap<(u16, u16), LineCell> {
        let layout = &workspace.tabs[0].layout;
        let infos = apply_pane_chrome(
            layout.panes(area),
            app.pane_borders,
            app.pane_gaps,
            app.pane_outer_borders,
        );
        let mut cells = std::collections::HashMap::new();
        for info in &infos {
            add_pane_border_cells(&mut cells, info);
        }
        add_split_border_cells(app.pane_gaps, &layout.splits(area), &mut cells);
        cells
    }

    /// Line cells with a present arm on the focused frame; each is accented.
    fn focus_owned_cells(app: &AppState, workspace: &Workspace, stock: &Buffer) -> Vec<(u16, u16)> {
        let area = stock.area;
        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(area),
            app.pane_borders,
            app.pane_gaps,
            app.pane_outer_borders,
        );
        let Some(frame) = infos
            .iter()
            .find(|info| info.is_focused)
            .and_then(|info| FocusFrame::of(info, app.pane_gaps))
        else {
            return Vec::new();
        };
        let accented = accented_line_cells(app, stock);
        let lines = border_line_cells(app, workspace, area);
        cells_where(stock, |cell| decode_glyph(cell.symbol()).is_some())
            .into_iter()
            .filter(|&(x, y)| {
                let Some(line) = lines.get(&(x, y)) else {
                    return false;
                };
                let owned = frame.owned_arms(x, y);
                let on_frame = (line.up && owned.up)
                    || (line.down && owned.down)
                    || (line.left && owned.left)
                    || (line.right && owned.right);
                assert!(
                    !on_frame || accented.contains(&(x, y)),
                    "focus-owned cell ({x}, {y}) is accented"
                );
                on_frame
            })
            .collect()
    }

    #[test]
    fn pane_border_styles_every_style_state_layout_and_focus_stay_connected() {
        let mut renders = 0;
        for (name, chosen) in PaneBorderStyle::NAMES {
            let line_style = LineStyle::of(chosen);
            for (state, keys) in [
                ("all", styles(Some(name), None, None)),
                ("active", styles(None, Some(name), None)),
                ("inactive", styles(None, None, Some(name))),
            ] {
                for workspace in matrix_layouts() {
                    for (gaps, outer) in GAPS_AND_OUTER {
                        for area in MATRIX_AREAS {
                            let context =
                                format!("{name} {state} gaps={gaps} outer={outer} area={area:?}");
                            let mut app = border_app(PaneBordersConfig::Auto, gaps, outer);
                            let stock = render_layout_borders(&app, &workspace, area);
                            let styled = render_styled(&mut app, &workspace, area, keys);
                            assert_connected_glyph_only_change(&stock, &styled, &context);
                            renders += 1;
                            let owned = focus_owned_cells(&app, &workspace, &stock);
                            for y in 0..area.height {
                                for x in 0..area.width {
                                    let Some(glyph) = decode_glyph(styled[(x, y)].symbol()) else {
                                        continue;
                                    };
                                    let on_focus = owned.contains(&(x, y));
                                    let styled_here = match state {
                                        "all" => true,
                                        "active" => on_focus,
                                        _ => !on_focus,
                                    };
                                    let weight = weight_code(line_style.weight);
                                    if state == "all" {
                                        // One style everywhere: uniform weight per cell.
                                        assert_eq!(
                                            glyph.uniform_weight(),
                                            Some(weight),
                                            "{context} ({x}, {y})"
                                        );
                                    }
                                    if !styled_here && state == "active" {
                                        // Unfocused cells stay stock.
                                        assert_eq!(
                                            styled[(x, y)].symbol(),
                                            stock[(x, y)].symbol(),
                                            "{context} ({x}, {y})"
                                        );
                                    }
                                    if styled_here && glyph.uniform_weight() == Some(weight) {
                                        let dash = line_style.dash.map_or(0, dash_count);
                                        if glyph.is_straight()
                                            || glyph.present().iter().filter(|&&a| a).count() == 1
                                        {
                                            assert_eq!(glyph.dash, dash, "{context} ({x}, {y})");
                                        }
                                        if glyph.is_corner() && weight == 1 {
                                            assert_eq!(
                                                glyph.arc, line_style.rounded,
                                                "{context} ({x}, {y})"
                                            );
                                        }
                                    }
                                    if glyph.dash > 0 {
                                        assert!(
                                            styled_here,
                                            "{context}: dash outside the style at ({x}, {y})"
                                        );
                                        assert_eq!(
                                            glyph.dash,
                                            line_style.dash.map_or(0, dash_count)
                                        );
                                    }
                                    if glyph.arc {
                                        assert!(
                                            styled_here && line_style.rounded,
                                            "{context}: stray arc at ({x}, {y})"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(renders, 13 * 3 * 7 * 4 * 5);
    }

    #[test]
    fn pane_border_dashes_only_on_straight_runs() {
        let (mut workspace, [top_left, ..]) = border_grid();
        workspace.tabs[0].layout.focus_pane(top_left);
        let area = Rect::new(0, 0, 12, 6);
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(Some("light-dashed-2"), None, None)
            )),
            vec![
                "┌╌╌╌╌╌┬╌╌╌╌┐",
                "╎     ╎    ╎",
                "╎     ╎    ╎",
                "├╌╌╌╌╌┼╌╌╌╌┤",
                "╎     ╎    ╎",
                "└╌╌╌╌╌┴╌╌╌╌┘",
            ]
        );
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(None, Some("heavy-dashed-3"), None)
            )),
            vec![
                "┏┅┅┅┅┅┱────┐",
                "┇     ┇    │",
                "┇     ┇    │",
                "┡┅┅┅┅┅╃────┤",
                "│     │    │",
                "└─────┴────┘",
            ]
        );
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(Some("heavy-dashed-4"), Some("light"), None)
            )),
            vec![
                "┌─────┮┉┉┉┉┓",
                "│     │    ┋",
                "│     │    ┋",
                "┟─────╆┉┉┉┉┫",
                "┋     ┋    ┋",
                "┗┉┉┉┉┉┻┉┉┉┉┛",
            ]
        );
        // Outer edges off: the divider's lone-arm ends are straight and dash too.
        let mut app = border_app(PaneBordersConfig::Auto, false, false);
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(Some("rounded-dashed-4"), None, None)
            )),
            vec![
                "      ┊     ",
                "      ┊     ",
                "      ┊     ",
                "┈┈┈┈┈┈┼┈┈┈┈┈",
                "      ┊     ",
                "      ┊     ",
            ]
        );
    }

    #[test]
    fn pane_border_style_arcs_only_on_light_corners() {
        let (mut workspace, [top_left, ..]) = border_grid();
        workspace.tabs[0].layout.focus_pane(top_left);
        let area = Rect::new(0, 0, 12, 6);
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        // Rounded inactive, heavy active: the focused frame stays square.
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(None, Some("heavy"), Some("rounded"))
            )),
            vec![
                "┏━━━━━┱────╮",
                "┃     ┃    │",
                "┃     ┃    │",
                "┡━━━━━╃────┤",
                "│     │    │",
                "╰─────┴────╯",
            ]
        );
        // Rounded active over heavy: only the focused frame's light corner arcs.
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(Some("heavy"), Some("rounded"), None)
            )),
            vec![
                "╭─────┮━━━━┓",
                "│     │    ┃",
                "│     │    ┃",
                "┟─────╆━━━━┫",
                "┃     ┃    ┃",
                "┗━━━━━┻━━━━┛",
            ]
        );
        // Separate frames: every unfocused corner arcs, the double frame stays square.
        let mut app = border_app(PaneBordersConfig::Auto, true, true);
        assert_eq!(
            buffer_rows(&render_styled(
                &mut app,
                &workspace,
                area,
                styles(Some("rounded"), Some("double"), None)
            )),
            vec![
                "╔════╗╭────╮",
                "║    ║│    │",
                "╚════╝╰────╯",
                "╭────╮╭────╮",
                "│    ││    │",
                "╰────╯╰────╯",
            ]
        );
        // Heavy, double and heavy-dashed styles never arc.
        for name in [
            "heavy",
            "double",
            "heavy-dashed-2",
            "light",
            "light-dashed-3",
        ] {
            let buffer = render_styled(&mut app, &workspace, area, styles(Some(name), None, None));
            assert!(
                cells_where(&buffer, |c| decode_glyph(c.symbol()).is_some_and(|g| g.arc))
                    .is_empty(),
                "{name}"
            );
        }
    }

    #[test]
    fn pane_border_style_mixed_junction_for_every_style_pair() {
        let area = Rect::new(0, 0, 12, 6);
        let (mut grid, [grid_top_left, ..]) = border_grid();
        grid.tabs[0].layout.focus_pane(grid_top_left);
        let (mut stack, [stack_left, ..]) = border_stack();
        stack.tabs[0].layout.focus_pane(stack_left);
        let mut checked = 0;
        for (active_name, active) in PaneBorderStyle::NAMES {
            for (inactive_name, inactive) in PaneBorderStyle::NAMES {
                let (a, i) = (LineStyle::of(active).weight, LineStyle::of(inactive).weight);
                let keys = styles(None, Some(active_name), Some(inactive_name));
                let mut app = border_app(PaneBordersConfig::Auto, false, true);
                // Grid centre cross: up and left on the focused frame.
                // Stack divider T: up and down on the focused frame, right not.
                for (workspace, (x, y), arms) in [
                    (&grid, (6u16, 3u16), [Some(a), Some(i), Some(a), Some(i)]),
                    (&stack, (6, 3), [Some(a), Some(a), None, Some(i)]),
                ] {
                    let buffer = render_styled(&mut app, workspace, area, keys);
                    let glyph = decode_glyph(buffer[(x, y)].symbol()).unwrap();
                    let present: Vec<LineWeight> = arms.iter().flatten().copied().collect();
                    let has = |w: LineWeight| present.contains(&w);
                    let expected = arms.map(|arm| arm.map_or(0, weight_code));
                    let axis_uniform = |first: Option<LineWeight>, second: Option<LineWeight>| {
                        axis_weight(first, second).is_some()
                            || (first.is_none() && second.is_none())
                    };
                    let exact = if !has(LineWeight::Double) {
                        true
                    } else if has(LineWeight::Heavy) {
                        false
                    } else {
                        // Light/double: exact when each axis has one weight.
                        axis_uniform(arms[0], arms[1]) && axis_uniform(arms[2], arms[3])
                    };
                    if exact {
                        assert_eq!(
                            glyph.arms, expected,
                            "{active_name}/{inactive_name} at ({x}, {y})"
                        );
                    } else {
                        assert_eq!(
                            glyph.arms,
                            arms.map(|arm| if arm.is_some() { 3 } else { 0 }),
                            "{active_name}/{inactive_name} falls back to double at ({x}, {y})"
                        );
                    }
                    // Junctions are never dashed or rounded.
                    assert_eq!((glyph.dash, glyph.arc), (0, false));
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 13 * 13 * 2);

        // Spot checks against the glyphs named in the docs.
        let mut app = border_app(PaneBordersConfig::Auto, false, true);
        for (keys, workspace, glyph) in [
            (styles(None, Some("heavy"), None), &grid, "╃"),
            (styles(None, Some("heavy"), None), &stack, "┠"),
            (styles(None, Some("double"), None), &stack, "╟"),
            (styles(Some("double"), Some("light"), None), &stack, "╞"),
            (styles(None, Some("double"), Some("heavy")), &stack, "╠"),
            (styles(None, Some("double"), None), &grid, "╬"),
        ] {
            assert_eq!(
                render_styled(&mut app, workspace, area, keys)[(6, 3)].symbol(),
                glyph,
                "{keys:?}"
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

    fn scrollback_lines() -> Vec<u8> {
        (0..40)
            .map(|n| format!("line {n}\r\n"))
            .collect::<String>()
            .into_bytes()
    }

    fn padded_split_app(padding: u16, bytes: &[u8]) -> AppState {
        let mut app = AppState::test_new();
        app.pane_padding_cells = padding;
        let mut workspace = Workspace::test_new("test");
        let left = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        for pane in [left, right] {
            workspace.tabs[0].runtimes.insert(
                pane,
                TerminalRuntime::test_with_scrollback_bytes(40, 12, 4096, bytes),
            );
        }
        app.workspaces = vec![workspace];
        app.active = Some(0);
        app
    }

    fn infos_for(app: &AppState, area: Rect, resize_panes: bool) -> Vec<PaneInfo> {
        compute_pane_infos(
            app,
            &TerminalRuntimeRegistry::new(),
            area,
            resize_panes,
            crate::kitty_graphics::HostCellSize::default(),
        )
    }

    fn pty_size(app: &AppState, pane: PaneId) -> (u16, u16) {
        app.workspaces[0].tabs[0].runtimes[&pane].current_size()
    }

    #[tokio::test]
    async fn pane_padding_insets_framed_split_content_inside_scrollbar_lane() {
        let area = Rect::new(0, 0, 81, 12);
        let baseline = infos_for(&padded_split_app(0, &scrollback_lines()), area, false);
        let app = padded_split_app(2, &scrollback_lines());
        let padded = infos_for(&app, area, true);

        assert_eq!(padded.len(), 2);
        // Separate apps allocate different pane ids; panes compare by layout order.
        for (base, info) in baseline.iter().zip(&padded) {
            assert_eq!(info.rect, base.rect, "allocation must not change");
            assert_eq!(info.borders, base.borders, "border sides must not change");
            assert_eq!(
                info.scrollbar_rect, base.scrollbar_rect,
                "lane must not move"
            );
            assert_eq!(
                info.inner_rect,
                Rect::new(
                    base.inner_rect.x + 2,
                    base.inner_rect.y + 2,
                    base.inner_rect.width - 4,
                    base.inner_rect.height - 4,
                )
            );
            let lane = info.scrollbar_rect.expect("scrollback shows the lane");
            assert_eq!(info.inner_rect.right() + 2, lane.x);
            assert_eq!(
                (lane.y, lane.height),
                (base.inner_rect.y, base.inner_rect.height)
            );
            assert_eq!(
                pty_size(&app, info.id),
                (info.inner_rect.height, info.inner_rect.width)
            );
        }
    }

    #[tokio::test]
    async fn pane_padding_insets_unframed_lone_pane() {
        let mut app = AppState::test_new();
        app.pane_scrollbars = false;
        app.pane_padding_cells = 3;
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\r\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let infos = infos_for(&app, area, true);

        assert_eq!(infos[0].borders, Borders::NONE);
        assert_eq!(infos[0].rect, area);
        assert_eq!(infos[0].scrollbar_rect, None);
        assert_eq!(infos[0].inner_rect, Rect::new(13, 6, 34, 2));
        assert_eq!(pty_size(&app, root), (2, 34));
    }

    #[test]
    fn pane_padding_clamps_symmetrically_to_the_pty_size_floor() {
        use crate::pane::{MIN_PANE_COLS, MIN_PANE_ROWS};
        assert_eq!((MIN_PANE_COLS, MIN_PANE_ROWS), (4, 2));
        for (content, padding, expected) in [
            (Rect::new(4, 2, 30, 10), 2, Rect::new(6, 4, 26, 6)),
            (Rect::new(4, 2, 8, 6), 10, Rect::new(6, 4, 4, 2)),
            (Rect::new(4, 2, 9, 5), 10, Rect::new(6, 3, 5, 3)),
            (Rect::new(4, 2, 5, 4), 10, Rect::new(4, 3, 5, 2)),
            (Rect::new(4, 2, 6, 3), 10, Rect::new(5, 2, 4, 3)),
            (Rect::new(4, 2, 30, 10), u16::MAX, Rect::new(17, 6, 4, 2)),
            // An axis already below the floor keeps its baseline size.
            (Rect::new(4, 2, 3, 3), 1, Rect::new(4, 2, 3, 3)),
            (Rect::new(4, 2, 2, 9), 5, Rect::new(4, 5, 2, 3)),
            (Rect::new(4, 2, 1, 1), 5, Rect::new(4, 2, 1, 1)),
            (Rect::new(4, 2, 0, 5), 5, Rect::new(4, 2, 0, 5)),
            (Rect::new(4, 2, 7, 0), 5, Rect::new(4, 2, 7, 0)),
            (Rect::new(4, 2, 0, 0), 5, Rect::new(4, 2, 0, 0)),
        ] {
            assert_eq!(
                pad_pane_content(content, padding),
                expected,
                "{content:?} padded by {padding}"
            );
        }
        for width in 0..12 {
            for height in 0..8 {
                let content = Rect::new(3, 1, width, height);
                let padded = pad_pane_content(content, 3);
                assert!(padded.width >= width.min(MIN_PANE_COLS), "{content:?}");
                assert!(padded.height >= height.min(MIN_PANE_ROWS), "{content:?}");
                assert_eq!(width - padded.width, 2 * (padded.x - content.x));
                assert_eq!(height - padded.height, 2 * (padded.y - content.y));
            }
        }
    }

    #[tokio::test]
    async fn padded_pty_size_matches_content_rect_at_the_size_floor() {
        let pane_app = |pane_borders, padding, cols, rows| {
            let mut app = AppState::test_new();
            app.pane_borders = pane_borders;
            app.pane_padding_cells = padding;
            let mut workspace = Workspace::test_new("test");
            let root = workspace.tabs[0].root_pane;
            workspace.tabs[0].runtimes.insert(
                root,
                TerminalRuntime::test_with_scrollback_bytes(cols, rows, 1024, b"$ \r\n"),
            );
            app.workspaces = vec![workspace];
            app.active = Some(0);
            (app, root)
        };

        // QA D13: 37x5 framed pane, padding 1. Frame interior 35x3, gutter
        // leaves 34x3; rows stay at 3 so the prompt row is not clipped.
        let (app, root) = pane_app(PaneBordersConfig::Always, 1, 37, 5);
        let infos = infos_for(&app, Rect::new(0, 0, 37, 5), true);
        assert_eq!(infos[0].inner_rect, Rect::new(2, 1, 32, 3));
        assert_eq!(pty_size(&app, root), (3, 32));

        // QA D14: narrow unframed panes, padding 3.
        for cols in 5..=7 {
            for rows in 2..=6 {
                let (app, root) = pane_app(PaneBordersConfig::Off, 3, cols, rows);
                let info = &infos_for(&app, Rect::new(0, 0, cols, rows), true)[0];
                assert!(info.inner_rect.width >= crate::pane::MIN_PANE_COLS);
                assert!(info.inner_rect.height >= crate::pane::MIN_PANE_ROWS);
                assert_eq!(
                    pty_size(&app, root),
                    (info.inner_rect.height, info.inner_rect.width),
                    "{cols}x{rows}"
                );
            }
        }
    }

    #[tokio::test]
    async fn zero_pane_padding_matches_baseline_geometry() {
        for rect in [
            Rect::new(0, 0, 0, 0),
            Rect::new(3, 4, 1, 1),
            Rect::new(3, 4, 80, 24),
        ] {
            assert_eq!(pad_pane_content(rect, 0), rect);
        }

        let area = Rect::new(0, 0, 81, 12);
        let app = padded_split_app(0, &scrollback_lines());
        for info in infos_for(&app, area, true) {
            let rt = &app.workspaces[0].tabs[0].runtimes[&info.id];
            let unpadded = terminal_inner_rect(rt, pane_inner_rect(info.rect, info.borders), true);
            assert_eq!(info.inner_rect, unpadded);
            assert_eq!(pty_size(&app, info.id), (unpadded.height, unpadded.width));
        }
    }

    #[tokio::test]
    async fn pane_padding_render_leaves_padding_cells_blank() {
        let area = Rect::new(0, 0, 41, 10);
        let draw = |padding| {
            let app = padded_split_app(padding, b"ab");
            let registry = TerminalRuntimeRegistry::new();
            let infos = compute_pane_infos(
                &app,
                &registry,
                area,
                true,
                crate::kitty_graphics::HostCellSize::default(),
            );
            let splits = app.workspaces[0].tabs[0].layout.splits(area);
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(41, 10)).unwrap();
            terminal
                .draw(|frame| {
                    render_panes(
                        &app,
                        &registry,
                        frame,
                        Some(super::super::tab_surface::TabSurfaceTarget {
                            workspace_index: 0,
                            tab_index: 0,
                        }),
                        &infos,
                        &splits,
                        PaneGaps::legacy(app.pane_gaps, app.pane_borders.draws_borders()),
                    )
                })
                .unwrap();
            (terminal.backend().buffer().clone(), infos)
        };
        let (baseline, base_infos) = draw(0);
        let (padded, infos) = draw(1);

        for y in 0..area.height {
            for x in 0..area.width {
                let cell = ratatui::layout::Position::new(x, y);
                if !base_infos.iter().any(|info| info.inner_rect.contains(cell)) {
                    assert_eq!(padded[(x, y)], baseline[(x, y)], "chrome cell {x},{y}");
                } else if !infos.iter().any(|info| info.inner_rect.contains(cell)) {
                    assert_eq!(padded[(x, y)].symbol(), " ", "padding cell {x},{y}");
                }
            }
        }
        for (base, info) in base_infos.iter().zip(&infos) {
            assert_eq!(
                baseline[(base.inner_rect.x, base.inner_rect.y)].symbol(),
                "a"
            );
            assert_eq!(padded[(info.inner_rect.x, info.inner_rect.y)].symbol(), "a");
            assert_eq!(
                padded[(info.inner_rect.x + 1, info.inner_rect.y)].symbol(),
                "b"
            );
        }
    }

    #[tokio::test]
    async fn zoomed_pane_padding_resizes_pty_to_padded_rect() {
        let mut app = AppState::test_new();
        app.pane_padding_cells = 1;
        let mut workspace = Workspace::test_new("test");
        let focused = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.zoomed = true;
        workspace.tabs[0].runtimes.insert(
            focused,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\r\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);
        let area = Rect::new(10, 3, 40, 8);

        // Frame interior (11,4,38,6), scrollbar gutter leaves (11,4,37,6).
        let infos = infos_for(&app, area, true);
        assert_eq!(infos[0].rect, area);
        assert_eq!(infos[0].inner_rect, Rect::new(12, 5, 35, 4));
        assert_eq!(pty_size(&app, focused), (4, 35));

        app.workspaces[0].tabs[0].runtimes[&focused].resize(8, 40, 0, 0);
        resize_tab_panes(
            &app,
            &TerminalRuntimeRegistry::new(),
            0,
            &app.workspaces[0].tabs[0],
            area,
            crate::kitty_graphics::HostCellSize::default(),
        );
        assert_eq!(pty_size(&app, focused), (4, 35));
    }

    #[tokio::test]
    async fn background_tab_resize_applies_pane_padding() {
        let area = Rect::new(0, 0, 81, 12);
        let app = padded_split_app(2, &scrollback_lines());
        resize_tab_panes(
            &app,
            &TerminalRuntimeRegistry::new(),
            0,
            &app.workspaces[0].tabs[0],
            area,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let baseline = infos_for(&padded_split_app(0, &scrollback_lines()), area, false);
        for (base, info) in baseline.iter().zip(infos_for(&app, area, false)) {
            assert_eq!(
                pty_size(&app, info.id),
                (base.inner_rect.height - 4, base.inner_rect.width - 4)
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

    /// C1 x pane border styles x FC1: composed titles still render on heavy, double
    /// and dashed active frames, take FC1's focused/unfocused colors as their
    /// fallback, keep token styles, and keep focus BOLD. The frame after the title
    /// keeps the pane's style (dashes included) and color.
    #[test]
    fn pane_title_tokens_render_on_weighted_frames_with_focus_colors() {
        let active = Color::Rgb(0xff, 0x00, 0x00);
        let inactive = Color::Rgb(0x00, 0xff, 0x00);
        let token_fg = Color::Rgb(0xf3, 0x8b, 0xa8);
        for (all, active_style, inactive_style, focused_line, unfocused_line, focused_corner) in [
            (None, None, None, "─", "─", "┌"),
            (None, Some("heavy"), None, "━", "─", "┏"),
            (Some("heavy"), None, None, "━", "━", "┏"),
            (Some("heavy"), Some("double"), None, "═", "━", "╔"),
            (None, Some("double"), Some("rounded"), "═", "─", "╔"),
            (
                None,
                Some("heavy-dashed-2"),
                Some("light-dashed-4"),
                "╍",
                "┈",
                "┏",
            ),
            (Some("rounded"), Some("heavy-dashed-3"), None, "┅", "─", "┏"),
            (
                None,
                Some("rounded-dashed-2"),
                Some("double"),
                "╌",
                "═",
                "╭",
            ),
        ] {
            let (mut app, panes) = title_test_app(true);
            for pane in &panes {
                let terminal = title_terminal(&mut app, *pane);
                set_tokens(terminal, &[("task", "fix")]);
                terminal.set_manual_label("api".into());
            }
            app.pane_title_tokens = Some(title_row(
                r##"["workspace", { token = "$task", fg = "#f38ba8", dim = true }, "pane"]"##,
            ));
            app.pane_border_styles = styles(all, active_style, inactive_style);
            app.palette.pane_border_active = Some(active);
            app.palette.pane_border_inactive = Some(inactive);
            let area = Rect::new(0, 0, 60, 6);
            let buffer = render_title_tab(&app, area);
            let right = title_pane_rect(&app, area, panes[1]);
            let case = format!("all={all:?} active={active_style:?} inactive={inactive_style:?}");

            let expected = " repo · fix · api ";
            let width = expected.chars().count() as u16;
            let rx = right.x + 1;
            assert_eq!(row_text(&buffer, 0, 1..1 + width), expected, "{case}");
            assert_eq!(row_text(&buffer, 0, rx..rx + width), expected, "{case}");
            assert_eq!(buffer[(0, 0)].symbol(), focused_corner, "{case}");
            for (x0, focused, fallback, line) in [
                (1, true, active, focused_line),
                (rx, false, inactive, unfocused_line),
            ] {
                let bold = if focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                };
                assert_cells_style(
                    &buffer,
                    0,
                    x0..x0 + 8,
                    Style::default().fg(fallback).add_modifier(bold),
                );
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
                    x0 + 11..x0 + width,
                    Style::default().fg(fallback).add_modifier(bold),
                );
                // The frame continues after the title in the pane's style and color.
                let after = &buffer[(x0 + width, 0)];
                assert_eq!(after.symbol(), line, "{case} focused={focused}");
                assert_eq!(after.fg, fallback, "{case} focused={focused}");
            }
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
}
