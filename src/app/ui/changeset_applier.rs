// src/app/ui/changeset_applier.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

/// Schema (and example) copied to clipboard for the AI/user.
pub const CHANGESET_SCHEMA_EXAMPLE: &str = r#"{
  \"version\": 1,
  \"description\": \"Optional human-readable note\",
  \"operations\": [
    { \"op\": \"git_apply\", \"patch\": \"diff --git a/src/lib.rs b/src/lib.rs\\n...\" },
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

    ui.add_space(6.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut st.payload)
                .font(egui::TextStyle::Monospace)
                .desired_rows(18)
                .hint_text("Paste JSON payload here..."),
        );
    });

    if let Some(msg) = &st.status {
        ui.add_space(8.0);
        ui.separator();
        ui.label(msg);
    }

    actions
}
