use anyhow::Result;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

pub async fn apply_repo_planner_capability(
    db: &SqlitePool,
    global_state: &mut Value,
    repo_ref: &str,
) -> Result<()> {
    if existing_planner_fragment_is_populated(global_state) {
        hydrate_existing_planner_selection(db, global_state).await?;
        return Ok(());
    }

    let planner_state = load_latest_supervisor_plan_from_db(db, repo_ref)
        .await?
        .or_else(|| load_repo_supervisor_planner_state(global_state, repo_ref));

    let Some(planner_state) = planner_state else {
        return Ok(());
    };

    let capabilities = ensure_object_field(global_state, "capabilities");
    capabilities.insert("planner".to_string(), planner_state);
    Ok(())
}

async fn hydrate_existing_planner_selection(
    db: &SqlitePool,
    global_state: &mut Value,
) -> Result<()> {
    let selected_feature_id = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
        .and_then(|value| value.get("selected_feature_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(selected_feature_id) = selected_feature_id else {
        return Ok(());
    };

    let already_has_selected_feature = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
        .and_then(|value| value.get("selected_feature"))
        .map(selected_feature_is_populated)
        .unwrap_or(false);

    if already_has_selected_feature {
        return Ok(());
    }

    let supervisor_run_id = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
        .and_then(|value| value.get("supervisor_run_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(supervisor_run_id) = supervisor_run_id else {
        return Ok(());
    };

    let Some(selected_feature) = load_supervisor_feature_by_id(db, &supervisor_run_id, &selected_feature_id).await? else {
        return Ok(());
    };

    let Some(planner_obj) = global_state
        .get_mut("capabilities")
        .and_then(|value| value.get_mut("planner"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };

    planner_obj.insert("selected_feature".to_string(), selected_feature);
    Ok(())
}

async fn load_supervisor_feature_by_id(
    db: &SqlitePool,
    supervisor_run_id: &str,
    selected_feature_id: &str,
) -> Result<Option<Value>> {
    let row = sqlx::query("SELECT features_json FROM supervisor_runs WHERE id = ?")
        .bind(supervisor_run_id)
        .fetch_optional(db)
        .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let features_json = row.get::<String, _>("features_json");
    let value: Value = serde_json::from_str(&features_json)?;
    let supervisor = value.get("supervisor").unwrap_or(&value);
    let items = if supervisor.is_array() {
        supervisor.as_array().cloned().unwrap_or_default()
    } else {
        supervisor
            .get("feature_plan_items")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| supervisor.get("features").and_then(Value::as_array).cloned())
            .unwrap_or_default()
    };

    Ok(items.into_iter().find(|item| {
        item.get("id")
            .and_then(Value::as_str)
            .map(|id| id == selected_feature_id)
            .unwrap_or(false)
    }))
}

async fn load_latest_supervisor_plan_from_db(
    db: &SqlitePool,
    repo_ref: &str,
) -> Result<Option<Value>> {
    let repo_ref = repo_ref.trim();
    if repo_ref.is_empty() {
        return Ok(None);
    }

    let row = sqlx::query(
        "SELECT id, features_json FROM supervisor_runs WHERE root_repo_path = ? ORDER BY updated_at DESC LIMIT 1",
    )
    .bind(repo_ref)
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let supervisor_run_id = row.get::<String, _>("id");
    let features_json = row.get::<String, _>("features_json");
    let value: Value = serde_json::from_str(&features_json)?;
    Ok(supervisor_value_to_planner_state(&value, Some(supervisor_run_id)))
}

fn load_repo_supervisor_planner_state(global_state: &Value, repo_ref: &str) -> Option<Value> {
    let candidates = repo_path_candidates(global_state, repo_ref);

    for candidate in candidates {
        for supervisor_dir in supervisor_dirs_for_path(&candidate) {
            if let Some(state) = read_supervisor_planner_state(&supervisor_dir) {
                return Some(state);
            }
        }
    }

    None
}

fn repo_path_candidates(global_state: &Value, repo_ref: &str) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    push_repo_path_candidate(&mut candidates, repo_ref);

    if let Some(repo) = global_state.get("resources").and_then(|value| value.get("repo")) {
        push_repo_path_candidate(&mut candidates, repo.get("repo_ref").and_then(Value::as_str).unwrap_or(""));
        push_repo_path_candidate(&mut candidates, repo.get("path").and_then(Value::as_str).unwrap_or(""));
        push_repo_path_candidate(&mut candidates, repo.get("root").and_then(Value::as_str).unwrap_or(""));
        push_repo_path_candidate(&mut candidates, repo.get("repo_root").and_then(Value::as_str).unwrap_or(""));
        push_repo_path_candidate(&mut candidates, repo.get("working_dir").and_then(Value::as_str).unwrap_or(""));
    }

    candidates
}

fn supervisor_dirs_for_path(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();

    for ancestor in path.ancestors() {
        let supervisors_dir = ancestor.join(".mdev").join("supervisors");
        if let Ok(entries) = std::fs::read_dir(supervisors_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !out.iter().any(|item| item == &path) {
                    out.push(path);
                }
            }
        }
    }

    out
}

