# REQ-001: Thence Simple Supervisor Runner V1

## Overview

This document specifies a radically simple V1 multi-agent implementation runner focused on correctness and trust boundaries.

The design intentionally chooses:

- single machine execution
- single supervisor process
- append-only event sourcing in SQLite
- prompt-first markdown -> SPL translation
- internal SPL policy reasoning via spindle-rust
- fixed worker pools
- pre-canned SPL rule bundles for review/closure gates

The user-facing goal is one command:

```bash
thence run <plan-file> --agent <provider> [--log <file>] [--workers N] [--reviewers N]
```

The system must enforce that implementation agents cannot self-certify task completion. A reviewer path and objective checks are required before closure.

---

## Goals

1. Provide a very small implementation surface area while keeping strong workflow guarantees.
2. Accept a chunked multi-part feature spec and execute it as a task graph.
3. Enforce ambiguity resolution before implementation starts.
4. Enforce review and checks before task closure.
5. Maintain a complete audit trail with deterministic replay.
6. Package as a single Rust executable with bundled SQLite.

## Non-Goals (V1)

1. Multi-machine/distributed leaderless runtime.
2. Cryptographic signatures for claims/events.
3. Dynamic autoscaling.
4. UI/web dashboard.
5. Multiple runtime modes (supervisor and leaderless simultaneously).
6. General plugin framework.

---

## Product UX

### Primary Command

```bash
thence run <plan-file> --agent codex --log run.ndjson
```

### Minimal Flags

- `--agent <provider>`: default implementer and reviewer provider (e.g. `codex`)
- `--workers <n>`: implementer worker count (default `2`)
- `--reviewers <n>`: reviewer worker count (default `1`)
- `--checks "cmd1;cmd2;..."`: optional global checks if not defined in plan
- `--log <path>`: optional NDJSON mirror stream
- `--resume`: resume existing run state from DB (uses `--run-id` if provided)
- `--run-id <id>`: explicit run identifier for resume/inspection flows
- `--state-db <path>`: optional state DB location (default `$XDG_STATE_HOME/thence/state.db`)
- `--allow-partial-completion`: do not fail whole run when a task terminal-fails
- `--trust-plan-checks`: allow check commands declared in generated plan SPL
- `--interactive`: allow inline question/answer prompts when human input is required

### Human Input Commands

```bash
thence questions --run <run-id>
thence answer --run <run-id> --question <question-id> --text "..."
thence resume --run <run-id>
```

### Pause Behavior

When human input is required, `thence run` appends `run_paused`, prints exact follow-up commands, and exits non-zero (distinct from success).

### User Experience Principles

1. SPL remains internal by default and is never required from the user.
2. Errors are actionable and specific.
3. Every major transition is visible in logs.
4. Deterministic behavior is preferred over "clever" heuristics.

---

## Architecture

### High-Level Components

1. `cli`: argument parsing and run bootstrap.
2. `translator`: prompt-based markdown -> SPL plan generator.
3. `validator`: `spindle validate` + sanity query gates.
4. `review_loop`: LLM spec-review loop driven by pre-canned SPL requirements.
5. `events`: append-only SQLite event store.
6. `projector`: deterministic current-state projection from events.
7. `policy`: spindle-rust invocation over projected facts + static rule bundle + translated plan.
8. `scheduler`: chooses next runnable tasks per role.
9. `workers`: implementer and reviewer subprocess orchestration.
10. `checks`: objective command checks.
11. `merge`: serialized integration queue.
12. `logging`: stderr progress + optional NDJSON mirror.

### Process Model

Single OS process (supervisor) orchestrates child agent subprocesses.

Concurrency exists only in child workers and supervisor-managed queues, not in multiple supervisors.

Child agents never get direct DB access; they communicate with supervisor via subprocess IO only.

---

## Core Workflow

### Phase 0: Boot

