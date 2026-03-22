use crate::app::actions::{Action, ComponentId, ComponentKind};
use crate::app::layout::{ComponentInstance, WindowLayout};
use crate::app::state::{AppState, ChangeSetApplierState, ContextExportMode, ContextExporterState};
use crate::app::state::ExecuteLoopState;

pub fn handle(state: &mut AppState, action: &Action) -> bool {
    match action {
        Action::AddComponent { kind } => {
            state.add_component(*kind);
            true
        }
        Action::FocusFileViewer(id) => {
            state.set_active_file_viewer_id(Some(*id));
            true
        }
        Action::CloseComponent(id) => {
            state.close_component(*id);
            true
        }
        Action::ToggleLock(id) => {
            if let Some(w) = state.active_layout_mut().get_window_mut(*id) {
                w.locked = !w.locked;
            }
            true
        }
        Action::ResetLayout => {
            state.load_workspace_from_appdata(None);
            true
        }
        Action::CanvasSelect { index } => {
            state.canvas_select(*index);
            true
        }
        Action::CanvasAdd => {
            state.canvas_add();
            true
        }
        Action::CanvasRename { index, name } => {
            state.canvas_rename(*index, name.clone());
            true
        }
        Action::CanvasDelete { index } => {
            state.canvas_delete(*index);
            true
        }
        _ => false,
    }
}

