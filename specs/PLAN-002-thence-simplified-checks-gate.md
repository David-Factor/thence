# PLAN-002: Thence Simplified Checks Gate + Context Packet Alignment

## Summary
This plan revises the prior direction to maximize implementation and UX simplicity while preserving trust boundaries and workflow guarantees.

The core pivot is:
- keep checks bootstrap as a pre-implementation gate phase (not a pseudo-task in the implementation loop)
- store approved checks in a simple repo-local file (`.thence/checks.json`)
- remove supervisor-native AGENTS/CLAUDE LLM extraction
- keep and expand context packets so agents receive actionable state (findings, dependency outcomes, checks)

This preserves the existing run model and event sourcing while reducing branching and schema complexity.

## Goals
1. Keep the one-command UX (`thence run ...`) intact.
2. Preserve strict review/check/merge trust boundaries.
3. Add a minimal checks configuration gate with human confirmation.
4. Make agent prompts packet-driven and state-rich.
5. Avoid new cross-run SQLite cache tables and avoid supervisor-native hint parsing calls.

## Non-Goals
1. Introducing generic `task_kind` / `requires_merge` in this iteration.
2. Adding repo baseline cache tables in SQLite.
3. Parsing free-form AGENTS docs in supervisor code.

## Locked Decisions
1. Checks bootstrap is a formal phase before the implementation loop.
2. Checks baseline persistence is file-based: `.thence/checks.json`.
3. Supervisor passes raw `AGENTS.md` / `CLAUDE.md` content to the checks-proposer agent packet when available.
4. User confirmation is required on first run without known checks and whenever proposal differs from existing baseline.
5. Context packets are the primary data plane for implementer/reviewer/check proposer.

## Proposed Runtime Flow

### Phase 0: Boot
1. Open/create state DB and migrate.
2. Read plan markdown.
3. Resolve run/resume as currently implemented.

### Phase 1: Translation + Validation
1. Translate markdown to SPL.
2. Validate via spindle parser/pipeline.
3. Run sanity checks.
4. Run spec review gate; pause if spec clarification is needed.

### Phase 1.5: Checks Configuration Gate (new formal phase)
Checks source precedence:
1. CLI `--checks` if provided.
2. `.thence/checks.json` if present and valid.
3. Otherwise run checks-proposer agent and request human confirmation.

Detailed behavior:
1. If source is CLI:
- normalize and use checks immediately.
- emit audit event indicating source `cli`.
2. If source is file:
- load checks, validate format, and use immediately.
- emit audit event indicating source `file`.
3. If no known checks:
- run checks-proposer agent with a packet containing repo context and raw `AGENTS.md`/`CLAUDE.md` text.
- emit proposal event.
- emit checks question + `human_input_requested` + `run_paused`.
- on answer: either accept proposed list or parse user-edited list.
- persist accepted checks to `.thence/checks.json`.
- emit approved event and `run_resumed`.

### Phase 2: Implementation Loop (unchanged in shape)
1. Scheduler claims runnable implementation tasks.
2. Implementer runs with packetized context.
3. `work_submitted` emitted; non-zero implementer exit gates to rework/fail (already implemented).
4. Reviewer runs with reviewer packet.
5. Missing/malformed reviewer output fails closed (already implemented).
6. Checks run using approved run checks.
7. Merge gate and close semantics remain unchanged.

### Phase 3: Completion
1. Existing run terminal behavior remains (`run_completed` / `run_failed` / `run_cancelled`).

## Data and Event Model

### New event types
Add:
1. `checks_proposed`
2. `checks_question_opened`
3. `checks_question_resolved`
4. `checks_approved`

These are distinct from spec question events.

### Existing event usage
1. Keep `human_input_requested`, `human_input_provided`, `run_paused`, `run_resumed` for pause/resume mechanics.
2. Keep all existing implementation/review/check/merge events.

### No new SQLite baseline table
- Do not add `repo_baselines`.
- Cross-run checks memory lives in `.thence/checks.json`.
- DB remains canonical per-run audit log.

## File Contract: `.thence/checks.json`
Location: repo root `.thence/checks.json`

