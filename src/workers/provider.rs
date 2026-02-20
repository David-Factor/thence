use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCommandConfig {
    #[serde(default)]
    pub default_cmd: Option<String>,
    #[serde(default)]
    pub codex: Option<String>,
    #[serde(default)]
    pub claude: Option<String>,
    #[serde(default)]
    pub opencode: Option<String>,
}

impl AgentCommandConfig {
    pub fn for_provider(&self, name: &str) -> Option<String> {
        let v = match name {
            "codex" => self.codex.as_deref(),
            "claude" => self.claude.as_deref(),
            "opencode" => self.opencode.as_deref(),
            _ => None,
        }
        .or(self.default_cmd.as_deref())?;
        let trimmed = v.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

pub fn provider_for(name: &str, cmd_cfg: &AgentCommandConfig) -> Result<Box<dyn AgentProvider>> {
    let cmd_override = cmd_cfg.for_provider(name);
    match name {
        "codex" => Ok(Box::new(crate::workers::codex::CodexProvider::new(
            "codex",
            cmd_override,
        ))),
        "claude" => Ok(Box::new(crate::workers::codex::CodexProvider::new(
            "claude",
            cmd_override,
        ))),
        "opencode" => Ok(Box::new(crate::workers::codex::CodexProvider::new(
            "opencode",
            cmd_override,
        ))),
        other => {
            bail!("unsupported --agent provider '{other}'. Supported: codex, claude, opencode")
        }
    }
}
