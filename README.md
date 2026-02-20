# whence

`whence` is an event-sourced supervisor for executing checklist-style plans with implementer/reviewer loops, checks gating, and resumable runs.

## How To Use

### 1. Write a plan

`whence` reads Markdown checklist items in this format:

```markdown
- [ ] task-id: objective | deps=task-a,task-b | checks=cargo check,cargo test
```

Notes:
- `task-id:` is optional; if omitted, `task1`, `task2`, ... are generated.
- `deps=...` is optional.
- `checks=...` is optional and only applied when `--trust-plan-checks` is set.

Example:

```markdown
- [ ] setup-db: create migration and schema
- [ ] api-read: add read endpoint | deps=setup-db
- [ ] api-write: add write endpoint | deps=setup-db
- [ ] tests: cover read/write flows | deps=api-read,api-write
```

### 2. Start a run

```bash
cargo run --bin whence -- run plan.md --agent codex --checks "cargo check;cargo test"
```

Checks resolution order:
1. `--checks` (if provided)
2. `.whence/checks.json` (if present and valid)
3. checks proposal gate (run pauses and asks for approval)

### 3. Answer questions and resume

When a run pauses for clarification or checks approval:

```bash
cargo run --bin whence -- questions --run <run-id>
cargo run --bin whence -- answer --run <run-id> --question <question-id> --text "..."
cargo run --bin whence -- resume --run <run-id>
```

### 4. Inspect state and artifacts

```bash
cargo run --bin whence -- inspect --run <run-id>
```

This reports phase/state, open questions, latest findings, and per-attempt artifact paths.

## Provider Command Overrides

Use external agent commands without environment variable setup:

```bash
cargo run --bin whence -- run plan.md \
  --agent codex \
  --agent-cmd-codex "bash /path/to/agent-adapter.sh"
```

Available flags:
- `--agent-cmd`
- `--agent-cmd-codex`
- `--agent-cmd-claude`
- `--agent-cmd-opencode`

## Reasoning Layer (Defeasible Logic)

`whence` composes:
- static supervisor policy rules,
- translated plan facts/rules, and
- projected lifecycle facts from the event log.

That combined SPL theory is evaluated through embedded `spindle-rust` components (`spindle-core` and `spindle-parser`) to derive `claimable`, `closable`, and `merge-ready` states. This architecture keeps orchestration logic explicit and extensible, while leveraging non-monotonic/defeasible reasoning machinery under the hood as policy complexity grows.

## Acknowledgements

`whence` takes strong direction from [hence](https://codeberg.org/anuna/hence), especially around LLM-supervisor workflow shape and SPL-centered policy modeling.

`whence` also directly embeds [spindle-rust](https://codeberg.org/anuna/spindle-rust) as its reasoning backend.

## Build

```bash
cargo build
```

## Test

```bash
cargo test
```

## State and Artifacts

- Default state DB: `$XDG_STATE_HOME/whence/state.db` (or `$HOME/.local/state/whence/state.db`)
- Run artifacts: `<repo>/.whence/runs/<run-id>/`

## License

LGPL-3.0-or-later. See `LICENSE`.
