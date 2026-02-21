use crate::events::{EventRow, NewEvent, schema};
use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: String,
    pub plan_path: String,
    pub plan_sha256: String,
    pub spl_plan_path: String,
    pub created_at: String,
    pub status: String,
    pub config_json: Value,
}

pub struct EventStore {
    conn: Connection,
}

impl EventStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db parent dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite db {}", path.display()))?;
        schema::migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn create_run(&self, row: &RunRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, plan_path, plan_sha256, spl_plan_path, created_at, status, config_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.id,
                row.plan_path,
                row.plan_sha256,
                row.spl_plan_path,
                row.created_at,
                row.status,
                row.config_json.to_string()
            ],
        )?;
        Ok(())
    }

    pub fn update_run_status(&self, run_id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = ?2 WHERE id = ?1",
            params![run_id, status],
        )?;
        Ok(())
    }

    pub fn update_run_config(&self, run_id: &str, config_json: &Value) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET config_json = ?2 WHERE id = ?1",
            params![run_id, config_json.to_string()],
        )?;
        Ok(())
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>> {
        self.conn
            .query_row(
                "SELECT id, plan_path, plan_sha256, spl_plan_path, created_at, status, config_json FROM runs WHERE id = ?1",
                params![run_id],
                |row| {
                    let cfg: String = row.get(6)?;
                    Ok(RunRow {
                        id: row.get(0)?,
                        plan_path: row.get(1)?,
                        plan_sha256: row.get(2)?,
                        spl_plan_path: row.get(3)?,
                        created_at: row.get(4)?,
                        status: row.get(5)?,
                        config_json: serde_json::from_str(&cfg).unwrap_or(Value::Null),
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_resumable_run_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM runs WHERE status = 'running' ORDER BY created_at ASC")?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn append_event(&self, run_id: &str, event: &NewEvent) -> Result<Option<i64>> {
        let ts = Utc::now().to_rfc3339();
        let tx = self.conn.unchecked_transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO events (run_id, ts, event_type, task_id, actor_role, actor_id, attempt, payload_json, dedupe_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run_id,
                ts,
                event.event_type,
                event.task_id,
                event.actor_role,
                event.actor_id,
                event.attempt,
                event.payload_json.to_string(),
                event.dedupe_key
            ],
        )?;
        let seq = if inserted == 0 {
            None
        } else {
            Some(tx.last_insert_rowid())
        };
        tx.commit()?;
        Ok(seq)
    }

    pub fn list_events(&self, run_id: &str) -> Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, run_id, ts, event_type, task_id, actor_role, actor_id, attempt, payload_json, dedupe_key
             FROM events WHERE run_id = ?1 ORDER BY seq ASC",
        )?;

        let rows = stmt
            .query_map(params![run_id], |row| {
                let payload_str: String = row.get(8)?;
                Ok(EventRow {
                    seq: row.get(0)?,
                    run_id: row.get(1)?,
                    ts: row.get(2)?,
                    event_type: row.get(3)?,
                    task_id: row.get(4)?,
                    actor_role: row.get(5)?,
                    actor_id: row.get(6)?,
                    attempt: row.get(7)?,
                    payload_json: serde_json::from_str(&payload_str).unwrap_or(Value::Null),
                    dedupe_key: row.get(9)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn unresolved_questions(&self, run_id: &str) -> Result<Vec<(String, String)>> {
        let events = self.list_events(run_id)?;
        let mut opened = Vec::new();
        for ev in &events {
            if ev.event_type == "spec_question_opened"
                && let Some(id) = ev.payload_json.get("question_id").and_then(|v| v.as_str())
            {
                let text = ev
                    .payload_json
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                opened.push((id.to_string(), text));
            }
        }
        let resolved: std::collections::HashSet<String> = events
            .iter()
            .filter(|ev| ev.event_type == "spec_question_resolved")
            .filter_map(|ev| {
                ev.payload_json
                    .get("question_id")
                    .and_then(|v| v.as_str())
                    .map(ToString::to_string)
            })
            .collect();

        Ok(opened
            .into_iter()
            .filter(|(id, _)| !resolved.contains(id))
            .collect())
    }
}
