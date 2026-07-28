use std::str::FromStr;
use std::collections::HashMap;

use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, AssertSqlSafe, Row, SqlitePool};
use uuid::Uuid;

use crate::engine::capabilities::changeset::persistence::{CHANGESET_ATTEMPTS_TABLE_SQL, CHANGESET_FILE_EFFECTS_TABLE_SQL};

pub fn repo_basename_for_workflow_key(repo_ref: &str) -> String {
    let normalized = repo_ref.trim().replace('\\', "/");
    let raw = normalized
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("workflow");

    let mut out = raw
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' })
        .collect::<String>();

    while out.contains("--") {
        out = out.replace("--", "-");
    }

    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "workflow".to_string() } else { out }
}

pub fn new_workflow_key(repo_ref: &str) -> String {
    format!("{}-{}", repo_basename_for_workflow_key(repo_ref), Uuid::new_v4())
}

async fn backfill_workflow_keys(db: &SqlitePool) -> anyhow::Result<()> {
    let rows = sqlx::query(
        "SELECT id, title, repo_ref FROM workflow_runs WHERE TRIM(COALESCE(workflow_key, '')) = '' ORDER BY created_at ASC, id ASC",
    )
    .fetch_all(db)
    .await?;

    let mut keys_by_group = HashMap::<(String, String), String>::new();
    for row in rows {
        let id: String = row.get("id");
        let title: String = row.get("title");
        let repo_ref: String = row.get("repo_ref");
        let key = keys_by_group
            .entry((title, repo_ref.clone()))
            .or_insert_with(|| new_workflow_key(&repo_ref))
            .clone();

        sqlx::query("UPDATE workflow_runs SET workflow_key = ? WHERE id = ?")
            .bind(key)
            .bind(id)
            .execute(db)
            .await?;
    }

    Ok(())
}

async fn backfill_changeset_workflow_keys(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE changeset_attempts
        SET workflow_key = (
            SELECT workflow_runs.workflow_key
            FROM workflow_runs
            WHERE workflow_runs.id = changeset_attempts.run_id
        )
        WHERE TRIM(COALESCE(workflow_key, '')) = ''
          AND run_id IS NOT NULL
          AND EXISTS (
              SELECT 1
              FROM workflow_runs
              WHERE workflow_runs.id = changeset_attempts.run_id
                AND TRIM(COALESCE(workflow_runs.workflow_key, '')) != ''
          )
        "#,
    )
    .execute(db)
    .await?;

    Ok(())
}

async fn ensure_column(db: &SqlitePool, table: &str, column: &str, definition: &str) -> anyhow::Result<()> {
    let rows = sqlx::query(AssertSqlSafe(format!("PRAGMA table_info({})", table)))
        .fetch_all(db)
        .await?;
    let exists = rows
        .iter()
        .any(|row| row.get::<String, _>("name") == column);
    if !exists {
        sqlx::query(AssertSqlSafe(format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition)))
            .execute(db)
            .await?;
    }
    Ok(())
}

pub async fn connect(url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal);

    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?)
}

