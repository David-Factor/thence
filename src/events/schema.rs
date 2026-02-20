use rusqlite::{Connection, Result};

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            plan_path TEXT NOT NULL,
            plan_sha256 TEXT NOT NULL,
            spl_plan_path TEXT NOT NULL,
            created_at TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('running','completed','failed','cancelled')),
            config_json TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS events (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL REFERENCES runs(id),
            ts TEXT NOT NULL,
            event_type TEXT NOT NULL,
            task_id TEXT,
            actor_role TEXT,
            actor_id TEXT,
            attempt INTEGER,
            payload_json TEXT NOT NULL,
            dedupe_key TEXT,
            FOREIGN KEY(run_id) REFERENCES runs(id)
        );

        CREATE INDEX IF NOT EXISTS idx_events_run_seq ON events(run_id, seq);
        CREATE INDEX IF NOT EXISTS idx_events_run_task_seq ON events(run_id, task_id, seq);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_events_run_dedupe ON events(run_id, dedupe_key) WHERE dedupe_key IS NOT NULL;

        CREATE TABLE IF NOT EXISTS snapshots (
            run_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            state_json TEXT NOT NULL,
            PRIMARY KEY(run_id, seq)
        );
        ",
    )?;

    Ok(())
}