1. Open/create SQLite DB.
2. Run schema migrations.
3. Load markdown spec file.
4. If `--resume`, resolve target run deterministically:
   - if `--run-id` provided, resume that run only
   - else if exactly one resumable run exists, resume it
   - else fail with actionable error listing candidate run IDs
5. On resume, replay events and reuse stored `plan.spl`; otherwise create new run.

### Phase 1: Plan Translation + Validation

1. Translator prompt converts markdown spec directly to `plan.spl`.
2. Append `plan_translated`.
3. Run `spindle validate plan.spl`.
4. Run fixed sanity queries over `plan.spl` + pre-canned rule bundle.
5. Append `plan_validated`.
6. If translation/validation/sanity fails, append `spec_question_opened` + `human_input_requested` and pause.

### Phase 2: Spec Review Gate

1. Run reviewer role over markdown spec + generated `plan.spl` + pre-canned review checklist.
2. Reviewer emits:
   - `spec_question_opened` (needs clarification)
   - or `spec_approved`
3. Policy gate: no implementation tasks may become pullable until:
   - run has `spec_approved`
   - no unresolved spec questions remain
4. If human answer is required:
   - append `run_paused` + `human_input_requested`
   - user submits answer -> append `human_input_provided` + `spec_question_resolved` + `run_resumed`

### Phase 3: Implementation/Review Loop

For each task cycle:

1. Scheduler selects pullable task for implementer.
2. Supervisor creates isolated worktree for that task attempt.
3. Implementer subprocess runs with context packet.
4. On submission, append `work_submitted` + `review_requested`.
5. Reviewer subprocess runs with review context packet.
6. Reviewer emits:
   - `review_found_issues` (reopen)
   - or `review_approved` (continue)
7. If findings require human decision, append `run_paused` + `human_input_requested`.
8. Checks runner executes required checks after `review_approved` and appends `checks_reported`.
9. Task becomes merge-ready only if policy derives closable state and run is not paused.

### Phase 4: Merge/Close

1. Merger queue runs one item at a time.
2. Attempt merge into integration branch.
3. Append:
   - `merge_succeeded` then `task_closed`
   - or `merge_conflict` then task reopened

### Phase 5: Completion

When run reaches a terminal state:

1. append `run_completed` only if all required tasks are closed, or partial mode allows terminal-failed tasks.
2. append `run_failed` if any required task is terminal-failed and partial mode is not enabled.
3. append `run_cancelled` only on explicit user cancellation.
4. print summary.

---

## Trust Boundaries

1. Agents (implementer/reviewer) never write to SQLite directly.
2. Agents return outputs via subprocess IO only; supervisor translates outputs into events.
3. Implementer role cannot cause approval/closure events.
4. Reviewer role cannot cause merge events.
5. Only supervisor emits merge, close, and run-terminal events.
6. Policy rejects invalid transition attempts even if buggy code tries to append them.

---

## Data Model (SQLite)

Use SQLite as canonical state source.

### PRAGMAs

- `journal_mode=WAL`
- `synchronous=NORMAL` (or `FULL` for stricter durability mode)
- `foreign_keys=ON`

### Tables

#### `runs`

- `id TEXT PRIMARY KEY`
- `plan_path TEXT NOT NULL`
- `plan_sha256 TEXT NOT NULL`
- `spl_plan_path TEXT NOT NULL`
- `created_at TEXT NOT NULL`
- `status TEXT NOT NULL CHECK(status IN ('running','completed','failed','cancelled'))`
- `config_json TEXT NOT NULL`

#### `events`

- `seq INTEGER PRIMARY KEY AUTOINCREMENT`
- `run_id TEXT NOT NULL REFERENCES runs(id)`
- `ts TEXT NOT NULL`
- `event_type TEXT NOT NULL`
- `task_id TEXT`
- `actor_role TEXT`
- `actor_id TEXT`
- `attempt INTEGER`
- `payload_json TEXT NOT NULL`
- `dedupe_key TEXT`
- `FOREIGN KEY(run_id) REFERENCES runs(id)`

