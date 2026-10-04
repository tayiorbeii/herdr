use ratatui::layout::Rect;

use crate::app;
use crate::protocol::{self, FrameData};

#[cfg(test)]
pub(super) fn snapshot(
    app: &app::App,
    boot_id: &str,
    revision: u64,
    config_diagnostic: Option<&str>,
    location: Option<&crate::server::clients::ClientShellLocation>,
) -> protocol::ClientShellSnapshot {
    snapshot_with_completions(app, boot_id, revision, config_diagnostic, location).0
}

pub(super) fn snapshot_with_completions(
    app: &app::App,
    boot_id: &str,
    revision: u64,
    config_diagnostic: Option<&str>,
    location: Option<&crate::server::clients::ClientShellLocation>,
) -> (
    protocol::ClientShellSnapshot,
    protocol::endpoint::EndpointAgentCompletions,
) {
    let snapshot = app.session_snapshot();
    let completions = protocol::endpoint::EndpointAgentCompletions {
        boot_id: boot_id.to_owned(),
        revision,
        completions: snapshot
            .agents
            .iter()
            .filter_map(|agent| agent.completion_seq.map(|seq| (agent.pane_id.clone(), seq)))
            .collect(),
    };
    let focused_workspace_id = location
        .and_then(|location| location.focused_workspace_id.clone())
        .or_else(|| snapshot.focused_workspace_id.clone());
    let focused_tab_id = location
        .and_then(|location| location.focused_tab_id().map(str::to_owned))
        .or_else(|| snapshot.focused_tab_id.clone());
    let focused_pane_id = focused_tab_id
        .as_deref()
        .and_then(|tab_id| app.parse_tab_id(tab_id))
        .and_then(|(workspace_index, tab_index)| {
            let pane_id = app
                .state
                .workspaces
                .get(workspace_index)?
                .tabs
                .get(tab_index)?
                .layout
                .focused();
            app.public_pane_id(workspace_index, pane_id)
        })
        .or_else(|| snapshot.focused_pane_id.clone());
    let workspaces = snapshot
        .workspaces
        .into_iter()
        .zip(&app.state.workspaces)
        .enumerate()
        .map(|(workspace_index, (workspace, state))| {
            let mut tokens = workspace.tokens.into_iter().collect::<Vec<_>>();
            tokens.sort_by(|left, right| left.0.cmp(&right.0));
            let workspace_id = workspace.workspace_id;
            let active_tab_id = location
                .and_then(|location| location.active_tab_ids.get(&workspace_id))
                .cloned()
                .unwrap_or(workspace.active_tab_id);
            let active_tab_index =
                app.parse_tab_id(&active_tab_id)
                    .and_then(|(tab_workspace_index, tab_index)| {
                        (tab_workspace_index == workspace_index).then_some(tab_index)
                    });
            protocol::ClientShellWorkspace {
                focused: focused_workspace_id.as_deref() == Some(workspace_id.as_str()),
                workspace_id,
                active_tab_id,
                new_workspace_cwd: app
                    .resolved_new_workspace_cwd_from_tab(workspace_index, active_tab_index)
                    .display()
                    .to_string(),
                number: workspace.number,
                label: workspace.label,
                custom_label: state.custom_name.is_some(),
                branch: state.branch(),
                git_ahead_behind: state.git_ahead_behind(),
                tokens,
                worktree: workspace
                    .worktree
                    .map(|worktree| protocol::ClientShellWorktree {
                        key: worktree.repo_key,
                        label: worktree.repo_name,
                        is_linked_worktree: worktree.is_linked_worktree,
                    }),
                agent_status: workspace.agent_status,
            }
        })
        .collect();
    let tabs = snapshot
        .tabs
        .into_iter()
        .zip(
            app.state
                .workspaces
                .iter()
                .flat_map(|workspace| workspace.tabs.iter()),
        )
        .map(|(tab, state)| {
            let tab_id = tab.tab_id;
            protocol::ClientShellTab {
                focused: focused_tab_id.as_deref() == Some(tab_id.as_str()),
                tab_id,
                workspace_id: tab.workspace_id,
                number: tab.number,
                label: tab.label,
                custom_label: !state.is_auto_named(),
                zoomed: state.zoomed,
                agent_status: tab.agent_status,
            }
        })
        .collect();
    let panes = snapshot
        .panes
        .into_iter()
        .map(|pane| {
            let pane_id = pane.pane_id;
            let focused = focused_pane_id.as_deref() == Some(pane_id.as_str());
            let right_click_passthrough = app
                .parse_pane_id(&pane_id)
                .and_then(|(workspace_index, pane_id)| {
                    app.state
                        .workspaces
                        .get(workspace_index)?
                        .pane_state(pane_id)
                })
                .is_some_and(|pane| pane.right_click_passthrough);
            protocol::ClientShellPane {
                pane_id,
                workspace_id: pane.workspace_id,
                tab_id: pane.tab_id,
                label: pane.label,
                cwd: pane.cwd,
                foreground_cwd: pane.foreground_cwd,
                focused,
                right_click_passthrough,
            }
        })
        .collect();
    let agents = snapshot
        .agents
        .into_iter()
        .map(|agent| {
            let pane_id = agent.pane_id;
            let focused = focused_pane_id.as_deref() == Some(pane_id.as_str());
            let mut state_labels = agent.state_labels.into_iter().collect::<Vec<_>>();
            state_labels.sort_by(|left, right| left.0.cmp(&right.0));
            let mut tokens = agent.tokens.into_iter().collect::<Vec<_>>();
            tokens.sort_by(|left, right| left.0.cmp(&right.0));
            protocol::ClientShellAgent {
                pane_id,
                workspace_id: agent.workspace_id,
                tab_id: agent.tab_id,
                name: agent.name,
                display_agent: agent.display_agent,
                agent: agent.agent,
                title: agent.title,
                terminal_title: agent.terminal_title,
                terminal_title_stripped: agent.terminal_title_stripped,
                agent_status: agent.agent_status,
                state_change_seq: agent.state_change_seq,
                state_labels,
                tokens,
                focused,
            }
        })
        .collect();

    let agent_view_label = app
        .state
        .agent_view_override
        .as_ref()
        .map(|view| view.label.clone().unwrap_or_else(|| "filtered".to_owned()));
    let agent_order = crate::ui::agent_panel_entries_from(&app.state, &app.terminal_runtimes)
        .into_iter()
        .filter_map(|entry| app.public_pane_id(entry.ws_idx, entry.pane_id))
        .collect();

    let zoomed = focused_tab_id
        .as_deref()
        .and_then(|tab_id| app.parse_tab_id(tab_id))
        .and_then(|(workspace_index, tab_index)| {
            app.state
                .workspaces
                .get(workspace_index)?
                .tabs
                .get(tab_index)
        })
        .is_some_and(|tab| tab.zoomed);
    let tab_bar_right = app
        .state
        .tab_bar_right
        .iter()
        .filter_map(|segment| match segment {
            crate::app::state::TabBarStatusSegment::Zoom if zoomed => {
                Some(protocol::ClientShellTabStatusSegment {
                    text: "ZOOM".to_owned(),
                    accent: true,
                })
            }
            crate::app::state::TabBarStatusSegment::Text(Some(text)) if !text.is_empty() => {
                Some(protocol::ClientShellTabStatusSegment {
                    text: text.clone(),
                    accent: false,
                })
            }
            crate::app::state::TabBarStatusSegment::Zoom
            | crate::app::state::TabBarStatusSegment::Text(_) => None,
        })
        .collect();

    let product_announcement = app.state.product_announcement.as_ref().map(|announcement| {
        protocol::ClientShellProductAnnouncement {
            version: announcement.version.clone(),
            id: announcement.id.clone(),
            title: announcement.title.clone(),
            body: announcement.body.clone(),
            preview: announcement.preview,
        }
    });
    let release_notes =
        app.state
            .latest_release_notes
            .as_ref()
            .map(|notes| protocol::ClientShellReleaseNotes {
                version: notes.version.clone(),
                body: notes.body.clone(),
                preview: notes.preview,
            });

    let shell = protocol::ClientShellSnapshot {
        boot_id: boot_id.to_owned(),
        revision,
        config_diagnostic: config_diagnostic.map(str::to_owned),
        product_announcement,
        update_available: app.state.update_available.clone(),
        update_install_command: app.state.update_install_command.clone(),
        server_keybindings_toml: app.client_shell_keybindings_profile().map(str::to_owned),
        latest_release_notes_available: app.state.latest_release_notes_available,
        integration_updates_available: app.state.integration_updates_available(),
        worktree_directory: app.state.worktree_directory.to_string_lossy().into_owned(),
        release_notes,
        focused_workspace_id,
        focused_tab_id,
        focused_pane_id,
        tab_bar_right,
        tab_bar_right_separator: app.state.tab_bar_right_separator.clone(),
        agent_view_label,
        agent_order,
        workspaces,
        tabs,
        panes,
        agents,
        commands: app.client_shell_command_manifest(),
    };
    (shell, completions)
}