Schema:
```json
{
  "version": 1,
  "commands": ["cargo test", "cargo clippy -- -D warnings"],
  "updated_at": "2026-02-20T21:00:00Z",
  "source": "human_approved"
}
```

Validation rules:
1. `version` must be `1`.
2. `commands` must be non-empty array of non-empty strings.
3. Invalid file triggers checks gate (do not silently proceed with bad data).

## Context Packet Design
Add packet builder module and stop using inline prompt strings.

### Check-proposer packet
Fields:
1. repo root
2. plan summary
3. current task graph summary
4. raw `AGENTS.md` content if present
5. raw `CLAUDE.md` content if present
6. safety instructions: propose checks only, do not execute

Expected output contract:
```json
{
  "commands": ["..."],
  "rationale": "..."
}
```

### Implementer packet
Fields:
1. task objective
2. acceptance criteria
3. dependency outcomes summary
4. unresolved findings from prior attempts
5. required checks for this run
6. artifact refs (stdout/stderr paths, prior review/check refs)

### Reviewer packet
Fields:
1. objective + acceptance criteria
2. submitted artifact refs
3. prior findings and status
4. required checks and gate state summary

## Transition and Policy Adjustments
1. Keep current transition invariants.
2. Add checks-phase invariants:
- implementation task claiming requires `checks_approved` (or equivalent projected run state)
- checks question must resolve before resume continues to implementation
3. Policy bridge can remain spindle-backed; add derived condition for checks gate completion as a run-level fact from events.

## Public Interface Changes
CLI additions:
1. `--reconfigure-checks`: force checks gate even when `.thence/checks.json` exists.
2. `--no-checks-file`: ignore `.thence/checks.json` for this run.

No changes to `questions`, `answer`, `resume` command shapes.

## Implementation Change List
1. `src/cli.rs`
- add new flags.
2. `src/run/mod.rs`
- add checks gate orchestration before entering implementation loop.
- add checks question open/resolve handlers in answer flow.
- load/save `.thence/checks.json`.
3. `src/run/loop.rs`
- consume run-approved checks and packet builders.
- replace inline prompt strings with packet-derived prompt bodies.
4. `src/run/transitions.rs`
- validate new checks events and sequencing.
5. `src/events/projector.rs`
- project checks-gate state from new events.
6. `src/events/store.rs`
- unresolved question listing to include checks questions.
7. `src/policy/spindle_bridge.rs`
- include checks-approved gate in claimability derivation.
8. `src/checks/`
- add file load/save/validation helpers for `.thence/checks.json`.
9. `src/run/packet.rs` (new)
- implement packet builders for check proposer, implementer, reviewer.

## Testing Plan

### Unit tests
1. `.thence/checks.json` parse/validate success/failure.
2. checks source precedence logic.
3. packet builder includes unresolved findings and dependency outcomes.
4. new transition validation for checks events.

### Integration tests
1. no checks file + no `--checks` -> checks gate pause -> answer accept -> resume -> run completes.
2. valid checks file present -> no checks gate pause -> implementation starts.
3. `--checks` overrides checks file.
4. `--reconfigure-checks` forces new proposal despite checks file.
5. malformed checks file triggers gate instead of silent proceed.
6. reviewer findings appear in subsequent implementer packet payload.

## Acceptance Criteria
1. First run in fresh repo without checks pauses for checks confirmation and then proceeds.
2. Subsequent runs use `.thence/checks.json` automatically unless overridden.
3. Implementation loop receives unresolved review findings in implementer packet.
4. Supervisor never performs AGENTS/CLAUDE hint extraction itself; hints are passed raw to the checks-proposer agent.
5. Existing trust boundaries remain intact (implementer cannot self-approve/close; reviewer cannot merge).

## Migration and Backward Compatibility
1. Existing runs/events remain readable.
2. New checks events are additive.
3. If `.thence/checks.json` does not exist, behavior is deterministic (enter checks gate).

## Defaults
1. Default checks source: file if present, else gate.
2. `--checks` always wins.
3. checks gate confirmation keywords:
- `accept` to accept proposal
- otherwise parse semicolon-separated commands as explicit override
