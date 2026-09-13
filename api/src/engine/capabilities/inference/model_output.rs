use anyhow::{bail, Context, Result};
use serde_json::Value;

pub fn extract_json_object_slice(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, &byte) in bytes.iter().enumerate() {
        let ch = byte as char;

        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if start.is_none() {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    return start.map(|start_idx| &text[start_idx..=idx]);
                }
            }
            _ => {}
        }
    }

    None
}

pub fn clean_model_json_text(text: &str) -> Result<String> {
    let mut cleaned = text.trim().to_string();

    if cleaned.starts_with("```") {
        let original = cleaned;
        let mut lines = original.lines();
        let first = lines.next().unwrap_or_default().to_string();
        cleaned = lines.collect::<Vec<_>>().join("\n");
        if first.trim_start_matches("```").trim().eq_ignore_ascii_case("json") {
            cleaned = cleaned.trim().to_string();
        }
        if let Some(idx) = cleaned.rfind("```") {
            cleaned.truncate(idx);
        }
    }

    let json_slice = extract_json_object_slice(cleaned.trim())
        .context("model output did not contain a JSON object")?;

    let value: Value = serde_json::from_str(json_slice)
        .context("model output JSON could not be parsed")?;

    if !value.is_object() {
        bail!("model output JSON must be an object");
    }

    serde_json::to_string_pretty(&value).context("failed to normalize model output JSON")
}

pub fn clean_model_json_value(text: &str) -> Result<Value> {
    let cleaned = clean_model_json_text(text)?;
    serde_json::from_str(&cleaned).context("failed to decode normalized model output JSON")
}