`payload_json` should contain compact structured metadata only. Large artifacts (full prompts, transcripts, diffs, logs) are stored as files under run artifacts directory, with paths/hashes recorded in payload.

Indexes:

- `CREATE INDEX idx_events_run_seq ON events(run_id, seq);`
- `CREATE INDEX idx_events_run_task_seq ON events(run_id, task_id, seq);`
- `CREATE UNIQUE INDEX idx_events_run_dedupe ON events(run_id, dedupe_key) WHERE dedupe_key IS NOT NULL;`

#### `snapshots` (optional optimization)

- `run_id TEXT NOT NULL`
- `seq INTEGER NOT NULL`
- `state_json TEXT NOT NULL`
- `PRIMARY KEY(run_id, seq)`

### Event Types (V1)

1. `run_started`
2. `plan_translated`
3. `plan_validated`
4. `task_registered`
5. `spec_question_opened`
6. `spec_question_resolved`
7. `spec_approved`
8. `task_claimed`
9. `work_submitted`
10. `review_requested`
11. `review_found_issues`
12. `review_approved`
13. `checks_reported`
14. `human_input_requested`
15. `human_input_provided`
16. `run_paused`
17. `run_resumed`
18. `merge_succeeded`
19. `merge_conflict`
20. `task_closed`
21. `task_failed_terminal`
22. `attempt_interrupted`
23. `run_completed`
24. `run_failed`
25. `run_cancelled`

All state is derived from replay; no mutable status table is required for correctness.
Exactly one run-terminal event (`run_completed`, `run_failed`, or `run_cancelled`) is allowed per run.

---

## Internal Policy Model (SPL)

### Rule Sources

1. static internal policy rules (checked into binary as string assets)
2. task/dependency facts from prompt-generated `plan.spl`
3. projected lifecycle facts from events

### Key Derived Literals

- `ready-{task}`
- `claimable-{task}`
- `reviewable-{task}`
- `rework-required-{task}`
- `checks-passed-{task}`
- `closable-{task}`
- `merge-ready-{task}`
- `blocked-ambiguity-{task}`
- `needs-human-{task}`
- `run-paused`

### Critical Policy Invariants

1. `closable-{task}` requires:
   - latest attempt has `review-approved-{task}-{attempt}`
   - checks passed for that same attempt
   - no unresolved findings for that attempt
2. `claimable-{task}` false while ambiguity unresolved or run paused.
3. reviewer identity must differ from implementer identity per attempt.
4. `task-closed` only after merge success.
5. any open `human_input_requested` blocks new claims and merges.

---

## Plan Input and Translation

### Expected Plan Input (V1)

Human-readable markdown feature spec. V1 does not require a strict markdown schema.

### Translation Pipeline

1. Prompt template converts markdown directly to `plan.spl`.
2. Supervisor writes generated SPL to run storage, records its hash, and treats it as canonical run input.
3. Run `spindle validate` as a hard gate.
4. Run fixed sanity checks as hard gates:
   - at least one task is declared
   - at least one task is initially ready
   - no task is closable before review/check gates
5. If any gate fails, append question events and pause for human input.

### Generated SPL Contract

For each task `<id>`, translator must emit:

1. task existence fact (for example `task-<id>`)
2. required spec completeness facts (for example `has-objective-<id>`, `has-acceptance-<id>`)
3. readiness rule deriving `ready-<id>` from dependency completion facts
4. optional metadata for prompt context (description, files, ownership hints)

### Debug-only Support

```bash
thence run plan.md --debug-dump-spl /tmp/generated-plan.spl
```

---

## Worker Context Packets

Each attempt gets deterministic context packet generated by supervisor.

### Implementer Packet

1. task objective
2. acceptance criteria
3. spec excerpt + generated SPL excerpt for task references
4. dependency outcomes/summaries
5. unresolved findings from prior reviews
6. required checks
7. relevant files

