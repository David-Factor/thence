# whence

`whence` is an event-sourced supervisor for executing free-form Markdown specs with implementer/reviewer loops, checks gating, and resumable runs.

## How To Use

### 1. Write a spec

`whence` accepts free-form Markdown and asks the plan-translator agent to produce SPL + a task graph.

Example:

```markdown
# Feature: Tiny Changelog Helper

Build a utility that parses markdown changelog text and extracts a version section.
Add tests for missing versions and malformed headers.
Add a README usage example.
```

### 2. Start a run

```bash
cargo run --bin whence -- run plan.md --agent codex --checks "cargo check;cargo test"
```

Checks resolution order:
1. `--checks` (if provided)
2. `.whence/checks.json` (if present and valid)
3. checks proposal gate (run pauses and asks for approval)

Useful timeout flags:
- `--attempt-timeout-secs <n>`: hard timeout for implementer/reviewer attempts (default `2700`).

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

### 5. Crash Lease Recovery

Each in-flight attempt writes a lease file under:

`<repo>/.whence/runs/<run-id>/leases/<task-id>/attempt<k>/{implementer,reviewer}.json`

On `resume`, `whence`:
- refuses to continue if a lease is still fresh (protects against double supervisors),
- marks stale orphan attempts interrupted and retries safely,
- fail-closes attempts that exceed retry budget.

## Provider Command Overrides

Use external agent commands without environment variable setup:

```bash
cargo run --bin whence -- run plan.md \
  --agent codex \
  --agent-cmd-codex "bash scripts/agent-codex.sh"
```

Available flags:
- `--agent-cmd`
- `--agent-cmd-codex`
- `--agent-cmd-claude`
- `--agent-cmd-opencode`

### Bundled Codex Adapter

This repo ships a maintained adapter at `scripts/agent-codex.sh`.

It expects `codex` on `PATH`, reads `WHENCE_PROMPT_FILE` and optional
`WHENCE_CAPSULE_FILE`, and writes structured JSON to `WHENCE_RESULT_FILE`.
Each role uses a strict output schema so malformed outputs fail closed in the
supervisor loop.

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
