use crate::events::projector::{RunProjection, TaskProjection};

pub fn claimable(run: &RunProjection, task: &TaskProjection) -> bool {
    if run.paused || !run.spec_approved || !run.open_questions.is_empty() || run.terminal.is_some()
    {
        return false;
    }
    if task.closed || task.terminal_failed || task.claimed {
        return false;
    }
    task.dependencies
        .iter()
        .all(|dep| run.tasks.get(dep).map(|t| t.closed).unwrap_or(false))
}

pub fn closable(task: &TaskProjection) -> bool {
    let a = task.latest_attempt;
    a > 0
        && task.review_approved_attempts.contains(&a)
        && task.checks_passed_attempts.contains(&a)
        && !task.unresolved_findings_attempts.contains(&a)
}

pub fn merge_ready(run: &RunProjection, task: &TaskProjection) -> bool {
    !run.paused && run.open_questions.is_empty() && closable(task)
}
