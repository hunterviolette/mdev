use anyhow::Result;
use serde_json::{json, Value};

use super::registry::{CapabilityContext, CapabilityInvocationRequest, CapabilityResult};

pub const CHANGESET_SCHEMA_EXAMPLE: &str = r#"{
  \"version\": 1,
  \"description\": \"Schema example. Do not waste tokens/operations inserting or adjusting comments unless required.\",
  \"operations\": [
    {
      \"op\": \"edit\",
      \"path\": \"src/app/ui/changeset_applier.rs\",
      \"changes\": [
        {
          \"action\": \"insert_before\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"ui.label(\\\"Payload\\\");\"
          },
          \"text\": \"    // inserted comment (example)\\n\"
        },
        {
          \"action\": \"replace_block\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"ui.label(\\\"Payload\\\");\"
          },
          \"replacement\": \"ui.label(\\\"Payload (example)\\\");\"
        },
        {
          \"action\": \"insert_after\",
          \"match\": {
            \"type\": \"literal\",
            \"mode\": \"normalized_newlines\",
            \"must_match\": \"exactly_one\",
            \"occurrence\": 1,
            \"text\": \"egui::ScrollArea::vertical().id_source(\\\"example_scroll_id\\\")\"
          },
          \"text\": \"\\n                .id_source(\\\"example_scroll_id\\\")\"
        }
      ]
    },
    {
      \"op\": \"write\",
      \"path\": \"tmp/changeset_example.txt\",
      \"contents\": \"hello from write\\n\"
    },
    {
      \"op\": \"move\",
      \"from\": \"tmp/changeset_example.txt\",
      \"to\": \"tmp/changeset_example_moved.txt\"
    },
    {
      \"op\": \"delete\",
      \"path\": \"tmp/changeset_example_moved.txt\"
    }
  ]
}"#;

pub async fn execute(
    _ctx: &CapabilityContext<'_>,
    _prior_results: &[CapabilityResult],
    _config: Value,
) -> Result<CapabilityResult> {
    Ok(CapabilityResult {
        ok: true,
        capability: "changeset_schema".to_string(),
        payload: json!({
            "ok": true,
            "message": "Changeset schema fragment enabled for inference prompt composition.",
            "schema": CHANGESET_SCHEMA_EXAMPLE,
        }),
        follow_ups: CapabilityInvocationRequest::None,
    })
}
