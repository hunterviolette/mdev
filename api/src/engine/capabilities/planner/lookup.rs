use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use super::{
    FeaturePlanItem,
    FeaturePlanItemStatus,
    PlannerCapabilityBinding,
    PlannerCapabilityState,
    PlannerWorkspace,
};

#[derive(Clone, Copy)]
pub struct PlannerService<'a> {
    db: &'a SqlitePool,
}

impl<'a> PlannerService<'a> {
    pub fn new(db: &'a SqlitePool) -> Self {
        Self { db }
    }

    pub async fn list(&self, repo_ref: &str) -> Result<Vec<PlannerWorkspace>> {
        list_planner_workspaces(self.db, repo_ref).await
    }

    pub async fn create(
        &self,
        repo_ref: &str,
        title: Option<String>,
        make_default: bool,
        features: Vec<FeaturePlanItem>,
    ) -> Result<PlannerWorkspace> {
        create_planner_workspace(self.db, repo_ref, title, make_default, features).await
    }

    pub async fn ensure(
        &self,
        repo_ref: &str,
        title: Option<String>,
    ) -> Result<(bool, PlannerWorkspace)> {
        ensure_planner_workspace(self.db, repo_ref, title).await
    }

    pub async fn get(&self, planner_id: &str) -> Result<Option<PlannerWorkspace>> {
        load_planner_workspace(self.db, planner_id).await
    }

    pub async fn delete(&self, planner_id: &str) -> Result<bool> {
        delete_planner_workspace(self.db, planner_id).await
    }

    pub async fn set_default(&self, planner_id: &str) -> Result<PlannerWorkspace> {
        set_default_planner_workspace(self.db, planner_id).await
    }

    pub async fn features(&self, planner_id: &str) -> Result<Vec<FeaturePlanItem>> {
        load_planner_features(self.db, planner_id).await
    }

    pub async fn feature(
        &self,
        planner_id: &str,
        feature_id: &str,
    ) -> Result<Option<FeaturePlanItem>> {
        load_planner_feature(self.db, planner_id, feature_id).await
    }

    pub async fn feature_by_id(&self, feature_id: &str) -> Result<Option<FeaturePlanItem>> {
        let row = sqlx::query(
            "SELECT id, title, status, payload_json FROM planner_features WHERE id = ? AND id NOT LIKE 'manual-%' AND COALESCE(status, '') != 'archived' LIMIT 1",
        )
        .bind(feature_id)
        .fetch_optional(self.db)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let payload_json: String = row.get("payload_json");
        let mut item: FeaturePlanItem = serde_json::from_str(&payload_json)
            .context("invalid persisted planner feature payload")?;
        item.id = row.get("id");
        item.title = row.get("title");

        let status_text: String = row.get("status");
        if let Ok(status) = serde_json::from_value::<FeaturePlanItemStatus>(Value::String(status_text)) {
            item.status = status;
        }

        Ok(Some(item))
    }

    pub async fn replace_features(
        &self,
        planner_id: &str,
        features: &[FeaturePlanItem],
    ) -> Result<()> {
        replace_planner_features(self.db, planner_id, features).await
    }

    pub async fn belongs_to_scope(
        &self,
        planner_id: &str,
        repo_ref: &str,
    ) -> Result<bool> {
        planner_belongs_to_repo_ref(self.db, planner_id, repo_ref).await
    }

    pub async fn set_refinement_workflow_run(
        &self,
        planner_id: &str,
        feature_id: &str,
        workflow_run_id: Uuid,
    ) -> Result<()> {
        set_planner_feature_refinement_workflow_run(
            self.db,
            planner_id,
            feature_id,
            workflow_run_id,
        )
        .await
    }

    pub async fn apply_repo_capability(
        &self,
        global_state: &mut Value,
        repo_ref: &str,
    ) -> Result<()> {
        apply_repo_planner_capability(self.db, global_state, repo_ref).await
    }

    pub async fn hydrate_prompt_fragment(&self, global_state: &mut Value) -> Result<()> {
        hydrate_repo_planner_prompt_fragment(self.db, global_state).await
    }
}


fn planner_state(global_state: &Value) -> Result<Option<PlannerCapabilityState>> {
    let Some(_) = global_state
        .get("capabilities")
        .and_then(|value| value.get("planner"))
    else {
        return Ok(None);
    };

    PlannerCapabilityState::from_global_state(global_state).map(Some)
}

