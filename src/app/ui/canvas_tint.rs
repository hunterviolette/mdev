use eframe::egui;

use crate::app::actions::Action;
use crate::app::state::AppState;

const DEFAULT_CANVAS_BG_TINT: [u8; 4] = [0, 128, 255, 18];


fn centered_rect(screen: egui::Rect, w: f32, h: f32) -> egui::Rect {
    let size = egui::vec2(w, h);
    egui::Rect::from_center_size(screen.center(), size)
}

/// Modal popup for setting a faint per-workspace canvas background tint.
///
/// Key property: we block input to the app *except* for the popup itself.
/// We do this by capturing clicks in four rectangles around the popup rect.
pub fn canvas_tint(ctx: &egui::Context, state: &mut AppState) -> Vec<Action> {
    let mut actions = Vec::new();

    if !state.ui.canvas_tint_popup_open {
        return actions;
    }

    let screen = ctx.screen_rect();
    // Taller popup to host an embedded picker (no floating picker popup),
    // which eliminates focus/input conflicts with the modal blocker.
    let popup_rect = centered_rect(screen, 480.0, 420.0);

    // Close on Esc.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(Action::CloseCanvasTintPopup);
        return actions;
    }

    // -------------------------------
    // Modal blocker (input outside popup)
    // -------------------------------
    // We draw a dimming layer across the whole screen, but only *capture input*
    // in the regions OUTSIDE the popup. This leaves the popup interactive.
    let mut clicked_outside = false;

    egui::Area::new(egui::Id::new("canvas_tint_popup_modal_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.set_min_size(screen.size());

            // Visual dim over everything.
            ui.painter().rect_filled(
                screen,
                0.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            );

            // Create four interactive regions around popup_rect.
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

            // Capture clicks (and drags) in these regions.
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
        actions.push(Action::CloseCanvasTintPopup);
        return actions;
    }

    // -------------------------------
    // Popup (on top)
    // -------------------------------
    egui::Area::new(egui::Id::new("canvas_tint_popup"))
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

                    ui.horizontal(|ui| {
                        ui.heading("Canvas background tint");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("✕").clicked() {
                                actions.push(Action::CloseCanvasTintPopup);
                            }
                        });
                    });

                    ui.add_space(6.0);
                    ui.label("Sets a faint sRGBA overlay behind the canvas to visually identify the current workspace.");
                    ui.separator();

                    // Use a stable draft value while the popup is open.
                    // This keeps the picker selector stable while dragging and avoids
                    // jumps due to state being reconstructed each frame.
                    let rgba_bytes = state
                        .ui
                        .canvas_tint_draft
                        .unwrap_or(state.ui.canvas_bg_tint.unwrap_or(DEFAULT_CANVAS_BG_TINT));

                    let mut col = egui::Color32::from_rgba_unmultiplied(
                        rgba_bytes[0],
                        rgba_bytes[1],
                        rgba_bytes[2],
                        rgba_bytes[3],
                    );

                    ui.label("Tint:");

                    // Embedded picker UI (not a floating popup). This prevents the modal
                    // blocker from stealing input from the picker.
                    let before = col;
                    egui::widgets::color_picker::color_picker_color32(
                        ui,
                        &mut col,
                        egui::widgets::color_picker::Alpha::OnlyBlend,
                    );

                    if col != before {
                        let arr = col.to_array();
                        let next = [arr[0], arr[1], arr[2], arr[3]];

                        // Update draft immediately for stable UI.
                        state.ui.canvas_tint_draft = Some(next);

                        // Apply immediately so the canvas tint updates live.
                        actions.push(Action::SetCanvasBgTint { rgba: Some(next) });
                    }

                    let arr = col.to_array();
                    ui.horizontal(|ui| {
                        ui.label("RGBA:");
                        ui.monospace(format!("[{},{},{},{}]", arr[0], arr[1], arr[2], arr[3]));
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            state.ui.canvas_tint_draft = Some(DEFAULT_CANVAS_BG_TINT);
                            actions.push(Action::SetCanvasBgTint {
                                rgba: Some(DEFAULT_CANVAS_BG_TINT),
                            });
                        }

                        if ui.button("Disable").clicked() {
                            state.ui.canvas_tint_draft = None;
                            actions.push(Action::SetCanvasBgTint { rgba: None });
                        }

                        if ui.button("Close").clicked() {
                            actions.push(Action::CloseCanvasTintPopup);
                        }
                    });
                });
        });

    actions
}
