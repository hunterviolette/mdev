use eframe::egui;

use super::super::actions::{Action, ComponentId, ComponentKind};
use super::super::state::AppState;
use crate::model::AnalysisResult;


use super::{changeset_applier, context_exporter, diff_viewer, changeset_loop, file_viewer, source_control, summary_panel, task_panel, terminal, tree_panel};

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

pub fn canvas(ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];


    // Do not hold a borrow of state.results across the large UI closure.
    // This keeps the borrow checker happy while still avoiding a deep clone.
    let has_results = state.results.result.is_some();

    let canvas_rect = ctx
        .data_mut(|d| d.get_persisted::<egui::Rect>(canvas_rect_id()))
        .unwrap_or_else(|| ctx.available_rect());

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.allocate_space(ui.available_size());
    });

    egui::Area::new(egui::Id::new("canvas_area"))
        .fixed_pos(canvas_rect.min)
        .constrain_to(canvas_rect)
        .show(ctx, |ui| {
            let clip_rect = canvas_rect.translate(-canvas_rect.min.to_vec2());
            ui.set_clip_rect(clip_rect);

            // Optional faint canvas background tint (per-workspace).
            // Drawn behind all component windows.
            if let Some([r, g, b, a]) = state.ui.canvas_bg_tint {
                let col = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                ui.painter().rect_filled(clip_rect, 0.0, col);
            }

            let max_canvas_w = clip_rect.width().max(1.0);
            let max_canvas_h = clip_rect.height().max(1.0);

            // Render all components in layout order.
            // Avoid cloning ComponentInstance list each frame; collect only what we need.
            let components: Vec<(ComponentId, ComponentKind, String)> = state
                .active_layout()
                .components
                .iter()
                .map(|c| (c.id, c.kind, c.title.clone()))
                .collect();

            for (id, kind, title_base) in components {
                let Some(w0) = state.active_layout().get_window(id).cloned() else { continue };
                if !w0.open {
                    continue;
                }

                let mut open = true;
                let default_pos = egui::pos2(w0.pos[0], w0.pos[1]);
                let default_size = egui::vec2(
                    w0.size[0].clamp(150.0, max_canvas_w),
                    w0.size[1].clamp(120.0, max_canvas_h),
                );

                let canvas_epoch = state.active_canvas_state().layout_epoch;
                let window_id = egui::Id::new(("canvas_window", state.active_canvas, canvas_epoch, id));

                let title = if kind == ComponentKind::FileViewer {
                    if state.active_file_viewer_id() == Some(id) {
                        format!("{}  (active)", title_base)
                    } else {
                        title_base.clone()
                    }
                } else {
                    title_base.clone()
                };

                let window = egui::Window::new(title)
                    .id(window_id)
                    .open(&mut open)
                    .default_pos(default_pos)
                    .default_size(default_size)
                    .movable(!w0.locked)
                    .resizable(!w0.locked)
                    .collapsible(true);

                let shown = window.show(ui.ctx(), |ui| {
                    let content_size = ui.available_size();

                    // Allow some panels to run WITHOUT analysis results.
                    match kind {
                        ComponentKind::Terminal => {
                            actions.extend(terminal::terminal_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        ComponentKind::ContextExporter => {
                            actions.extend(context_exporter::context_exporter(ui, state, id));
                            return content_size;
                        }
                        ComponentKind::ChangeSetApplier => {
                            actions.extend(changeset_applier::changeset_applier_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        ComponentKind::ExecuteLoop => {
                            actions.extend(changeset_loop::changeset_loop_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        ComponentKind::Task => {
                            actions.extend(task_panel::task_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        ComponentKind::SourceControl => {
                            actions.extend(source_control::source_control_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        ComponentKind::DiffViewer => {
                            if ui.rect_contains_pointer(ui.max_rect()) {
                                state.set_active_diff_viewer_id(Some(id));
                            }
                            actions.extend(diff_viewer::diff_viewer_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        _ => {}
                    }

                    // For Tree/FileViewer/Summary we want analysis results
                    if !has_results {
                        ui.label("No analysis results yet");
                        return content_size;
                    }

                    // IMPORTANT: do not hold an immutable borrow of `state.results` while calling panel fns
                    // that take `&mut AppState`. Use a raw pointer to avoid borrow conflicts without cloning.
                    let res_ptr: *const AnalysisResult = match state.results.result.as_ref() {
                        Some(r) => r as *const AnalysisResult,
                        None => {
                            ui.label("Select a repo (it will auto-run), or click Run.");
                            return content_size;
                        }
                    };

                    match kind {
                        ComponentKind::Tree => {
                            let res = unsafe { &*res_ptr };
                            actions.extend(tree_panel::tree_panel(ctx, ui, state, res));
                        }
                        ComponentKind::FileViewer => {
                            if ui.rect_contains_pointer(ui.max_rect()) {
                                state.set_active_file_viewer_id(Some(id));
                            }
                            actions.extend(file_viewer::file_viewer(ctx, ui, state, id));
                        }
                        ComponentKind::Summary => {
                            let res: &AnalysisResult = unsafe { &*res_ptr };
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                summary_panel::summary_panel(ui, res);
                            });
                        }
                        ComponentKind::Terminal
                        | ComponentKind::ContextExporter
                        | ComponentKind::ChangeSetApplier
                        | ComponentKind::ExecuteLoop
                        | ComponentKind::Task
                        | ComponentKind::SourceControl
                        | ComponentKind::DiffViewer => {
                            // handled above
                        }
                    }

                    content_size
                });

                if let Some(w) = state.active_layout_mut().get_window_mut(id) {
                    w.open = open;

                    if let Some(inner) = shown {
                        let mut outer_rect = inner.response.rect;
                        if outer_rect.min.y < 0.0 {
                            outer_rect = outer_rect.translate(egui::vec2(0.0, -outer_rect.min.y));
                        }
                        if outer_rect.min.x < 0.0 {
                            outer_rect = outer_rect.translate(egui::vec2(-outer_rect.min.x, 0.0));
                        }

                        w.pos = [outer_rect.min.x, outer_rect.min.y];

                        if let Some(cs) = inner.inner {
                            w.size = [
                                cs.x.clamp(150.0, max_canvas_w),
                                cs.y.clamp(120.0, max_canvas_h),
                            ];
                        } else {
                            w.size = [
                                w.size[0].clamp(150.0, max_canvas_w),
                                w.size[1].clamp(120.0, max_canvas_h),
                            ];
                        }
                    }
                }
            }
        });

    actions
}
