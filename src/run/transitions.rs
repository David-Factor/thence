use crate::events::projector::RunProjection;
use crate::events::{EventRow, NewEvent};
use anyhow::{Result, bail};

const TERMINAL_EVENTS: [&str; 3] = ["run_completed", "run_failed", "run_cancelled"];

pub fn validate_transition(history: &[EventRow], next: &NewEvent) -> Result<()> {
    let state = RunProjection::replay(history);

    if state.terminal.is_some() && !TERMINAL_EVENTS.contains(&next.event_type.as_str()) {
        bail!("invalid transition: run already terminal")
    }

    if TERMINAL_EVENTS.contains(&next.event_type.as_str())
        && history
            .iter()
            .any(|ev| TERMINAL_EVENTS.contains(&ev.event_type.as_str()))
    {
        bail!("invalid transition: run terminal event already exists")
    }

    if (next.event_type == "task_claimed" || next.event_type == "merge_succeeded")
        && (state.paused || !state.open_questions.is_empty())
    {
        bail!("invalid transition: run paused or human input pending")
    }

    if next.event_type == "task_claimed" {
        let task_id = next
            .task_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("task_claimed missing task_id"))?;
        let task = state
            .tasks
            .get(task_id)
            .ok_or_else(|| anyhow::anyhow!("task_claimed references unknown task '{task_id}'"))?;
        if !state.spec_approved
            || !state.checks_approved
            || !state.open_questions.is_empty()
            || state.paused
        {
            bail!("invalid transition: cannot claim before spec approval/unpaused run")
        }
        if task.closed || task.terminal_failed {
            bail!("invalid transition: task already terminal")
        }
    }

    if next.event_type == "review_approved" && next.actor_role.as_deref() == Some("implementer") {
        bail!("invalid transition: implementer cannot approve review")
    }

    if next.event_type == "merge_succeeded" && next.actor_role.as_deref() == Some("reviewer") {
        bail!("invalid transition: reviewer cannot emit merge events")
    }

    if next.event_type == "task_closed" {
        let task_id = next
            .task_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("task_closed missing task_id"))?;
        let attempt = next
            .attempt
            .ok_or_else(|| anyhow::anyhow!("task_closed missing attempt"))?;
        let merged = history.iter().any(|ev| {
            ev.event_type == "merge_succeeded"
                && ev.task_id.as_deref() == Some(task_id)
                && ev.attempt == Some(attempt)
        });
        if !merged {
            bail!("invalid transition: task_closed requires merge_succeeded for same attempt")
        }
    }

    if next.event_type == "checks_approved" {
        let has_commands = next
            .payload_json
            .get("commands")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);
        if !has_commands {
            bail!("invalid transition: checks_approved requires non-empty commands")
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_approved_requires_non_empty_commands() {
        let next = NewEvent::simple("checks_approved", json!({"commands": []}));
        let err = validate_transition(&[], &next).unwrap_err();
        assert!(format!("{err}").contains("requires non-empty commands"));
    }
}
