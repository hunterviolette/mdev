pub const PLANNER_SCHEMA_PROMPT_FRAGMENT: &str = r#"Respond with only raw JSON matching this exact planner refinement schema:

{
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
  ],
  "dependencies": []
}

Rules:
- Output raw JSON only.
- Do not wrap the JSON in markdown.
- Use only the fields shown in the schema.
- The server owns planner identity, feature identity, title, status, rough summary, workflow linkage, and timestamps.
- Leave arrays empty only when there is genuinely no content for that section.
- Put likely affected files, modules, screens, endpoints, or subsystems in target_files_or_areas.
- Do not include implementation code unless it belongs in implementation_notes.
"#;
