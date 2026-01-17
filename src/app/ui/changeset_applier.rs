// src/app/ui/changeset_applier.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

/// Notes:
/// - Prefer `edit` for small, anchored changes (more reliable than `git_apply`).
/// - `post_commands` is kept but defaults to empty; run `cargo run` manually during development.
pub const CHANGESET_SCHEMA_EXAMPLE: &str = r#"{
  \"version\": 1,
  \"description\": \"Optional human-readable note\",
  \"operations\": [
    {
      \"op\": \"edit\",
      \"path\": \"src/app/ui/changeset_applier.rs\",
      \"changes\": [
        {
          \"action\": \"insert_after\",
          \"match\": { \"type\": \"literal\", \"mode\": \"normalized_newlines\", \"must_match\": \"exactly_one\", \"text\": \"egui::ScrollArea::vertical()\", \"occurrence\": 1 },
          \"text\": \"\\n                .id_source(\\\"example_scroll_id\\\")\"
        }
      ]
    },

    { \"op\": \"write\", \"path\": \"src/new_file.rs\", \"contents\": \"fn main() {}\\n\" },
    { \"op\": \"move\", \"from\": \"old/path.rs\", \"to\": \"new/path.rs\" },
    { \"op\": \"delete\", \"path\": \"src/old_file.rs\" }
  ],
  \"post_commands\": [
    { \"shell\": \"Auto\", \"cmd\": \"cargo build\", \"cwd\": \".\" }
  ]
}"#;

pub fn changeset_applier_panel(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    state: &mut AppState,
    applier_id: ComponentId,
) -> Vec<Action> {
    let mut actions = vec![];

    let Some(st) = state.changeset_appliers.get_mut(&applier_id) else {
        ui.label("Missing ChangeSetApplier state.");
        return actions;
    };

    ui.heading("ChangeSet Applier");
    ui.add_space(6.0);

    ui.label("Paste a JSON payload (from an AI chat or manual) and apply it to the repo working tree.");
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        if ui.button("Copy schema + example").clicked() {
            ctx.output_mut(|o| o.copied_text = CHANGESET_SCHEMA_EXAMPLE.to_string());
            st.status = Some("Copied schema/example to clipboard.".into());
        }

        let has_output = st
            .status
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);

        if ui
            .add_enabled(has_output, egui::Button::new("Copy output"))
            .clicked()
        {
            if let Some(s) = &st.status {
                ctx.output_mut(|o| o.copied_text = s.clone());
            }
        }

        if ui.button("Clear").clicked() {
            actions.push(Action::ClearChangeSet { applier_id });
        }

        let can_apply = state.inputs.repo.is_some() && !st.payload.trim().is_empty();
        if ui.add_enabled(can_apply, egui::Button::new("Apply")).clicked() {
            actions.push(Action::ApplyChangeSet { applier_id });
        }
    });

    ui.add_space(8.0);

    // Keep this component from "blowing out" its layout.
    // Use two scrollable panes with capped heights.
    let available_h = ui.available_height().max(200.0);
    let pane_h = (available_h * 0.45).clamp(140.0, 320.0);

    // Payload pane
    ui.label("Payload");
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_source("changeset_applier_payload_scroll")
                .max_height(pane_h)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut st.payload)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .hint_text("Paste JSON payload here..."),
                    );
                });
        });

    ui.add_space(8.0);

    // Output pane
    ui.label("Output");
    let mut output = st.status.clone().unwrap_or_default();
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_source("changeset_applier_output_scroll")
                .max_height(pane_h)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut output)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                });
        });

    actions
}
