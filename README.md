# whence

`whence` is a simple, event-sourced supervisor for executing checklist-style plans with implementer/reviewer loops, checks gating, and resumable runs.

## Build

```bash
cargo build
```

## Test

```bash
cargo test
```

## Run

```bash
cargo run --bin whence -- run <plan.md> --agent codex --checks "cargo check;cargo test"
```

If `--checks` is not provided, `whence` will use `.whence/checks.json` when present, otherwise it opens a checks proposal gate.

## Resume Flow

```bash
cargo run --bin whence -- questions --run <run-id>
cargo run --bin whence -- answer --run <run-id> --question <question-id> --text "..."
cargo run --bin whence -- resume --run <run-id>
```

## Inspect Runs

```bash
cargo run --bin whence -- inspect --run <run-id>
```

This shows phase/state, open questions, latest findings, and per-attempt artifact paths.

## Provider Command Overrides

Use external agent commands without env var setup:

```bash
cargo run --bin whence -- run <plan.md> \
  --agent codex \
  --agent-cmd-codex "bash /path/to/agent-adapter.sh"
```

Available flags:
- `--agent-cmd`
- `--agent-cmd-codex`
- `--agent-cmd-claude`
- `--agent-cmd-opencode`

## State and Artifacts

- Default state DB: `$XDG_STATE_HOME/whence/state.db` (or `$HOME/.local/state/whence/state.db`)
- Run artifacts: `<repo>/.whence/runs/<run-id>/`
