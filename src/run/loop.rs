use crate::checks;
use crate::events::NewEvent;
use crate::events::projector::RunProjection;
use crate::events::store::EventStore;
use crate::policy;
use crate::run::{RunConfig, append_event, packet, run_artifact_dir, scheduler, sha256_hex};
use crate::vcs;
use crate::workers::provider::{AgentRequest, provider_for};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
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

            let implementer_payload = parse_prompt_json(&packet::build_implementer_prompt(
                &projected,
                &events,
                task,
                attempt,
                &projected.checks_commands,
            ));
            let implementer_capsule = json!({
                "capsule_version": 1,
                "role": "implementer",
                "run_id": input.run_id,
                "task_id": task_id,
                "attempt": attempt,
                "payload": implementer_payload
            });
            let (implementer_capsule_path, implementer_capsule_sha) = write_capsule(
                &input.base_dir,
                &input.run_id,
                &task_id,
                attempt,
                "implementer",
                &implementer_capsule,
            )?;
            let implementer_capsule_file = implementer_capsule_path.display().to_string();

            let implementer_res = provider.run(AgentRequest {
                role: "implementer".to_string(),
                task_id: task_id.clone(),
                attempt,
                worktree_path: worktree.clone(),
                prompt: json!({
                    "role": "implementer",
                    "capsule_file": implementer_capsule_file,
                    "critical": {
                        "task_id": task_id,
                        "attempt": attempt,
                        "objective": task.objective,
                        "acceptance": task.acceptance
                    }
                })
                .to_string(),
                env: capsule_env(
                    &implementer_capsule_path,
                    &implementer_capsule_sha,
                    "implementer",
                ),
                timeout: Duration::from_secs(45 * 60),
            })?;
            let implementer_output =
                validate_implementer_output(implementer_res.structured_output.as_ref());
            let implementer_output_error = implementer_output.as_ref().err().cloned();

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
                        "stderr_path": implementer_res.stderr_path,
                        "capsule_path": implementer_capsule_file,
                        "output_valid": implementer_output.is_ok(),
                        "output_error": implementer_output_error
                    }),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;

            if implementer_res.exit_code != 0 || implementer_output.is_err() {
                let mut findings = Vec::new();
                if implementer_res.exit_code != 0 {
                    findings.push(format!(
                        "implementer exited non-zero (exit_code={})",
                        implementer_res.exit_code
                    ));
                }
                if let Err(err) = implementer_output {
                    findings.push(format!("invalid implementer output: {err}"));
                }
                if findings.is_empty() {
                    findings.push("implementer did not produce valid submission output".to_string());
                }
                let reason = findings[0].clone();
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("implementer-output-gate".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": reason, "findings": findings, "source": "implementer_output_validation"}),
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
                            payload_json: json!({"reason": "max attempts reached after implementer gate failure"}),
                            dedupe_key: None,
                        },
                        input.ndjson_log.as_deref(),
                    )?;
                }
                continue;
            }

            let reviewer_id = format!("rev-{}", (attempt as usize % input.cfg.reviewers) + 1);
            let submission_refs = json!({
                "work_submitted": {
                    "stdout_path": implementer_res.stdout_path,
                    "stderr_path": implementer_res.stderr_path,
                    "exit_code": implementer_res.exit_code,
                    "capsule_path": implementer_capsule_file
                }
            });
            let reviewer_payload = parse_prompt_json(&packet::build_reviewer_prompt(
                &events,
                task,
                attempt,
                &projected.checks_commands,
                submission_refs,
            ));
            let reviewer_capsule = json!({
                "capsule_version": 1,
                "role": "reviewer",
                "run_id": input.run_id,
                "task_id": task_id,
                "attempt": attempt,
                "payload": reviewer_payload
            });
            let (reviewer_capsule_path, reviewer_capsule_sha) = write_capsule(
                &input.base_dir,
                &input.run_id,
                &task_id,
                attempt,
                "reviewer",
                &reviewer_capsule,
            )?;
            let reviewer_capsule_file = reviewer_capsule_path.display().to_string();
            append_event(
                store,
                &input.run_id,
                &NewEvent {
                    event_type: "review_requested".to_string(),
                    task_id: Some(task_id.clone()),
                    actor_role: Some("supervisor".to_string()),
                    actor_id: Some("supervisor-1".to_string()),
                    attempt: Some(attempt),
                    payload_json: json!({"attempt": attempt, "capsule_path": reviewer_capsule_file}),
                    dedupe_key: None,
                },
                input.ndjson_log.as_deref(),
            )?;
            let reviewer_res = provider.run(AgentRequest {
                role: "reviewer".to_string(),
                task_id: task_id.clone(),
                attempt,
                worktree_path: worktree.clone(),
                prompt: json!({
                    "role": "reviewer",
                    "capsule_file": reviewer_capsule_file,
                    "critical": {
                        "task_id": task_id,
                        "attempt": attempt,
                        "objective": task.objective,
                        "acceptance": task.acceptance
                    }
                })
                .to_string(),
                env: capsule_env(&reviewer_capsule_path, &reviewer_capsule_sha, "reviewer"),
                timeout: Duration::from_secs(20 * 60),
            })?;

            let reviewer_output = match validate_reviewer_output(reviewer_res.structured_output.as_ref()) {
                Ok(output) => output,
                Err(err) => {
                    let findings = vec![format!("invalid reviewer output: {err}")];
                    let reason = findings[0].clone();
                    append_event(
                        store,
                        &input.run_id,
                        &NewEvent {
                            event_type: "review_found_issues".to_string(),
                            task_id: Some(task_id.clone()),
                            actor_role: Some("reviewer".to_string()),
                            actor_id: Some(reviewer_id.clone()),
                            attempt: Some(attempt),
                            payload_json: json!({
                                "reason": reason,
                                "findings": findings,
                                "source": "reviewer_output_validation"
                            }),
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
                                payload_json: json!({"reason": "max attempts reached after invalid reviewer output"}),
                                dedupe_key: None,
                            },
                            input.ndjson_log.as_deref(),
                        )?;
                    }
                    continue;
                }
            };

            if !reviewer_output.approved {
                let findings = if reviewer_output.findings.is_empty() {
                    vec!["reviewer rejected submission without findings".to_string()]
                } else {
                    reviewer_output.findings
                };
                let reason = findings[0].clone();
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("reviewer".to_string()),
                        actor_id: Some(reviewer_id),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": reason, "findings": findings, "source": "reviewer"}),
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
                    payload_json: json!({"approved": true, "finding_count": reviewer_output.findings.len()}),
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
            let checks_findings = if checks_ok {
                Vec::new()
            } else {
                checks_failure_findings(&checks_payload)
            };
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
                let findings = checks_findings;
                let reason = findings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "checks failed".to_string());
                append_event(
                    store,
                    &input.run_id,
                    &NewEvent {
                        event_type: "review_found_issues".to_string(),
                        task_id: Some(task_id.clone()),
                        actor_role: Some("supervisor".to_string()),
                        actor_id: Some("checks-gate".to_string()),
                        attempt: Some(attempt),
                        payload_json: json!({"reason": reason, "findings": findings, "source": "checks_gate"}),
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

fn parse_prompt_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| json!({"raw_prompt": raw}))
}