### Reviewer Packet

1. same acceptance criteria + spec excerpt
2. submitted diff/commit refs
3. previously raised findings and status (addressed/unaddressed)
4. policy-derived gate state for the attempt

### Context Rules

1. no hidden mutable memory required in V1.
2. packet generation must be deterministic from run state.
3. packet content is logged in summary form (not full prompt by default).
4. checks run after reviewer approval in V1.

---

## Provider Abstraction

Define one provider interface:

```rust
trait AgentProvider {
    fn run(&self, req: AgentRequest) -> Result<AgentResult>;
}
```

### `AgentRequest`

1. `role` (`implementer` | `reviewer`)
2. `task_id`
3. `attempt`
4. `worktree_path`
5. `prompt`
6. `timeout`

### `AgentResult`

1. `exit_code`
2. `stdout_path`
3. `stderr_path`
4. `structured_output` (optional parsed JSON contract)

V1 providers:

1. `codex` required
2. `claude` optional
3. `opencode` optional

---

## Git/Worktree Strategy

### Branch Naming

- integration: `thence/<plan-id>`
- task attempt: `thence/<task-id>/v<attempt>/<worker-id>`

### Isolation

1. one worktree per active task attempt
2. worktree created from current integration branch HEAD
3. cleanup after close or configured retention

### Merge Queue

1. strictly single-threaded merge executor
2. if conflict:
   - append `merge_conflict`
   - reopen task for rework
3. only successful merge allows `task_closed`

---

## Retry and Timeout Policy

V1 defaults:

1. max attempts per task: `3`
2. implementer timeout: `45m`
3. reviewer timeout: `20m`
4. checks timeout per command: `10m`

If max attempts exceeded:

- append `task_failed_terminal`
- mark run failed unless user chose `--allow-partial-completion`

---

## Logging and Observability

### Mandatory

1. structured event rows in SQLite
2. concise human progress on stderr

### Optional NDJSON Mirror

If `--log` is provided, every committed event is mirrored as one NDJSON line:

```json
{"seq":42,"ts":"2026-02-20T20:11:00Z","event":"review_found_issues","task":"auth","attempt":2}
```

NDJSON mirror is observability output only, not canonical state.

### Artifact Storage

Large runtime artifacts (full prompts, transcripts, diffs, stdout/stderr blobs) should be stored on disk under `.thence/runs/<run-id>/artifacts/` (or configured equivalent). Events keep only references.

---

## Failure Recovery and Resume

On restart with `--resume`:

1. resolve target run using the Phase 0 deterministic resume rules
2. replay events to reconstruct state
3. detect in-flight attempts without terminal event
4. append explicit `attempt_interrupted` for each orphaned attempt
5. if unresolved `human_input_requested` exists, append/retain `run_paused` and do not schedule
6. requeue eligible tasks only when run is not paused

No manual DB edits required.

---

## Security Model (V1)

1. local trust model only.
2. no remote untrusted plan execution.
3. check commands run in task worktree under local user permissions.
4. clearly label `--trust-plan-checks` if plan-provided commands are enabled.

Default: only configured safe checks unless explicitly overridden.

---

## Packaging

1. Rust binary via `cargo build --release`.
2. SQLite bundled using `rusqlite` `bundled` feature.
3. spindle-rust linked in-process as a crate (no external spindle binary required).
4. no external DB/runtime dependencies.

---

## Module Layout (Suggested)

```text
src/
  main.rs
  cli.rs
  run/
    mod.rs
    loop.rs
    scheduler.rs
  plan/
    translator.rs
    validate.rs
    sanity.rs
    review_loop.rs
  policy/
    mod.rs
    rules.rs
    spindle_bridge.rs
  events/
    mod.rs
    schema.rs
    store.rs
    projector.rs
  workers/
    mod.rs
    provider.rs
    codex.rs
    reviewer.rs
  vcs/
    worktree.rs
    merge.rs
  checks/
    runner.rs
  logging/
    ndjson.rs
```