impl AppState {
    pub fn remap_default_layout_ids(&mut self) -> (crate::app::layout::LayoutConfig, ComponentId) {
        let mut layout = crate::app::layout::LayoutConfig::default();
        layout.merge_with_defaults();

        let mut map = std::collections::HashMap::<ComponentId, ComponentId>::new();
        for c in layout.components.iter() {
            map.insert(c.id, self.alloc_component_id());
        }

        for c in layout.components.iter_mut() {
            if let Some(nid) = map.get(&c.id).cloned() {
                c.id = nid;
            }
        }

        let mut windows = std::collections::HashMap::new();
        for (id, w) in layout.windows.iter() {
            if let Some(nid) = map.get(id).cloned() {
                windows.insert(nid, w.clone());
            }
        }
        layout.windows = windows;

        let fv_id = layout
            .components
            .iter()
            .find(|c| c.kind == ComponentKind::FileViewer)
            .map(|c| c.id)
            .unwrap_or_else(|| {
                let id = self.alloc_component_id();
                layout.components.push(ComponentInstance {
                    id,
                    kind: ComponentKind::FileViewer,
                    title: "File Viewer".to_string(),
                });
                layout.windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [60.0, 60.0],
                        size: [760.0, 700.0],
                    },
                );
                id
            });

        (layout, fv_id)
    }

    fn add_component(&mut self, kind: ComponentKind) {
        match kind {
            ComponentKind::FileViewer => self.new_file_viewer(),
            ComponentKind::Terminal => self.new_terminal(),
            ComponentKind::Task => {
                let id = self.alloc_component_id();
                let title = format!("Task {}", id);

                self.active_layout_mut().components.push(ComponentInstance { id, kind, title });
                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [200.0, 200.0],
                        size: [520.0, 260.0],
                    },
                );

                self.tasks.entry(id).or_default();

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::DiffViewer => {
                        let id = self.alloc_component_id();
                let title = format!("Diff Viewer {}", id);

                self.active_layout_mut().components.push(ComponentInstance { id, kind, title });

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [180.0, 180.0],
                        size: [980.0, 720.0],
                    },
                );

                self.diff_viewers.insert(id, crate::app::state::DiffViewerState::new());
                self.set_active_diff_viewer_id(Some(id));

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::SapAdt => {
                let existing_id = self
                    .all_layouts()
                    .flat_map(|l| l.components.iter())
                    .find(|c| c.kind == ComponentKind::SapAdt)
                    .map(|c| c.id)
                    .or_else(|| self.sap_adts.keys().copied().next());
                let id = match existing_id {
                    Some(id) => id,
                    None => self.alloc_component_id(),
                };
                let title = "SAP ADT".to_string();

                let already_present = self
                    .active_layout()
                    .components
                    .iter()
                    .any(|c| c.id == id && c.kind == ComponentKind::SapAdt);

                if !already_present {
                    self.active_layout_mut().components.push(ComponentInstance { id, kind, title });
                }

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [220.0, 220.0],
                        size: [760.0, 520.0],
                    },
                );

                self.sap_adts.entry(id).or_insert_with(crate::app::state::SapAdtState::new);
                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }


            ComponentKind::ContextExporter => {
                self.active_layout_mut().merge_with_defaults();

                let id = self.alloc_component_id();
                let title = format!("Context Exporter {}", id);

                self.active_layout_mut().components.push(ComponentInstance { id, kind, title });

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [120.0, 120.0],
                        size: [560.0, 260.0],
                    },
                );

                self.context_exporters.insert(
                    id,
                    ContextExporterState {
                        save_path: None,
                        skip_binary: true,
                        skip_gitignore: true,
                        include_staged_diff: false,
                        include_unstaged_diff: false,
                        mode: ContextExportMode::EntireRepo,
                        status: None,
                        selection_defaults: std::collections::HashSet::new(),
                        export_pending: false,
                        export_rx: None,
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }

            ComponentKind::SourceControl => {
                self.active_layout_mut().merge_with_defaults();

                let id = self.alloc_component_id();
                let title = format!("Source Control {}", id);

                self.active_layout_mut().components.push(ComponentInstance { id, kind, title });

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
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
                self.active_layout_mut().merge_with_defaults();

                let id = self.alloc_component_id();
                let title = format!("ChangeSet Applier {}", id);

                self.active_layout_mut().components.push(ComponentInstance { id, kind, title });

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
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

            ComponentKind::ExecuteLoop => {
                self.new_execute_loop_component();
            }

            ComponentKind::Tree | ComponentKind::Summary => {
                self.active_layout_mut().merge_with_defaults();

                let id = self.alloc_component_id();
                let title = match kind {
                    ComponentKind::Tree => format!("Tree {}", id),
                    ComponentKind::Summary => format!("Summary {}", id),
                    ComponentKind::FileViewer
                    | ComponentKind::Terminal
                    | ComponentKind::ContextExporter
                    | ComponentKind::SourceControl
                    | ComponentKind::ChangeSetApplier
                    | ComponentKind::ExecuteLoop
                    | ComponentKind::Task
                    | ComponentKind::DiffViewer
                    | ComponentKind::SapAdt => unreachable!(),
                };

                self.active_layout_mut()
                    .components
                    .push(ComponentInstance { id, kind, title });

                self.active_layout_mut().windows.insert(
                    id,
                    WindowLayout {
                        open: true,
                        locked: false,
                        pos_norm: None,
                        size_norm: None,
                        pos: [80.0, 80.0],
                        size: [520.0, 700.0],
                    },
                );

                self.layout_epoch = self.layout_epoch.wrapping_add(1);
            }
        }
    }

    pub fn new_execute_loop_component(&mut self) -> ComponentId {
        self.active_layout_mut().merge_with_defaults();

        let id = self.alloc_component_id();
        let title = format!("Execute Loop {}", id);

        self.active_layout_mut().components.push(ComponentInstance {
            id,
            kind: ComponentKind::ExecuteLoop,
            title,
        });

        self.active_layout_mut().windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [150.0, 150.0],
                size: [860.0, 680.0],
            },
        );

        self.execute_loops.insert(id, ExecuteLoopState::new());

        self.layout_epoch = self.layout_epoch.wrapping_add(1);
        id
    }

    fn new_file_viewer(&mut self) {
        self.active_layout_mut().merge_with_defaults();

        let id = self.alloc_component_id();

        let fv_count = self
            .active_layout()
            .components
            .iter()
            .filter(|c| c.kind == ComponentKind::FileViewer)
            .count();
        let title = format!("File Viewer {}", fv_count + 1);

        self.active_layout_mut().components.push(ComponentInstance {
            id,
            kind: ComponentKind::FileViewer,
            title,
        });

        self.active_layout_mut().windows.insert(
            id,
            WindowLayout {
                open: true,
                locked: false,
                pos_norm: None,
                size_norm: None,
                pos: [60.0, 60.0],
                size: [760.0, 700.0],
            },
        );

        self.file_viewers
            .insert(id, crate::app::state::FileViewerState::new());

        self.set_active_file_viewer_id(Some(id));
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    fn close_component(&mut self, id: ComponentId) {
        if let Some(w) = self.active_layout_mut().get_window_mut(id) {
            w.open = false;
        }

        self.context_exporters.remove(&id);
        self.terminals.remove(&id);
        self.changeset_appliers.remove(&id);
        self.persist_execute_loop_snapshot(id);
        if self.task_store_dirty {
            self.save_repo_task_store();
        }

        self.execute_loops.remove(&id);
        self.source_controls.remove(&id);
        self.diff_viewers.remove(&id);

        for canvas in self.canvases.iter_mut() {
            if canvas.active_file_viewer == Some(id) {
                canvas.active_file_viewer = canvas
                    .layout
                    .components
                    .iter()
                    .find(|c| c.kind == ComponentKind::FileViewer && c.id != id)
                    .map(|c| c.id);
            }
            if canvas.active_diff_viewer == Some(id) {
                canvas.active_diff_viewer = None;
            }
        }
    }
}
