use eframe::egui;

use crate::app::actions::{Action, ComponentId};
use crate::app::state::AppState;

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
            "text": "egui::ScrollArea::vertical().id_source("example_scroll_id")"
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
            "text": ""
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

    let panel_nonce = ui.next_auto_id();

    ui.push_id(("changeset_applier", applier_id, panel_nonce), |ui| {
    ui.heading("ChangeSet Applier
");
    ui.add_space(6.0);

    ui.label("Paste a JSON payload (from an AI chat or manual) and apply it to the repo working tree.");
    ui.add_space(6.0);

    ui.push_id("header_buttons", |ui| {
        ui.horizontal(|ui| {
            ui.push_id("copy_schema", |ui| {
                if ui.button("Copy schema + example").clicked() {
                    ctx.output_mut(|o| o.copied_text = CHANGESET_SCHEMA_EXAMPLE.to_string());
                    st.status = Some("Copied schema/example to clipboard.".into());
                }
            });

            let has_output = st
                .status
                .as_ref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

            ui.push_id("copy_output", |ui| {
                if ui
                    .add_enabled(has_output, egui::Button::new("Copy output"))
                    .clicked()
                {
                    if let Some(s) = &st.status {
                        ctx.output_mut(|o| o.copied_text = s.clone());
                    }
                }
            });

            ui.push_id("clear", |ui| {
                if ui.button("Clear").clicked() {
                    actions.push(Action::ClearChangeSet { applier_id });
                }
            });

            let can_apply = state.inputs.repo.is_some() && !st.payload.trim().is_empty();
            ui.push_id("apply", |ui| {
                if ui.add_enabled(can_apply, egui::Button::new("Apply")).clicked() {
                    actions.push(Action::ApplyChangeSet { applier_id });
                }
            });
        });
    });

    ui.add_space(8.0);

    let available_h = ui.available_height().max(1.0);
    let available_w = ui.available_width().max(1.0);

    let pane_h = ((available_h - 8.0).max(1.0) * 0.5).max(1.0);

    let row_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let desired_rows = ((pane_h / row_h).floor() as usize).max(1);

    ui.push_id("payload_pane", |ui| {

    ui.label("Payload (example)");
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_source(("changeset_applier_payload_scroll", applier_id))
                    .auto_shrink([false, false])
                    .max_height(pane_h)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut st.payload)
                                .id_source(("changeset_applier_payload_text", applier_id))
                                .font(egui::TextStyle::Monospace)
                                .desired_width(available_w)
                                .desired_rows(desired_rows)
                                .hint_text("Paste JSON payload here..."),
                        );
                    });
            });
    });

    ui.add_space(8.0);

    ui.push_id("output_pane", |ui| {
        ui.label("Output");
        let mut output = st.status.clone().unwrap_or_default();
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::same(6.0))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_source(("changeset_applier_output_scroll", applier_id))
                    .auto_shrink([false, false])
                    .max_height(pane_h)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut output)
                                .id_source(("changeset_applier_output_text", applier_id))
                                .font(egui::TextStyle::Monospace)
                                .desired_width(available_w)
                                .desired_rows(desired_rows)
                                .interactive(false),
                        );
                    });
            });
        });
    });

    actions
}
