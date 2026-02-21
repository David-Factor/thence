# thence

`thence` is an experiment in long-horizon, spec-driven coding supervision.

It is a derivative of [hence](https://codeberg.org/anuna/hence), building on ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and the broader direction of defeasible logic + LLM workflows pioneered by [Hugo O'Connor](https://www.anuna.io/).

## What You Need

- A repo with a markdown spec (`spec.md`)
- A coding agent command (for real implementation/review work)
- Optional prompt context files: `AGENTS.md`, `CLAUDE.md`

Notes:

- Built-in providers are `codex`, `claude`, and `opencode`.
- Without a configured agent command, provider behavior is stubbed (useful for testing, not real coding output).

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
```

Verify:

```bash
thence --version
thence --help
```

## How To Run

1. Write a spec file.

```markdown
# Feature: OCR harness validation loop

Build a harness that validates OCR extraction quality against expected fixtures.
Add deterministic checks for pass/fail thresholds.
Add docs showing how to run the harness against new OCR changes.
```

2. Run thence.

```bash
thence run spec.md --agent codex --agent-cmd "./scripts/agent-codex.sh" --checks "cargo check;cargo test"
```

3. If paused, answer and resume.

```bash
thence questions --run <run-id>
thence answer --run <run-id> --question <question-id> --text "..."
thence resume --run <run-id>
```

4. Inspect current state.

```bash
thence inspect --run <run-id>
```

## Configuration That Matters

Checks resolution order:

1. `--checks` CLI value
2. `.thence/checks.json`
3. Checks proposal gate (run pauses for approval)

High-impact flags:

- `--checks "cmd1;cmd2"`
- `--reconfigure-checks`
- `--no-checks-file`
- `--workers <n>`
- `--reviewers <n>`
- `--attempt-timeout-secs <secs>`
- `--state-db <path>`
- `--log <path>`
- `--trust-plan-checks`
- `--allow-partial-completion`

Agent command config:

- Per-run: `--agent-cmd`, `--agent-cmd-codex`, `--agent-cmd-claude`, `--agent-cmd-opencode`
- Env vars: `THENCE_AGENT_CMD`, `THENCE_AGENT_CMD_CODEX`, `THENCE_AGENT_CMD_CLAUDE`, `THENCE_AGENT_CMD_OPENCODE`

## How Context Is Shared Today

- The spec is frozen per run at: `<repo>/.thence/runs/<run-id>/spec.md`
- Plan translation gets full spec markdown plus optional `AGENTS.md` / `CLAUDE.md` content
- Implementer/reviewer get task-scoped context through per-attempt capsules (objective, acceptance, findings, checks, references)
- Capsule path is passed via `THENCE_CAPSULE_FILE`

Important current behavior:

- Implementer/reviewer do not currently get the full spec injected directly as a dedicated prompt field.
- The full spec is available on disk under run artifacts, but task execution context is intentionally task-scoped.

## Worktrees and Merge Behavior

Worktrees:

- Per-attempt worktrees are created under:
  - `<repo>/.thence/runs/<run-id>/worktrees/thence/<task-id>/v<attempt>/<worker-id>`
- Worktrees are currently retained for audit/debug; automatic cleanup is not implemented yet.

Merge behavior:

- thence emits logical merge-queue events (`merge_succeeded`, `merge_conflict`) and only closes tasks after merge success.
- Current merge behavior is lightweight/simulated in code, not a full VCS merge queue implementation.

Manual cleanup (if needed):

```bash
rm -rf .thence/runs/<run-id>/worktrees
```

## CLI Discovery

- `thence --help`
- `thence <command> --help`
- `thence completion <shell>`
- `thence man --output docs/thence.1`

This help style is intentionally aligned with [CLIG](https://clig.dev/): concise defaults, command-specific examples, and discoverable support paths.

## Roadmap

- Richer policy/rules for more expressive dependency and parallel execution semantics
- Hooks at key lifecycle points (attempt start/end, checks, merge outcomes)
- Explicit full-spec reference in implementer/reviewer capsules
- Real merge queue integration
- Automatic run/worktree garbage collection policies

## License

LGPL-3.0-or-later. See `LICENSE`.
