mod r#loop;
pub mod packet;
pub mod scheduler;
mod transitions;

use crate::events::projector::RunProjection;
use crate::events::store::{EventStore, RunRow};
use crate::events::{EventRow, NewEvent};
use crate::logging::ndjson;
use crate::plan::{review_loop, sanity, translator, validate};
use crate::workers::provider::{provider_for, AgentRequest};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RunCommand {
    pub plan_file: PathBuf,
    pub agent: String,
    pub workers: usize,
    pub reviewers: usize,
    pub checks: Option<String>,
    pub reconfigure_checks: bool,
    pub no_checks_file: bool,
    pub log: Option<PathBuf>,
    pub resume: bool,
    pub run_id: Option<String>,
    pub state_db: Option<PathBuf>,
    pub allow_partial_completion: bool,
    pub trust_plan_checks: bool,
    pub interactive: bool,
    pub debug_dump_spl: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub agent: String,
    pub workers: usize,
    pub reviewers: usize,
    #[serde(default = "default_checks")]
    pub checks: Vec<String>,
    #[serde(default)]
    pub checks_from_cli: bool,
    #[serde(default = "default_use_checks_file")]
    pub use_checks_file: bool,
    #[serde(default)]
    pub reconfigure_checks: bool,
    pub allow_partial_completion: bool,
    pub trust_plan_checks: bool,
    pub interactive: bool,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i64,
    #[serde(default = "default_check_timeout_secs")]
    pub check_timeout_secs: u64,
}

fn default_state_db() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("whence").join("state.db");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("whence")
            .join("state.db");
    }
    PathBuf::from(".whence/state.db")
}

fn default_use_checks_file() -> bool {
    true
}

fn default_max_attempts() -> i64 {
    3
}

fn default_check_timeout_secs() -> u64 {
    10 * 60
}

