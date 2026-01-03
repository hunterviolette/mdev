use eframe::egui;

use super::super::actions::{Action, ComponentKind};
use super::super::state::AppState;

use super::{context_exporter, file_viewer, summary_panel, terminal, tree_panel};

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

pub fn canvas(ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
    let mut actions = vec![];

    state.layout.merge_with_defaults();

    let res_opt = state.results.result.clone();

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

            let max_canvas_w = clip_rect.width().max(1.0);
            let max_canvas_h = clip_rect.height().max(1.0);

            // Render all components in layout order
            let components = state.layout.components.clone();
            for c in components {
                let Some(w0) = state.layout.get_window(c.id).cloned() else { continue };
                if !w0.open {
                    continue;
                }

                let mut open = true;
                let default_pos = egui::pos2(w0.pos[0], w0.pos[1]);
                let default_size = egui::vec2(
                    w0.size[0].clamp(150.0, max_canvas_w),
                    w0.size[1].clamp(120.0, max_canvas_h),
                );

                let window_id = egui::Id::new(("canvas_window", c.id, state.layout_epoch));

                let title = if c.kind == ComponentKind::FileViewer {
                    if state.active_file_viewer == Some(c.id) {
                        format!("{}  (active)", c.title)
                    } else {
                        c.title.clone()
                    }
                } else {
                    c.title.clone()
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
                    match c.kind {
                        ComponentKind::Terminal => {
                            actions.extend(terminal::terminal_panel(ctx, ui, state, c.id));
                            return content_size;
                        }
                        ComponentKind::ContextExporter => {
                            actions.extend(context_exporter::context_exporter(ui, state, c.id));
                            return content_size;
                        }
                        _ => {}
                    }

                    // For Tree/FileViewer/Summary we want analysis results
                    let Some(res) = res_opt.as_ref() else {
                        ui.label("Select a repo (it will auto-run), or click Run.");
                        return content_size;
                    };

                    match c.kind {
                        ComponentKind::Tree => {
                            actions.extend(tree_panel::tree_panel(ctx, ui, state, res));
                        }
                        ComponentKind::FileViewer => {
                            if ui.rect_contains_pointer(ui.max_rect()) {
                                state.active_file_viewer = Some(c.id);
                            }
                            actions.extend(file_viewer::file_viewer(ctx, ui, state, c.id));
                        }
                        ComponentKind::Summary => {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                summary_panel::summary_panel(ui, res);
                            });
                        }
                        ComponentKind::Terminal | ComponentKind::ContextExporter => {
                            // handled above
                        }
                    }

                    content_size
                });

                if let Some(w) = state.layout.get_window_mut(c.id) {
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
