use crate::run::run_artifact_dir;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(crate) const LEASE_SCHEMA_VERSION: u32 = 1;
pub(crate) const LEASE_TICK_SECS: u64 = 15;
pub(crate) const LEASE_STALE_AFTER_SECS: i64 = 90;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LeaseState {
    Active,
    Released,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptLeaseRecord {
    version: u32,
    run_id: String,
    task_id: String,
    attempt: i64,
    role: String,
    owner_pid: u32,
    started_at: String,
    last_seen_at: String,
    state: LeaseState,
}

#[derive(Debug, Clone)]
struct ParsedLease {
    path: PathBuf,
    record: AttemptLeaseRecord,
    last_seen_at: DateTime<Utc>,
    age_secs: i64,
    owner_alive: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum OrphanLeaseDecision {
    Interrupt { reason: String, details: Value },
    LikelyActive { reason: String, details: Value },
}

pub(crate) struct LeaseTicker {
    stop_tx: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl LeaseTicker {
    pub(crate) fn start(path: PathBuf, interval: Duration) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            loop {
                match stop_rx.recv_timeout(interval) {
                    Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let _ = tick_active_lease(&path);
                    }
                }
            }
        });
        Self {
            stop_tx: Some(stop_tx),
            handle: Some(handle),
        }
    }

    pub(crate) fn stop(mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn lease_path(
    repo_root: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    role: &str,
) -> PathBuf {
    run_artifact_dir(repo_root, run_id)
        .join("leases")
        .join(task_id)
        .join(format!("attempt{attempt}"))
        .join(format!("{role}.json"))
}

pub(crate) fn init_active_lease(
    repo_root: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    role: &str,
) -> Result<PathBuf> {
    let path = lease_path(repo_root, run_id, task_id, attempt, role);
    let now = Utc::now().to_rfc3339();
    let record = AttemptLeaseRecord {
        version: LEASE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        task_id: task_id.to_string(),
        attempt,
        role: role.to_string(),
        owner_pid: std::process::id(),
        started_at: now.clone(),
        last_seen_at: now,
        state: LeaseState::Active,
    };
    write_lease(&path, &record)?;
    Ok(path)
}

pub(crate) fn tick_active_lease(path: &Path) -> Result<()> {
    let mut record = read_lease(path)?;
    if record.state == LeaseState::Active {
        record.last_seen_at = Utc::now().to_rfc3339();
        write_lease(path, &record)?;
    }
    Ok(())
}

pub(crate) fn release_lease(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut record = read_lease(path)?;
    record.state = LeaseState::Released;
    record.last_seen_at = Utc::now().to_rfc3339();
    write_lease(path, &record)?;
    Ok(())
}

pub(crate) fn evaluate_orphan_attempt(
    repo_root: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
) -> Result<OrphanLeaseDecision> {
    evaluate_orphan_attempt_at(repo_root, run_id, task_id, attempt, Utc::now())
}

pub(crate) fn evaluate_orphan_attempt_at(
    repo_root: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    now: DateTime<Utc>,
) -> Result<OrphanLeaseDecision> {
    let mut parsed = Vec::<ParsedLease>::new();
    for role in ["implementer", "reviewer"] {
        let path = lease_path(repo_root, run_id, task_id, attempt, role);
        if !path.exists() {
            continue;
        }
        let record = read_lease(&path)
            .with_context(|| format!("read lease for {task_id} attempt {attempt} role {role}"))?;
        let last_seen_at = DateTime::parse_from_rfc3339(&record.last_seen_at)
            .with_context(|| format!("parse lease last_seen_at from {}", path.display()))?
            .with_timezone(&Utc);
        let age_secs = now.signed_duration_since(last_seen_at).num_seconds().max(0);
        let owner_alive = process_alive(record.owner_pid);
        parsed.push(ParsedLease {
            path,
            record,
            last_seen_at,
            age_secs,
            owner_alive,
        });
    }

    if parsed.is_empty() {
        return Ok(OrphanLeaseDecision::Interrupt {
            reason: "orphaned in-flight attempt detected on resume (no lease found)".to_string(),
            details: json!({
                "state": "missing",
                "stale_after_secs": LEASE_STALE_AFTER_SECS
            }),
        });
    }

    parsed.sort_by_key(|p| p.last_seen_at);
    let newest = parsed
        .last()
        .cloned()
        .context("missing newest lease after parsing")?;
    let details = json!({
        "path": newest.path,
        "role": newest.record.role,
        "owner_pid": newest.record.owner_pid,
        "owner_alive": newest.owner_alive,
        "started_at": newest.record.started_at,
        "last_seen_at": newest.record.last_seen_at,
        "age_secs": newest.age_secs,
        "stale_after_secs": LEASE_STALE_AFTER_SECS,
        "state": match newest.record.state {
            LeaseState::Active => "active",
            LeaseState::Released => "released",
        }
    });

    if newest.record.state == LeaseState::Released {
        return Ok(OrphanLeaseDecision::Interrupt {
            reason: "orphaned in-flight attempt detected on resume (lease released without terminal event)"
                .to_string(),
            details,
        });
    }

    if newest.age_secs <= LEASE_STALE_AFTER_SECS {
        let reason = if newest.owner_alive {
            format!(
                "run appears active: recent active lease for task '{}' attempt {} (owner pid {} alive; age={}s)",
                task_id, attempt, newest.record.owner_pid, newest.age_secs
            )
        } else {
            format!(
                "run has recent active lease for task '{}' attempt {} (owner pid {} not alive; age={}s). wait until stale window ({}s) before resuming",
                task_id, attempt, newest.record.owner_pid, newest.age_secs, LEASE_STALE_AFTER_SECS
            )
        };
        return Ok(OrphanLeaseDecision::LikelyActive { reason, details });
    }

    Ok(OrphanLeaseDecision::Interrupt {
        reason: format!(
            "orphaned in-flight attempt detected on resume (stale lease age={}s)",
            newest.age_secs
        ),
        details,
    })
}

pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut cmd = Command::new("kill");
    cmd.arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.status().map(|status| status.success()).unwrap_or(false)
}