pub fn execute_run(cmd: RunCommand) -> Result<()> {
    let db = cmd.state_db.clone().unwrap_or_else(default_state_db);
    let store = EventStore::open(&db)?;

    if cmd.resume {
        let run_id = resolve_resume_run_id(&store, cmd.run_id.as_deref())?;
        return continue_run(&store, &run_id, cmd.log.clone());
    }

    let markdown = std::fs::read_to_string(&cmd.plan_file)
        .with_context(|| format!("read plan file {}", cmd.plan_file.display()))?;
    let cli_checks = parse_checks(cmd.checks.as_deref());
    let repo_root = repo_root_for_plan(&cmd.plan_file)?;

    let run_id = cmd.run_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let run_dir = run_artifact_dir(&repo_root, &run_id);
    std::fs::create_dir_all(&run_dir)?;
    let spl_path = run_dir.join("plan.spl");

    let plan_sha256 = sha256_hex(&markdown);
    let cfg = RunConfig {
        agent: cmd.agent,
        workers: cmd.workers.max(1),
        reviewers: cmd.reviewers.max(1),
        checks: if cli_checks.is_empty() {
            default_checks()
        } else {
            cli_checks.clone()
        },
        checks_from_cli: cmd.checks.is_some(),
        use_checks_file: !cmd.no_checks_file,
        reconfigure_checks: cmd.reconfigure_checks,
        allow_partial_completion: cmd.allow_partial_completion,
        trust_plan_checks: cmd.trust_plan_checks,
        interactive: cmd.interactive,
        max_attempts: 3,
        check_timeout_secs: 10 * 60,
    };

    store.create_run(&RunRow {
        id: run_id.clone(),
        plan_path: cmd.plan_file.display().to_string(),
        plan_sha256,
        spl_plan_path: spl_path.display().to_string(),
        created_at: Utc::now().to_rfc3339(),
        status: "running".to_string(),
        config_json: serde_json::to_value(&cfg)?,
    })?;

    append_event(
        &store,
        &run_id,
        &NewEvent::simple(
            "run_started",
            json!({
                "plan_file": cmd.plan_file,
                "agent": cfg.agent,
                "workers": cfg.workers,
                "reviewers": cfg.reviewers
            }),
        ),
        cmd.log.as_deref(),
    )?;

    let translated = match translator::translate_markdown_to_spl(&markdown, &default_checks()) {
        Ok(t) => t,
        Err(e) => {
            let qid = "spec-q-translate";
            append_event(
                &store,
                &run_id,
                &NewEvent::simple(
                    "spec_question_opened",
                    json!({"question_id": qid, "question": format!("Plan translation failed: {e}")}),
                ),
                cmd.log.as_deref(),
            )?;
            pause_for_question(&store, &run_id, qid, cmd.log.as_deref())?;
            bail!("run paused due to translation failure")
        }
    };
    std::fs::write(&spl_path, &translated.spl)?;
    if let Some(path) = cmd.debug_dump_spl.as_ref() {
        std::fs::write(path, &translated.spl)?;
    }

    append_event(
        &store,
        &run_id,
        &NewEvent::simple(
            "plan_translated",
            json!({
                "spl_path": spl_path,
                "task_count": translated.tasks.len()
            }),
        ),
        cmd.log.as_deref(),
    )?;

    if let Err(e) =
        validate::validate_spl(&translated.spl).and_then(|_| sanity::run_sanity_checks(&translated))
    {
        let qid = "spec-q-validate";
        append_event(
            &store,
            &run_id,
            &NewEvent::simple(
                "spec_question_opened",
                json!({"question_id": qid, "question": format!("Plan generation failed: {e}")}),
            ),
            cmd.log.as_deref(),
        )?;
        pause_for_question(&store, &run_id, qid, cmd.log.as_deref())?;
        bail!("run paused due to invalid translated plan")
    }

    append_event(
        &store,
        &run_id,
        &NewEvent::simple("plan_validated", json!({"ok": true})),
        cmd.log.as_deref(),
    )?;

    match review_loop::review_spec(&markdown, &translated) {
        review_loop::SpecReviewOutcome::Approved => {
            append_event(
                &store,
                &run_id,
                &NewEvent::simple("spec_approved", json!({"approved": true})),
                cmd.log.as_deref(),
            )?;
        }
        review_loop::SpecReviewOutcome::Question {
            question_id,
            question,
        } => {
            append_event(
                &store,
                &run_id,
                &NewEvent::simple(
                    "spec_question_opened",
                    json!({"question_id": question_id, "question": question}),
                ),
                cmd.log.as_deref(),
            )?;
            pause_for_question(&store, &run_id, &question_id, cmd.log.as_deref())?;
            bail!("run paused awaiting spec clarification")
        }
    }

    resolve_checks_configuration(
        &store,
        &run_id,
        &cfg,
        &repo_root,
        &cmd.plan_file,
        &markdown,
        &translated,
        cmd.log.as_deref(),
    )?;

    for t in &translated.tasks {
        append_event(
            &store,
            &run_id,
            &NewEvent {
                event_type: "task_registered".to_string(),
                task_id: Some(t.id.clone()),
                actor_role: None,
                actor_id: None,
                attempt: None,
                payload_json: json!({
                    "task_id": t.id,
                    "objective": t.objective,
                    "acceptance": t.acceptance,
                    "dependencies": t.dependencies,
                    "checks": if cfg.trust_plan_checks { t.checks.clone() } else { default_checks() }
                }),
                dedupe_key: Some(format!("task_registered:{}", t.id)),
            },
            cmd.log.as_deref(),
        )?;
    }

    continue_run(&store, &run_id, cmd.log)
}

pub fn list_questions(run_id: &str, state_db: Option<PathBuf>) -> Result<()> {
    let store = EventStore::open(&state_db.unwrap_or_else(default_state_db))?;
    let unresolved = store.unresolved_questions(run_id)?;
    if unresolved.is_empty() {
        println!("No open questions for run {run_id}");
        return Ok(());
    }
    for (id, q) in unresolved {
        println!("{id}: {q}");
    }
    Ok(())
}

