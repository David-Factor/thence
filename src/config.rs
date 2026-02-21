use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

const CONFIG_RELATIVE_PATH: &str = ".thence/config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub version: u32,
    pub agent: Option<AgentConfig>,
    pub checks: Option<ChecksConfig>,
    pub prompts: Option<PromptsConfig>,
    pub worktree: Option<WorktreeConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    pub provision: Option<WorktreeProvisionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeProvisionConfig {
    pub files: Vec<ProvisionedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProvisionedFile {
    pub from: PathBuf,
    pub to: PathBuf,
    pub required: bool,
    pub mode: ProvisionMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProvisionMode {
    Symlink,
    Copy,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRepoConfig {
    version: Option<u32>,
    agent: Option<RawAgentConfig>,
    checks: Option<RawChecksConfig>,
    prompts: Option<RawPromptsConfig>,
    worktree: Option<RawWorktreeConfig>,
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

#[derive(Debug, Clone, Deserialize)]
struct RawWorktreeConfig {
    provision: Option<RawWorktreeProvisionConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawWorktreeProvisionConfig {
    files: Option<Vec<RawProvisionedFile>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProvisionedFile {
    from: Option<String>,
    to: Option<String>,
    required: Option<bool>,
    mode: Option<String>,
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
    if version != 2 {
        bail!(
            "{} has unsupported version {version}; expected version = 2",
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

    let worktree = raw
        .worktree
        .map(|worktree| validate_worktree_config(worktree, path))
        .transpose()?;

    Ok(RepoConfig {
        version,
        agent,
        checks,
        prompts,
        worktree,
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

fn validate_worktree_config(raw: RawWorktreeConfig, path: &Path) -> Result<WorktreeConfig> {
    let provision = raw
        .provision
        .map(|provision| validate_worktree_provision_config(provision, path))
        .transpose()?;
    Ok(WorktreeConfig { provision })
}

fn validate_worktree_provision_config(
    raw: RawWorktreeProvisionConfig,
    path: &Path,
) -> Result<WorktreeProvisionConfig> {
    let mut files = Vec::new();
    for (idx, file) in raw.files.unwrap_or_default().into_iter().enumerate() {
        files.push(validate_provisioned_file(file, path, idx)?);
    }
    Ok(WorktreeProvisionConfig { files })
}

fn validate_provisioned_file(
    raw: RawProvisionedFile,
    path: &Path,
    idx: usize,
) -> Result<ProvisionedFile> {
    let from_raw = raw
        .from
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} missing `from` for `[[worktree.provision.files]]` at index {idx}",
                path.display()
            )
        })?;
    let from = PathBuf::from(from_raw);
    if !from.is_absolute() {
        bail!(
            "{} has non-absolute `from` for `[[worktree.provision.files]]` at index {idx}",
            path.display()
        );
    }

    let to_raw = raw
        .to
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} missing `to` for `[[worktree.provision.files]]` at index {idx}",
                path.display()
            )
        })?;
    let to = sanitize_destination_path(&to_raw).with_context(|| {
        format!(
            "{} invalid `to` for `[[worktree.provision.files]]` at index {idx}",
            path.display()
        )
    })?;

    let mode = match raw.mode.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        None | Some("symlink") => ProvisionMode::Symlink,
        Some("copy") => ProvisionMode::Copy,
        Some(other) => {
            bail!(
                "{} has unsupported `mode = \"{}\"` for `[[worktree.provision.files]]` at index {idx}; expected `symlink` or `copy`",
                path.display(),
                other
            )
        }
    };

    Ok(ProvisionedFile {
        from,
        to,
        required: raw.required.unwrap_or(true),
        mode,
    })
}

fn sanitize_destination_path(raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        bail!("destination path must be relative");
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir => bail!("destination path must not contain `..`"),
            Component::RootDir | Component::Prefix(_) => bail!("destination path must be relative"),
        }
    }
    if clean.as_os_str().is_empty() {
        bail!("destination path is empty");
    }
    Ok(clean)
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
version = 2
[checks]
commands = ["cargo check", "cargo test"]
"#,
        )
        .unwrap();

        let cfg = load_repo_config(repo).unwrap().unwrap();
        assert_eq!(cfg.version, 2);
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
        std::fs::write(&path, "version = 1").unwrap();

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
version = 2
[checks]
"#,
        )
        .unwrap();
        let err_missing = load_repo_config(repo).unwrap_err();
        assert!(format!("{err_missing}").contains("missing `[checks].commands`"));

        std::fs::write(
            &path,
            r#"
version = 2
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
version = 2
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

    #[test]
    fn parses_worktree_provisioning_with_defaults() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 2
[checks]
commands = ["cargo test"]

[[worktree.provision.files]]
from = "/tmp/source.env"
to = ".env"
"#,
        )
        .unwrap();

        let cfg = load_repo_config(repo).unwrap().unwrap();
        let files = cfg
            .worktree
            .and_then(|w| w.provision)
            .map(|p| p.files)
            .unwrap_or_default();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].from, PathBuf::from("/tmp/source.env"));
        assert_eq!(files[0].to, PathBuf::from(".env"));
        assert!(files[0].required);
        assert_eq!(files[0].mode, ProvisionMode::Symlink);
    }

    #[test]
    fn rejects_relative_worktree_provision_source() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 2
[checks]
commands = ["cargo test"]

[[worktree.provision.files]]
from = ".env.shared"
to = ".env"
"#,
        )
        .unwrap();

        let err = load_repo_config(repo).unwrap_err();
        assert!(format!("{err}").contains("non-absolute `from`"));
    }

    #[test]
    fn rejects_escaping_worktree_destination() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 2
[checks]
commands = ["cargo test"]

[[worktree.provision.files]]
from = "/tmp/source.env"
to = "../.env"
"#,
        )
        .unwrap();

        let err = load_repo_config(repo).unwrap_err();
        assert!(format!("{err}").contains("invalid `to`"));
    }

    #[test]
    fn rejects_unknown_worktree_provision_mode() {
        let tmp = tempdir().unwrap();
        let repo = tmp.path();
        let path = repo.join(".thence").join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"
version = 2
[checks]
commands = ["cargo test"]

[[worktree.provision.files]]
from = "/tmp/source.env"
to = ".env"
mode = "hardlink"
"#,
        )
        .unwrap();

        let err = load_repo_config(repo).unwrap_err();
        assert!(format!("{err}").contains("unsupported `mode"));
    }
}
