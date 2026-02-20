use anyhow::Result;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

pub fn run_checks(
    worktree: &Path,
    commands: &[String],
    timeout: Duration,
) -> Result<(bool, serde_json::Value)> {
    let mut results = Vec::new();
    let mut passed = true;

    for cmd in commands {
        let mut child = Command::new("sh")
            .arg("-lc")
            .arg(cmd)
            .current_dir(worktree)
            .spawn()?;
        let start = Instant::now();
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() >= timeout {
                timed_out = true;
                let _ = child.kill();
                break child.wait()?;
            }
            thread::sleep(Duration::from_millis(100));
        };
        let ok = status.success() && !timed_out;
        if !ok {
            passed = false;
        }
        results.push(json!({
            "command": cmd,
            "ok": ok,
            "timed_out": timed_out,
            "timeout_secs": timeout.as_secs()
        }));
    }

    Ok((passed, json!({"passed": passed, "results": results})))
}