pub fn answer_question(
    run_id: &str,
    question_id: &str,
    text: &str,
    state_db: Option<PathBuf>,
) -> Result<()> {
    let store = EventStore::open(&state_db.unwrap_or_else(default_state_db))?;
    let unresolved = store.unresolved_questions(run_id)?;
    if !unresolved.iter().any(|(id, _)| id == question_id) {
        bail!("question {question_id} is not currently open for run {run_id}")
    }

    let run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
    let repo_root = repo_root_for_plan(Path::new(&run.plan_path))?;
    let events = store.list_events(run_id)?;
    let is_checks_question = events.iter().any(|ev| {
        ev.event_type == "checks_question_opened"
            && ev.payload_json.get("question_id").and_then(|v| v.as_str()) == Some(question_id)
    });

    append_event(
        &store,
        run_id,
        &NewEvent::simple(
            "human_input_provided",
            json!({"question_id": question_id, "text": text}),
        ),
        None,
    )?;

    if is_checks_question {
        let commands = if text.trim().eq_ignore_ascii_case("accept") {
            proposed_checks_for_question(&events, question_id)?
        } else {
            let parsed = parse_checks(Some(text));
            if parsed.is_empty() {
                bail!("no checks provided in answer override")
            }
            parsed
        };

        append_event(
            &store,
            run_id,
            &NewEvent::simple(
                "checks_question_resolved",
                json!({"question_id": question_id}),
            ),
            None,
        )?;
        append_event(
            &store,
            run_id,
            &NewEvent::simple(
                "checks_approved",
                json!({
                    "commands": commands,
                    "source": if text.trim().eq_ignore_ascii_case("accept") { "human_accept" } else { "human_override" }
                }),
            ),
            None,
        )?;
        crate::checks::config::save_checks_file(&repo_root, &commands, "human_approved")?;
    } else {
        append_event(
            &store,
            run_id,
            &NewEvent::simple(
                "spec_question_resolved",
                json!({"question_id": question_id}),
            ),
            None,
        )?;

        let is_spec_review_question = question_id.starts_with("spec-q-")
            && question_id != "spec-q-translate"
            && question_id != "spec-q-validate";
        if is_spec_review_question {
            let events_after = store.list_events(run_id)?;
            let has_spec_approval = events_after.iter().any(|ev| ev.event_type == "spec_approved");
            let has_open_spec_questions = events_after.iter().any(|ev| {
                ev.event_type == "spec_question_opened"
                    && ev
                        .payload_json
                        .get("question_id")
                        .and_then(|v| v.as_str())
                        .map(|qid| {
                            !events_after.iter().any(|r| {
                                r.event_type == "spec_question_resolved"
                                    && r.payload_json.get("question_id").and_then(|v| v.as_str())
                                        == Some(qid)
                            })
                        })
                        .unwrap_or(false)
            });
            if !has_spec_approval && !has_open_spec_questions {
                append_event(
                    &store,
                    run_id,
                    &NewEvent::simple(
                        "spec_approved",
                        json!({"approved": true, "source": "human_clarification"}),
                    ),
                    None,
                )?;
            }
        }
    }
    append_event(
        &store,
        run_id,
        &NewEvent::simple("run_resumed", json!({"reason": "human_input_provided"})),
        None,
    )?;

    println!("Recorded answer for {question_id}. Resume with: whence resume --run {run_id}");
    Ok(())
}

pub fn resume_run(run_id: &str, state_db: Option<PathBuf>) -> Result<()> {
    let store = EventStore::open(&state_db.unwrap_or_else(default_state_db))?;
    append_event(
        &store,
        run_id,
        &NewEvent::simple("run_resumed", json!({"reason": "manual_resume"})),
        None,
    )?;
    continue_run(&store, run_id, None)
}

