use crate::checks;
use crate::events::projector::RunProjection;
use crate::events::store::EventStore;
use crate::events::NewEvent;
use crate::policy;
use crate::run::{append_event, packet, scheduler, RunConfig};
use crate::vcs;
use crate::workers::provider::{provider_for, AgentRequest};
use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

pub struct LoopInput {
    pub run_id: String,
    pub cfg: RunConfig,
    pub base_dir: PathBuf,
    pub plan_spl: String,
    pub ndjson_log: Option<PathBuf>,
}

pub fn run_supervisor_loop(store: &EventStore, input: LoopInput) -> Result<String> {
    let provider = provider_for(&input.cfg.agent, &input.cfg.agent_cmd)?;

    loop {
        let events = store.list_events(&input.run_id)?;
        let projected = RunProjection::replay(&events);
        let policy_state =
            policy::spindle_bridge::derive_policy_state(&projected, &input.plan_spl)?;

        if let Some(term) = projected.terminal {
            return Ok(term);
        }
        if policy_state.run_paused {
            return Ok("run_paused".to_string());
        }

        if let Some(task_id) =
            scheduler::next_claimable_task(&projected, &policy_state, input.cfg.max_attempts)
        {
            let task = projected.tasks.get(&task_id).expect("task exists");
            let attempt = task.attempts + 1;

            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "task_claimed".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("implementer".to_string()),
                    actor_id: Some(format!(
                        "impl-{}",
                        (attempt as usize % input.cfg.workers) + 1
                    )),
                    attempt: Some(attempt),
                    payload_json: json!({"attempt": attempt}),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            let worktree = vcs::worktree::prepare_worktree(
                &input.base_dir,
                &input.run_id,
                &task_id,
                attempt,
                &format!("impl-{}", (attempt as usize % input.cfg.workers) + 1),
            )?;

            let implementer_res = provider.run(AgentRequest {
                role: "implementer".to_string(),
                task_id: task_id.clone(),
                attempt,
                worktree_path: worktree.clone(),
                prompt: packet::build_implementer_prompt(
                    &projected,
                    &events,
                    task,
                    attempt,
                    &projected.checks_commands,
                ),
                timeout: Duration::from_secs(45 * 60),
            })?;

            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "work_submitted".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("implementer".to_string()),
                    actor_id: Some(format!(
                        "impl-{}",
                        (attempt as usize % input.cfg.workers) + 1
                    )),
                    attempt: Some(attempt),
                    payload_json: json!({
                        "exit_code": implementer_res.exit_code,
                        "stdout_path": implementer_res.stdout_path,
                        "stderr_path": implementer_res.stderr_path
                    }),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            if implementer_res.exit_code != 0 {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("implementer-exit-gate".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": format!("implementer exit_code={}", implementer_res.exit_code)}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
                if attempt >= input.cfg.max_attempts {
                    append_event(
                        store,
                        &input.run_id,
                        &NewEvent {
                            event_type: "task_failed_terminal".to_string(),
                            task_id: Some(task_id),
                            actor_role: Some("supervisor".to_string()),
                            actor_id: Some("supervisor-1".to_string()),
                            attempt: Some(attempt),
                            payload_json: json!({"reason": "max attempts reached after implementer failure"}),
                            dedupe_key: None,
                        },
                        input.ndjson_log.as_deref(),
                    )?;
                }
                continue;
            }

            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "review_requested".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("supervisor".to_string()),
                    actor_id: Some("supervisor-1".to_string()),
                    attempt: Some(attempt),
                    payload_json: json!({"attempt": attempt}),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            let reviewer_id = format!("rev-{}", (attempt as usize % input.cfg.reviewers) + 1);
            let submission_refs = json!({
                "work_submitted": {
                    "stdout_path": implementer_res.stdout_path,
                    "stderr_path": implementer_res.stderr_path,
                    "exit_code": implementer_res.exit_code
                }
            });
            let reviewer_res = provider.run(AgentRequest {
                role: "reviewer".to_string(),
                task_id: task_id.clone(),
                attempt,
                worktree_path: worktree.clone(),
                prompt: packet::build_reviewer_prompt(
                    &events,
                    task,
                    attempt,
                    &projected.checks_commands,
                    submission_refs,
                ),
                timeout: Duration::from_secs(20 * 60),
            })?;

            let reviewer_approved = reviewer_res
                .structured_output
                .as_ref()
                .and_then(|v| v.get("approved"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !reviewer_approved {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("reviewer".to_string()),
                        actor_id: Some(reviewer_id),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": "review findings"}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;

                if attempt >= input.cfg.max_attempts {
                    append_event(
                        store,
                        &input.run_id,
                        &NewEvent {
                            event_type: "task_failed_terminal".to_string(),
                            task_id: Some(task_id),
                            actor_role: Some("supervisor".to_string()),
                            actor_id: Some("supervisor-1".to_string()),
                            attempt: Some(attempt),
                            payload_json: json!({"reason": "max attempts reached after review findings"}),
                            dedupe_key: None,
                        },
                        input.ndjson_log.as_deref(),
                    )?;
                }
                continue;
            }

            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "review_approved".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("reviewer".to_string()),
                    actor_id: Some(reviewer_id),
                    attempt: Some(attempt),
                    payload_json: json!({"approved": true}),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            let checks = if !projected.checks_commands.is_empty() {
                projected.checks_commands.clone()
            } else if task.required_checks.is_empty() {
                input.cfg.checks.clone()
            } else {
                task.required_checks.clone()
            };
            let (checks_ok, checks_payload) = checks::runner::run_checks(
                &worktree,
                &checks,
                Duration::from_secs(input.cfg.check_timeout_secs),
            )?;
            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "checks_reported".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("supervisor".to_string()),
                    actor_id: Some("checks-1".to_string()),
                    attempt: Some(attempt),
                    payload_json: checks_payload,
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            if !checks_ok {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("checks-gate".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": "checks failed"}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
                if attempt >= input.cfg.max_attempts {
                    append_event(
                        store,
                        &input.run_id,
                        &NewEvent {
                            event_type: "task_failed_terminal".to_string(),
                            task_id: Some(task_id),
                            actor_role: Some("supervisor".to_string()),
                            actor_id: Some("supervisor-1".to_string()),
                            attempt: Some(attempt),
                            payload_json: json!({"reason": "max attempts reached after failed checks"}),
                            dedupe_key: None,
                        },
                        input.ndjson_log.as_deref(),
                    )?;
                }
                continue;
            }

            let current = RunProjection::replay(&store.list_events(&input.run_id)?);
            let policy_after_checks =
                policy::spindle_bridge::derive_policy_state(&current, &input.plan_spl)?;
            if !policy_after_checks.merge_ready.contains(&task_id) {
                continue;
            }

            let merged = vcs::merge::attempt_merge(&task.objective, attempt);
            if merged {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "merge_succeeded".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("merge-queue".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"integration_branch": format!("whence/{}", input.run_id)}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "task_closed".to_string(),
                        task_id: Some(task_id),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("supervisor-1".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"closed": true}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
            } else {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "merge_conflict".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("merge-queue".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": "simulated conflict"}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("merge-queue".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": "merge conflict; reopen"}),
                        dedupe_key: None,
                    },
                    input.ndjson_log.as_deref(),
                )?;
            }

            continue;
        }

        let all_done = !projected.tasks.is_empty()
            && projected
                .tasks
                .values()
                .all(|t| t.closed || t.terminal_failed);
        if all_done {
            let has_terminal_failed = projected.tasks.values().any(|t| t.terminal_failed);
            let final_event = if has_terminal_failed && !input.cfg.allow_partial_completion {
                "run_failed"
            } else {
                "run_completed"
            };
            append_event(
                store,
                &input.run_id,
                &NewEvent::simple(final_event, json!({"task_count": projected.tasks.len()})),
                input.ndjson_log.as_deref(),
            )?;
            return Ok(final_event.to_string());
        }

        let pending_tasks = projected
            .tasks
            .values()
            .filter(|t| !t.closed && !t.terminal_failed)
            .count();
        if pending_tasks > 0 {
            let any_attempt_room = projected
                .tasks
                .values()
                .any(|t| !t.closed && !t.terminal_failed && t.attempts < input.cfg.max_attempts);
            if !any_attempt_room {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent::simple(
                        "run_failed",
                        json!({"reason": "no schedulable tasks and no attempt budget"}),
                    ),
                    input.ndjson_log.as_deref(),
                )?;
                return Ok("run_failed".to_string());
            }

            // Deadlock on unresolved dependencies (e.g. dependency failed terminal)
            let block_all = projected
                .tasks
                .values()
                .filter(|t| !t.closed && !t.terminal_failed)
                .all(|t| {
                    t.dependencies.iter().any(|dep| {
                        projected
                            .tasks
                            .get(dep)
                            .map(|d| d.terminal_failed)
                            .unwrap_or(true)
                    })
                });
            if block_all {
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent::simple("run_failed", json!({"reason": "dependency deadlock"})),
                    input.ndjson_log.as_deref(),
                )?;
                return Ok("run_failed".to_string());
            }
        }

        append_event(
            store,
            &input.run_id,
            &NewEvent::simple("run_failed", json!({"reason": "unschedulable state"})),
            input.ndjson_log.as_deref(),
        )?;
        return Ok("run_failed".to_string());
    }
}