fn read_supervisor_planner_state(supervisor_dir: &std::path::Path) -> Option<Value> {
    let supervisor_run_id = supervisor_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);

    let files = [
        supervisor_dir.join("supervisor.json"),
        supervisor_dir.join("state.json"),
        supervisor_dir.join("run.json"),
        supervisor_dir.join("metadata.json"),
    ];

    for file in files {
        let Ok(contents) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&contents) else {
            continue;
        };
        if let Some(state) = supervisor_value_to_planner_state(&value, supervisor_run_id.clone()) {
            return Some(state);
        }
    }

    None
}

fn supervisor_value_to_planner_state(value: &Value, supervisor_run_id: Option<String>) -> Option<Value> {
    let supervisor = value.get("supervisor").unwrap_or(value);
    let items = if supervisor.is_array() {
        supervisor.as_array()?.clone()
    } else {
        supervisor
            .get("feature_plan_items")
            .and_then(Value::as_array)
            .cloned()
            .or_else(|| supervisor.get("features").and_then(Value::as_array).cloned())?
    };

    let selected_feature_id = supervisor
        .get("feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| {
            supervisor
                .get("selected_feature_ids")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find_map(Value::as_str))
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })
        .or_else(|| {
            items
                .first()
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
        })?;

    let selected_feature = items
        .iter()
        .find(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .map(|id| id == selected_feature_id)
                .unwrap_or(false)
        })
        .cloned()
        .or_else(|| items.first().cloned());

    let mut out = json!({
        "fragment_armed": true,
        "schema_armed": false,
        "auto_apply_armed": false,
        "selected_feature_id": selected_feature_id,
        "schema_id": "supervisor_feature_plan_item_v1",
        "preserve_rough_definition": true
    });

    if let Some(selected_feature) = selected_feature {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("selected_feature".to_string(), selected_feature);
        }
    }

    if let Some(supervisor_run_id) = supervisor_run_id.filter(|value| !value.trim().is_empty()) {
        if let Some(obj) = out.as_object_mut() {
            obj.insert("supervisor_run_id".to_string(), Value::String(supervisor_run_id));
        }
    }

    Some(out)
}

fn selected_feature_is_populated(value: &Value) -> bool {
    let Some(obj) = value.as_object() else {
        return false;
    };

    let has_id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .is_some();

    if !has_id {
        return false;
    }

    obj.get("summary")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .is_some()
        || obj
            .get("rough_summary")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .is_some()
        || obj
            .get("requirements")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
        || obj
            .get("acceptance_criteria")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
        || obj
            .get("implementation_notes")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
        || obj
            .get("review_expectations")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
        || obj
            .get("target_files_or_areas")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
}

fn existing_planner_fragment_is_populated(global_state: &Value) -> bool {
    let Some(planner) = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
    else {
        return false;
    };

    let has_selected_feature_id = planner
        .get("selected_feature_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .is_some();

    if !has_selected_feature_id {
        return false;
    }

    planner
        .get("fragment_armed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || planner
            .get("schema_armed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || planner
            .get("auto_apply_armed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn ensure_object_field<'a>(root: &'a mut Value, key: &str) -> &'a mut serde_json::Map<String, Value> {
    if !root.is_object() {
        *root = json!({});
    }
    let obj = root.as_object_mut().expect("root must be object");
    let value = obj.entry(key.to_string()).or_insert_with(|| json!({}));
    if !value.is_object() {
        *value = json!({});
    }
    value.as_object_mut().expect("field must be object")
}

fn push_repo_path_candidate(out: &mut Vec<std::path::PathBuf>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }

    let path = std::path::PathBuf::from(value);
    if !out.iter().any(|item| item == &path) {
        out.push(path);
    }
}