fn continue_run(store: &EventStore, run_id: &str, log: Option<PathBuf>) -> Result<()> {
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
    let cfg: RunConfig = serde_json::from_value(run.config_json.clone())?;
    let plan_path = PathBuf::from(&run.plan_path);
    let repo_root = repo_root_for_plan(&plan_path)?;

    append_attempt_interrupted_for_orphans(store, run_id, log.as_deref())?;
    let events = store.list_events(run_id)?;
    let state = RunProjection::replay(&events);
    if state.terminal.is_some() {
        println!(
            "Run {run_id} already terminal: {}",
            state.terminal.unwrap_or_default()
        );
        return Ok(());
    }

    if !state.open_questions.is_empty() {
        let mut ids = state.open_questions.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        let first_question_id = ids
            .first()
            .map(|s| s.as_str())
            .ok_or_else(|| anyhow!("unresolved questions present but no IDs found"))?;
        pause_for_question(store, run_id, first_question_id, log.as_deref())?;
        bail!("run paused; unresolved questions remain")
    }

    if !state.checks_approved {
        resolve_checks_configuration_on_resume(
            store,
            run_id,
            &cfg,
            &repo_root,
            &plan_path,
            log.as_deref(),
        )?;
        let events_after_gate = store.list_events(run_id)?;
        let state_after_gate = RunProjection::replay(&events_after_gate);
        if !state_after_gate.open_questions.is_empty() {
            let mut ids = state_after_gate
                .open_questions
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            ids.sort();
            let first_question_id = ids
                .first()
                .map(|s| s.as_str())
                .ok_or_else(|| anyhow!("unresolved questions present but no IDs found"))?;
            pause_for_question(store, run_id, first_question_id, log.as_deref())?;
            bail!("run paused; unresolved questions remain")
        }
    }

    if state.spec_approved {
        ensure_tasks_registered_on_resume(store, run_id, &run, &cfg, log.as_deref())?;
    }

    let spl_path = PathBuf::from(&run.spl_plan_path);
    if !spl_path.exists() {
        regenerate_plan_spl_if_missing(store, run_id, &run, log.as_deref())?;
    }

    let plan_spl = std::fs::read_to_string(&run.spl_plan_path)
        .with_context(|| format!("read SPL plan from {}", run.spl_plan_path))?;

    let work = r#loop::LoopInput {
        run_id: run_id.to_string(),
        cfg,
        base_dir: repo_root,
        plan_spl,
        ndjson_log: log,
    };
    let outcome = r#loop::run_supervisor_loop(store, work)?;

    match outcome.as_str() {
        "run_completed" => store.update_run_status(run_id, "completed")?,
        "run_failed" => store.update_run_status(run_id, "failed")?,
        "run_cancelled" => store.update_run_status(run_id, "cancelled")?,
        _ => {}
    }

    println!("Run {run_id} finished with {outcome}");
    Ok(())
}

fn pause_for_question(
    store: &EventStore,
    run_id: &str,
    question_id: &str,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "human_input_requested",
            json!({"question_id": question_id, "command": format!("whence answer --run {run_id} --question {question_id} --text \"...\"")}),
        ),
        ndjson_log,
    )?;
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "run_paused",
            json!({
                "next": [
                    format!("whence questions --run {run_id}"),
                    format!("whence answer --run {run_id} --question {question_id} --text \"...\""),
                    format!("whence resume --run {run_id}")
                ]
            }),
        ),
        ndjson_log,
    )?;
    eprintln!("Run paused. Next commands:");
    eprintln!("  whence questions --run {run_id}");
    eprintln!("  whence answer --run {run_id} --question {question_id} --text \"...\"");
    eprintln!("  whence resume --run {run_id}");
    Ok(())
}

pub(crate) fn append_event(
    store: &EventStore,
    run_id: &str,
    ev: &NewEvent,
    ndjson_log: Option<&Path>,
) -> Result<Option<EventRow>> {
    let history = store.list_events(run_id)?;
    transitions::validate_transition(&history, ev)?;
    let seq = store.append_event(run_id, ev)?;
    if let Some(seq) = seq {
        let inserted = store
            .list_events(run_id)?
            .into_iter()
            .find(|e| e.seq == seq)
            .ok_or_else(|| anyhow!("event sequence {seq} was not readable"))?;
        if let Some(path) = ndjson_log {
            ndjson::mirror_event(path, &inserted)?;
        }
        Ok(Some(inserted))
    } else {
        Ok(None)
    }
}

fn append_attempt_interrupted_for_orphans(
    store: &EventStore,
    run_id: &str,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let events = store.list_events(run_id)?;
    let mut claimed_attempts = Vec::<(String, i64)>::new();
    for ev in &events {
        if ev.event_type == "task_claimed" {
            if let (Some(task_id), Some(attempt)) = (ev.task_id.clone(), ev.attempt) {
                claimed_attempts.push((task_id, attempt));
            }
        }
    }

    for (task_id, attempt) in claimed_attempts {
        let complete = events.iter().any(|ev| {
            ev.task_id.as_deref() == Some(task_id.as_str())
                && ev.attempt == Some(attempt)
                && matches!(
                    ev.event_type.as_str(),
                    "review_found_issues"
                        | "review_approved"
                        | "task_failed_terminal"
                        | "task_closed"
                        | "attempt_interrupted"
                )
        });
        if complete {
            continue;
        }
        append_event(
            store,
            run_id,
            &NewEvent {
                event_type: "attempt_interrupted".to_string(),
                task_id: Some(task_id.clone()),
                actor_role: Some("supervisor".to_string()),
                actor_id: Some("supervisor-recovery".to_string()),
                attempt: Some(attempt),
                payload_json: json!({"reason": "orphaned in-flight attempt detected on resume"}),
                dedupe_key: Some(format!("attempt_interrupted:{task_id}:{attempt}")),
            },
            ndjson_log,
        )?;
    }

    Ok(())
}