pub async fn migrate(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflow_templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            repo_ref TEXT NOT NULL DEFAULT '',
            definition_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflow_runs (
            id TEXT PRIMARY KEY,
            template_id TEXT,
            definition_json TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL,
            current_step_id TEXT,
            title TEXT NOT NULL,
            repo_ref TEXT NOT NULL,
            workflow_key TEXT NOT NULL DEFAULT '',
            context_json TEXT NOT NULL,
            archived_at TEXT,
            archived_reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS workflow_events (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL,
            step_id TEXT,
            stage_execution_id TEXT,
            capability_invocation_id TEXT,
            parent_invocation_id TEXT,
            sequence_no INTEGER NOT NULL,
            is_header_event INTEGER NOT NULL,
            level TEXT NOT NULL,
            kind TEXT NOT NULL,
            message TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_workflow_events_run_seq ON workflow_events (run_id, sequence_no)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_workflow_events_stage_exec_seq ON workflow_events (run_id, step_id, stage_execution_id, sequence_no)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_workflow_events_header_seq ON workflow_events (run_id, is_header_event, sequence_no)")
    .execute(db)
    .await?;

    sqlx::query(CHANGESET_ATTEMPTS_TABLE_SQL)
        .execute(db)
        .await?;

    sqlx::query(CHANGESET_FILE_EFFECTS_TABLE_SQL)
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_attempts_repo_created ON changeset_attempts (repo_ref, created_at)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_key ON workflow_runs (workflow_key)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_attempts_workflow_created ON changeset_attempts (workflow_key, created_at)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_attempts_status ON changeset_attempts (status)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_attempts_reverses ON changeset_attempts (reverses_attempt_id)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_file_effects_attempt ON changeset_file_effects (attempt_id, op_index, action_index)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_changeset_file_effects_action_status ON changeset_file_effects (action, status)")
    .execute(db)
    .await?;

    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_templates_name ON workflow_templates (name)")
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS app_settings (
            id TEXT PRIMARY KEY,
            settings_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS supervisor_runs (
            id TEXT PRIMARY KEY,
            mode TEXT NOT NULL,
            status TEXT NOT NULL,
            title TEXT NOT NULL,
            root_repo_path TEXT NOT NULL,
            selected_planner_id TEXT,
            flight_deck_json TEXT NOT NULL DEFAULT '{}',
            context_json TEXT NOT NULL DEFAULT '{}',
            archived_at TEXT,
            archived_reason TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_supervisor_runs_status_updated ON supervisor_runs (status, updated_at)")
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS supervisor_work_units (
            id TEXT PRIMARY KEY,
            supervisor_run_id TEXT NOT NULL,
            repo_id TEXT,
            feature_id TEXT,
            workflow_run_id TEXT,
            patch_id TEXT,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            state TEXT NOT NULL,
            root_repo_path TEXT NOT NULL,
            workspace_path TEXT,
            shard_path TEXT,
            integration_path TEXT,
            archived_at TEXT,
            archived_reason TEXT,
            priority INTEGER NOT NULL DEFAULT 0,
            queue_position INTEGER,
            blocked_reason TEXT,
            waiting_user_input_json TEXT NOT NULL DEFAULT '{}',
            context_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    ensure_column(db, "workflow_runs", "archived_at", "TEXT").await?;
    ensure_column(db, "workflow_runs", "archived_reason", "TEXT").await?;
    ensure_column(db, "supervisor_runs", "archived_at", "TEXT").await?;
    ensure_column(db, "supervisor_runs", "archived_reason", "TEXT").await?;
    ensure_column(db, "supervisor_runs", "selected_planner_id", "TEXT").await?;
    ensure_column(db, "supervisor_runs", "flight_deck_json", "TEXT NOT NULL DEFAULT '{}'").await?;

    let supervisor_run_columns = sqlx::query("PRAGMA table_info(supervisor_runs)")
        .fetch_all(db)
        .await?;
    let supervisor_runs_needs_rebuild = supervisor_run_columns.iter().any(|row| {
        matches!(
            row.get::<String, _>("name").as_str(),
            "snapshot_path"
                | "integration_path"
                | "features_json"
                | "child_runs_json"
                | "integration_run_id"
                | "final_patch_path"
                | "merge_report_json"
                | "validation_report_json"
        )
    });

    if supervisor_runs_needs_rebuild {
        sqlx::query(
            r#"
            CREATE TABLE supervisor_runs_next (
                id TEXT PRIMARY KEY,
                mode TEXT NOT NULL,
                status TEXT NOT NULL,
                title TEXT NOT NULL,
                root_repo_path TEXT NOT NULL,
                selected_planner_id TEXT,
                flight_deck_json TEXT NOT NULL DEFAULT '{}',
                context_json TEXT NOT NULL DEFAULT '{}',
                archived_at TEXT,
                archived_reason TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
            "#,
        )
        .execute(db)
        .await?;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO supervisor_runs_next (
                id,
                mode,
                status,
                title,
                root_repo_path,
                selected_planner_id,
                flight_deck_json,
                context_json,
                archived_at,
                archived_reason,
                created_at,
                updated_at
            )
            SELECT
                id,
                mode,
                status,
                title,
                root_repo_path,
                COALESCE(
                    NULLIF(selected_planner_id, ''),
                    NULLIF(json_extract(context_json, '$.queue_planner_id'), ''),
                    NULLIF(json_extract(context_json, '$.selected_planner_id'), ''),
                    NULLIF(json_extract(context_json, '$.planner_workspace_id'), ''),
                    NULLIF(json_extract(context_json, '$.planner_id'), '')
                ),
                CASE
                    WHEN TRIM(COALESCE(flight_deck_json, '')) = '' OR flight_deck_json = '{}' THEN COALESCE(json_extract(context_json, '$.flight_deck_settings'), '{}')
                    ELSE flight_deck_json
                END,
                context_json,
                archived_at,
                archived_reason,
                created_at,
                updated_at
            FROM supervisor_runs
            "#,
        )
        .execute(db)
        .await?;

        sqlx::query("DROP TABLE supervisor_runs")
            .execute(db)
            .await?;
        sqlx::query("ALTER TABLE supervisor_runs_next RENAME TO supervisor_runs")
            .execute(db)
            .await?;
    }

    sqlx::query(
        r#"
        UPDATE supervisor_runs
        SET selected_planner_id = COALESCE(
                NULLIF(selected_planner_id, ''),
                NULLIF(json_extract(context_json, '$.queue_planner_id'), ''),
                NULLIF(json_extract(context_json, '$.selected_planner_id'), ''),
                NULLIF(json_extract(context_json, '$.planner_workspace_id'), ''),
                NULLIF(json_extract(context_json, '$.planner_id'), '')
            ),
            flight_deck_json = CASE
                WHEN TRIM(COALESCE(flight_deck_json, '')) = '' OR flight_deck_json = '{}' THEN COALESCE(json_extract(context_json, '$.flight_deck_settings'), '{}')
                ELSE flight_deck_json
            END
        WHERE json_valid(context_json)
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_supervisor_runs_status_updated ON supervisor_runs (status, updated_at)")
        .execute(db)
        .await?;
    ensure_column(db, "supervisor_work_units", "workspace_path", "TEXT").await?;
    ensure_column(db, "supervisor_work_units", "shard_id", "TEXT").await?;
    ensure_column(db, "supervisor_work_units", "archived_at", "TEXT").await?;
    ensure_column(db, "supervisor_work_units", "archived_reason", "TEXT").await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_work_units_supervisor_state ON supervisor_work_units (supervisor_run_id, state, updated_at)")
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_work_units_repo_state ON supervisor_work_units (root_repo_path, state, updated_at)")
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_work_units_workflow ON supervisor_work_units (workflow_run_id)")
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_work_units_feature ON supervisor_work_units (feature_id)")
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_work_units_workspace ON supervisor_work_units (workspace_path)")
        .execute(db)
        .await?;

    sqlx::query(
        "UPDATE supervisor_work_units SET workspace_path = COALESCE(NULLIF(workspace_path, ''), NULLIF(shard_path, ''), NULLIF(integration_path, '')) WHERE TRIM(COALESCE(workspace_path, '')) = ''"
    )
    .execute(db)
    .await?;



    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_workspaces (
            id TEXT PRIMARY KEY,
            root_repo_path TEXT NOT NULL,
            repo_key TEXT NOT NULL DEFAULT '',
            title TEXT NOT NULL,
            is_default INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    ensure_column(db, "planner_workspaces", "repo_key", "TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(db, "planner_workspaces", "is_default", "INTEGER NOT NULL DEFAULT 0").await?;

    let has_legacy_planner_repos = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'planner_repos'")
        .fetch_optional(db)
        .await?
        .is_some();

    if has_legacy_planner_repos {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO planner_workspaces (id, root_repo_path, repo_key, title, is_default, created_at, updated_at)
            SELECT id, root_repo_path, repo_key, repo_key || ' Planner', 1, created_at, updated_at
            FROM planner_repos
            "#,
        )
        .execute(db)
        .await?;
    }

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_features (
            id TEXT PRIMARY KEY,
            planner_id TEXT NOT NULL DEFAULT '' REFERENCES planner_workspaces(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'rough',
            sort_order INTEGER NOT NULL DEFAULT 0,
            payload_json TEXT NOT NULL DEFAULT '{}',
            refined_at TEXT,
            locked_supervisor_run_id TEXT,
            locked_at TEXT,
            completed_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    ensure_column(db, "planner_features", "planner_id", "TEXT NOT NULL DEFAULT ''").await?;
    ensure_column(db, "planner_features", "refined_at", "TEXT").await?;
    ensure_column(db, "planner_features", "locked_supervisor_run_id", "TEXT").await?;
    ensure_column(db, "planner_features", "locked_at", "TEXT").await?;
    ensure_column(db, "planner_features", "completed_at", "TEXT").await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_feature_patches (
            id TEXT PRIMARY KEY,
            feature_id TEXT NOT NULL REFERENCES planner_features(id) ON DELETE CASCADE,
            planner_id TEXT NOT NULL DEFAULT '' REFERENCES planner_workspaces(id) ON DELETE CASCADE,
            supervisor_run_id TEXT,
            workflow_run_id TEXT,
            patch_kind TEXT NOT NULL DEFAULT 'development',
            repo_ref TEXT NOT NULL,
            base_commit TEXT,
            head_commit TEXT,
            patch_text TEXT NOT NULL,
            patch_hash TEXT NOT NULL,
            patch_path TEXT,
            created_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    ensure_column(db, "planner_feature_patches", "planner_id", "TEXT NOT NULL DEFAULT ''").await?;

    let patch_columns = sqlx::query("PRAGMA table_info(planner_feature_patches)")
        .fetch_all(db)
        .await?;
    let patches_need_rebuild = patch_columns.iter().any(|row| {
        matches!(row.get::<String, _>("name").as_str(), "repo_id" | "sprint_id")
    });

    if patches_need_rebuild {
        sqlx::query(
            r#"
            CREATE TABLE planner_feature_patches_next (
                id TEXT PRIMARY KEY,
                feature_id TEXT NOT NULL REFERENCES planner_features(id) ON DELETE CASCADE,
                planner_id TEXT NOT NULL DEFAULT '' REFERENCES planner_workspaces(id) ON DELETE CASCADE,
                supervisor_run_id TEXT,
                workflow_run_id TEXT,
                patch_kind TEXT NOT NULL DEFAULT 'development',
                repo_ref TEXT NOT NULL,
                base_commit TEXT,
                head_commit TEXT,
                patch_text TEXT NOT NULL,
                patch_hash TEXT NOT NULL,
                patch_path TEXT,
                created_at TEXT NOT NULL
            )
            "#,
        )
        .execute(db)
        .await?;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO planner_feature_patches_next (id, feature_id, planner_id, supervisor_run_id, workflow_run_id, patch_kind, repo_ref, base_commit, head_commit, patch_text, patch_hash, patch_path, created_at)
            SELECT id, feature_id, COALESCE(NULLIF(planner_id, ''), NULLIF(repo_id, ''), ''), supervisor_run_id, workflow_run_id, patch_kind, repo_ref, base_commit, head_commit, patch_text, patch_hash, patch_path, created_at
            FROM planner_feature_patches
            "#,
        )
        .execute(db)
        .await?;

        sqlx::query("DROP TABLE planner_feature_patches")
            .execute(db)
            .await?;
        sqlx::query("ALTER TABLE planner_feature_patches_next RENAME TO planner_feature_patches")
            .execute(db)
            .await?;
    }

    sqlx::query("DROP TABLE IF EXISTS sprint_events")
        .execute(db)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS sprint_features")
        .execute(db)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS sprints")
        .execute(db)
        .await?;
    sqlx::query("DROP TABLE IF EXISTS planner_repos")
        .execute(db)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_workspaces_root_default_updated ON planner_workspaces (root_repo_path, is_default, updated_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_planner_order ON planner_features (planner_id, sort_order, created_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_planner_status_updated ON planner_features (planner_id, status, updated_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_supervisor_lock ON planner_features (locked_supervisor_run_id, planner_id, completed_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_feature_patches_feature_created ON planner_feature_patches (feature_id, created_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_feature_patches_planner_created ON planner_feature_patches (planner_id, created_at)")
        .execute(db)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_feature_patches_workflow ON planner_feature_patches (workflow_run_id)")
        .execute(db)
        .await?;

    let template_columns = sqlx::query("PRAGMA table_info(workflow_templates)")
        .fetch_all(db)
        .await?;
    let has_template_repo_ref = template_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "repo_ref");
    if !has_template_repo_ref {
        sqlx::query("ALTER TABLE workflow_templates ADD COLUMN repo_ref TEXT NOT NULL DEFAULT ''")
            .execute(db)
            .await?;
    }

    let run_columns = sqlx::query("PRAGMA table_info(workflow_runs)")
        .fetch_all(db)
        .await?;
    let has_run_definition_json = run_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "definition_json");
    if !has_run_definition_json {
        sqlx::query("ALTER TABLE workflow_runs ADD COLUMN definition_json TEXT NOT NULL DEFAULT ''")
            .execute(db)
            .await?;
    }

    let run_columns = sqlx::query("PRAGMA table_info(workflow_runs)")
        .fetch_all(db)
        .await?;
    let has_run_workflow_key = run_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "workflow_key");
    if !has_run_workflow_key {
        sqlx::query("ALTER TABLE workflow_runs ADD COLUMN workflow_key TEXT NOT NULL DEFAULT ''")
            .execute(db)
            .await?;
    }

    let attempt_columns = sqlx::query("PRAGMA table_info(changeset_attempts)")
        .fetch_all(db)
        .await?;
    let has_attempt_workflow_key = attempt_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "workflow_key");
    if !has_attempt_workflow_key {
        sqlx::query("ALTER TABLE changeset_attempts ADD COLUMN workflow_key TEXT NOT NULL DEFAULT ''")
            .execute(db)
            .await?;
    }

    sqlx::query("UPDATE workflow_runs SET status = 'complete' WHERE status = 'success'")
        .execute(db)
        .await?;

    sqlx::query(
        r#"
        UPDATE supervisor_work_units
        SET state = CASE
                WHEN workflow_run_id IS NULL THEN state
                WHEN EXISTS (
                    SELECT 1
                    FROM workflow_runs wr
                    WHERE wr.id = supervisor_work_units.workflow_run_id
                      AND wr.status = 'complete'
                ) THEN 'complete'
                WHEN EXISTS (
                    SELECT 1
                    FROM workflow_runs wr
                    WHERE wr.id = supervisor_work_units.workflow_run_id
                ) THEN (
                    SELECT wr.status
                    FROM workflow_runs wr
                    WHERE wr.id = supervisor_work_units.workflow_run_id
                )
                ELSE state
            END,
            updated_at = CASE
                WHEN workflow_run_id IS NOT NULL THEN ?
                ELSE updated_at
            END
        WHERE workflow_run_id IS NOT NULL
          AND archived_at IS NULL
        "#,
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(db)
    .await?;

    backfill_workflow_keys(db).await?;
    backfill_changeset_workflow_keys(db).await?;

    sqlx::query(
        r#"
        UPDATE workflow_runs
        SET definition_json = (
            SELECT workflow_templates.definition_json
            FROM workflow_templates
            WHERE workflow_templates.id = workflow_runs.template_id
        )
        WHERE TRIM(COALESCE(definition_json, '')) = ''
          AND template_id IS NOT NULL
        "#,
    )
    .execute(db)
    .await?;

    Ok(())
}
