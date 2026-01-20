use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::layout::{ComponentInstance, LayoutConfig, WindowLayout};
use crate::app::state::{AppState, ChangeSetApplierState, ContextExportMode, ContextExporterState};

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::AddComponent { kind } => {
            state.add_component(*kind);
            true
        }
        Action::FocusFileViewer(id) => {
            state.active_file_viewer = Some(*id);
            true
        }
        Action::CloseComponent(id) => {
            state.close_component(*id);
            true
        }
        Action::ToggleLock(id) => {
            if let Some(w) = state.layout.get_window_mut(*id) {
                w.locked = !w.locked;
            }
            true
        }
        Action::ResetLayout => {
            state.layout = LayoutConfig::default();
            state.layout.merge_with_defaults();
            state.layout_epoch = state.layout_epoch.wrapping_add(1);

            // Ensure default FV exists
            state
                .file_viewers
                .entry(2)
                .or_insert_with(crate::app::state::FileViewerState::new);
            state.active_file_viewer = Some(2);

            // Ephemeral components rebuilt from layout
            state.rebuild_terminals_from_layout();
            state.rebuild_context_exporters_from_layout();
            state.rebuild_changeset_appliers_from_layout();
            state.rebuild_source_controls_from_layout();
            state.rebuild_diff_viewers_from_layout();
            true
        }
        _ => false,
    }
}

impl AppState {
    fn add_component(&mut self, kind: ComponentKind) {
        match kind {
            ComponentKind::FileViewer => self.new_file_viewer(),
            ComponentKind::Terminal => self.new_terminal(),
            ComponentKind::DiffViewer => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = format!("Diff Viewer {}", id);

                self.layout.components.push(ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos: [180.0, 180.0],
                        size: [980.0, 720.0],
                    },
                );

                self.diff_viewers.insert(id, crate::app::state::DiffViewerState::new());
                self.active_diff_viewer = Some(id);

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }


            ComponentKind::ContextExporter => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = format!("Context Exporter {}", id);

                self.layout.components.push(ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos: [120.0, 120.0],
                        size: [560.0, 260.0],
                    },
                );

                self.context_exporters.insert(
                    id,
                    ContextExporterState {
                        save_path: None,
                        max_bytes_per_file: 200_000,
                        skip_binary: true,
                        mode: ContextExportMode::EntireRepo,
                        status: None,
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::SourceControl => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = format!("Source Control {}", id);

                self.layout.components.push(ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos: [160.0, 160.0],
                        size: [760.0, 620.0],
                    },
                );

                self.source_controls.insert(
                    id,
                    crate::app::state::SourceControlState {
                        branch: "".to_string(),
                        branch_options: vec![],
                        remote: "origin".to_string(),
                        remote_options: vec!["origin".to_string()],
                        commit_message: String::new(),
                        files: vec![],
                        selected: std::collections::HashSet::new(),
                        last_output: None,
                        last_error: None,
                        needs_refresh: true,
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::ChangeSetApplier => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = format!("ChangeSet Applier {}", id);

                self.layout.components.push(ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos: [140.0, 140.0],
                        size: [640.0, 520.0],
                    },
                );

                self.changeset_appliers.insert(
                    id,
                    ChangeSetApplierState {
                        payload: String::new(),
                        status: None,
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::Tree | ComponentKind::Summary => {
                self.layout.merge_with_defaults();

                let id = self.layout.next_free_id();
                let title = match kind {
                    ComponentKind::Tree => format!("Tree {}", id),
                    ComponentKind::Summary => format!("Summary {}", id),
                    ComponentKind::FileViewer
                    | ComponentKind::Terminal
                    | ComponentKind::ContextExporter
                    | ComponentKind::SourceControl
                    | ComponentKind::ChangeSetApplier
                    | ComponentKind::DiffViewer => unreachable!(),
                };

                self.layout
                    .components
                    .push(ComponentInstance { id, kind, title });

                self.layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos: [80.0, 80.0],
                        size: [520.0, 700.0],
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }
        }
    }

    fn new_file_viewer(&mut self) {
        self.layout.merge_with_defaults();

        let id = self.layout.next_free_id();

        let fv_count = self
            .layout
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::FileViewer)
            .count();
        let title = format!("File Viewer {}", fv_count + 1);

        self.layout.components.push(ComponentInstance {
            id,
            kind: ComponentKind::FileViewer,
            title,
        });

        self.layout.windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos: [60.0, 60.0],
                size: [760.0, 700.0],
            },
        );

        self.file_viewers
            .insert(id, crate::app::state::FileViewerState::new());

        self.active_file_viewer = Some(id);
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn close_component(&mut self, id: ComponentId) {
        if let Some(w) = self.layout.get_window_mut(id) {
            w.open = false;
        }

        // Clean up ephemeral component state (safe no-op if not present)
        self.context_exporters.remove(&id);
        self.terminals.remove(&id);
        self.changeset_appliers.remove(&id); // NEW
        self.source_controls.remove(&id);
        self.source_controls.remove(&id); // NEW

        if self.active_file_viewer == Some(id) {
            self.active_file_viewer = self
                .layout
                .components
                .iter()
                .find(|c| c.kind == ComponentKind::FileViewer && c.id != id)
                .map(|c| c.id);
        }
    }
}