fn read_lease(path: &Path) -> Result<AttemptLeaseRecord> {
    let raw = fs::read_to_string(path).with_context(|| format!("read lease {}", path.display()))?;
    let record = serde_json::from_str(&raw)
        .with_context(|| format!("parse lease JSON {}", path.display()))?;
    Ok(record)
}

fn write_lease(path: &Path, record: &AttemptLeaseRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create lease dir {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(record)?;
    write_atomic(path, &raw)
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&tmp, content).with_context(|| format!("write temp lease {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename temp lease {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use tempfile::tempdir;

    #[test]
    fn lease_lifecycle_roundtrip() {
        let tmp = tempdir().unwrap();
        let path = init_active_lease(tmp.path(), "run-1", "task-a", 1, "implementer").unwrap();
        tick_active_lease(&path).unwrap();
        release_lease(&path).unwrap();
        let raw = fs::read_to_string(path).unwrap();
        assert!(raw.contains("\"state\": \"released\""));
    }

    #[test]
    fn recent_active_lease_is_likely_active() {
        let tmp = tempdir().unwrap();
        let _ = init_active_lease(tmp.path(), "run-1", "task-a", 1, "implementer").unwrap();
        let decision = evaluate_orphan_attempt(tmp.path(), "run-1", "task-a", 1).unwrap();
        assert!(matches!(decision, OrphanLeaseDecision::LikelyActive { .. }));
    }

    #[test]
    fn stale_active_lease_interrupts() {
        let tmp = tempdir().unwrap();
        let path = lease_path(tmp.path(), "run-1", "task-a", 1, "implementer");
        let now = Utc::now();
        let stale = now - ChronoDuration::seconds(LEASE_STALE_AFTER_SECS + 5);
        let record = AttemptLeaseRecord {
            version: LEASE_SCHEMA_VERSION,
            run_id: "run-1".to_string(),
            task_id: "task-a".to_string(),
            attempt: 1,
            role: "implementer".to_string(),
            owner_pid: 999_999,
            started_at: stale.to_rfc3339(),
            last_seen_at: stale.to_rfc3339(),
            state: LeaseState::Active,
        };
        write_lease(&path, &record).unwrap();
        let decision = evaluate_orphan_attempt_at(tmp.path(), "run-1", "task-a", 1, now).unwrap();
        assert!(matches!(decision, OrphanLeaseDecision::Interrupt { .. }));
    }
}
