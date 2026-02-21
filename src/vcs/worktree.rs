use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn prepare_worktree(
    base: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    worker_id: &str,
) -> Result<PathBuf> {
    let dir = base
        .join(".thence")
        .join("runs")
        .join(run_id)
        .join("worktrees")
        .join(format!("thence/{task_id}/v{attempt}/{worker_id}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
