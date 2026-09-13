use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::{FeaturePlanItem, PlannerCapabilityBinding, PlannerCapabilityState};

fn planner_state(global_state: &Value) -> Result<Option<PlannerCapabilityState>> {
    let Some(_) = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
    else {
        return Ok(None);
    };

    PlannerCapabilityState::from_global_state(global_state).map(Some)
}

async fn bound_planner_feature(
    db: &SqlitePool,
    global_state: &Value,
) -> Result<Option<(PlannerCapabilityBinding, FeaturePlanItem)>> {
    let Some(state) = planner_state(global_state)? else {
        return Ok(None);
    };
    let Some(binding) = state.binding_if_present()? else {
        return Ok(None);
    };
    let feature = load_planner_feature(db, &binding.planner_id, &binding.feature_id)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "planner feature '{}' was not found in planner '{}'",
                binding.feature_id,
                binding.planner_id
            )
        })?;

    Ok(Some((binding, feature)))
}

pub async fn apply_repo_planner_capability(
    db: &SqlitePool,
    global_state: &mut Value,
    _repo_ref: &str,
) -> Result<()> {
    let _ = bound_planner_feature(db, global_state).await?;
    Ok(())
}

pub async fn hydrate_repo_planner_prompt_fragment(
    db: &SqlitePool,
    global_state: &mut Value,
) -> Result<()> {
    let Some((_binding, feature)) = bound_planner_feature(db, global_state).await? else {
        return Ok(());
    };

    let capabilities = global_state
        .get_mut("capabilities")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("workflow global capabilities must be an object"))?;
    let inference = capabilities
        .entry("inference".to_string())
        .or_insert_with(|| json!({}));
    if !inference.is_object() {
        *inference = json!({});
    }
    let prompt_fragments = inference
        .as_object_mut()
        .expect("inference capability must be an object")
        .entry("prompt_fragments".to_string())
        .or_insert_with(|| json!({}));
    if !prompt_fragments.is_object() {
        *prompt_fragments = json!({});
    }
    prompt_fragments
        .as_object_mut()
        .expect("inference prompt fragments must be an object")
        .insert(
            "planning_fragment".to_string(),
            Value::String(serde_json::to_string_pretty(&feature)?),
        );

    Ok(())
}

pub async fn load_planner_feature(
    db: &SqlitePool,
    planner_id: &str,
    feature_id: &str,
) -> Result<Option<FeaturePlanItem>> {
    let row = sqlx::query(
        "SELECT id, title, payload_json FROM planner_features WHERE planner_id = ? AND id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'deleted' LIMIT 1",
    )
    .bind(planner_id)
    .bind(feature_id)
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let mut feature: FeaturePlanItem = serde_json::from_str(
        row.get::<String, _>("payload_json").as_str(),
    )
    .context("invalid persisted planner feature payload")?;

    feature.id = row.get("id");
    feature.title = row.get("title");

    Ok(Some(feature))
}

pub async fn set_planner_feature_refinement_workflow_run(
    db: &SqlitePool,
    planner_id: &str,
    feature_id: &str,
    workflow_run_id: Uuid,
) -> Result<()> {
    let mut feature = load_planner_feature(db, planner_id, feature_id)
        .await?
        .ok_or_else(|| {
            anyhow!(
                "planner feature '{}' was not found in planner '{}'",
                feature_id,
                planner_id
            )
        })?;
    feature.refinement_workflow_run_id = Some(workflow_run_id);

    let result = sqlx::query(
        "UPDATE planner_features SET payload_json = ?, updated_at = ? WHERE planner_id = ? AND id = ?",
    )
    .bind(serde_json::to_string(&feature)?)
    .bind(Utc::now().to_rfc3339())
    .bind(planner_id)
    .bind(feature_id)
    .execute(db)
    .await?;

    if result.rows_affected() != 1 {
        return Err(anyhow!(
            "planner feature '{}' was not updated in planner '{}'",
            feature_id,
            planner_id
        ));
    }

    Ok(())
}
