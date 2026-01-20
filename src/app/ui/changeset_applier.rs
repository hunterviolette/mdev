// src/app/ui/changeset_applier.rs
use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

/// Notes:
/// - Prefer `edit` for small, anchored changes (more reliable than `git_apply`).
/// - `post_commands` is kept but defaults to empty; run `cargo run` manually during development.
pub const CHANGESET_SCHEMA_EXAMPLE: &str = r#"{
  "version": 1,
  "description": Schema example. Do not waste tokens/operations inserting or adjusting comments unless required.
  "operations": [
    {
      "op": "edit",
      "path": "src/app/ui/changeset_applier.rs",
      "changes": [
        {
          "action": "insert_after",
          "match": {
            "type": "literal",
            "mode": "normalized_newlines",
            "must_match": "exactly_one",
            "occurrence": 1,
            "text": "egui::ScrollArea::vertical()"
          },
          "text": "\n                .id_source(\"example_scroll_id\")"
        },
        {
          "action": "insert_before",
          "match": {
            "type": "literal",
            "mode": "normalized_newlines",
            "must_match": "exactly_one",
            "occurrence": 1,
            "text": "ui.label(\"Payload\");"
          },
          "text": "    // inserted comment (example)\n"
        },
        {
          "action": "replace_block",
          "match": {
            "type": "literal",
            "mode": "normalized_newlines",
            "must_match": "exactly_one",
            "occurrence": 1,
            "text": "ui.label(\"Payload\");"
          },
          "replacement": "ui.label(\"Payload (example)\");"
        },
        {
          "action": "delete_block",
          "match": {
            "type": "literal",
            "mode": "normalized_newlines",
            "must_match": "at_least_one",
            "text": "TODO:"
          }
        }
      ]
    },

    { "op": "write", "path": "tmp/changeset_example.txt", "contents": "hello from write\n" },
    { "op": "move", "from": "tmp/changeset_example.txt", "to": "tmp/changeset_example_moved.txt" },
    { "op": "delete", "path": "tmp/changeset_example_moved.txt" }
  ]
}"#;

//   "post_commands": [
//     { "shell": "Auto", "cmd": "cargo build", "cwd": "." }
//   ]

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

    let available_h = ui.available_height().max(200.0);
    let pane_h = ((available_h - 8.0) * 0.5).max(120.0);

    let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let desired_rows = ((pane_h / row_h).floor() as usize).max(6);

    // Payload pane
    ui.label("Payload");
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(6.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_source("changeset_applier_payload_scroll")
                .auto_shrink([false, false])
                .max_height(pane_h)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut st.payload)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(desired_rows)
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
                .auto_shrink([false, false])
                .max_height(pane_h)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut output)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(desired_rows)
                            .interactive(false),
                    );
                });
        });

    actions
}
