use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksFile {
    pub version: u32,
    pub commands: Vec<String>,
    pub updated_at: String,
    pub source: String,
}

fn checks_file_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".thence").join("checks.json")
}

fn validate(commands: &[String]) -> Result<()> {
    if commands.is_empty() {
        bail!("checks file has empty command list")
    }
    if commands.iter().any(|c| c.trim().is_empty()) {
        bail!("checks file contains empty command")
    }
    Ok(())
}

pub fn load_checks_file(repo_root: &Path) -> Result<Option<Vec<String>>> {
    let path = checks_file_path(repo_root);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read checks file {}", path.display()))?;
    let parsed: ChecksFile = serde_json::from_str(&raw)
        .with_context(|| format!("parse checks file {}", path.display()))?;
    if parsed.version != 1 {
        bail!(
            "checks file {} has unsupported version {}",
            path.display(),
            parsed.version
        )
    }
    validate(&parsed.commands)?;
    Ok(Some(parsed.commands))
}

pub fn save_checks_file(repo_root: &Path, commands: &[String], source: &str) -> Result<()> {
    validate(commands)?;
    let path = checks_file_path(repo_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create checks dir {}", parent.display()))?;
    }
    let payload = ChecksFile {
        version: 1,
        commands: commands.to_vec(),
        updated_at: Utc::now().to_rfc3339(),
        source: source.to_string(),
    };
    std::fs::write(&path, serde_json::to_string_pretty(&payload)?)
        .with_context(|| format!("write checks file {}", path.display()))?;
    Ok(())
}
