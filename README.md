# thence

`thence` is a spec-driven supervisor for long-horizon coding runs.

It is derived from [hence](https://codeberg.org/anuna/hence), and explores ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and [Hugo O'Connor](https://www.anuna.io/)'s work at the seam between defeasible logic systems (rules with defaults and exceptions) and LLM-driven software workflows.

## Why This Exists

Modern models are good enough to be more hands-off if you have a way to keep them grounded as they work.

In practice, the bottleneck isn't "can it code?"; it's "is the spec right?" and "is the verification right?". A lot of engineering work boils down to writing a spec, then running a loop that tells you whether the result actually meets it.

What works for me is an outer loop: one agent implements from the spec, I run checks, a second agent reviews, and its findings become the next prompt. I only step in when the loop needs a human decision.

Good runs also need solid inner loops: project-specific harnesses that turn "is this feature actually working?" into a concrete signal (OCR verification, scrape replays, golden outputs). Those harnesses are often the grounding that makes the outer loop work.

`thence` exists to mechanize it: specs in, implementation/review/check loops out, with explicit pause points when a human answer is needed and a resumable event history.

It is also an exploration, inspired by Hugo's work, of treating "what can run next?" as policy (defaults, exceptions, priorities) instead of burying it in supervisor control flow, while keeping the UX markdown-first.

## Under The Hood

`thence` is an event-sourced supervisor loop. Agents do the text-and-code work (translate spec, implement, review). The supervisor does the bookkeeping, runs the configured checks locally, and decides what to do next.

On start, `thence` translates your `spec.md` into (1) a task list and (2) a small rule file (`plan.spl`) written in Spindle Lisp (SPL), a tiny language for facts and rules, consumed by the `spindle-rust` reasoner. At runtime, `thence` combines that translated plan with built-in policy rules and the current run state.

On each tick, it replays the run's event log into a current projection, derives policy facts, and asks the Spindle reasoner what is provable right now. Those conclusions are things like "this task is claimable", "this task is closable", or "this task is merge-ready". As new events arrive, the derived facts (and therefore the conclusions) can change; this is non-monotonic reasoning (new information can change conclusions).

Defeasible logic is a good fit for this domain because software work is mostly defaults with explicit exceptions: "keep going" is the default, but an open question, missing approvals, failing checks, or new findings should override that default. It gives you a place to write strict rules (always apply) and defeasible rules (defaults that can be defeated by exceptions), plus priorities between them. Today `thence` ships a conservative policy built from strict rules plus projected lifecycle facts; adding defeasible rules and priorities is the next step.

### Core Mental Model

```text
                 +------------------------+
                 | spec.md                |
                 | .thence/config.toml    |
                 +-----------+------------+
                             |
                             v
                 +-----------+------------+
                 | plan translator (codex)|
                 | -> tasks + plan.spl    |
                 +-----------+------------+
                             |
                             v
  +----------------------+     +----------------------+
  | codex workers        |     | checks runner        |
  | implement / review   |     | (shell commands)     |
  +----------+-----------+     +----------+-----------+
             \\                   //
              \\ append events    // append events
               v                 v
  +----------------------+  replay   +----------------------+
  | event log (run DB)   +---------> | projected state       |
  | append-only          |           | (current state)       |
  +----------+-----------+           +----------+-----------+
                                               |
                                               | facts + plan.spl
                                               v
                                     +----------+-----------+
                                     | Spindle policy       |
                                     | query provable       |
                                     | (claimable, etc)     |
                                     +----------+-----------+
                                               |
                                               v
                                     scheduler -> run / pause

If a spec question opens: pause -> answer -> resume -> continue.
```

## Setup

Prerequisites:

- A repo with a markdown spec file
- Rust toolchain (for building from source) or the install script
- Codex CLI available in `PATH` for non-simulated runs

Install:

```bash
curl -fsSL https://raw.githubusercontent.com/David-Factor/thence/main/install.sh | bash
```

Verify:

```bash
thence --version
thence --help
```

## How To Run

Real run:

```bash
thence run spec.md --checks "cargo check;cargo test"
```

Config-first run (checks from `.thence/config.toml`):

```bash
thence run spec.md
```

Explicit simulation mode:

```bash
thence run spec.md --simulate --checks "true"
```

When paused:

```bash
thence questions --run <run-id>
thence answer --run <run-id> --question <question-id> --text "..."
thence resume --run <run-id>
```

## Minimal Configuration

Create `.thence/config.toml`:

```toml
version = 2

[agent]
provider = "codex"
# optional; defaults to `codex` from PATH
command = "codex"

[checks]
commands = ["cargo check", "cargo test"]

[prompts]
# optional reviewer instruction override
reviewer = """
Review implementation against objective/acceptance.
Return strict JSON with: approved (bool), findings (string[]).
"""
```

Checks resolution order:

1. `--checks`
2. `[checks].commands` in `.thence/config.toml`

If neither is set, run start fails with:

```text
No checks configured. Set `--checks` or `[checks].commands` in `.thence/config.toml`.
```

## Context Model

Per run:

- Frozen spec path: `<repo>/.thence/runs/<run-id>/spec.md`
- Each agent request includes `spec_ref` (a stable pointer to the frozen spec) with:
  - `path` to the frozen spec
  - `sha256` of the frozen spec

This keeps task prompts compact while preserving a stable full-spec reference.

## Worktrees

Per-attempt worktrees are created at:

- `<repo>/.thence/runs/<run-id>/worktrees/thence/<task-id>/v<attempt>/<worker-id>`

Worktrees are retained for debugging and audit in this release.

### Worktree Provisioning

You can materialize required untracked files (for example, `.env`) into each task attempt worktree:

```toml
version = 2

[checks]
commands = ["cargo test"]

[[worktree.provision.files]]
from = "/absolute/path/to/source.env"
to = ".env"
required = true
mode = "symlink" # default: symlink; also supports "copy"
```

Rules:

- `from` must be an absolute path.
- `to` must be a relative path inside the worktree (no `..` traversal).
- `required` defaults to `true` (missing source fails the attempt).
- `mode` defaults to `symlink`; use `copy` for per-worktree file snapshots.

Manual cleanup:

```bash
rm -rf .thence/runs/<run-id>/worktrees
```

## Roadmap

- Richer rule/policy modeling
- Better parallelism controls
- Lifecycle hooks
- Merge queue integration

## License

LGPL-3.0-or-later. See `LICENSE`.
