# thence 0.2.0 (Breaking)

## Breaking Changes

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

- Checks resolution order:
  1. `--checks`
  2. `.thence/config.toml` `[checks].commands`
- Missing checks fail fast with:
  - `No checks configured. Set --checks or [checks].commands in .thence/config.toml.`
- Reviewer prompt override is supported via:
  - `.thence/config.toml` `[prompts].reviewer`
