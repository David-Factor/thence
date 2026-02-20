use crate::workers::provider::{AgentProvider, AgentRequest, AgentResult};
use anyhow::Result;
use serde_json::json;
use std::fs;

pub struct CodexProvider {
    provider_name: String,
}

impl CodexProvider {
    pub fn new(provider_name: &str) -> Self {
        Self {
            provider_name: provider_name.to_string(),
        }
    }
}

impl AgentProvider for CodexProvider {
    fn run(&self, req: AgentRequest) -> Result<AgentResult> {
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
                req.role, self.provider_name, req.task_id
            ),
        )?;
        fs::write(&stderr_path, "")?;
        fs::write(
            &metadata_path,
            format!(
                "timeout_secs={}\nprompt_len={}\n",
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
}
