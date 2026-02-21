# thence

`thence` is a spec-driven supervisor for long-horizon coding runs.

It is derived from [hence](https://codeberg.org/anuna/hence), and builds on ideas from [spindle-rust](https://codeberg.org/anuna/spindle-rust) and Hugo O'Connor's work on defeasible logic workflows.

## Why This Exists

Specs, implementation, review, and deterministic checks usually fail for process reasons, not model capability alone. `thence` makes that loop explicit and resumable.

## Setup

Prerequisites:

- A repo with a markdown spec file
- Rust toolchain (for building from source) or the install script
- Codex CLI available in `PATH` for real runs

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

## Minimal Config

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

If neither is set, run start fails.

## Context Model

Per run:

- Frozen spec path: `<repo>/.thence/runs/<run-id>/spec.md`
- Capsules carry `spec_ref` with:
  - `path` to the frozen spec
  - `sha256` of the frozen spec

This keeps task-level prompts compact while preserving a stable full-spec reference.

## Worktrees

Per attempt worktrees are created at:

- `<repo>/.thence/runs/<run-id>/worktrees/thence/<task-id>/v<attempt>/<worker-id>`

Worktrees are retained for debugging/audit and are not auto-cleaned in this release.

Manual cleanup:

```bash
rm -rf .thence/runs/<run-id>/worktrees
```

## Roadmap

- Richer rules/policy modeling
- Better parallelism controls
- Lifecycle hooks
- Merge queue integration

## License

LGPL-3.0-or-later. See `LICENSE`.