fn resolve_checks_configuration(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    repo_root: &Path,
    plan_file: &Path,
    markdown: &str,
    translated: &crate::plan::translator::TranslatedPlan,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    if cfg.checks_from_cli {
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "checks_approved",
                json!({"commands": cfg.checks, "source": "cli"}),
            ),
            ndjson_log,
        )?;
        return Ok(());
    }

    if cfg.use_checks_file && !cfg.reconfigure_checks {
        match crate::checks::config::load_checks_file(repo_root) {
            Ok(Some(file_checks)) => {
                append_event(
                    store,
                    run_id,
                    &NewEvent::simple(
                        "checks_approved",
                        json!({"commands": file_checks, "source": "file"}),
                    ),
                    ndjson_log,
                )?;
                return Ok(());
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("Ignoring invalid checks file and entering checks gate: {err}");
            }
        }
    }

    propose_checks_and_pause(
        store, run_id, cfg, repo_root, plan_file, markdown, translated, ndjson_log,
    )
}

fn resolve_checks_configuration_on_resume(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    repo_root: &Path,
    plan_file: &Path,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    if cfg.checks_from_cli {
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "checks_approved",
                json!({"commands": cfg.checks, "source": "cli_resume"}),
            ),
            ndjson_log,
        )?;
        return Ok(());
    }

    if cfg.use_checks_file && !cfg.reconfigure_checks {
        match crate::checks::config::load_checks_file(repo_root) {
            Ok(Some(file_checks)) => {
                append_event(
                    store,
                    run_id,
                    &NewEvent::simple(
                        "checks_approved",
                        json!({"commands": file_checks, "source": "file_resume"}),
                    ),
                    ndjson_log,
                )?;
                return Ok(());
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("Ignoring invalid checks file and entering checks gate: {err}");
            }
        }
    }

    let markdown = fs::read_to_string(plan_file)
        .with_context(|| format!("read plan file {}", plan_file.display()))?;
    let translated =
        crate::plan::translator::translate_markdown_to_spl(&markdown, &default_checks())
            .context("translate plan for checks proposal on resume")?;
    propose_checks_and_pause(
        store,
        run_id,
        cfg,
        repo_root,
        plan_file,
        &markdown,
        &translated,
        ndjson_log,
    )
}

fn regenerate_plan_spl_if_missing(
    store: &EventStore,
    run_id: &str,
    run: &RunRow,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let markdown = fs::read_to_string(&run.plan_path)
        .with_context(|| format!("read plan file {}", run.plan_path))?;
    let translated = translator::translate_markdown_to_spl(&markdown, &default_checks())
        .context("translate plan during resume SPL regeneration")?;

    if let Some(parent) = Path::new(&run.spl_plan_path).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create SPL parent dir {}", parent.display()))?;
    }
    fs::write(&run.spl_plan_path, &translated.spl)
        .with_context(|| format!("write regenerated SPL {}", run.spl_plan_path))?;

    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "plan_translated",
            json!({"spl_path": run.spl_plan_path, "task_count": translated.tasks.len(), "source": "resume_regenerated"}),
        ),
        ndjson_log,
    )?;

    validate::validate_spl(&translated.spl)
        .and_then(|_| sanity::run_sanity_checks(&translated))
        .context("validate regenerated SPL on resume")?;
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "plan_validated",
            json!({"ok": true, "source": "resume_regenerated"}),
        ),
        ndjson_log,
    )?;

    Ok(())
}

