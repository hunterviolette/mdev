use std::str::FromStr;

use sqlx::{sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions}, Row, SqlitePool};

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

    sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS idx_workflow_templates_name ON workflow_templates (name)")
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