---

## Implementation Plan

### Milestone 1: Skeleton and Storage

Deliverables:

1. CLI skeleton (`run` command only).
2. SQLite schema + migration runner.
3. event append API with transaction boundaries.
4. replay projector producing in-memory run/task state.

Acceptance:

1. append/replay deterministically reconstructs same state.
2. event dedupe works for duplicate writes.

### Milestone 2: Prompt Translation + Spec Gate

Deliverables:

1. prompt-based markdown -> SPL translator.
2. `spindle validate` integration and sanity query gates.
3. spec review loop with question events and human pause/resume events.
4. policy rules blocking implementation until spec is approved.

Acceptance:

1. invalid/generated-bad SPL cannot start execution.
2. unresolved spec questions block all claimability.
3. resolved questions + spec approval unlock claimability.

### Milestone 3: Policy Engine Integration

Deliverables:

1. projected event-facts adapter as SPL facts.
2. spindle-rust bridge.
3. pre-canned internal rule bundle for readiness/review/close/human-pause gates.

Acceptance:

1. policy conclusions match expected task state scenarios.

### Milestone 4: Implementer Execution

Deliverables:

1. provider abstraction with `codex` adapter.
2. worktree provisioning.
3. implementer task attempt execution.
4. `work_submitted` event generation.

Acceptance:

1. two implementer workers can run distinct tasks concurrently.

### Milestone 5: Reviewer + Findings Loop

Deliverables:

1. reviewer adapter flow.
2. structured findings model.
3. reopen semantics for findings.
4. approval semantics.

Acceptance:

1. task with findings is not closable.
2. reviewer pass on latest attempt can unblock close when checks pass.

### Milestone 6: Checks and Merge Queue

Deliverables:

1. checks runner and `checks_reported` events.
2. single-threaded merge queue.
3. conflict reopen behavior.
4. close event only on merge success.

Acceptance:

1. merge conflicts reopen task.
2. no close without merge success.

### Milestone 7: Resume, Logging, Hardening

Deliverables:

1. resume and recovery behavior.
2. optional NDJSON mirror logging.
3. human input commands (`questions`, `answer`, `resume`).
4. integration tests with fake providers.
5. release packaging docs.

Acceptance:

1. kill/restart supervisor and resume without corruption.
2. final state reproducible from event replay.

---

## Test Strategy

### Unit

1. event validation and serialization
2. projector transitions
3. translator output contract checks
4. terminal run-event exclusivity (`completed` vs `failed` vs `cancelled`)
5. policy rule tests for each invariant
6. branch naming/worktree path safety

### Integration

1. happy path: translate -> validate -> spec approval -> implement -> review pass -> checks pass -> merge -> close
2. findings loop: implement -> findings -> rework -> approval
3. translation failure: opens spec question and pauses run
4. checks failure: blocks close until fixed
5. merge conflict: reopen + retry
6. timeout/retry exhaustion: terminal failure
7. crash and resume deterministic replay
8. human input pause/resume flow with pending question

### Golden Scenario

Maintain at least one fixed scenario fixture where full event sequence is asserted by exact ordered event types and terminal status.

---

## Acceptance Criteria (V1 Release)

1. One-command run starts and completes an end-to-end plan.
2. Prompt-generated SPL is validated and sanity-checked before execution.
3. Spec/review ambiguity gates block implementation until resolved.
4. Implementer cannot self-certify completion.
5. Reviewer + checks gates are mandatory for closure.
6. Human input requests can pause and later resume runs without corruption.
7. All state is reconstructible from SQLite event log.
8. Binary runs without external DB/runtime installation.

---

## Open Questions

1. How strict should translator prompt version pinning be (hard-coded vs configurable)?
2. Whether reviewers should always be different provider instances, or only different role IDs.
3. Default behavior on partial failure (fail run vs continue with partial success).
4. Required redaction policy for prompts/transcripts in logs.