fn ensure_tasks_registered_on_resume(
    store: &EventStore,
    run_id: &str,
    run: &RunRow,
    cfg: &RunConfig,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let events = store.list_events(run_id)?;
    if events.iter().any(|ev| ev.event_type == "task_registered") {
        return Ok(());
    }

    let markdown = fs::read_to_string(&run.plan_path)
        .with_context(|| format!("read plan file {}", run.plan_path))?;
    let translated = translator::translate_markdown_to_spl(&markdown, &default_checks())
        .context("translate plan during resume task registration")?;
    for t in &translated.tasks {
        append_event(
            store,
            run_id,
            &NewEvent {
                event_type: "task_registered".to_string(),
                task_id: Some(t.id.clone()),
                actor_role: None,
                actor_id: None,
                attempt: None,
                payload_json: json!({
                    "task_id": t.id,
                    "objective": t.objective,
                    "acceptance": t.acceptance,
                    "dependencies": t.dependencies,
                    "checks": if cfg.trust_plan_checks { t.checks.clone() } else { default_checks() }
                }),
                dedupe_key: Some(format!("task_registered:{}", t.id)),
            },
            ndjson_log,
        )?;
    }
    Ok(())
}

fn propose_checks_and_pause(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    repo_root: &Path,
    plan_file: &Path,
    markdown: &str,
    translated: &crate::plan::translator::TranslatedPlan,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let provider = provider_for(&cfg.agent)?;
    let prompt = packet::build_checks_proposer_prompt(
        repo_root,
        plan_file,
        markdown,
        translated,
        read_optional_file(&repo_root.join("AGENTS.md")),
        read_optional_file(&repo_root.join("CLAUDE.md")),
    );

    let worktree = run_artifact_dir(repo_root, run_id)
        .join("checks-proposal")
        .join("attempt1");
    fs::create_dir_all(&worktree)?;
    let res = provider.run(AgentRequest {
        role: "checks-proposer".to_string(),
        task_id: "__checks__".to_string(),
        attempt: 1,
        worktree_path: worktree,
        prompt,
        timeout: Duration::from_secs(10 * 60),
    })?;
    let proposed = res
        .structured_output
        .as_ref()
        .and_then(|v| v.get("commands"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default_checks);

    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "checks_proposed",
            json!({"commands": proposed, "source": "agent_proposal"}),
        ),
        ndjson_log,
    )?;

    let qid = "checks-q-1";
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "checks_question_opened",
            json!({
                "question_id": qid,
                "question": "Approve proposed checks?",
                "proposed_commands": proposed
            }),
        ),
        ndjson_log,
    )?;

    eprintln!("Proposed checks:");
    for (i, cmd) in proposed.iter().enumerate() {
        eprintln!("  {}. {}", i + 1, cmd);
    }
    pause_for_question(store, run_id, qid, ndjson_log)?;
    bail!("run paused awaiting checks approval")
}

fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn proposed_checks_for_question(events: &[EventRow], question_id: &str) -> Result<Vec<String>> {
    let ev = events
        .iter()
        .rev()
        .find(|ev| {
            ev.event_type == "checks_question_opened"
                && ev.payload_json.get("question_id").and_then(|v| v.as_str()) == Some(question_id)
        })
        .ok_or_else(|| anyhow!("no checks proposal found for question {question_id}"))?;

    let checks = ev
        .payload_json
        .get("proposed_commands")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if checks.is_empty() {
        bail!("question {question_id} has no proposed checks")
    }
    Ok(checks)
}

fn repo_root_for_plan(plan_file: &Path) -> Result<PathBuf> {
    let p = plan_file
        .canonicalize()
        .with_context(|| format!("resolve plan path {}", plan_file.display()))?;
    Ok(p.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("cannot derive repo root from {}", p.display()))?)
}

pub(crate) fn default_checks() -> Vec<String> {
    vec!["true".to_string()]
}

pub(crate) fn parse_checks(raw: Option<&str>) -> Vec<String> {
    let checks = raw
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    checks
}

pub(crate) fn run_artifact_dir(base: &Path, run_id: &str) -> PathBuf {
    base.join(".whence").join("runs").join(run_id)
}

fn resolve_resume_run_id(store: &EventStore, explicit: Option<&str>) -> Result<String> {
    if let Some(id) = explicit {
        return Ok(id.to_string());
    }
    let candidates = store.list_resumable_run_ids()?;
    match candidates.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("no resumable runs found; provide a plan file without --resume"),
        _ => bail!(
            "multiple resumable runs found: {}. Re-run with --run-id <id>",
            candidates.join(", ")
        ),
    }
}

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}
