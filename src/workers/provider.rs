use anyhow::{bail, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub role: String,
    pub task_id: String,
    pub attempt: i64,
    pub worktree_path: PathBuf,
    pub prompt: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub exit_code: i32,
    pub stdout_path: PathBuf,
    pub stderr_path: PathBuf,
    pub structured_output: Option<Value>,
}

pub trait AgentProvider {
    fn run(&self, req: AgentRequest) -> Result<AgentResult>;
}

pub fn provider_for(name: &str) -> Result<Box<dyn AgentProvider>> {
    match name {
        "codex" => Ok(Box::new(crate::workers::codex::CodexProvider::new("codex"))),
        "claude" => Ok(Box::new(crate::workers::codex::CodexProvider::new(
            "claude",
        ))),
        "opencode" => Ok(Box::new(crate::workers::codex::CodexProvider::new(
            "opencode",
        ))),
        other => {
            bail!("unsupported --agent provider '{other}'. Supported: codex, claude, opencode")
        }
    }
}
