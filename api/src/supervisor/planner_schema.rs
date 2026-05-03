pub const PLANNER_SCHEMA_PROMPT_FRAGMENT: &str = r#"Respond with only valid JSON matching this supervisor planner feature schema:

{
  "feature": {
    "id": "existing-feature-id",
    "title": "Short feature title",
    "status": "fine",
    "summary": "Clear refined feature summary.",
    "rough_summary": "Original rough feature prompt preserved exactly when available.",
    "requirements": [
      "Concrete implementation requirement"
    ],
    "acceptance_criteria": [
      "Observable condition that proves the feature is complete"
    ],
    "implementation_notes": [
      "Technical implementation note"
    ],
    "review_expectations": [
      "What reviewers should verify"
    ],
    "target_files_or_areas": [
      "Likely file, module, UI, API, or subsystem expected to change"
    ]
  }
}

Rules:
- Output JSON only.
- Do not wrap the JSON in markdown.
- Preserve the existing feature id.
- Set status to "fine".
- Preserve rough_summary when it is present.
- Put likely affected files, modules, screens, endpoints, or subsystems in target_files_or_areas.
- Do not include implementation code unless it belongs in implementation_notes.
"#;
