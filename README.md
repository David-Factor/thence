# thence

`thence` is an experiment in long-horizon LLM-assisted execution.

It is explicitly a derivative of [hence](https://codeberg.org/anuna/hence), building on ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and defeasible logic orchestration.

## Why This Exists

This project comes from a workflow shift:

- Models are increasingly good at implementation when each step is grounded and verified.
- The bottleneck often shifts from raw coding to spec quality and verification quality.
- Verification is usually hybrid: LLM review plus deterministic checks.
- `thence` is an outer-loop experiment for that pattern.

## Core Idea

You provide a free-form Markdown spec.

`thence` then:

1. translates the spec into an internal plan,
2. runs implementer and reviewer attempts,
3. gates closure with deterministic checks,
4. records facts and events so runs are resumable and auditable.

Under the hood, assertions/facts are appended as attempts progress. Policy reasoning uses those facts to decide what should happen next (claim, retry, pause, close, or fail).

Mental model:

```text
+-----------+
|  spec.md  |
+-----------+
      |
      v
+------------------------+
|       thence run       |
|    (supervisor loop)   |
+------------------------+
      |
      v
+-------------+    +----------+    +--------+
| implementer | -> | reviewer | -> | checks |
+-------------+    +----------+    +--------+
      ^                                   |
      |___________________________________|
             findings + retries

      |
      v
+------------------------+
| event log + artifacts  |
+------------------------+
      |
      v
+------------------------+
| resume / inspect       |
+------------------------+
```

## Relationship to hence

`thence` is not trying to replace `hence`.

It is a focused experiment exploring ideas on top of [Hugo O'Connor](https://www.anuna.io/)'s work in `hence` and `spindle-rust`, especially the seam of defeasible logic systems paired with LLM workflows.

## Install

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
```

This installs the latest release for your OS/arch to `~/.local/bin/thence`.

If needed:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Verify install:

```bash
thence --help
```

## Quickstart

### 1. Write a spec

Example `spec.md`:

```markdown
# Feature: OCR harness validation loop

Build a harness that can validate OCR extraction quality against expected fixtures.
Add deterministic checks for pass/fail thresholds.
Add docs showing how to run the harness against new OCR changes.
```

### 2. Start a run

```bash
thence run spec.md --agent codex --checks "cargo check;cargo test"
```

### 3. If paused, answer and resume

```bash
thence questions --run <run-id>
thence answer --run <run-id> --question <question-id> --text "..."
thence resume --run <run-id>
```

### 4. Inspect status and artifacts

```bash
thence inspect --run <run-id>
```

## Configuration

### Checks

Checks resolution order:

1. `--checks` CLI value
2. `.thence/checks.json`
3. checks proposal gate (run pauses for approval)

Common checks options:

- `thence run spec.md --checks "cargo check;cargo test"`
- `thence run spec.md --reconfigure-checks`
- `thence run spec.md --no-checks-file`

Other useful run flags:

- `--workers <n>` implementer worker count
- `--reviewers <n>` reviewer worker count
- `--attempt-timeout-secs <n>` timeout for implementer/reviewer attempts
- `--state-db <path>` custom run state DB path
- `--run-id <id>` set an explicit run id
- `--log <path>` write NDJSON event log
- `--trust-plan-checks` use per-task checks from translated plan
- `--allow-partial-completion` allow successful tasks to close even if some fail

Minimal `.thence/checks.json` example:

```json
{
  "version": 1,
  "commands": ["cargo check", "cargo test"],
  "updated_at": "2026-02-21T00:00:00Z",
  "source": "human_approved"
}
```

`AGENTS.md` and `CLAUDE.md` are used as prompt context for plan translation and checks proposal. They are not a direct checks configuration source.

### Agent Providers

Supported `--agent` values:

- `codex` (default)
- `claude`
- `opencode`

Configure agent command execution with flags:

- `--agent-cmd` default command for all providers
- `--agent-cmd-codex` override for `codex`
- `--agent-cmd-claude` override for `claude`
- `--agent-cmd-opencode` override for `opencode`

Or environment variables:

- `THENCE_AGENT_CMD`
- `THENCE_AGENT_CMD_CODEX`
- `THENCE_AGENT_CMD_CLAUDE`
- `THENCE_AGENT_CMD_OPENCODE`

Custom adapter example:

```bash
thence run spec.md --agent codex --agent-cmd "./scripts/agent-codex.sh"
```

### Subprocess Adapter Contract

When using `--agent-cmd*`, `thence` runs your command in the task worktree and sets:

- `THENCE_ROLE`
- `THENCE_WORKTREE`
- `THENCE_PROMPT_FILE`
- `THENCE_RESULT_FILE`
- `THENCE_TIMEOUT_SECS`

For implementer/reviewer attempts it also sets:

- `THENCE_CAPSULE_FILE`
- `THENCE_CAPSULE_SHA256`
- `THENCE_CAPSULE_ROLE`

Your adapter must write structured JSON to `THENCE_RESULT_FILE`:

- `plan-translator`: `{ "spl": string, "tasks": [...] }`
- `implementer`: `{ "submitted": true }`
- `reviewer`: `{ "approved": boolean, "findings": string[] }`
- `checks-proposer`: `{ "commands": string[], "rationale": string }`

Bundled example adapter:

- `scripts/agent-codex.sh`

## Runtime Behavior

### Pause/Resume Flow

Runs pause for unresolved questions (for example, checks approval or spec clarification). Use:

- `thence questions --run <run-id>`
- `thence answer --run <run-id> --question <question-id> --text "..."`
- `thence resume --run <run-id>`

### Failure and Retry Semantics

A task attempt can be retried when:

- implementer exits non-zero,
- implementer output is missing/invalid,
- reviewer rejects with findings,
- reviewer output is invalid,
- deterministic checks fail or time out,
- merge conflict reopens the task.

A task is marked terminal-failed when max attempts are exhausted. A run fails when policy reaches unschedulable/blocked terminal state (unless partial completion is allowed with `--allow-partial-completion`).

### Crash Safety and Leases

In-flight attempts maintain lease files under:

- `<repo>/.thence/runs/<run-id>/leases/<task-id>/attempt<k>/{implementer,reviewer}.json`

On resume:

- fresh active lease blocks resume to prevent double supervisors,
- stale lease is marked interrupted and safely retried.

### Artifact Layout

Key runtime artifacts live under `<repo>/.thence/runs/<run-id>/`:

```text
spec.md
plan.spl
translated_plan.json
capsules/<task-id>/attempt<k>/{implementer,reviewer}.json
leases/<task-id>/attempt<k>/{implementer,reviewer}.json
worktrees/
```

## Development

Build:

```bash
cargo build
```

Test:

```bash
cargo test
```

State:

- default DB: `$XDG_STATE_HOME/thence/state.db` (or `$HOME/.local/state/thence/state.db`)
- run artifacts: `<repo>/.thence/runs/<run-id>/`

## License

LGPL-3.0-or-later. See `LICENSE`.
