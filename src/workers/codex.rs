use crate::workers::provider::{AgentProvider, AgentRequest, AgentResult};
use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct CodexProvider {
    provider_name: String,
    cmd_override: Option<String>,
}

impl CodexProvider {
    pub fn new(provider_name: &str, cmd_override: Option<String>) -> Self {
        Self {
            provider_name: provider_name.to_string(),
            cmd_override,
        }
    }
}

impl AgentProvider for CodexProvider {
    fn run(&self, req: AgentRequest) -> Result<AgentResult> {
        if let Some(cmd) = resolve_agent_cmd(&self.provider_name, self.cmd_override.as_deref()) {
            return run_subprocess_agent(&cmd, &self.provider_name, req);
        }
        run_stub_agent(&self.provider_name, req)
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
            "provider={}\nmode=stub\ntimeout_secs={}\nprompt_len={}\n",
            provider_name,
            req.timeout.as_secs(),
            req.prompt.len()
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
    } else {
        Some(json!({"submitted": true}))
    };

    Ok(AgentResult {
        exit_code: if req.role == "implementer" && req.prompt.contains("[impl-fail]") {
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
    let mut child = Command::new("sh")
        .arg("-lc")
        .arg(cmd)
        .current_dir(&req.worktree_path)
        .env("WHENCE_PROVIDER", provider_name)
        .env("WHENCE_ROLE", &req.role)
        .env("WHENCE_TASK_ID", &req.task_id)
        .env("WHENCE_ATTEMPT", req.attempt.to_string())
        .env("WHENCE_WORKTREE", &req.worktree_path)
        .env("WHENCE_PROMPT_FILE", &prompt_path)
        .env("WHENCE_RESULT_FILE", &result_path)
        .env("WHENCE_TIMEOUT_SECS", req.timeout.as_secs().to_string())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
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

fn resolve_agent_cmd(provider_name: &str, override_cmd: Option<&str>) -> Option<String> {
    let provider_key = format!("WHENCE_AGENT_CMD_{}", provider_name.to_ascii_uppercase());
    override_cmd
        .map(str::to_string)
        .or_else(|| std::env::var(&provider_key).ok())
        .or_else(|| std::env::var("WHENCE_AGENT_CMD").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
