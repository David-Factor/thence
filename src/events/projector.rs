use crate::events::EventRow;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskProjection {
    pub id: String,
    pub objective: String,
    pub acceptance: String,
    pub dependencies: Vec<String>,
    pub required_checks: Vec<String>,
    pub attempts: i64,
    pub claimed: bool,
    pub latest_attempt: i64,
    pub review_approved_attempts: HashSet<i64>,
    pub checks_passed_attempts: HashSet<i64>,
    pub unresolved_findings_attempts: HashSet<i64>,
    pub merged_attempts: HashSet<i64>,
    pub closed: bool,
    pub terminal_failed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunProjection {
    pub run_id: String,
    pub spec_approved: bool,
    pub checks_approved: bool,
    pub checks_commands: Vec<String>,
    pub paused: bool,
    pub terminal: Option<String>,
    pub tasks: BTreeMap<String, TaskProjection>,
    pub open_questions: HashMap<String, String>,
}

impl RunProjection {
    pub fn apply_event(&mut self, ev: &EventRow) {
        self.run_id = ev.run_id.clone();
        match ev.event_type.as_str() {
            "task_registered" => {
                let task_id = ev.task_id.clone().or_else(|| {
                    ev.payload_json
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string)
                });
                if let Some(task_id) = task_id {
                    let objective = ev
                        .payload_json
                        .get("objective")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let deps = ev
                        .payload_json
                        .get("dependencies")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(ToString::to_string))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let acceptance = ev
                        .payload_json
                        .get("acceptance")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let checks = ev
                        .payload_json
                        .get("checks")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(ToString::to_string))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    self.tasks.entry(task_id.clone()).or_insert(TaskProjection {
                        id: task_id,
                        objective,
                        acceptance,
                        dependencies: deps,
                        required_checks: checks,
                        ..TaskProjection::default()
                    });
                }
            }
            "spec_approved" => self.spec_approved = true,
            "checks_approved" => {
                self.checks_approved = true;
                self.checks_commands = ev
                    .payload_json
                    .get("commands")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(ToString::to_string))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
            }
            "run_paused" | "human_input_requested" => self.paused = true,
            "run_resumed" => self.paused = false,
            "spec_question_opened" | "checks_question_opened" => {
                if let Some(qid) = ev.payload_json.get("question_id").and_then(|v| v.as_str()) {
                    let q = ev
                        .payload_json
                        .get("question")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.open_questions.insert(qid.to_string(), q);
                }
            }
            "spec_question_resolved" | "checks_question_resolved" => {
                if let Some(qid) = ev.payload_json.get("question_id").and_then(|v| v.as_str()) {
                    self.open_questions.remove(qid);
                }
            }
            "task_claimed" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    task.claimed = true;
                    task.attempts += 1;
                    task.latest_attempt = ev.attempt.unwrap_or(task.attempts);
                }
            }
            "review_found_issues" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    task.claimed = false;
                    let attempt = ev.attempt.unwrap_or(task.latest_attempt);
                    task.unresolved_findings_attempts.insert(attempt);
                }
            }
            "review_approved" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    let attempt = ev.attempt.unwrap_or(task.latest_attempt);
                    task.review_approved_attempts.insert(attempt);
                    task.unresolved_findings_attempts.remove(&attempt);
                }
            }
            "checks_reported" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    let attempt = ev.attempt.unwrap_or(task.latest_attempt);
                    if ev
                        .payload_json
                        .get("passed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        task.checks_passed_attempts.insert(attempt);
                    } else {
                        task.checks_passed_attempts.remove(&attempt);
                    }
                }
            }
            "merge_succeeded" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    let attempt = ev.attempt.unwrap_or(task.latest_attempt);
                    task.merged_attempts.insert(attempt);
                }
            }
            "task_closed" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    task.closed = true;
                    task.claimed = false;
                }
            }
            "task_failed_terminal" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    task.terminal_failed = true;
                    task.claimed = false;
                }
            }
            "attempt_interrupted" => {
                if let Some(task) = ev.task_id.as_ref().and_then(|id| self.tasks.get_mut(id)) {
                    task.claimed = false;
                }
            }
            "run_completed" | "run_failed" | "run_cancelled" => {
                self.terminal = Some(ev.event_type.clone());
            }
            _ => {}
        }
    }

    pub fn replay(events: &[EventRow]) -> Self {
        let mut s = Self::default();
        for ev in events {
            s.apply_event(ev);
        }
        s
    }
}
