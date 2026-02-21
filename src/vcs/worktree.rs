use crate::config::{ProvisionMode, ProvisionedFile};
use anyhow::{Context, Result, bail};
use std::path::{Component, Path, PathBuf};

pub fn prepare_worktree(
    base: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    worker_id: &str,
    provision_files: &[ProvisionedFile],
) -> Result<PathBuf> {
    let dir = base
        .join(".thence")
        .join("runs")
        .join(run_id)
        .join("worktrees")
        .join(format!("thence/{task_id}/v{attempt}/{worker_id}"));
    std::fs::create_dir_all(&dir)?;
    materialize_provisioned_files(&dir, provision_files)?;
    Ok(dir)
}

fn materialize_provisioned_files(worktree_dir: &Path, files: &[ProvisionedFile]) -> Result<()> {
    for (idx, file) in files.iter().enumerate() {
        if !file.from.exists() {
            if file.required {
                bail!(
                    "missing required source `{}` for provision rule index {}",
                    file.from.display(),
                    idx
                );
            }
            continue;
        }
        let source_metadata = std::fs::metadata(&file.from).with_context(|| {
            format!(
                "read source metadata `{}` for provision rule index {}",
                file.from.display(),
                idx
            )
        })?;
        if !source_metadata.is_file() {
            bail!(
                "source `{}` for provision rule index {} is not a regular file",
                file.from.display(),
                idx
            );
        }

        let dest_rel = sanitize_relative_path(&file.to).with_context(|| {
            format!("invalid destination path for provision rule index {}", idx)
        })?;
        let dest = worktree_dir.join(dest_rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        replace_path_if_needed(&dest)?;

        match file.mode {
            ProvisionMode::Symlink => create_symlink(&file.from, &dest).with_context(|| {
                format!(
                    "materialize symlink from `{}` to `{}` for provision rule index {}",
                    file.from.display(),
                    dest.display(),
                    idx
                )
            })?,
            ProvisionMode::Copy => {
                std::fs::copy(&file.from, &dest).with_context(|| {
                    format!(
                        "copy `{}` to `{}` for provision rule index {}",
                        file.from.display(),
                        dest.display(),
                        idx
                    )
                })?;
                if let Ok(meta) = std::fs::metadata(&file.from) {
                    let _ = std::fs::set_permissions(&dest, meta.permissions());
                }
            }
        }
    }
    Ok(())
}

fn sanitize_relative_path(path: &Path) -> Result<PathBuf> {
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

fn replace_path_if_needed(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if metadata.is_dir() {
        bail!("destination `{}` is a directory", path.display());
    }
    std::fs::remove_file(path)
        .with_context(|| format!("remove existing destination `{}`", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn create_symlink(from: &Path, to: &Path) -> Result<()> {
    std::os::unix::fs::symlink(from, to).map_err(Into::into)
}

#[cfg(windows)]
fn create_symlink(from: &Path, to: &Path) -> Result<()> {
    std::os::windows::fs::symlink_file(from, to).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn rule(from: &Path, to: &str, required: bool, mode: ProvisionMode) -> ProvisionedFile {
        ProvisionedFile {
            from: from.to_path_buf(),
            to: PathBuf::from(to),
            required,
            mode,
        }
    }

    #[test]
    fn creates_symlink_destination() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source.env");
        std::fs::write(&src, "DB_PATH=/tmp/test.db\n").unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        materialize_provisioned_files(
            &worktree,
            &[rule(&src, ".env", true, ProvisionMode::Symlink)],
        )
        .unwrap();

        let dest = worktree.join(".env");
        assert!(dest.exists());
        let link_target = std::fs::read_link(&dest).unwrap();
        assert_eq!(link_target, src);
    }

    #[test]
    fn copies_destination_file() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source.env");
        std::fs::write(&src, "DB_PATH=/tmp/test.db\n").unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        materialize_provisioned_files(&worktree, &[rule(&src, ".env", true, ProvisionMode::Copy)])
            .unwrap();

        let dest = worktree.join(".env");
        assert_eq!(
            std::fs::read_to_string(dest).unwrap(),
            "DB_PATH=/tmp/test.db\n"
        );
    }

    #[test]
    fn missing_required_source_errors() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("missing.env");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let err = materialize_provisioned_files(
            &worktree,
            &[rule(&missing, ".env", true, ProvisionMode::Symlink)],
        )
        .unwrap_err();
        assert!(format!("{err}").contains("missing required source"));
    }

    #[test]
    fn missing_optional_source_is_skipped() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("missing.env");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        materialize_provisioned_files(
            &worktree,
            &[rule(&missing, ".env", false, ProvisionMode::Symlink)],
        )
        .unwrap();

        assert!(!worktree.join(".env").exists());
    }

    #[test]
    fn directory_source_errors() {
        let tmp = tempdir().unwrap();
        let source_dir = tmp.path().join("source-dir");
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();

        let err = materialize_provisioned_files(
            &worktree,
            &[rule(&source_dir, ".env", true, ProvisionMode::Symlink)],
        )
        .unwrap_err();
        assert!(format!("{err}").contains("not a regular file"));
    }

    #[test]
    fn replaces_existing_file_destination() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source.env");
        std::fs::write(&src, "DB_PATH=/tmp/test.db\n").unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".env"), "OLD=1\n").unwrap();

        materialize_provisioned_files(
            &worktree,
            &[rule(&src, ".env", true, ProvisionMode::Symlink)],
        )
        .unwrap();
        assert_eq!(std::fs::read_link(worktree.join(".env")).unwrap(), src);
    }

    #[test]
    fn directory_destination_errors() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source.env");
        std::fs::write(&src, "x\n").unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(worktree.join(".env")).unwrap();

        let err = materialize_provisioned_files(
            &worktree,
            &[rule(&src, ".env", true, ProvisionMode::Symlink)],
        )
        .unwrap_err();
        assert!(format!("{err}").contains("is a directory"));
    }

    #[test]
    fn parent_traversal_destination_errors() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("source.env");
        std::fs::write(&src, "x\n").unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let err = materialize_provisioned_files(
            &worktree,
            &[rule(&src, "../.env", true, ProvisionMode::Symlink)],
        )
        .unwrap_err();
        assert!(format!("{err}").contains("invalid destination path"));
    }
}
