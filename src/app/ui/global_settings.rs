use eframe::egui;

use crate::app::actions::Action;
use crate::app::state::AppState;

fn centered_rect(screen: egui::Rect, w: f32, h: f32) -> egui::Rect {
    let size = egui::vec2(w, h);
    egui::Rect::from_center_size(screen.center(), size)
}

pub fn global_settings(ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
    let mut actions = Vec::new();

    if !state.ui.global_settings_open {
        return actions;
    }

    let screen = ctx.screen_rect();
    let popup_rect = centered_rect(
        screen,
        (screen.width() - 32.0).min(640.0).max(480.0),
        (screen.height() - 32.0).min(560.0).max(360.0),
    );

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(Action::CloseGlobalSettings);
        return actions;
    }

    let mut clicked_outside = false;

    egui::Area::new(egui::Id::new("global_settings_modal_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());

            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            );

            let top = egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, popup_rect.min.y));
            let bottom = egui::Rect::from_min_max(egui::pos2(screen.min.x, popup_rect.max.y), screen.max);
            let left = egui::Rect::from_min_max(
                egui::pos2(screen.min.x, popup_rect.min.y),
                egui::pos2(popup_rect.min.x, popup_rect.max.y),
            );
            let right = egui::Rect::from_min_max(
                egui::pos2(popup_rect.max.x, popup_rect.min.y),
                egui::pos2(screen.max.x, popup_rect.max.y),
            );

            for r in [top, bottom, left, right] {
                if r.is_positive() {
                    let resp = ui.allocate_rect(r, egui::Sense::click_and_drag());
                    if resp.clicked() {
                        clicked_outside = true;
                    }
                }
            }
        });

    if clicked_outside {
        actions.push(Action::CloseGlobalSettings);
        return actions;
    }

    egui::Area::new(egui::Id::new("global_settings_popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(popup_rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(popup_rect.size());
            ui.set_max_size(popup_rect.size());

            egui::Frame::popup(ui.style())
                .rounding(egui::Rounding::same(10.0))
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_min_size(popup_rect.size());

                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.heading("Global Settings");
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("✕").clicked() {
                                        actions.push(Action::CloseGlobalSettings);
                                    }
                                });
                            });

                            ui.add_space(6.0);
                            ui.label("Machine-wide settings applied across all repos and workspaces.");
                            ui.separator();
                            ui.heading("Performance");
                            ui.label("Polling intervals are shown in seconds. Changes apply only when you press Apply.");
                            ui.add_space(8.0);

                            ui.horizontal(|ui| {
                                ui.label("Git status poll (s)");
                                ui.add(egui::DragValue::new(&mut state.ui.global_settings_git_status_poll_s).speed(1.0).clamp_range(1..=999));
                            });

                            ui.horizontal(|ui| {
                                ui.label("Analysis refresh poll (s)");
                                ui.add(egui::DragValue::new(&mut state.ui.global_settings_analysis_refresh_poll_s).speed(1.0).clamp_range(1..=999));
                            });

                            ui.horizontal(|ui| {
                                ui.label("Browser response poll (s)");
                                ui.add(egui::DragValue::new(&mut state.ui.global_settings_browser_response_poll_s).speed(1.0).clamp_range(1..=999));
                            });

                            ui.horizontal(|ui| {
                                ui.label("Browser DOM poll (s)");
                                ui.add(egui::DragValue::new(&mut state.ui.global_settings_browser_dom_poll_s).speed(1.0).clamp_range(1..=999));
                            });

                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.button("Reset to defaults").clicked() {
                                    let defaults = crate::app::state::GlobalPerfConfig::default();
                                    state.ui.global_settings_git_status_poll_s = (defaults.git_status_poll_ms / 1000).clamp(1, 999);
                                    state.ui.global_settings_analysis_refresh_poll_s = (defaults.analysis_refresh_poll_ms / 1000).clamp(1, 999);
                                    state.ui.global_settings_browser_response_poll_s = (defaults.browser_response_poll_ms / 1000).clamp(1, 999);
                                    state.ui.global_settings_browser_dom_poll_s = (defaults.browser_dom_poll_ms / 1000).clamp(1, 999);
                                }

                                if ui.button("Apply").clicked() {
                                    state.perf.git_status_poll_ms = state.ui.global_settings_git_status_poll_s.clamp(1, 999) * 1000;
                                    state.perf.analysis_refresh_poll_ms = state.ui.global_settings_analysis_refresh_poll_s.clamp(1, 999) * 1000;
                                    state.perf.browser_response_poll_ms = state.ui.global_settings_browser_response_poll_s.clamp(1, 999) * 1000;
                                    state.perf.browser_dom_poll_ms = state.ui.global_settings_browser_dom_poll_s.clamp(1, 999) * 1000;
                                    state.perf = state.perf.normalized();
                                    state.save_global_perf_config_to_appdata();
                                }

                                if ui.button("Close").clicked() {
                                    actions.push(Action::CloseGlobalSettings);
                                }
                            });
                        });
                });
        });

    actions
}
