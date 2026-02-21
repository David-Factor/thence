use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFIG_RELATIVE_PATH: &str = ".thence/config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub version: u32,
    pub agent: Option<AgentConfig>,
    pub checks: Option<ChecksConfig>,
    pub prompts: Option<PromptsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub provider: Option<String>,
    pub command: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChecksConfig {
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptsConfig {
    pub reviewer: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRepoConfig {
    version: Option<u32>,
    agent: Option<RawAgentConfig>,
    checks: Option<RawChecksConfig>,
    prompts: Option<RawPromptsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAgentConfig {
    provider: Option<String>,
    command: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawChecksConfig {
    commands: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawPromptsConfig {
    reviewer: Option<String>,
}

pub fn repo_config_path(repo_root: &Path) -> PathBuf {
    repo_root.join(CONFIG_RELATIVE_PATH)
}

pub fn load_repo_config(repo_root: &Path) -> Result<Option<RepoConfig>> {
    let path = repo_config_path(repo_root);
    if !path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read repo config {}", path.display()))?;
    let parsed: RawRepoConfig =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(validate_repo_config(parsed, &path)?))
}

fn validate_repo_config(raw: RawRepoConfig, path: &Path) -> Result<RepoConfig> {
    let version = raw
        .version
        .ok_or_else(|| anyhow::anyhow!("{} missing required `version`", path.display()))?;
    if version != 1 {
        bail!(
            "{} has unsupported version {version}; expected version = 1",
            path.display()
        );
    }

    let agent = raw
        .agent
        .map(|agent| {
            let provider = sanitize_optional(agent.provider);
            if let Some(provider) = provider.as_deref()
                && provider != "codex"
            {
                bail!("only `codex` supported in this version");
            }
            Ok(AgentConfig {
                provider,
                command: sanitize_optional(agent.command),
            })
        })
        .transpose()?;

    let checks = raw
        .checks
        .map(|checks| {
            let commands = checks.commands.ok_or_else(|| {
                anyhow::anyhow!("{} missing `[checks].commands` in config", path.display())
            })?;
            let commands = sanitize_commands(commands);
            if commands.is_empty() {
                bail!("{} has empty `[checks].commands`", path.display());
            }
            Ok(ChecksConfig { commands })
        })
        .transpose()?;

    let prompts = raw.prompts.map(|prompts| PromptsConfig {
        reviewer: sanitize_optional(prompts.reviewer),
    });

    Ok(RepoConfig {
        version,
        agent,
        checks,
        prompts,
    })
}

fn sanitize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn sanitize_commands(commands: Vec<String>) -> Vec<String> {
    commands
        .into_iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_valid_minimal_config() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 1
[checks]
commands = ["cargo check", "cargo test"]
"#,
        )
        .unwrap();

        let cfg = load_repo_config(repo).unwrap().unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(
            cfg.checks.unwrap().commands,
            vec!["cargo check".to_string(), "cargo test".to_string()]
        );
    }

    #[test]
    fn rejects_invalid_version() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "version = 2").unwrap();

        let err = load_repo_config(repo).unwrap_err();
        assert!(format!("{err}").contains("unsupported version"));
    }

    #[test]
    fn rejects_missing_or_empty_checks_commands() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        std::fs::write(
            &path,
            r#"
version = 1
[checks]
"#,
        )
        .unwrap();
        let err_missing = load_repo_config(repo).unwrap_err();
        assert!(format!("{err_missing}").contains("missing `[checks].commands`"));

        std::fs::write(
            &path,
            r#"
version = 1
[checks]
commands = []
"#,
        )
        .unwrap();
        let err_empty = load_repo_config(repo).unwrap_err();
        assert!(format!("{err_empty}").contains("empty `[checks].commands`"));
    }

    #[test]
    fn loads_reviewer_prompt_override() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 1
[checks]
commands = ["cargo test"]
[prompts]
reviewer = "Return strict JSON only."
"#,
        )
        .unwrap();

        let cfg = load_repo_config(repo).unwrap().unwrap();
        let reviewer = cfg
            .prompts
            .and_then(|p| p.reviewer)
            .expect("missing reviewer");
        assert_eq!(reviewer, "Return strict JSON only.");
    }
}