pub(super) struct RenderedPaneSurface {
    pub(super) frame: FrameData,
    pub(super) panes: Vec<protocol::PaneSurfacePane>,
    pub(super) splits: Vec<protocol::PaneSurfaceSplit>,
    pub(super) popup: Option<Box<protocol::ClientShellPopupSurface>>,
    pub(super) graphics: protocol::SurfaceGraphicsScene,
    pub(super) graphics_delivery: crate::kitty_graphics::surface::DeliveryCache,
    pub(super) graphics_sources: crate::kitty_graphics::surface::SourceFiles,
}

#[derive(Debug)]
pub(super) enum SurfaceRenderDeferred {
    Synchronized,
    Changed,
}

pub(super) fn render_pane_surface(
    app: &mut app::App,
    target: Option<crate::ui::TabSurfaceTarget>,
    area: Rect,
    resize_panes: bool,
    show_popup: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
    graphics_delivery: &crate::kitty_graphics::surface::DeliveryCache,
    client_id: u64,
) -> Result<RenderedPaneSurface, SurfaceRenderDeferred> {
    let layout = crate::ui::compute_tab_surface_for(
        &app.state,
        &app.terminal_runtimes,
        target,
        area,
        resize_panes,
        cell_size,
    );
    let mut content_revisions_before = std::collections::HashMap::new();
    if let Some(target) = target {
        for pane in &layout.pane_infos {
            if let Some(runtime) = app.state.runtime_for_pane_in_workspace(
                &app.terminal_runtimes,
                target.workspace_index,
                pane.id,
            ) {
                let (synchronized, epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                let revision = runtime.content_seq();
                content_revisions_before.insert(pane.id, (epoch, revision));
            }
        }
    }
    let popup_revision_before = if show_popup {
        app.state
            .popup_pane
            .as_ref()
            .and_then(|popup| app.terminal_runtimes.get(&popup.terminal_id))
            .map(|runtime| {
                let (synchronized, epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                Ok(epoch)
            })
            .transpose()?
    } else {
        None
    };
    let (buffer, cursor, hyperlinks, layout) =
        crate::server::render_stream::render_tab_surface_virtual(
            &app.state,
            &app.terminal_runtimes,
            layout,
            area,
        );
    let panes = target
        .map(|target| {
            let workspace_index = target.workspace_index;
            layout
                .pane_infos
                .iter()
                .filter_map(|pane| {
                    app.public_pane_id(workspace_index, pane.id).map(|pane_id| {
                        let runtime = app.state.runtime_for_pane_in_workspace(
                            &app.terminal_runtimes,
                            workspace_index,
                            pane.id,
                        );
                        let mouse_reporting =
                            runtime.is_some_and(|runtime| runtime.mouse_reporting_enabled());
                        let sgr_pixel_mouse =
                            runtime.is_some_and(|runtime| runtime.sgr_pixel_mouse_enabled());
                        let (pixel_width, pixel_height) = if cell_size.is_known() {
                            (
                                u32::from(pane.inner_rect.width) * cell_size.width_px,
                                u32::from(pane.inner_rect.height) * cell_size.height_px,
                            )
                        } else {
                            (0, 0)
                        };
                        let content_revision = runtime.map_or(0, |runtime| {
                            let after = runtime.content_seq();
                            if content_revisions_before
                                .get(&pane.id)
                                .is_some_and(|&(_, before)| before == after)
                                && after.is_multiple_of(2)
                            {
                                after
                            } else {
                                after | 1
                            }
                        });
                        protocol::PaneSurfacePane {
                            pane_id,
                            content_revision,
                            rect: pane.rect.into(),
                            inner_rect: pane.inner_rect.into(),
                            scrollbar_rect: pane.scrollbar_rect.map(Into::into),
                            scroll: runtime.and_then(|runtime| runtime.scroll_metrics()).map(
                                |metrics| protocol::PaneSurfaceScrollMetrics {
                                    offset_from_bottom: metrics.offset_from_bottom as u64,
                                    max_offset_from_bottom: metrics.max_offset_from_bottom as u64,
                                    viewport_rows: metrics.viewport_rows as u64,
                                },
                            ),
                            focused: pane.is_focused,
                            mouse_reporting,
                            sgr_pixel_mouse,
                            alternate_screen_active: runtime
                                .is_some_and(|runtime| runtime.alternate_screen_active()),
                            pixel_width,
                            pixel_height,
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let pane_frames = layout
        .pane_infos
        .iter()
        .map(|pane| pane.rect)
        .collect::<Vec<_>>();
    let splits = layout
        .split_borders
        .iter()
        .filter_map(|split| {
            let hit_rect = split_hit_rect(
                split,
                app.state.pane_borders.draws_borders(),
                app.state.pane_gaps,
                &pane_frames,
            )?;
            let direction = match split.direction {
                ratatui::layout::Direction::Horizontal => {
                    protocol::PaneSurfaceSplitDirection::Horizontal
                }
                ratatui::layout::Direction::Vertical => {
                    protocol::PaneSurfaceSplitDirection::Vertical
                }
            };
            Some(protocol::PaneSurfaceSplit {
                direction,
                pos: split.pos,
                area: split.area.into(),
                hit_rect: hit_rect.into(),
                path: split.path.clone(),
            })
        })
        .collect();
    let popup = show_popup
        .then(|| render_popup_surface(app, area, resize_panes, cell_size))
        .flatten();
    let (graphics, next_graphics_delivery, graphics_sources) =
        crate::server::client_shell_graphics::collect(
            app,
            &layout.pane_infos,
            &layout.split_borders,
            popup.as_deref(),
            target,
            cell_size,
            graphics_delivery,
            client_id,
        );
    if let Some(target) = target {
        for (&pane_id, &(epoch, _)) in &content_revisions_before {
            if let Some(runtime) = app.state.runtime_for_pane_in_workspace(
                &app.terminal_runtimes,
                target.workspace_index,
                pane_id,
            ) {
                let (synchronized, after_epoch) = runtime.synchronized_output_state();
                if synchronized {
                    return Err(SurfaceRenderDeferred::Synchronized);
                }
                if after_epoch != epoch {
                    return Err(SurfaceRenderDeferred::Changed);
                }
            }
        }
    }
    if let Some(before) = popup_revision_before {
        if let Some(runtime) = app
            .state
            .popup_pane
            .as_ref()
            .and_then(|popup| app.terminal_runtimes.get(&popup.terminal_id))
        {
            let (synchronized, after_epoch) = runtime.synchronized_output_state();
            if synchronized {
                return Err(SurfaceRenderDeferred::Synchronized);
            }
            if after_epoch != before {
                return Err(SurfaceRenderDeferred::Changed);
            }
        }
    }
    Ok(RenderedPaneSurface {
        frame: FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, cursor, &hyperlinks),
        panes,
        splits,
        popup,
        graphics,
        graphics_delivery: next_graphics_delivery,
        graphics_sources,
    })
}

fn render_popup_surface(
    app: &app::App,
    area: Rect,
    resize_runtime: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Option<Box<protocol::ClientShellPopupSurface>> {
    let popup = app.state.popup_pane.as_ref()?;
    let geometry = if resize_runtime {
        resize_popup_runtime(app, area, cell_size)?
    } else {
        crate::popup_size::resolve_popup_geometry(popup.width, popup.height, area)?
    };
    let runtime = app.terminal_runtimes.get(&popup.terminal_id)?;
    let content_area = Rect::new(0, 0, geometry.inner.width, geometry.inner.height);
    let (buffer, cursor) =
        crate::server::render_stream::render_terminal_virtual(runtime, content_area);
    let hyperlinks = runtime.visible_hyperlinks(content_area);
    let title = app
        .state
        .terminals
        .get(&popup.terminal_id)
        .and_then(|terminal| terminal.manual_label.clone())
        .unwrap_or_else(|| "popup".to_owned());
    let (pixel_width, pixel_height) = if cell_size.is_known() {
        (
            u32::from(content_area.width) * cell_size.width_px,
            u32::from(content_area.height) * cell_size.height_px,
        )
    } else {
        (0, 0)
    };
    Some(Box::new(protocol::ClientShellPopupSurface {
        terminal_id: popup.terminal_id.to_string(),
        title,
        width: popup.width.map(client_popup_size),
        height: popup.height.map(client_popup_size),
        frame: FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, cursor, &hyperlinks),
        mouse_reporting: runtime.mouse_reporting_enabled(),
        sgr_pixel_mouse: runtime.sgr_pixel_mouse_enabled(),
        pixel_width,
        pixel_height,
    }))
}

pub(super) fn resize_popup_runtime(
    app: &app::App,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Option<crate::popup_size::PopupResolvedGeometry> {
    let popup = app.state.popup_pane.as_ref()?;
    let geometry = crate::popup_size::resolve_popup_geometry(popup.width, popup.height, area)?;
    let runtime = app.terminal_runtimes.get(&popup.terminal_id)?;
    if !app
        .state
        .direct_attach_resize_locks
        .contains(&popup.terminal_id)
    {
        runtime.resize(
            geometry.inner.height,
            geometry.inner.width,
            cell_size.width_px,
            cell_size.height_px,
        );
    }
    Some(geometry)
}

fn client_popup_size(size: crate::popup_size::PopupSize) -> protocol::ClientShellPopupSize {
    match size {
        crate::popup_size::PopupSize::Cells(cells) => protocol::ClientShellPopupSize::Cells(cells),
        crate::popup_size::PopupSize::Percent(percent) => {
            protocol::ClientShellPopupSize::Percent(percent)
        }
    }
}

fn split_hit_rect(
    split: &crate::layout::SplitBorder,
    pane_borders: bool,
    pane_gaps: bool,
    pane_frames: &[Rect],
) -> Option<Rect> {
    let hit = match (split.direction, pane_borders, pane_gaps) {
        (ratatui::layout::Direction::Horizontal, true, false) => {
            Rect::new(split.pos, split.area.y, 1, split.area.height)
        }
        (ratatui::layout::Direction::Horizontal, true, true) => {
            let start = split.pos.saturating_sub(1);
            Rect::new(
                start,
                split.area.y,
                split.pos.saturating_sub(start).saturating_add(1),
                split.area.height,
            )
        }
        (ratatui::layout::Direction::Horizontal, false, true) => Rect::new(
            split.pos.checked_sub(1)?,
            split.area.y,
            1,
            split.area.height,
        ),
        (ratatui::layout::Direction::Vertical, true, false) => {
            Rect::new(split.area.x, split.pos, split.area.width, 1)
        }
        (ratatui::layout::Direction::Vertical, true, true) => {
            let start = split.pos.saturating_sub(1);
            Rect::new(
                split.area.x,
                start,
                split.area.width,
                split.pos.saturating_sub(start).saturating_add(1),
            )
        }
        (ratatui::layout::Direction::Vertical, false, true) => {
            Rect::new(split.area.x, split.pos.checked_sub(1)?, split.area.width, 1)
        }
        (_, false, false) => return None,
    };
    if !pane_borders
        && pane_frames.iter().any(|pane| {
            hit.x < pane.right()
                && hit.right() > pane.x
                && hit.y < pane.bottom()
                && hit.bottom() > pane.y
        })
    {
        return None;
    }
    Some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_projects_cached_release_and_update_facts() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.integration_recommendations.clear();
        app.state.update_available = Some("0.8.3".into());
        app.state.update_install_command = "herdr update".into();
        app.state.latest_release_notes_available = true;
        app.state.latest_release_notes = Some(crate::release_notes::ReleaseNotes {
            version: "0.8.3".into(),
            body: "### Changed\n- Client shell".into(),
            preview: true,
        });

        let snapshot = snapshot(&app, "boot", 7, None, None);

        assert_eq!(snapshot.update_available.as_deref(), Some("0.8.3"));
        assert_eq!(snapshot.update_install_command, "herdr update");
        assert!(snapshot.latest_release_notes_available);
        assert!(!snapshot.integration_updates_available);
        assert_eq!(
            snapshot.release_notes.as_ref().map(|notes| (
                notes.version.as_str(),
                notes.body.as_str(),
                notes.preview
            )),
            Some(("0.8.3", "### Changed\n- Client shell", true))
        );
    }

    #[test]
    fn snapshot_badges_only_outdated_integrations() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.integration_recommendations =
            vec![crate::integration::IntegrationRecommendation {
                target: crate::api::schema::IntegrationTarget::Claude,
                label: "claude",
                command: "claude",
                available: true,
                path: std::path::PathBuf::from("claude-hook"),
                state: crate::integration::IntegrationStatusKind::NotInstalled,
            }];

        assert!(!snapshot(&app, "boot", 1, None, None).integration_updates_available);

        app.state.integration_recommendations[0].state =
            crate::integration::IntegrationStatusKind::Outdated;
        assert!(snapshot(&app, "boot", 2, None, None).integration_updates_available);
    }

    #[test]
    fn split_hits_follow_released_border_and_gap_geometry() {
        let horizontal = crate::layout::SplitBorder {
            pos: 20,
            direction: ratatui::layout::Direction::Horizontal,
            ratio: 0.5,
            area: Rect::new(2, 3, 40, 12),
            path: vec![false],
        };
        assert_eq!(
            split_hit_rect(&horizontal, true, false, &[]),
            Some(Rect::new(20, 3, 1, 12))
        );
        assert_eq!(
            split_hit_rect(&horizontal, true, true, &[]),
            Some(Rect::new(19, 3, 2, 12))
        );
        assert_eq!(
            split_hit_rect(&horizontal, false, true, &[]),
            Some(Rect::new(19, 3, 1, 12))
        );
        assert_eq!(split_hit_rect(&horizontal, false, false, &[]), None);

        let vertical = crate::layout::SplitBorder {
            pos: 9,
            direction: ratatui::layout::Direction::Vertical,
            ratio: 0.5,
            area: Rect::new(2, 3, 40, 12),
            path: vec![true],
        };
        assert_eq!(
            split_hit_rect(&vertical, true, true, &[]),
            Some(Rect::new(2, 8, 40, 2))
        );

        let edge = crate::layout::SplitBorder {
            pos: 0,
            direction: ratatui::layout::Direction::Horizontal,
            ratio: 0.5,
            area: Rect::new(0, 0, 1, 4),
            path: Vec::new(),
        };
        assert_eq!(
            split_hit_rect(&edge, true, true, &[]),
            Some(Rect::new(0, 0, 1, 4))
        );
        assert_eq!(split_hit_rect(&edge, false, true, &[]), None);
        assert_eq!(
            split_hit_rect(&horizontal, false, true, &[Rect::new(19, 3, 1, 12)]),
            None
        );
    }

    fn rounded_corner_changes(
        build: impl Fn() -> crate::workspace::Workspace,
        configure: impl Fn(&mut crate::config::Config),
    ) -> Vec<((u16, u16), String, String)> {
        const COLS: u16 = 100;
        const ROWS: u16 = 30;
        let mut frames = Vec::new();
        for rounded in [false, true] {
            let mut config = crate::config::Config::default();
            configure(&mut config);
            config.ui.rounded_borders = rounded;
            let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
            let mut app = crate::app::App::new(
                &config,
                crate::app::AppPolicy::TEST,
                None,
                api_rx,
                crate::api::EventHub::default(),
            );
            app.state.workspaces = vec![build()];
            app.state.active = Some(0);
            app.state.selected = 0;
            let mut client = crate::client::ClientShellState::new(
                crate::client::ClientShellConfig::from_config(&config),
            );
            client.set_snapshot(Box::new(snapshot(&app, "boot", 1, None, None)));
            let size = client.surface_size(COLS, ROWS);
            let target = Some(crate::ui::TabSurfaceTarget {
                workspace_index: 0,
                tab_index: app.state.workspaces[0].active_tab_index(),
            });
            let rendered = render_pane_surface(
                &mut app,
                target,
                Rect::new(0, 0, size.cols, size.rows),
                true,
                true,
                crate::kitty_graphics::HostCellSize {
                    width_px: 1,
                    height_px: 1,
                },
                &crate::kitty_graphics::surface::DeliveryCache::default(),
                1,
            )
            .expect("pane surface");
            client.set_pane_surface(crate::protocol::PaneSurfaceFrame {
                boot_id: "boot".into(),
                projection_revision: 1,
                surface_revision: 0,
                frame: rendered.frame,
                panes: rendered.panes,
                splits: rendered.splits,
                popup: rendered.popup,
                graphics: rendered.graphics,
            });
            frames.push(client.compose(COLS, ROWS).expect("composed frame").frame);
        }
        let (square, rounded) = (&frames[0], &frames[1]);
        let symbols: Vec<&str> = square
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect();
        assert!(
            symbols.windows(3).any(|w| w == ["┌", " ", "B"]),
            "terminal content control must render"
        );
        assert_eq!(rounded.cursor, square.cursor);
        assert_eq!(rounded.hyperlinks, square.hyperlinks);
        let mut changes = Vec::new();
        for (index, (before, after)) in square.cells.iter().zip(&rounded.cells).enumerate() {
            if before == after {
                continue;
            }
            let mut expected = before.clone();
            expected.symbol = crate::ui::rounded_light_corner(&before.symbol)
                .expect("only light corners may change")
                .into();
            assert_eq!(after, &expected);
            let point = (
                (index % usize::from(COLS)) as u16,
                (index / usize::from(COLS)) as u16,
            );
            changes.push((point, before.symbol.clone(), after.symbol.clone()));
        }
        changes
    }

    fn rounded_test_workspace(panes: usize, zoomed: bool) -> crate::workspace::Workspace {
        let mut workspace = crate::workspace::Workspace::test_new("rounded");
        let root = workspace.tabs[0].root_pane;
        workspace.insert_test_runtime(
            root,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 23, b"\xe2\x94\x8c BOX"),
        );
        for _ in 1..panes {
            let pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
            workspace.insert_test_runtime(
                pane,
                crate::terminal::TerminalRuntime::test_with_screen_bytes(
                    80,
                    23,
                    b"\xe2\x94\x8c BOX",
                ),
            );
        }
        workspace.tabs[0].zoomed = zoomed;
        workspace
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rounded_borders_follow_real_pane_geometry_through_the_client() {
        let changes = |panes, zoomed, configure: fn(&mut crate::config::Config)| {
            rounded_corner_changes(|| rounded_test_workspace(panes, zoomed), configure)
        };
        let corners = |c: &Vec<((u16, u16), String, String)>| {
            let mut arcs: Vec<&str> = c.iter().map(|(_, _, after)| after.as_str()).collect();
            arcs.sort_unstable();
            arcs.join("")
        };
        assert!(
            changes(1, false, |_| {}).is_empty(),
            "lone auto pane stays unframed"
        );
        let lone_always = changes(1, false, |config| {
            config.ui.pane_borders = crate::config::PaneBordersConfig::Always
        });
        assert_eq!(
            corners(&lone_always),
            "╭╮╯╰",
            "lone pane framed by pane_borders=always rounds its four corners"
        );
        assert!(
            changes(2, false, |config| config.ui.pane_outer_borders = false).is_empty(),
            "outer borders off leaves only a straight divider"
        );
        let zoomed = changes(2, true, |_| {});
        assert_eq!(
            corners(&zoomed),
            "╭╮╯╰",
            "zoom rounds only the visible frame"
        );
        let shared = changes(2, false, |config| config.ui.pane_gaps = false);
        assert_eq!(
            corners(&shared),
            "╭╮╯╰",
            "shared grid rounds only outer corners"
        );
        assert_eq!(zoomed, shared);
        let gaps = changes(2, false, |_| {});
        assert_eq!(
            corners(&gaps),
            "╭╭╮╮╯╯╰╰",
            "separated panes round each frame"
        );
    }

    fn focus_weight_app(pane_focus_weight: bool) -> (crate::app::App, [crate::layout::PaneId; 2]) {
        let mut config = crate::config::Config::default();
        config.ui.pane_focus_weight = pane_focus_weight;
        split_shell_app(&config)
    }

    /// Two side-by-side panes, left focused, from a real `App::new(config)`.
    fn split_shell_app(
        config: &crate::config::Config,
    ) -> (crate::app::App, [crate::layout::PaneId; 2]) {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = crate::workspace::Workspace::test_new("focus");
        let left = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(left);
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view_with_runtime_registry(
            &mut app.state,
            &crate::terminal::TerminalRuntimeRegistry::new(),
            Rect::new(0, 0, 40, 12),
        );
        (app, [left, right])
    }

    fn focus_weight_surface(app: &mut crate::app::App) -> RenderedPaneSurface {
        let target = Some(crate::ui::TabSurfaceTarget {
            workspace_index: 0,
            tab_index: app.state.workspaces[0].active_tab_index(),
        });
        render_pane_surface(
            app,
            target,
            Rect::new(0, 0, 40, 12),
            true,
            true,
            crate::kitty_graphics::HostCellSize {
                width_px: 1,
                height_px: 1,
            },
            &crate::kitty_graphics::surface::DeliveryCache::default(),
            1,
        )
        .expect("pane surface")
    }

    fn surface_corner(surface: &RenderedPaneSurface, pane_index: usize) -> &str {
        let rect = surface.panes[pane_index].rect;
        let index = usize::from(rect.y) * usize::from(surface.frame.width) + usize::from(rect.x);
        &surface.frame.cells[index].symbol
    }

    /// Pane geometry with public ids cleared; each test app gets fresh ids.
    fn anonymous_panes(surface: &RenderedPaneSurface) -> Vec<protocol::PaneSurfacePane> {
        surface
            .panes
            .iter()
            .cloned()
            .map(|mut pane| {
                pane.pane_id.clear();
                pane
            })
            .collect()
    }

    /// Geometry, hit data and every non-symbol cell attribute are unchanged; only
    /// light line glyphs on the focused frame become heavy.
    fn assert_glyph_only_surface_change(
        plain: &RenderedPaneSurface,
        emphasized: &RenderedPaneSurface,
    ) {
        assert_eq!(anonymous_panes(plain), anonymous_panes(emphasized));
        assert_eq!(plain.splits, emphasized.splits);
        assert_eq!(plain.frame.width, emphasized.frame.width);
        assert_eq!(plain.frame.height, emphasized.frame.height);
        assert_eq!(plain.frame.cursor, emphasized.frame.cursor);
        let mut changed = 0;
        for (before, after) in plain.frame.cells.iter().zip(&emphasized.frame.cells) {
            assert_eq!(
                (
                    before.fg,
                    before.bg,
                    before.modifier,
                    before.skip,
                    before.hyperlink
                ),
                (
                    after.fg,
                    after.bg,
                    after.modifier,
                    after.skip,
                    after.hyperlink
                )
            );
            if before.symbol != after.symbol {
                assert!("─│┌┐└┘├┤┬┴┼".contains(before.symbol.as_str()));
                changed += 1;
            }
        }
        assert!(changed > 0);
    }

    #[test]
    fn pane_focus_weight_follows_keyboard_and_mouse_focus_with_unchanged_geometry() {
        let (mut plain_app, _) = focus_weight_app(false);
        let (mut app, [left, right]) = focus_weight_app(true);
        let left_index = |surface: &RenderedPaneSurface, app: &crate::app::App| {
            let public = app.public_pane_id(0, left).unwrap();
            surface
                .panes
                .iter()
                .position(|pane| pane.pane_id == public)
                .unwrap()
        };

        let plain = focus_weight_surface(&mut plain_app);
        let surface = focus_weight_surface(&mut app);
        assert_glyph_only_surface_change(&plain, &surface);
        let left_at = left_index(&surface, &app);
        let right_at = 1 - left_at;
        assert_eq!(surface_corner(&plain, left_at), "┌");
        assert_eq!(surface_corner(&surface, left_at), "┏");
        assert_eq!(surface_corner(&surface, right_at), "┌");

        // Keyboard focus movement goes through `pane.focus_direction`.
        app.handle_api_request(crate::api::schema::Request {
            id: "keyboard".into(),
            method: crate::api::schema::Method::PaneFocusDirection(
                crate::api::schema::PaneFocusDirectionParams {
                    pane_id: app.public_pane_id(0, left),
                    direction: crate::api::schema::PaneDirection::Right,
                },
            ),
        });
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
        let surface = focus_weight_surface(&mut app);
        assert_eq!(surface_corner(&surface, left_at), "┌");
        assert_eq!(surface_corner(&surface, right_at), "┏");
        assert_eq!(
            anonymous_panes(&surface),
            anonymous_panes(&plain)
                .into_iter()
                .map(|mut pane| {
                    pane.focused = !pane.focused;
                    pane
                })
                .collect::<Vec<_>>()
        );

        // A mouse click on a pane sends `pane.focus`.
        app.handle_api_request(crate::api::schema::Request {
            id: "mouse".into(),
            method: crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget {
                pane_id: app.public_pane_id(0, left).unwrap(),
            }),
        });
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(left));
        let surface = focus_weight_surface(&mut app);
        assert_eq!(surface_corner(&surface, left_at), "┏");
        assert_eq!(surface_corner(&surface, right_at), "┌");
        assert_glyph_only_surface_change(&plain, &surface);
    }

    #[test]
    fn pane_focus_weight_zoomed_pane_is_heavy() {
        let (mut plain_app, _) = focus_weight_app(false);
        let (mut app, _) = focus_weight_app(true);
        plain_app.state.workspaces[0].tabs[0].zoomed = true;
        app.state.workspaces[0].tabs[0].zoomed = true;

        let plain = focus_weight_surface(&mut plain_app);
        let surface = focus_weight_surface(&mut app);
        assert_eq!(surface.panes.len(), 1);
        assert_glyph_only_surface_change(&plain, &surface);
        assert_eq!(surface_corner(&plain, 0), "┌");
        assert_eq!(surface_corner(&surface, 0), "┏");
    }

    fn heavy_borders_app(
        pane_heavy_borders: bool,
        pane_focus_weight: bool,
    ) -> (crate::app::App, [crate::layout::PaneId; 2]) {
        let mut config = crate::config::Config::default();
        config.ui.pane_heavy_borders = pane_heavy_borders;
        config.ui.pane_focus_weight = pane_focus_weight;
        let (app, panes) = split_shell_app(&config);
        // The option reaches the renderer through `App::new` like `pane_gaps`.
        assert_eq!(app.state.pane_heavy_borders, pane_heavy_borders);
        (app, panes)
    }

    #[test]
    fn pane_heavy_borders_keep_geometry_and_follow_focus() {
        let (mut plain_app, _) = heavy_borders_app(false, false);
        let plain = focus_weight_surface(&mut plain_app);
        for focus_weight in [false, true] {
            let (mut app, [left, right]) = heavy_borders_app(true, focus_weight);
            let surface = focus_weight_surface(&mut app);
            assert_glyph_only_surface_change(&plain, &surface);
            let public_left = app.public_pane_id(0, left).unwrap();
            let left_at = surface
                .panes
                .iter()
                .position(|pane| pane.pane_id == public_left)
                .unwrap();
            let right_at = 1 - left_at;
            let focused_corner = if focus_weight { "╔" } else { "┏" };
            assert_eq!(surface_corner(&plain, left_at), "┌");
            assert_eq!(surface_corner(&surface, left_at), focused_corner);
            assert_eq!(surface_corner(&surface, right_at), "┏");

            app.handle_api_request(crate::api::schema::Request {
                id: "keyboard".into(),
                method: crate::api::schema::Method::PaneFocusDirection(
                    crate::api::schema::PaneFocusDirectionParams {
                        pane_id: app.public_pane_id(0, left),
                        direction: crate::api::schema::PaneDirection::Right,
                    },
                ),
            });
            assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
            let surface = focus_weight_surface(&mut app);
            assert_eq!(surface_corner(&surface, left_at), "┏");
            assert_eq!(surface_corner(&surface, right_at), focused_corner);
            assert_eq!(
                anonymous_panes(&surface),
                anonymous_panes(&plain)
                    .into_iter()
                    .map(|mut pane| {
                        pane.focused = !pane.focused;
                        pane
                    })
                    .collect::<Vec<_>>()
            );
            assert_eq!(surface.splits, plain.splits);
        }
    }

    #[test]
    fn pane_heavy_borders_zoomed_pane() {
        let (mut plain_app, _) = heavy_borders_app(false, false);
        plain_app.state.workspaces[0].tabs[0].zoomed = true;
        let plain = focus_weight_surface(&mut plain_app);
        for (focus_weight, corner) in [(false, "┏"), (true, "╔")] {
            let (mut app, _) = heavy_borders_app(true, focus_weight);
            app.state.workspaces[0].tabs[0].zoomed = true;
            let surface = focus_weight_surface(&mut app);
            assert_eq!(surface.panes.len(), 1);
            assert_glyph_only_surface_change(&plain, &surface);
            assert_eq!(surface_corner(&plain, 0), "┌");
            assert_eq!(surface_corner(&surface, 0), corner);
        }
    }
}
