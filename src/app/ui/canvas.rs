use eframe::egui;

use std::collections::HashSet;

use super::super::actions::{Action, ComponentId, ComponentKind};
use super::super::state::AppState;
use crate::model::AnalysisResult;


use super::{changeset_applier, context_exporter, diff_viewer, changeset_loop, file_viewer, sap_adt, source_control, summary_panel, task_panel, terminal, tree_panel};

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

pub fn canvas(ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];
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

            if let Some([r, g, b, a]) = state.ui.canvas_bg_tint {
                let col = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                ui.painter().rect_filled(clip_rect, 0.0, col);
            }

            let ppp = ctx.pixels_per_point().max(1.0);
            let snap_round = |v: f32| (v * ppp).round() / ppp;
            let snap_floor = |v: f32| (v * ppp).floor() / ppp;

            let origin = egui::pos2(
                snap_round(canvas_rect.min.x),
                snap_round(canvas_rect.min.y),
            );
            let max_canvas_w = snap_floor(clip_rect.width().max(1.0));
            let max_canvas_h = snap_floor(clip_rect.height().max(1.0));

            let safe_canvas_rect = egui::Rect::from_min_size(origin, egui::vec2(max_canvas_w, max_canvas_h))
                .shrink(0.5 / ppp);

            {
                let canvas_size = [max_canvas_w, max_canvas_h];
                let size_id = egui::Id::new(("canvas_last_size", state.active_canvas));
                let last: Option<[f32; 2]> = ctx.data(|d| d.get_temp(size_id));

                let changed = last
                    .map(|l| (l[0] - canvas_size[0]).abs() > 1.0 || (l[1] - canvas_size[1]).abs() > 1.0)
                    .unwrap_or(true);

                if changed {
                    ctx.data_mut(|d| d.insert_temp(size_id, canvas_size));
                    state.active_canvas_state_mut().layout_epoch =
                        state.active_canvas_state().layout_epoch.wrapping_add(1);
                }
            }

            let components: Vec<(ComponentId, ComponentKind, String)> = state
                .active_layout()
                .components
                .iter()
                .map(|c| (c.id, c.kind, c.title.clone()))
                .collect();

            let mut rendered_ids: HashSet<ComponentId> = HashSet::new();

            for (id, kind, title_base) in components {
                if !rendered_ids.insert(id) {
                    continue;
                }

                if let Some(wm) = state.active_layout_mut().get_window_mut(id) {
                    if wm.pos_norm.is_none() || wm.size_norm.is_none() {
                        wm.ensure_normalized_from_legacy([max_canvas_w, max_canvas_h]);
                    }
                }

                let Some(w0) = state.active_layout().get_window(id).cloned() else { continue };
                if !w0.open {
                    continue;
                }

                let mut open = true;

                let (pos_px, size_px) = w0.denormalized_px([max_canvas_w, max_canvas_h]);

                let default_pos = egui::pos2(
                    snap_round(origin.x + pos_px[0]),
                    snap_round(origin.y + pos_px[1]),
                );

                let min_pt = 1.0 / ppp;
                let default_size = egui::vec2(
                    snap_floor(size_px[0].clamp(min_pt, max_canvas_w)),
                    snap_floor(size_px[1].clamp(min_pt, max_canvas_h)),
                );

                let canvas_epoch = state.active_canvas_state().layout_epoch;
                let window_id = egui::Id::new(("canvas_window", state.active_canvas, canvas_epoch, id));

                let interacting_id = egui::Id::new(("canvas_window_interacting", state.active_canvas, id));
                let was_interacting: bool = ctx.data_mut(|d| d.get_temp::<bool>(interacting_id)).unwrap_or(false);

                let epoch_applied_id = egui::Id::new(("canvas_window_epoch_applied", state.active_canvas, id));
                let canvas_epoch_u64: u64 = canvas_epoch as u64;
                let last_epoch: u64 = ctx.data_mut(|d| d.get_temp::<u64>(epoch_applied_id)).unwrap_or(u64::MAX);
                let epoch_changed = last_epoch != canvas_epoch_u64;
                if epoch_changed {
                    ctx.data_mut(|d| d.insert_temp(epoch_applied_id, canvas_epoch_u64));
                }




                let title = if kind == ComponentKind::FileViewer {
                    if state.active_file_viewer_id() == Some(id) {
                        format!("{}  (active)", title_base)
                    } else {
                        title_base.clone()
                    }
                } else {
                    title_base.clone()
                };

                let target_rect = egui::Rect::from_min_size(default_pos, default_size).intersect(safe_canvas_rect);



                let mut window = egui::Window::new(title)
                    .id(window_id)
                    .open(&mut open)
                    .movable(!w0.locked)
                    .resizable(!w0.locked)
                    .collapsible(true)
                    .min_width(1.0)
                    .min_height(1.0);

                if w0.locked {
                    window = window.fixed_rect(target_rect);
                } else if epoch_changed && !was_interacting {
                    window = window.fixed_rect(target_rect);
                } else {
                    window = window.default_pos(default_pos).default_size(default_size);
                }

                let shown = window.show(ui.ctx(), |ui| {

                    let content_size = ui.available_size();

                    ui.set_min_size(content_size);


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
                        ComponentKind::SapAdt => {
                            actions.extend(sap_adt::sap_adt_panel(ctx, ui, state, id));
                            return content_size;
                        }
                        _ => {}
                    }

                    if !has_results {
                        ui.label("No analysis results yet");
                        return content_size;
                    }

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
                        | ComponentKind::DiffViewer
                        | ComponentKind::SapAdt => {
                        }
                    }

                    content_size
                });

                if let Some(ref shown) = shown {
                    let pointer_down = ctx.input(|i| {
                        i.pointer.primary_down() || i.pointer.secondary_down() || i.pointer.middle_down()
                    });
                    let now_interacting = !w0.locked
                        && (shown.response.dragged() || (pointer_down && shown.response.hovered()));
                    ctx.data_mut(|d| d.insert_temp(interacting_id, now_interacting));

                    if !open {
                        if let Some(w) = state.active_layout_mut().get_window_mut(id) {
                            w.open = false;
                        }
                    }

                    if let Some(w) = state.active_layout_mut().get_window_mut(id) {

                        let outer_rect = shown.response.rect;
                        let pos_px = [outer_rect.min.x - origin.x, outer_rect.min.y - origin.y];
                        let size_px = if let Some(inner_size) = shown.inner {
                            [inner_size.x, inner_size.y]
                        } else {
                            [outer_rect.width(), outer_rect.height()]
                        };

                        let mut pos_px = pos_px;
                        let mut size_px = size_px;

                        size_px[0] = size_px[0].clamp(1.0, max_canvas_w);
                        size_px[1] = size_px[1].clamp(1.0, max_canvas_h);

                        pos_px[0] = pos_px[0].clamp(0.0, (max_canvas_w - 1.0).max(0.0));
                        pos_px[1] = pos_px[1].clamp(0.0, (max_canvas_h - 1.0).max(0.0));

                        w.pos = pos_px;
                        w.size = size_px;

                        let user_changed = !w0.locked && shown.response.dragged();
                        if user_changed {
                        size_px[0] = size_px[0].clamp(1.0, max_canvas_w);
                        size_px[1] = size_px[1].clamp(1.0, max_canvas_h);

                        pos_px[0] = pos_px[0].clamp(0.0, (max_canvas_w - 1.0).max(0.0));
                        pos_px[1] = pos_px[1].clamp(0.0, (max_canvas_h - 1.0).max(0.0));

                        w.set_from_px(pos_px, size_px, [max_canvas_w, max_canvas_h]);
                        }
                    }
                }
            }
        });

    actions
}
