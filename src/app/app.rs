use eframe::egui;

use super::{theme, ui};
use super::AppState;

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

fn current_canvas_size(ctx: &egui::Context) -> [f32; 2] {
    let r = ctx.available_rect();
    [r.width().max(1.0), r.height().max(1.0)]
}

fn viewport_outer_pos(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.input(|i| i.viewport().outer_rect.map(|r| [r.min.x, r.min.y]))
}

fn viewport_inner_size(ctx: &egui::Context) -> Option<[f32; 2]> {
    ctx.input(|i| i.viewport().inner_rect.map(|r| [r.width(), r.height()]))
}

fn current_viewport_inner_size(ctx: &egui::Context) -> Option<[f32; 2]> {
    viewport_inner_size(ctx)
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        

        theme::apply_from_state(ctx, self);

        // Ctrl+Shift+E toggles palette
        let pressed = ctx.input(|i| {
            i.key_pressed(egui::Key::E) && i.modifiers.ctrl && i.modifiers.shift
        });

        // Native window title
        let repo_name = self
            .inputs
            .repo
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("(no repo)");
        let repo_header = self.inputs.git_ref.as_str();
        let workspace_name = self.current_workspace_name.as_str();
        let title = format!("Repo Analyzer - {} - {} - {}", repo_name, repo_header, workspace_name);
        if self.last_window_title.as_deref() != Some(title.as_str()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = Some(title);
        }

        if pressed {
            self.palette.open = !self.palette.open;
            if self.palette.open {
                self.palette.query.clear();
                self.palette.selected = 0;
            }
        }

        // Top bar
        egui::TopBottomPanel::top("top").show(ctx, |ui_top| {
            let actions = ui::top_bar::top_bar(ctx, ui_top, self);
            for a in actions {
                self.apply_action(a);
            }
        });

        // Personalization modal (theme + canvas tint)
        let actions = ui::personalization::personalization(ctx, self);
        for a in actions {
            self.apply_action(a);
        }

        // Persist the "canvas rect" AFTER top panel has taken its space
        let canvas_rect = ctx.available_rect();
        ctx.data_mut(|d| d.insert_persisted(canvas_rect_id(), canvas_rect));

        // Apply viewport restore (best-effort)
        if let Some(vr) = self.pending_viewport_restore.take() {
            if let Some([x, y]) = vr.outer_pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
            if let Some([w, h]) = vr.inner_size {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
            ctx.request_repaint();
        }

        // If we’re waiting to apply a workspace, keep repainting until it settles.
        if self.pending_workspace_apply.is_some() {
            ctx.request_repaint();
        }

        // Workspace apply logic needs these every frame
        let canvas_size = current_canvas_size(ctx);
        let inner_size = current_viewport_inner_size(ctx);
        let _applied = self.try_apply_pending_workspace(canvas_size, inner_size);

        let actions = ui::canvas::canvas(ctx, self);
        for a in actions {
            self.apply_action(a);
        }

        // Command palette (drawn on top)
        let palette_actions = ui::command_palette::command_palette(
            ctx,
            self,
            canvas_size,
            viewport_outer_pos(ctx),
            viewport_inner_size(ctx),
        );
        for a in palette_actions {
            self.apply_action(a);
        }

        // Canvas tint popup (modal; freezes underlying canvas while open)
        // Must be called every frame so it can draw.
        let tint_actions = ui::personalization::personalization(ctx, self);
        for a in tint_actions {
            self.apply_action(a);
        }

        self.finalize_frame();
    }
}
