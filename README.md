# thence

`thence` is an experiment in long-horizon, hands-off LLM execution.

It is explicitly a derivative of [hence](https://codeberg.org/anuna/hence), and directly builds on ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and defeasible logic orchestration.

## Why This Exists

This project comes from a workflow shift:

- Models are now strong enough to be more hands-off, if each step is grounded and verified.
- In practice, the hard part is no longer raw coding alone, it is spec quality plus verification quality.
- The verification loop can vary by project, but usually combines LLM review plus deterministic checks.
- `thence` is an outer-loop experiment for that pattern: take a rich spec, run execution/review loops, and keep progress grounded.

## Core Idea

You provide a free-form Markdown spec.

`thence` then:

1. translates the spec into an internal plan,
2. runs implementer/reviewer attempts,
3. gates closure with checks,
4. records everything in an event log so runs are resumable and auditable.

Important UX choice:

- SPL/defeasible logic is used under the hood, but is intentionally not required from the user.
- You interact through specs and simple CLI commands, not through logic syntax.

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

`SPL + defeasible reasoning runs internally; users interact via specs + CLI only.`

## Relationship to hence

`thence` is not trying to replace `hence`.

It is a focused experiment that borrows the reasoning foundation and applies it to a different user experience:

- no plan language exposure for end users,
- free-form spec in, supervised execution out,
- explicit grounding loops for real coding workflow.

## Install

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
```

Defaults:

- installs to `~/.local/bin/thence`
- installs latest release for your OS/arch

Useful overrides:

```bash
VERSION=v0.1.2 curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
```

For private forks/repos, use authenticated GitHub CLI:

```bash
bash <(gh api "repos/<owner>/<repo>/contents/install.sh?ref=main" --jq '.content' | base64 --decode)
```

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

### 3. Respond to pauses (if any)

```bash
thence questions --run <run-id>
thence answer --run <run-id> --question <question-id> --text "..."
thence resume --run <run-id>
```

### 4. Inspect status and artifacts

```bash
thence inspect --run <run-id>
```

## Verification and Grounding Model

`thence` is built around two layers:

- Inner loop: implementer + reviewer + deterministic checks.
- Outer loop: event-sourced supervision that tracks progress, findings, retries, and terminal outcomes.

This is the central experiment: can a strong outer loop make longer-horizon autonomous execution safer and more useful in day-to-day workflow?

## Runtime Behavior You Should Know

Checks resolution order:

1. `--checks` CLI value
2. `.thence/checks.json`
3. checks proposal gate (run pauses for approval)

Crash safety:

- In-flight attempts write lease files under:
  - `<repo>/.thence/runs/<run-id>/leases/<task-id>/attempt<k>/{implementer,reviewer}.json`
- On resume:
  - fresh active lease blocks resume (prevents double supervisors),
  - stale lease gets interrupted and retried safely.

Useful flags:

- `--attempt-timeout-secs <n>` hard timeout for implementer/reviewer attempts
- `--agent-cmd`, `--agent-cmd-codex`, `--agent-cmd-claude`, `--agent-cmd-opencode` for external adapters

Bundled adapter:

- `scripts/agent-codex.sh`

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
- artifacts: `<repo>/.thence/runs/<run-id>/`

## License

LGPL-3.0-or-later. See `LICENSE`.
