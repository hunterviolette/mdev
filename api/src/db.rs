use std::str::FromStr;
use std::collections::HashMap;

use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, Row, SqlitePool};
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
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("DROP TABLE IF EXISTS workflow_events")
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE workflow_events (
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
            snapshot_path TEXT,
            integration_path TEXT,
            features_json TEXT NOT NULL DEFAULT '[]',
            child_runs_json TEXT NOT NULL DEFAULT '[]',
            integration_run_id TEXT,
            final_patch_path TEXT,
            merge_report_json TEXT NOT NULL DEFAULT '{}',
            validation_report_json TEXT NOT NULL DEFAULT '{}',
            context_json TEXT NOT NULL DEFAULT '{}',
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
        CREATE TABLE IF NOT EXISTS planner_repos (
            id TEXT PRIMARY KEY,
            root_repo_path TEXT NOT NULL UNIQUE,
            repo_key TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS planner_features (
            id TEXT PRIMARY KEY,
            repo_id TEXT NOT NULL REFERENCES planner_repos(id) ON DELETE CASCADE,
            title TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'planned',
            sort_order INTEGER NOT NULL DEFAULT 0,
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sprints (
            id TEXT PRIMARY KEY,
            repo_id TEXT NOT NULL REFERENCES planner_repos(id) ON DELETE CASCADE,
            sprint_key TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'planned',
            workflow_run_id TEXT,
            supervisor_run_id TEXT,
            sprint_started_at TEXT,
            development_started_at TEXT,
            development_completed_at TEXT,
            integration_started_at TEXT,
            integration_completed_at TEXT,
            sprint_completed_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            summary_json TEXT NOT NULL DEFAULT '{}'
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sprint_features (
            id TEXT PRIMARY KEY,
            sprint_id TEXT NOT NULL REFERENCES sprints(id) ON DELETE CASCADE,
            feature_id TEXT NOT NULL REFERENCES planner_features(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'planned',
            completed_at TEXT,
            sort_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE (sprint_id, feature_id)
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sprint_events (
            id TEXT PRIMARY KEY,
            sprint_id TEXT NOT NULL REFERENCES sprints(id) ON DELETE CASCADE,
            sequence_no INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            event_time TEXT NOT NULL,
            feature_id TEXT REFERENCES planner_features(id) ON DELETE SET NULL,
            actor TEXT NOT NULL DEFAULT 'system',
            message TEXT NOT NULL DEFAULT '',
            payload_json TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            UNIQUE (sprint_id, sequence_no)
        )
        "#,
    )
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_planner_features_repo_order ON planner_features (repo_id, sort_order)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sprints_repo_key ON sprints (repo_id, sprint_key)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sprint_features_sprint_order ON sprint_features (sprint_id, sort_order)")
    .execute(db)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_sprint_events_sprint_seq ON sprint_events (sprint_id, sequence_no)")
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
