use anyhow::{Result, bail};
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
    pub env: Vec<(String, String)>,
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

pub fn provider_for(
    name: &str,
    simulate: bool,
    command: Option<&str>,
) -> Result<Box<dyn AgentProvider>> {
    if name != "codex" {
        bail!("only `codex` supported in this version");
    }
    Ok(Box::new(crate::workers::codex::CodexProvider::new(
        simulate, command,
    )?))
}
