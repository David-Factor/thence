# thence 0.3.0 (Breaking)

## Breaking Changes

- `.thence/config.toml` now requires `version = 2` (older versions are rejected).
- Removed run flags:
  - `--reconfigure-checks`
  - `--no-checks-file`
  - `--agent-cmd`
  - `--agent-cmd-codex`
  - `--agent-cmd-claude`
  - `--agent-cmd-opencode`
- Removed `.thence/checks.json` runtime behavior. Checks now resolve only from CLI/config.
- Simulation is now explicit via `--simulate`; non-simulated runs require a runnable codex command.
- Provider scope is codex-only in this version (`--agent` rejects non-`codex`).
- Added `.thence/config.toml` (versioned, config-first runtime settings).

## New Runtime Contract

- Worktree provisioning for untracked files is supported via:
  - `[[worktree.provision.files]]` in `.thence/config.toml`
  - fields: `from` (absolute path), `to` (relative path), `required` (default `true`), `mode` (`symlink` default, or `copy`)
- Checks resolution order:
  1. `--checks`
  2. `.thence/config.toml` `[checks].commands`
- Missing checks fail fast with:
  - `No checks configured. Set --checks or [checks].commands in .thence/config.toml.`
- Reviewer prompt override is supported via:
  - `.thence/config.toml` `[prompts].reviewer`
