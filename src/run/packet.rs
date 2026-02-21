use crate::events::EventRow;
use crate::events::projector::{RunProjection, TaskProjection};
use crate::plan::translator::TranslatedPlan;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const PLAN_TRANSLATOR_SPL_REFERENCE: &str = r#"SPL QUICK REFERENCE (whence translator subset)

- Facts: (given atom) or (given (pred arg1 arg2))
- Strict rule: (always label body head)
- Defeasible rule: (normally label body head)
- Defeater: (except label body head)
- Priority: (prefer winner loser)
- Metadata: (meta label (key "value"))
- Conjunction: (and lit1 lit2 ...)
- Negation: (not literal)

Canonical orchestration facts required in translated SPL:
- (given (task <id>))
- (given (depends-on <task-id> <dep-id>)) for each dependency edge

Constraints:
- Keep the plan self-contained.
- Do NOT use (import ...).
- Task ids must be stable and match tasks[].id exactly.
"#;

pub fn build_plan_translator_prompt(
    repo_root: &Path,
    plan_file: &Path,
    markdown: &str,
    default_checks: &[String],
    agents_md: Option<String>,
    claude_md: Option<String>,
) -> String {
    let payload = json!({
        "role": "plan-translator",
        "instruction": "Translate the specification into a self-contained SPL plan and a normalized task graph JSON. Return ONLY JSON.",
        "output_contract": {
            "required_keys": ["spl", "tasks"],
            "tasks_item_keys": ["id", "objective", "acceptance", "dependencies", "checks"],
            "task_id_charset": "[A-Za-z0-9_-]+",
            "constraints": [
                "spl must be valid spindle SPL",
                "no import directives",
                "every tasks[].id appears as (given (task <id>)) fact",
                "every dependency edge appears as (given (depends-on <task> <dep>)) fact",
                "dependencies must reference existing task ids"
            ]
        },
        "repo_root": repo_root,
        "plan_file": plan_file,
        "default_checks": default_checks,
        "spl_reference": PLAN_TRANSLATOR_SPL_REFERENCE,
        "spec_markdown": markdown,
        "agents_md": agents_md,
        "claude_md": claude_md
    });
    payload.to_string()
}

pub fn build_checks_proposer_prompt(
    repo_root: &Path,
    plan_file: &Path,
    markdown: &str,
    translated: &TranslatedPlan,
    agents_md: Option<String>,
    claude_md: Option<String>,
) -> String {
    let payload = json!({
        "role": "checks-proposer",
        "instruction": "Propose objective, deterministic check commands for this repo. Output JSON with key 'commands' as non-empty list of shell commands.",
        "repo_root": repo_root,
        "plan_file": plan_file,
        "plan_excerpt": markdown.lines().take(80).collect::<Vec<_>>().join("\n"),
        "task_ids": translated.tasks.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        "agents_md": agents_md,
        "claude_md": claude_md
    });
    payload.to_string()
}

pub fn build_implementer_prompt(
    run: &RunProjection,
    events: &[EventRow],
    task: &TaskProjection,
    attempt: i64,
    run_checks: &[String],
) -> String {
    let dep_outcomes = dependency_outcomes(run, task);
    let unresolved = unresolved_findings(events, &task.id);
    let artifact_refs = artifact_refs(events, &task.id, attempt);

    json!({
        "role": "implementer",
        "task_id": task.id,
        "attempt": attempt,
        "objective": task.objective,
        "acceptance": task.acceptance,
        "dependency_outcomes": dep_outcomes,
        "unresolved_findings": unresolved,
        "required_checks": run_checks,
        "artifact_refs": artifact_refs
    })
    .to_string()
}

pub fn build_reviewer_prompt(
    events: &[EventRow],
    task: &TaskProjection,
    attempt: i64,
    run_checks: &[String],
    submission_refs: serde_json::Value,
) -> String {
    let findings = unresolved_findings(events, &task.id);
    let artifact_refs = artifact_refs(events, &task.id, attempt);

    json!({
        "role": "reviewer",
        "task_id": task.id,
        "attempt": attempt,
        "objective": task.objective,
        "acceptance": task.acceptance,
        "submission_refs": submission_refs,
        "prior_findings": findings,
        "required_checks": run_checks,
        "artifact_refs": artifact_refs
    })
    .to_string()
}

fn dependency_outcomes(run: &RunProjection, task: &TaskProjection) -> Vec<serde_json::Value> {
    task.dependencies
        .iter()
        .map(|dep| {
            let status = run.tasks.get(dep);
            json!({
                "task_id": dep,
                "closed": status.map(|t| t.closed).unwrap_or(false),
                "terminal_failed": status.map(|t| t.terminal_failed).unwrap_or(false)
            })
        })
        .collect()
}

fn unresolved_findings(events: &[EventRow], task_id: &str) -> Vec<serde_json::Value> {
    let mut by_attempt: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let mut resolved: HashMap<i64, bool> = HashMap::new();

    for ev in events {
        if ev.task_id.as_deref() != Some(task_id) {
            continue;
        }
        match ev.event_type.as_str() {
            "review_found_issues" => {
                let attempt = ev.attempt.unwrap_or(0);
                let reason = ev
                    .payload_json
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("review findings")
                    .to_string();
                by_attempt.entry(attempt).or_default().push(reason);
                resolved.insert(attempt, false);
            }
            "review_approved" => {
                let attempt = ev.attempt.unwrap_or(0);
                resolved.insert(attempt, true);
            }
            _ => {}
        }
    }

    by_attempt
        .into_iter()
        .filter(|(attempt, _)| !resolved.get(attempt).copied().unwrap_or(false))
        .map(|(attempt, reasons)| json!({"attempt": attempt, "reasons": reasons}))
        .collect()
}

fn artifact_refs(
    events: &[EventRow],
    task_id: &str,
    current_attempt: i64,
) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter(|ev| ev.task_id.as_deref() == Some(task_id))
        .filter(|ev| ev.attempt.unwrap_or(0) <= current_attempt)
        .filter(|ev| {
            matches!(
                ev.event_type.as_str(),
                "work_submitted" | "review_found_issues" | "review_approved" | "checks_reported"
            )
        })
        .rev()
        .take(8)
        .map(|ev| {
            json!({
                "event": ev.event_type,
                "attempt": ev.attempt,
                "payload": ev.payload_json
            })
        })
        .collect()
}
