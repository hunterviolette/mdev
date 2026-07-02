pub const PLANNER_SCHEMA_PROMPT_FRAGMENT: &str = r#"Respond with only raw JSON matching this exact supervisor planner refinement schema:

{
  "feature": {
    "summary": "Clear refined feature summary.",
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
- Output raw JSON only.
- Do not wrap the JSON in markdown.
- Do not include any fields outside the schema above.
- Do not include id, title, status, rough_summary, header name, refinement_workflow_run_id, or timestamps.
- The server owns id, title/header, status, rough_summary, and workflow linkage.
- Leave arrays empty only when there is genuinely no content for that section.
- Put likely affected files, modules, screens, endpoints, or subsystems in target_files_or_areas.
- Do not include implementation code unless it belongs in implementation_notes.
"#;
