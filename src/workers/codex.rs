use crate::workers::provider::{AgentProvider, AgentRequest, AgentResult};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CODEX_SETUP_ERROR: &str = "Non-simulated runs require a runnable codex command. Install codex or set `[agent].command` in `.thence/config.toml`.";

#[derive(Debug)]
pub struct CodexProvider {
    simulate: bool,
    command: Option<String>,
}

impl CodexProvider {
    pub fn new(simulate: bool, command: Option<&str>) -> Result<Self> {
        let resolved = if simulate {
            None
        } else {
            Some(resolve_agent_cmd(command)?)
        };
        Ok(Self {
            simulate,
            command: resolved,
        })
    }
}

impl AgentProvider for CodexProvider {
    fn run(&self, req: AgentRequest) -> Result<AgentResult> {
        if self.simulate {
            return run_stub_agent("codex", req);
        }
        let cmd = self
            .command
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(CODEX_SETUP_ERROR))?;
        run_subprocess_agent(cmd, "codex", req)
    }
}

fn run_stub_agent(provider_name: &str, req: AgentRequest) -> Result<AgentResult> {
    fs::create_dir_all(&req.worktree_path)?;
    let stdout_path = req
        .worktree_path
        .join(format!("{}_attempt{}_stdout.log", req.role, req.attempt));
    let stderr_path = req
        .worktree_path
        .join(format!("{}_attempt{}_stderr.log", req.role, req.attempt));
    let metadata_path = req
        .worktree_path
        .join(format!("{}_attempt{}_meta.log", req.role, req.attempt));
    fs::write(
        &stdout_path,
        format!(
            "{} provider={} completed task {}\n",
            req.role, provider_name, req.task_id
        ),
    )?;
    fs::write(&stderr_path, "")?;
    fs::write(
        &metadata_path,
        format!(
            "provider={}\nmode=stub\ntimeout_secs={}\nprompt_len={}\nenv_count={}\n",
            provider_name,
            req.timeout.as_secs(),
            req.prompt.len(),
            req.env.len()
        ),
    )?;

    let structured = if req.role == "reviewer" {
        if req.prompt.contains("[missing-review-output]") {
            None
        } else if req.prompt.contains("[needs-fix]") && req.attempt == 1 {
            Some(
                json!({"approved": false, "findings": ["Auto finding from reviewer token [needs-fix]"]}),
            )
        } else {
            Some(json!({"approved": true, "findings": []}))
        }
    } else if req.role == "checks-proposer" {
        Some(json!({
            "commands": ["true"],
            "rationale": "default local proposal"
        }))
    } else if req.role == "plan-translator" {
        stub_plan_translation(&req.prompt).ok()
    } else {
        Some(json!({"submitted": true}))
    };

    Ok(AgentResult {
        exit_code: if req.role == "implementer" && req.prompt.contains("[impl-fail]") {
            2
        } else if req.role == "plan-translator" && structured.is_none() {
            2
        } else {
            0
        },
        stdout_path,
        stderr_path,
        structured_output: structured,
    })
}

fn run_subprocess_agent(cmd: &str, provider_name: &str, req: AgentRequest) -> Result<AgentResult> {
    fs::create_dir_all(&req.worktree_path)?;
    let stdout_path = req
        .worktree_path
        .join(format!("{}_attempt{}_stdout.log", req.role, req.attempt));
    let stderr_path = req
        .worktree_path
        .join(format!("{}_attempt{}_stderr.log", req.role, req.attempt));
    let prompt_path = req
        .worktree_path
        .join(format!("{}_attempt{}_prompt.json", req.role, req.attempt));
    let result_path = req
        .worktree_path
        .join(format!("{}_attempt{}_result.json", req.role, req.attempt));
    let metadata_path = req
        .worktree_path
        .join(format!("{}_attempt{}_meta.log", req.role, req.attempt));

    fs::write(&prompt_path, &req.prompt)
        .with_context(|| format!("write prompt file for {} attempt {}", req.role, req.attempt))?;

    let stdout_file = fs::File::create(&stdout_path)?;
    let stderr_file = fs::File::create(&stderr_path)?;
    let mut command = Command::new("sh");
    command
        .arg("-lc")
        .arg(cmd)
        .current_dir(&req.worktree_path)
        .env("THENCE_PROVIDER", provider_name)
        .env("THENCE_ROLE", &req.role)
        .env("THENCE_TASK_ID", &req.task_id)
        .env("THENCE_ATTEMPT", req.attempt.to_string())
        .env("THENCE_WORKTREE", &req.worktree_path)
        .env("THENCE_PROMPT_FILE", &prompt_path)
        .env("THENCE_RESULT_FILE", &result_path)
        .env("THENCE_TIMEOUT_SECS", req.timeout.as_secs().to_string())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    for (k, v) in &req.env {
        command.env(k, v);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn subprocess provider command for {}", req.role))?;

    let started = Instant::now();
    let mut timed_out = false;
    let exit_code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().unwrap_or(1);
        }
        if started.elapsed() >= req.timeout {
            timed_out = true;
            let _ = child.kill();
            let status = child.wait()?;
            break status.code().unwrap_or(124);
        }
        thread::sleep(Duration::from_millis(100));
    };

    let structured_output = if result_path.exists() {
        let raw = fs::read_to_string(&result_path)
            .with_context(|| format!("read result file {}", result_path.display()))?;
        serde_json::from_str(&raw).ok()
    } else {
        let mut stdout_raw = String::new();
        fs::File::open(&stdout_path)?.read_to_string(&mut stdout_raw)?;
        serde_json::from_str(&stdout_raw).ok()
    };

    fs::write(
        &metadata_path,
        format!(
            "provider={}\nmode=subprocess\ncommand={}\ntimeout_secs={}\ntimed_out={}\nprompt_file={}\nresult_file={}\n",
            provider_name,
            cmd,
            req.timeout.as_secs(),
            timed_out,
            prompt_path.display(),
            result_path.display(),
        ),
    )?;

    Ok(AgentResult {
        exit_code: if timed_out { 124 } else { exit_code },
        stdout_path,
        stderr_path,
        structured_output,
    })
}

fn resolve_agent_cmd(command: Option<&str>) -> Result<String> {
    let cmd = command.unwrap_or("codex").trim().to_string();
    if cmd.is_empty() {
        bail!(CODEX_SETUP_ERROR);
    }

    let executable = cmd.split_whitespace().next().unwrap_or("");
    if executable.is_empty() || !is_runnable(executable) {
        bail!(CODEX_SETUP_ERROR);
    }
    Ok(cmd)
}

fn is_runnable(executable: &str) -> bool {
    let quoted = shell_quote(executable);
    match Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {quoted} >/dev/null 2>&1"))
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\"'\"'"))
}

fn stub_plan_translation(prompt: &str) -> Result<serde_json::Value> {
    let parsed: serde_json::Value =
        serde_json::from_str(prompt).context("parse translator prompt")?;
    let markdown = parsed
        .get("spec_markdown")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("translator prompt missing spec_markdown"))?;
    let default_checks = parsed
        .get("default_checks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec!["true".to_string()]);

    let translated = crate::plan::translator::translate_markdown_to_spl(markdown, &default_checks)?;

    Ok(serde_json::json!({
        "spl": translated.spl,
        "tasks": translated.tasks
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_mode_allows_stub_without_command() {
        let provider = CodexProvider::new(true, None);
        assert!(provider.is_ok());
    }

    #[test]
    fn non_simulated_mode_requires_runnable_command() {
        let err = CodexProvider::new(false, Some("this-command-does-not-exist-xyz"));
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("Install codex or set `[agent].command`"));
    }
}
