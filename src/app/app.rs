use eframe::egui;

use super::{theme, ui};
use super::AppState;
use super::state::WORKTREE_REF;

fn canvas_rect_id() -> egui::Id {
    egui::Id::new("canvas_rect_after_top_panel")
}

fn current_canvas_size(ctx: &egui::Context) -> [f32; 2] {
    let r = ctx
        .data_mut(|d| d.get_persisted::<egui::Rect>(canvas_rect_id()))
        .unwrap_or_else(|| ctx.available_rect());

    let ppp = ctx.pixels_per_point().max(1.0);
    let snap_floor = |v: f32| (v * ppp).floor() / ppp;

    [
        snap_floor(r.width().max(1.0)),
        snap_floor(r.height().max(1.0)),
    ]
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


        let now_s = ctx.input(|i| i.time);
        let git_status_interval_s = (self.perf.git_status_poll_ms.max(250) as f64) / 1000.0;
        let analysis_refresh_interval_s = (self.perf.analysis_refresh_poll_ms.max(250) as f64) / 1000.0;
        if self.inputs.repo.is_some() && self.inputs.git_ref == WORKTREE_REF {
            if now_s - self.tree.last_git_status_refresh_s >= git_status_interval_s {
                self.tree.last_git_status_refresh_s = now_s;
                self.refresh_tree_git_status();
            }

            if now_s - self.tree.last_auto_refresh_s >= analysis_refresh_interval_s {
                self.tree.last_auto_refresh_s = now_s;
                if !self.any_file_load_pending() {
                    self.start_analysis_refresh_async();
                }
            }

            if self.tree.analysis_job.is_pending() {
                ctx.request_repaint();
            }
            if self.poll_analysis_refresh() {
                ctx.request_repaint();
            }
        }

        if self.poll_git_status_refresh() {
            ctx.request_repaint();
        }
        if self.poll_diff_stats_refresh() {
            ctx.request_repaint();
        }
        if self.poll_diff_viewer_loads() {
            ctx.request_repaint();
        }
        if self.diff_viewers.values().any(|v| v.loading)
            || self.diff_viewer_jobs.values().any(|job| job.is_pending())
        {
            ctx.request_repaint();
        }

        if self.sap_adts.values().any(|sap| sap.import_job.is_pending() || sap.export_job.is_pending()) {
            ctx.request_repaint();
        }

        let canvas_shortcut = ctx.input(|i| {
            if !i.modifiers.ctrl {
                return None;
            }

            if i.key_pressed(egui::Key::Num1) { return Some(0); }
            if i.key_pressed(egui::Key::Num2) { return Some(1); }
            if i.key_pressed(egui::Key::Num3) { return Some(2); }
            if i.key_pressed(egui::Key::Num4) { return Some(3); }
            if i.key_pressed(egui::Key::Num5) { return Some(4); }
            if i.key_pressed(egui::Key::Num6) { return Some(5); }
            if i.key_pressed(egui::Key::Num7) { return Some(6); }
            if i.key_pressed(egui::Key::Num8) { return Some(7); }
            if i.key_pressed(egui::Key::Num9) { return Some(8); }
            if i.key_pressed(egui::Key::Num0) { return Some(9); }
            None
        });

        let pressed = ctx.input(|i| {
            i.key_pressed(egui::Key::E) && i.modifiers.ctrl && i.modifiers.shift
        });

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

        if let Some(idx) = canvas_shortcut {
            self.apply_action(super::actions::Action::CanvasSelect { index: idx });
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui_top| {
            let actions = ui::top_bar::top_bar(ctx, ui_top, self);
            for a in actions {
                self.apply_action(a);
            }
        });

        let actions = ui::personalization::personalization(ctx, self);
        for a in actions {
            self.apply_action(a);
        }

        let actions = ui::global_settings::global_settings(ctx, self);
        for a in actions {
            self.apply_action(a);
        }

        let r = ctx.available_rect();
        let ppp = ctx.pixels_per_point().max(1.0);
        let snap_round = |v: f32| (v * ppp).round() / ppp;
        let snap_floor = |v: f32| (v * ppp).floor() / ppp;

        let origin = egui::pos2(snap_round(r.min.x), snap_round(r.min.y));
        let size = egui::vec2(snap_floor(r.width().max(1.0)), snap_floor(r.height().max(1.0)));
        let canvas_rect = egui::Rect::from_min_size(origin, size);

        ctx.data_mut(|d| d.insert_persisted(canvas_rect_id(), canvas_rect));

        if let Some(vr) = self.pending_viewport_restore.take() {
            if let Some([x, y]) = vr.outer_pos {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x, y)));
            }
            if let Some([w, h]) = vr.inner_size {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            }
            ctx.request_repaint();
        }

        if self.pending_workspace_apply.is_some() {
            ctx.request_repaint();
        }

        let canvas_size = current_canvas_size(ctx);
        let inner_size = current_viewport_inner_size(ctx);
        let ppp_now = ctx.pixels_per_point().max(1.0);
        let _applied = self.try_apply_pending_workspace(canvas_size, inner_size, ppp_now);

        let actions = ui::canvas::canvas(ctx, self);
        for a in actions {
            self.apply_action(a);
        }

        let palette_actions = ui::command_palette::command_palette(
            ctx,
            self,
            canvas_size,
            viewport_outer_pos(ctx),
            viewport_inner_size(ctx),
            ppp_now,
        );
        for a in palette_actions {
            self.apply_action(a);
        }

        let tint_actions = ui::personalization::personalization(ctx, self);
        for a in tint_actions {
            self.apply_action(a);
        }

        self.finalize_frame();

        if self.poll_file_loads() {
            ctx.request_repaint();
        }
        if self.any_file_load_pending() {
            ctx.request_repaint();
        }
    }
}