#[derive(Debug, Deserialize)]
struct ImplementerOutput {
    submitted: bool,
}

#[derive(Debug, Deserialize)]
struct ReviewerOutput {
    approved: bool,
    #[serde(default)]
    findings: Vec<String>,
}

fn validate_implementer_output(output: Option<&serde_json::Value>) -> std::result::Result<ImplementerOutput, String> {
    let raw = output
        .cloned()
        .ok_or_else(|| "missing structured JSON output".to_string())?;
    let parsed: ImplementerOutput =
        serde_json::from_value(raw).map_err(|err| format!("output schema mismatch: {err}"))?;
    if !parsed.submitted {
        return Err("field 'submitted' must be true".to_string());
    }
    Ok(parsed)
}

fn validate_reviewer_output(output: Option<&serde_json::Value>) -> std::result::Result<ReviewerOutput, String> {
    let raw = output
        .cloned()
        .ok_or_else(|| "missing structured JSON output".to_string())?;
    let mut parsed: ReviewerOutput =
        serde_json::from_value(raw).map_err(|err| format!("output schema mismatch: {err}"))?;
    parsed.findings = parsed
        .findings
        .into_iter()
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty())
        .collect();
    if !parsed.approved && parsed.findings.is_empty() {
        parsed
            .findings
            .push("reviewer rejected submission without findings".to_string());
    }
    Ok(parsed)
}

fn checks_failure_findings(checks_payload: &serde_json::Value) -> Vec<String> {
    let mut findings = checks_payload
        .get("results")
        .and_then(|v| v.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|entry| {
                    let ok = entry.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let timed_out = entry
                        .get("timed_out")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if ok && !timed_out {
                        return None;
                    }
                    let command = entry
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("<unknown>");
                    if timed_out {
                        Some(format!("check timed out: {command}"))
                    } else {
                        Some(format!("check failed: {command}"))
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if findings.is_empty() {
        findings.push("checks failed".to_string());
    }
    findings
}

fn write_capsule(
    repo_root: &Path,
    run_id: &str,
    task_id: &str,
    attempt: i64,
    role: &str,
    capsule: &serde_json::Value,
) -> Result<(PathBuf, String)> {
    let path = run_artifact_dir(repo_root, run_id)
        .join("capsules")
        .join(task_id)
        .join(format!("attempt{attempt}"))
        .join(format!("{role}.json"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(capsule)?;
    fs::write(&path, &raw)?;
    let digest = sha256_hex(&raw);
    Ok((path, digest))
}

fn capsule_env(path: &Path, digest: &str, role: &str) -> Vec<(String, String)> {
    vec![
        (
            "WHENCE_CAPSULE_FILE".to_string(),
            path.display().to_string(),
        ),
        ("WHENCE_CAPSULE_SHA256".to_string(), digest.to_string()),
        ("WHENCE_CAPSULE_ROLE".to_string(), role.to_string()),
    ]
}
