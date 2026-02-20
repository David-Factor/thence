use crate::events::EventRow;
use anyhow::Result;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

pub fn mirror_event(path: &Path, ev: &EventRow) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    let line = json!({
        "seq": ev.seq,
        "ts": ev.ts,
        "event": ev.event_type,
        "task": ev.task_id,
        "attempt": ev.attempt
    });
    writeln!(f, "{}", line)?;
    Ok(())
}
