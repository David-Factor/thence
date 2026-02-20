use crate::events::EventRow;
use crate::events::projector::{RunProjection, TaskProjection};
use crate::plan::translator::TranslatedPlan;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

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
