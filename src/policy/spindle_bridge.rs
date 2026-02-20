use crate::events::projector::RunProjection;
use anyhow::{Context, Result};
use spindle_core::literal::Literal;
use spindle_core::mode::Mode;
use spindle_core::query::{query, QueryStatus};
use spindle_core::temporal::Temporal;
use spindle_parser::parse_spl;
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct PolicySnapshot {
    pub run_paused: bool,
    pub claimable: HashSet<String>,
    pub closable: HashSet<String>,
    pub merge_ready: HashSet<String>,
}

const STATIC_POLICY_RULES: &str = r#"
(always policy-claimable
  (and
    (task ?t)
    (ready ?t)
    spec-approved
    checks-approved
    no-open-questions
    run-active
    (unclaimed ?t)
    (unclosed ?t)
    (unfailed ?t))
  (claimable ?t))

(always policy-closable
  (and
    (task ?t)
    (latest-attempt ?t ?a)
    (review-approved ?t ?a)
    (checks-passed ?t ?a)
    (findings-clear ?t ?a))
  (closable ?t))

(always policy-merge-ready
  (and
    (task ?t)
    (closable ?t)
    no-open-questions
    run-active)
  (merge-ready ?t))
"#;

pub fn derive_policy_state(run: &RunProjection, plan_spl: &str) -> Result<PolicySnapshot> {
    let mut composed = String::new();
    composed.push_str("; static policy rules\n");
    composed.push_str(STATIC_POLICY_RULES);
    composed.push_str("\n; translated plan facts/rules\n");
    composed.push_str(plan_spl);
    composed.push_str("\n; lifecycle projected facts\n");

    if run.spec_approved {
        composed.push_str("(given spec-approved)\n");
    }
    if run.checks_approved {
        composed.push_str("(given checks-approved)\n");
    }
    if run.open_questions.is_empty() {
        composed.push_str("(given no-open-questions)\n");
    }
    if !run.paused && run.terminal.is_none() {
        composed.push_str("(given run-active)\n");
    }
    if run.paused {
        composed.push_str("(given run-paused)\n");
    }

    for task in run.tasks.values() {
        composed.push_str(&format!("(given (task {}))\n", task.id));
        let deps_closed = task
            .dependencies
            .iter()
            .all(|dep| run.tasks.get(dep).map(|t| t.closed).unwrap_or(false));
        if deps_closed {
            composed.push_str(&format!("(given (ready {}))\n", task.id));
        }
        if !task.claimed {
            composed.push_str(&format!("(given (unclaimed {}))\n", task.id));
        }
        if !task.closed {
            composed.push_str(&format!("(given (unclosed {}))\n", task.id));
        }
        if !task.terminal_failed {
            composed.push_str(&format!("(given (unfailed {}))\n", task.id));
        }
        if task.claimed {
            composed.push_str(&format!("(given (claimed {}))\n", task.id));
        }
        if task.closed {
            composed.push_str(&format!("(given (closed {}))\n", task.id));
        }
        if task.terminal_failed {
            composed.push_str(&format!("(given (terminal-failed {}))\n", task.id));
        }
        if task.latest_attempt > 0 {
            composed.push_str(&format!(
                "(given (latest-attempt {} a{}))\n",
                task.id, task.latest_attempt
            ));
            if !task
                .unresolved_findings_attempts
                .contains(&task.latest_attempt)
            {
                composed.push_str(&format!(
                    "(given (findings-clear {} a{}))\n",
                    task.id, task.latest_attempt
                ));
            }
        }
        for a in &task.review_approved_attempts {
            composed.push_str(&format!("(given (review-approved {} a{}))\n", task.id, a));
        }
        for a in &task.checks_passed_attempts {
            composed.push_str(&format!("(given (checks-passed {} a{}))\n", task.id, a));
        }
        for a in &task.unresolved_findings_attempts {
            composed.push_str(&format!("(given (findings-open {} a{}))\n", task.id, a));
        }
    }

    let theory = parse_spl(&composed).context("policy SPL parse failed")?;

    let mut snapshot = PolicySnapshot {
        run_paused: run.paused || !run.open_questions.is_empty(),
        ..PolicySnapshot::default()
    };

    for task_id in run.tasks.keys() {
        if is_provable(&theory, "claimable", &[task_id.as_str()])? {
            snapshot.claimable.insert(task_id.clone());
        }
        if is_provable(&theory, "closable", &[task_id.as_str()])? {
            snapshot.closable.insert(task_id.clone());
        }
        if is_provable(&theory, "merge-ready", &[task_id.as_str()])? {
            snapshot.merge_ready.insert(task_id.clone());
        }
    }

    Ok(snapshot)
}

fn is_provable(theory: &spindle_core::theory::Theory, name: &str, args: &[&str]) -> Result<bool> {
    let lit = Literal::new(
        name,
        false,
        Mode::empty(),
        Temporal::empty(),
        args.iter().map(|s| s.to_string()).collect(),
    );
    let res = query(theory, &lit)?;
    Ok(matches!(res.status, QueryStatus::Provable))
}