pub fn normalize_planner_repo_ref(value: &str) -> String {
    let normalized = crate::db::normalize_repo_ref(value)
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn planner_feature_is_refined(item: &FeaturePlanItem) -> bool {
    !item.summary.trim().is_empty()
        && !item.requirements.is_empty()
        && !item.acceptance_criteria.is_empty()
        && !item.implementation_notes.is_empty()
        && !item.review_expectations.is_empty()
        && !item.target_files_or_areas.is_empty()
}

fn normalize_planner_feature_status(mut item: FeaturePlanItem) -> FeaturePlanItem {
    item.status = if planner_feature_is_refined(&item) {
        FeaturePlanItemStatus::Fine
    } else {
        FeaturePlanItemStatus::Rough
    };
    item
}

pub async fn load_planner_features(
    db: &SqlitePool,
    planner_id: &str,
) -> Result<Vec<FeaturePlanItem>> {
    let rows = sqlx::query(
        r#"
        SELECT id, title, status, payload_json
        FROM planner_features
        WHERE planner_id = ?
          AND id NOT LIKE 'manual-%'
          AND COALESCE(status, '') != 'archived'
        ORDER BY sort_order ASC, created_at ASC
        "#,
    )
    .bind(planner_id)
    .fetch_all(db)
    .await?;

    rows.into_iter()
        .map(|row| {
            let payload_json: String = row.get("payload_json");
            let mut item = serde_json::from_str::<FeaturePlanItem>(&payload_json)
                .context("invalid persisted planner feature payload")?;
            item.id = row.get("id");
            item.title = row.get("title");
            let status_text: String = row.get("status");
            if let Ok(status) = serde_json::from_value::<FeaturePlanItemStatus>(Value::String(status_text)) {
                item.status = status;
            }
            Ok(item)
        })
        .collect()
}

pub async fn replace_planner_features(
    db: &SqlitePool,
    planner_id: &str,
    items: &[FeaturePlanItem],
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let items = items
        .iter()
        .filter(|item| !item.id.starts_with("manual-"))
        .cloned()
        .map(normalize_planner_feature_status)
        .collect::<Vec<_>>();
    let ids = items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
    let mut tx = db.begin().await?;

    for (index, item) in items.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO planner_features (
                id, planner_id, title, status, sort_order, payload_json, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                planner_id = excluded.planner_id,
                title = excluded.title,
                status = excluded.status,
                sort_order = excluded.sort_order,
                payload_json = excluded.payload_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&item.id)
        .bind(planner_id)
        .bind(&item.title)
        .bind(serde_json::to_value(&item.status)?.as_str().unwrap_or("rough"))
        .bind(index as i64)
        .bind(serde_json::to_string(item)?)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        UPDATE planner_features
        SET status = 'archived',
            locked_supervisor_run_id = NULL,
            locked_at = NULL,
            updated_at = ?
        WHERE planner_id = ?
          AND id NOT LIKE 'manual-%'
          AND id NOT IN (SELECT value FROM json_each(?))
        "#,
    )
    .bind(&now)
    .bind(planner_id)
    .bind(serde_json::to_string(&ids)?)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE planner_workspaces SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(planner_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn planner_belongs_to_repo_ref(
    db: &SqlitePool,
    planner_id: &str,
    repo_ref: &str,
) -> Result<bool> {
    let stored_repo_ref = sqlx::query_scalar::<_, String>(
        "SELECT root_repo_path FROM planner_workspaces WHERE id = ? LIMIT 1",
    )
    .bind(planner_id)
    .fetch_optional(db)
    .await?;

    Ok(stored_repo_ref
        .map(|stored| normalize_planner_repo_ref(&stored) == normalize_planner_repo_ref(repo_ref))
        .unwrap_or(false))
}

pub async fn load_planner_workspace(
    db: &SqlitePool,
    planner_id: &str,
) -> Result<Option<PlannerWorkspace>> {
    let row = sqlx::query(
        r#"
        SELECT id, root_repo_path, title, is_default, created_at, updated_at
        FROM planner_workspaces
        WHERE id = ?
        LIMIT 1
        "#,
    )
    .bind(planner_id)
    .fetch_optional(db)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let features = load_planner_features(db, planner_id).await?;
    Ok(Some(PlannerWorkspace {
        id: row.get("id"),
        repo_ref: row.get("root_repo_path"),
        title: row.get("title"),
        is_default: row.get::<i64, _>("is_default") != 0,
        feature_count: features.len() as i64,
        features,
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }))
}

pub async fn list_planner_workspaces(
    db: &SqlitePool,
    repo_ref: &str,
) -> Result<Vec<PlannerWorkspace>> {
    let repo_ref = normalize_planner_repo_ref(repo_ref);
    if repo_ref.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query(
        r#"
        SELECT id
        FROM planner_workspaces
        WHERE root_repo_path = ?
        ORDER BY is_default DESC, updated_at DESC, created_at DESC
        "#,
    )
    .bind(&repo_ref)
    .fetch_all(db)
    .await?;

    let mut planners = Vec::with_capacity(rows.len());
    for row in rows {
        let planner_id: String = row.get("id");
        if let Some(planner) = load_planner_workspace(db, &planner_id).await? {
            planners.push(planner);
        }
    }
    Ok(planners)
}

pub async fn create_planner_workspace(
    db: &SqlitePool,
    repo_ref: &str,
    title: Option<String>,
    make_default: bool,
    features: Vec<FeaturePlanItem>,
) -> Result<PlannerWorkspace> {
    let repo_ref = normalize_planner_repo_ref(repo_ref);
    if repo_ref.is_empty() {
        return Err(anyhow!("repo_ref is required"));
    }

    let existing_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM planner_workspaces WHERE root_repo_path = ?",
    )
    .bind(&repo_ref)
    .fetch_one(db)
    .await?;
    let make_default = make_default || existing_count == 0;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let default_title = repo_ref
        .rsplit('/')
        .find(|part| !part.trim().is_empty())
        .map(|name| format!("{} Planner", name))
        .unwrap_or_else(|| "Repo Planner".to_string());
    let title = title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(default_title);

    let mut tx = db.begin().await?;
    if make_default {
        sqlx::query("UPDATE planner_workspaces SET is_default = 0, updated_at = ? WHERE root_repo_path = ?")
            .bind(&now)
            .bind(&repo_ref)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO planner_workspaces (
            id, root_repo_path, repo_key, title, is_default, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&id)
    .bind(&repo_ref)
    .bind(crate::db::repo_basename_for_workflow_key(&repo_ref))
    .bind(&title)
    .bind(if make_default { 1_i64 } else { 0_i64 })
    .bind(&now)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    replace_planner_features(db, &id, &features).await?;
    load_planner_workspace(db, &id)
        .await?
        .ok_or_else(|| anyhow!("created planner was not found"))
}

pub async fn ensure_planner_workspace(
    db: &SqlitePool,
    repo_ref: &str,
    title: Option<String>,
) -> Result<(bool, PlannerWorkspace)> {
    let planners = list_planner_workspaces(db, repo_ref).await?;
    if let Some(planner) = planners.into_iter().find(|planner| planner.is_default) {
        return Ok((false, planner));
    }

    let planner = create_planner_workspace(db, repo_ref, title, true, Vec::new()).await?;
    Ok((true, planner))
}

pub async fn delete_planner_workspace(db: &SqlitePool, planner_id: &str) -> Result<bool> {
    let result = sqlx::query("DELETE FROM planner_workspaces WHERE id = ?")
        .bind(planner_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn set_default_planner_workspace(
    db: &SqlitePool,
    planner_id: &str,
) -> Result<PlannerWorkspace> {
    let planner = load_planner_workspace(db, planner_id)
        .await?
        .ok_or_else(|| anyhow!("planner not found"))?;
    let now = Utc::now().to_rfc3339();
    let mut tx = db.begin().await?;

    sqlx::query("UPDATE planner_workspaces SET is_default = 0, updated_at = ? WHERE root_repo_path = ?")
        .bind(&now)
        .bind(normalize_planner_repo_ref(&planner.repo_ref))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE planner_workspaces SET is_default = 1, updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(planner_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    load_planner_workspace(db, planner_id)
        .await?
        .ok_or_else(|| anyhow!("planner not found after setting default"))
}

async fn bound_planner_feature(
    db: &SqlitePool,
    global_state: &Value,
    workflow_repo_ref: Option<&str>,
) -> Result<Option<(PlannerCapabilityBinding, FeaturePlanItem)>> {
    let Some(state) = planner_state(global_state)? else {
        return Ok(None);
    };
    let Some(binding) = state.binding_if_present()? else {
        return Ok(None);
    };

    let effective_repo_ref = state
        .repo_ref
        .trim()
        .is_empty()
        .then_some(())
        .and_then(|_| workflow_repo_ref)
        .map(str::to_string)
        .unwrap_or_else(|| state.repo_ref.trim().to_string());

    if !effective_repo_ref.is_empty()
        && !planner_belongs_to_repo_ref(db, &binding.planner_id, &effective_repo_ref).await?
    {
        return Err(anyhow!(
            "planner '{}' does not belong to repo_ref '{}'",
            binding.planner_id,
            effective_repo_ref
        ));
    }

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
    repo_ref: &str,
) -> Result<()> {
    let _ = bound_planner_feature(db, global_state, Some(repo_ref)).await?;
    Ok(())
}

pub async fn hydrate_repo_planner_prompt_fragment(
    db: &SqlitePool,
    global_state: &mut Value,
) -> Result<()> {
    let Some((_binding, feature)) = bound_planner_feature(db, global_state, None).await? else {
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
