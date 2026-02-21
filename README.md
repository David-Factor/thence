# thence

`thence` is a spec-driven supervisor for long-horizon coding runs.

It is derived from [hence](https://codeberg.org/anuna/hence), and explores ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and [Hugo O'Connor](https://www.anuna.io/)'s work at the seam between defeasible logic systems and LLM-driven software workflows.

## Why This Exists

This project comes from a workflow shift:

- models are good enough to be more hands-off, if each step is grounded and verified,
- the bottleneck is less "can it code?" and more "is the spec right?" plus "is verification right?",
- verification varies by project, but often ends up hybrid: LLM review plus deterministic checks.

That pattern shows up repeatedly in real work. For example: writing a spec for an OCR verification harness, building it, then using that harness to validate the OCR step itself.

`thence` is an outer-loop experiment for that way of working: specs in, implementation/review/check loops out, with resumable event history and explicit gates.

It is also an exploration of the seam from Hugo's work: using defeasible-logic-style orchestration ideas for runtime policy and state transitions, while keeping the user experience simple and markdown-first.

## Under The Hood

`thence` is an event-sourced supervisor loop. Agents do the text-and-code work (translate spec, implement, review). The supervisor does the bookkeeping and scheduling.

On start, `thence` translates your `spec.md` into (1) a task list and (2) a small rule file (`plan.spl`) written in Spindle Lisp (SPL), a tiny language for facts and rules. At runtime, `thence` combines that translated plan with built-in policy rules and the current run state.

On each tick, it replays the run's event log into a current projection, derives policy facts, and asks the Spindle reasoner what is provable right now. Those conclusions are things like "this task is claimable", "this task is closable", or "this task is merge-ready". As new events arrive, the derived facts (and therefore the conclusions) can change. That's the non-monotonic part: new information can change what is runnable next.

Defeasible logic is a good fit for this domain because software work is mostly defaults with explicit exceptions: "keep going" is the default, but an open question, missing approvals, failing checks, or new findings should override that default. Today `thence` ships a conservative policy built from strict rules (they apply whenever their conditions are true) plus projected lifecycle facts; adding defeasible rules and priorities is the next step.

### Core Mental Model

```text
                 +------------------------+
                 | spec.md + config.toml  |
                 +-----------+------------+
                             |
                             v
                 +-----------+------------+
                 | plan translator (codex)|
                 | -> tasks + plan.spl    |
                 +-----------+------------+
                             |
                             v
  +----------------------+  replay   +----------------------+
  | event log (run DB)   +---------> | RunProjection         |
  | append-only          |           | (current state)       |
  +----------+-----------+           +----------+-----------+
             ^                                  |
             | append events                     | facts + plan.spl
             |                                  v
  +----------+-----------+           +----------+-----------+
  | workers (codex)      |           | Spindle policy       |
  | implement / review   |           | query provable       |
  | checks               |           | (claimable, etc)     |
  +----------------------+           +----------+-----------+
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

Config-first run:

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
version = 1

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

`No checks configured. Set --checks or [checks].commands in .thence/config.toml.`

## Context Model

Per run:

- Frozen spec path: `<repo>/.thence/runs/<run-id>/spec.md`
- Capsules carry `spec_ref` with:
  - `path` to the frozen spec
  - `sha256` of the frozen spec

This keeps task prompts compact while preserving a stable full-spec reference.

## Worktrees

Per-attempt worktrees are created at:

- `<repo>/.thence/runs/<run-id>/worktrees/thence/<task-id>/v<attempt>/<worker-id>`

Worktrees are retained for debugging and audit in this release.

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
