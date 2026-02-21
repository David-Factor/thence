pub(crate) mod lease;
mod r#loop;
pub mod packet;
pub mod scheduler;
mod transitions;

use crate::events::projector::RunProjection;
use crate::events::store::{EventStore, RunRow};
use crate::events::{EventRow, NewEvent};
use crate::logging::ndjson;
use crate::plan::{review_loop, sanity, translator, validate};
use crate::workers::provider::{AgentRequest, provider_for};
use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

const NO_CHECKS_CONFIGURED_ERROR: &str =
    "No checks configured. Set `--checks` or `[checks].commands` in `.thence/config.toml`.";
const DEFAULT_REVIEWER_INSTRUCTION: &str = "Review implementation against objective/acceptance.\nReturn strict JSON with: approved (bool), findings (string[]).";

#[derive(Debug, Clone)]
pub struct RunCommand {
    pub plan_file: PathBuf,
    pub agent: String,
    pub workers: usize,
    pub reviewers: usize,
    pub checks: Option<String>,
    pub simulate: bool,
    pub log: Option<PathBuf>,
    pub resume: bool,
    pub run_id: Option<String>,
    pub state_db: Option<PathBuf>,
    pub allow_partial_completion: bool,
    pub trust_plan_checks: bool,
    pub interactive: bool,
    pub attempt_timeout_secs: Option<u64>,
    pub debug_dump_spl: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunConfig {
    pub agent: String,
    pub workers: usize,
    pub reviewers: usize,
    #[serde(default)]
    pub checks: Vec<String>,
    #[serde(default)]
    pub checks_from_cli: bool,
    #[serde(default)]
    pub simulate: bool,
    pub allow_partial_completion: bool,
    pub trust_plan_checks: bool,
    pub interactive: bool,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: i64,
    #[serde(default = "default_check_timeout_secs")]
    pub check_timeout_secs: u64,
    #[serde(default = "default_attempt_timeout_secs")]
    pub attempt_timeout_secs: u64,
    #[serde(default)]
    pub reviewer_prompt_override: Option<String>,
    #[serde(default)]
    pub agent_command: Option<String>,
    #[serde(default)]
    pub worktree_provision_files: Vec<crate::config::ProvisionedFile>,
}

impl RunConfig {
    pub fn effective_reviewer_instruction(&self) -> &str {
        self.reviewer_prompt_override
            .as_deref()
            .unwrap_or(DEFAULT_REVIEWER_INSTRUCTION)
    }
}

fn default_state_db() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("thence").join("state.db");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("thence")
            .join("state.db");
    }
    PathBuf::from(".thence/state.db")
}

fn default_max_attempts() -> i64 {
    3
}

fn default_check_timeout_secs() -> u64 {
    10 * 60
}

fn default_attempt_timeout_secs() -> u64 {
    45 * 60
}

fn translated_plan_path(run_dir: &Path) -> PathBuf {
    run_dir.join("translated_plan.json")
}

fn frozen_spec_path(run_dir: &Path) -> PathBuf {
    run_dir.join("spec.md")
}

fn write_frozen_spec(run_dir: &Path, markdown: &str) -> Result<PathBuf> {
    let path = frozen_spec_path(run_dir);
    std::fs::write(&path, markdown)
        .with_context(|| format!("write frozen spec {}", path.display()))?;
    Ok(path)
}

fn read_spec_markdown(run_dir: &Path, plan_path: &Path) -> Result<String> {
    let frozen = frozen_spec_path(run_dir);
    if frozen.exists() {
        return fs::read_to_string(&frozen)
            .with_context(|| format!("read frozen spec {}", frozen.display()));
    }
    fs::read_to_string(plan_path).with_context(|| format!("read plan file {}", plan_path.display()))
}

fn translate_spec_with_agent(
    cfg: &RunConfig,
    repo_root: &Path,
    plan_file: &Path,
    markdown: &str,
    run_dir: &Path,
) -> Result<(
    translator::TranslatedPlan,
    crate::workers::provider::AgentResult,
)> {
    let provider = provider_for(&cfg.agent, cfg.simulate, cfg.agent_command.as_deref())?;
    let prompt = packet::build_plan_translator_prompt(
        repo_root,
        plan_file,
        markdown,
        &default_checks(),
        read_optional_file(&repo_root.join("AGENTS.md")),
        read_optional_file(&repo_root.join("CLAUDE.md")),
    );
    let worktree = run_dir.join("plan-translation").join("attempt1");
    fs::create_dir_all(&worktree)?;
    let res = provider.run(AgentRequest {
        role: "plan-translator".to_string(),
        task_id: "__plan__".to_string(),
        attempt: 1,
        worktree_path: worktree,
        prompt,
        env: Vec::new(),
        timeout: Duration::from_secs(20 * 60),
    })?;
    if res.exit_code != 0 {
        bail!(
            "plan-translator exited non-zero (exit_code={}); see logs: stdout={} stderr={}",
            res.exit_code,
            res.stdout_path.display(),
            res.stderr_path.display()
        );
    }
    let structured = res
        .structured_output
        .as_ref()
        .ok_or_else(|| anyhow!("plan-translator did not return structured JSON output"))?;
    let translated = translator::parse_translated_plan_output(structured, &default_checks())?;
    Ok((translated, res))
}

fn register_translated_tasks(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    translated: &translator::TranslatedPlan,
    ndjson_log: Option<&Path>,
) -> Result<()> {
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
    let repo_cfg = crate::config::load_repo_config(&repo_root)?;

    if cmd.agent != "codex" {
        bail!("only `codex` supported in this version");
    }

    let run_id = cmd.run_id.unwrap_or_else(|| Uuid::new_v4().to_string());
    let run_dir = run_artifact_dir(&repo_root, &run_id);
    std::fs::create_dir_all(&run_dir)?;
    let spl_path = run_dir.join("plan.spl");
    let translated_path = translated_plan_path(&run_dir);

    let plan_sha256 = sha256_hex(&markdown);
    let cfg = RunConfig {
        agent: cmd.agent,
        workers: cmd.workers.max(1),
        reviewers: cmd.reviewers.max(1),
        checks: if !cli_checks.is_empty() {
            cli_checks.clone()
        } else {
            repo_cfg
                .as_ref()
                .and_then(|cfg| cfg.checks.as_ref())
                .map(|checks| checks.commands.clone())
                .unwrap_or_default()
        },
        checks_from_cli: !cli_checks.is_empty(),
        simulate: cmd.simulate,
        allow_partial_completion: cmd.allow_partial_completion,
        trust_plan_checks: cmd.trust_plan_checks,
        interactive: cmd.interactive,
        max_attempts: 3,
        check_timeout_secs: 10 * 60,
        attempt_timeout_secs: cmd
            .attempt_timeout_secs
            .unwrap_or_else(default_attempt_timeout_secs),
        reviewer_prompt_override: repo_cfg
            .as_ref()
            .and_then(|cfg| cfg.prompts.as_ref())
            .and_then(|prompts| prompts.reviewer.clone()),
        agent_command: repo_cfg
            .as_ref()
            .and_then(|cfg| cfg.agent.as_ref())
            .and_then(|agent| agent.command.clone()),
        worktree_provision_files: repo_cfg
            .as_ref()
            .and_then(|cfg| cfg.worktree.as_ref())
            .and_then(|worktree| worktree.provision.as_ref())
            .map(|provision| provision.files.clone())
            .unwrap_or_default(),
    };
    ensure_checks_configured(&cfg.checks)?;

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

    let (translated, translation_res) = match translate_spec_with_agent(
        &cfg,
        &repo_root,
        &cmd.plan_file,
        &markdown,
        &run_dir,
    ) {
        Ok(result) => result,
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
    std::fs::write(&spl_path, &translated.spl)
        .with_context(|| format!("write translated SPL {}", spl_path.display()))?;
    translator::save_translated_plan(&translated_path, &translated)?;
    let frozen_spec = write_frozen_spec(&run_dir, &markdown)?;
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
                "translated_plan_path": translated_path,
                "frozen_spec_path": frozen_spec,
                "task_count": translated.tasks.len(),
                "source": "agent",
                "translator_stdout_path": translation_res.stdout_path,
                "translator_stderr_path": translation_res.stderr_path
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

    resolve_checks_configuration(&store, &run_id, &cfg, cmd.log.as_deref())?;

    register_translated_tasks(&store, &run_id, &cfg, &translated, cmd.log.as_deref())?;

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

    let _run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run not found: {run_id}"))?;

    append_event(
        &store,
        run_id,
        &NewEvent::simple(
            "human_input_provided",
            json!({"question_id": question_id, "text": text}),
        ),
        None,
    )?;

    append_event(
        &store,
        run_id,
        &NewEvent::simple(
            "spec_question_resolved",
            json!({"question_id": question_id}),
        ),
        None,
    )?;

    let is_spec_review_question = is_spec_review_question_id(question_id);
    if is_spec_review_question {
        let events_after = store.list_events(run_id)?;
        let has_spec_approval = events_after
            .iter()
            .any(|ev| ev.event_type == "spec_approved");
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
    append_event(
        &store,
        run_id,
        &NewEvent::simple("run_resumed", json!({"reason": "human_input_provided"})),
        None,
    )?;

    println!("Recorded answer for {question_id}. Resume with: thence resume --run {run_id}");
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

pub fn inspect_run(run_id: &str, state_db: Option<PathBuf>) -> Result<()> {
    let store = EventStore::open(&state_db.unwrap_or_else(default_state_db))?;
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
    let events = store.list_events(run_id)?;
    let state = RunProjection::replay(&events);
    let repo_root = repo_root_for_plan(Path::new(&run.plan_path))?;
    let run_dir = run_artifact_dir(&repo_root, run_id);

    println!("run_id: {}", run.id);
    println!("status: {}", run.status);
    println!("plan_path: {}", run.plan_path);
    println!("spl_path: {}", run.spl_plan_path);
    println!("artifacts_dir: {}", run_dir.display());
    println!(
        "state: spec_approved={} checks_approved={} paused={} terminal={}",
        state.spec_approved,
        state.checks_approved,
        state.paused,
        state.terminal.as_deref().unwrap_or("none")
    );
    let phase = if state.terminal.is_some() {
        "terminal"
    } else if !state.open_questions.is_empty() {
        "paused_for_question"
    } else if !state.spec_approved {
        "spec_gate"
    } else if !state.checks_approved {
        "checks_gate"
    } else if state.tasks.values().any(|t| t.claimed) {
        "implementation_loop"
    } else {
        "scheduler_idle"
    };
    println!("phase: {phase}");

    if let Some(task) = state.tasks.values().find(|t| t.claimed) {
        println!("current: task={} attempt={}", task.id, task.latest_attempt);
    }

    if !state.open_questions.is_empty() {
        println!("open_questions:");
        for (id, q) in &state.open_questions {
            println!("  - {}: {}", id, q);
        }
    }

    let mut latest_findings = BTreeMap::<String, (i64, String)>::new();
    for ev in events.iter().rev() {
        if ev.event_type != "review_found_issues" {
            continue;
        }
        let Some(task_id) = ev.task_id.as_ref() else {
            continue;
        };
        if latest_findings.contains_key(task_id) {
            continue;
        }
        let reason = ev
            .payload_json
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("review findings")
            .to_string();
        latest_findings.insert(task_id.clone(), (ev.attempt.unwrap_or(0), reason));
    }
    if !latest_findings.is_empty() {
        println!("latest_findings:");
        for (task, (attempt, reason)) in latest_findings {
            println!("  - task={} attempt={} reason={}", task, attempt, reason);
        }
    }

    let mut seen_attempts = std::collections::HashSet::<(String, i64)>::new();
    let mut attempts = Vec::<(String, i64)>::new();
    for ev in events.iter().rev() {
        if let (Some(task_id), Some(attempt)) = (ev.task_id.as_ref(), ev.attempt) {
            let key = (task_id.clone(), attempt);
            if seen_attempts.insert(key.clone()) {
                attempts.push(key);
            }
        }
        if attempts.len() >= 8 {
            break;
        }
    }

    if !attempts.is_empty() {
        println!("attempt_artifacts:");
        for (task_id, attempt) in attempts {
            println!("  - task={} attempt={}", task_id, attempt);
            for role in ["implementer", "reviewer"] {
                let artifacts = discover_attempt_artifacts(&run_dir, &task_id, attempt, role)?;
                for path in artifacts {
                    println!("      {}: {}", role, path.display());
                }
            }
        }
    }

    Ok(())
}

fn continue_run(store: &EventStore, run_id: &str, log: Option<PathBuf>) -> Result<()> {
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run not found: {run_id}"))?;
    let mut cfg: RunConfig = serde_json::from_value(run.config_json.clone())?;
    let plan_path = PathBuf::from(&run.plan_path);
    let repo_root = repo_root_for_plan(&plan_path)?;

    append_attempt_interrupted_for_orphans(store, run_id, &repo_root, log.as_deref())?;
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

    if !state.spec_approved {
        refresh_agent_command_before_initial_translation(
            store, run_id, &repo_root, &events, &mut cfg,
        )?;
        rerun_spec_gate_on_resume(store, run_id, &run, &cfg, &repo_root, log.as_deref())?;
        let events_after_spec = store.list_events(run_id)?;
        let state_after_spec = RunProjection::replay(&events_after_spec);
        if !state_after_spec.open_questions.is_empty() {
            let mut ids = state_after_spec
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

    let events = store.list_events(run_id)?;
    let state = RunProjection::replay(&events);

    if !state.checks_approved {
        resolve_checks_configuration_on_resume(store, run_id, &cfg, log.as_deref())?;
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
        ensure_tasks_registered_on_resume(store, run_id, &run, &cfg, &repo_root, log.as_deref())?;
    }

    let spl_path = PathBuf::from(&run.spl_plan_path);
    if !spl_path.exists() {
        regenerate_plan_spl_if_missing(store, run_id, &cfg, &repo_root, &run, log.as_deref())?;
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

fn refresh_agent_command_before_initial_translation(
    store: &EventStore,
    run_id: &str,
    repo_root: &Path,
    events: &[EventRow],
    cfg: &mut RunConfig,
) -> Result<()> {
    let already_translated = events.iter().any(|ev| ev.event_type == "plan_translated");
    if already_translated {
        return Ok(());
    }

    let latest = crate::config::load_repo_config(repo_root)?
        .and_then(|repo| repo.agent)
        .and_then(|agent| agent.command);
    if latest == cfg.agent_command {
        return Ok(());
    }

    cfg.agent_command = latest;
    store.update_run_config(run_id, &serde_json::to_value(cfg)?)?;
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
            json!({"question_id": question_id, "command": format!("thence answer --run {run_id} --question {question_id} --text \"...\"")}),
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
                    format!("thence questions --run {run_id}"),
                    format!("thence answer --run {run_id} --question {question_id} --text \"...\""),
                    format!("thence resume --run {run_id}")
                ]
            }),
        ),
        ndjson_log,
    )?;
    eprintln!("Run paused. Next commands:");
    eprintln!("  thence questions --run {run_id}");
    eprintln!("  thence answer --run {run_id} --question {question_id} --text \"...\"");
    eprintln!("  thence resume --run {run_id}");
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
    repo_root: &Path,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let events = store.list_events(run_id)?;
    let mut claimed_attempts = Vec::<(String, i64)>::new();
    for ev in &events {
        if ev.event_type == "task_claimed"
            && let (Some(task_id), Some(attempt)) = (ev.task_id.clone(), ev.attempt)
        {
            claimed_attempts.push((task_id, attempt));
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
        let (reason, lease_details) =
            match lease::evaluate_orphan_attempt(repo_root, run_id, &task_id, attempt)? {
                lease::OrphanLeaseDecision::Interrupt { reason, details } => (reason, details),
                lease::OrphanLeaseDecision::LikelyActive { reason, details } => {
                    let details_str = serde_json::to_string_pretty(&details)
                        .unwrap_or_else(|_| details.to_string());
                    bail!("{reason}\nlease_details: {details_str}");
                }
            };
        append_event(
            store,
            run_id,
            &NewEvent {
                event_type: "attempt_interrupted".to_string(),
                task_id: Some(task_id.clone()),
                actor_role: Some("supervisor".to_string()),
                actor_id: Some("supervisor-recovery".to_string()),
                attempt: Some(attempt),
                payload_json: json!({"reason": reason, "lease": lease_details}),
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
    ndjson_log: Option<&Path>,
) -> Result<()> {
    ensure_checks_configured(&cfg.checks)?;
    let source = if cfg.checks_from_cli { "cli" } else { "config" };
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "checks_approved",
            json!({"commands": cfg.checks, "source": source}),
        ),
        ndjson_log,
    )?;
    Ok(())
}

fn load_or_translate_plan_for_run(
    store: &EventStore,
    run_id: &str,
    run: &RunRow,
    cfg: &RunConfig,
    repo_root: &Path,
    ndjson_log: Option<&Path>,
) -> Result<(String, translator::TranslatedPlan)> {
    let run_dir = run_artifact_dir(repo_root, run_id);
    fs::create_dir_all(&run_dir)?;
    let plan_path = Path::new(&run.plan_path);
    let translated_path = translated_plan_path(&run_dir);
    let (markdown, translated, translated_now) = if translated_path.exists() {
        let markdown = read_spec_markdown(&run_dir, plan_path)?;
        if !frozen_spec_path(&run_dir).exists() {
            write_frozen_spec(&run_dir, &markdown)?;
        }
        (
            markdown,
            translator::load_translated_plan(&translated_path)?,
            false,
        )
    } else {
        // When there is no frozen translated plan yet, always translate from the live spec.
        let markdown = fs::read_to_string(plan_path)
            .with_context(|| format!("read plan file {}", plan_path.display()))?;
        let (translated, translation_res) = match translate_spec_with_agent(
            cfg, repo_root, plan_path, &markdown, &run_dir,
        ) {
            Ok(result) => result,
            Err(err) => {
                let qid = "spec-q-translate";
                append_event(
                    store,
                    run_id,
                    &NewEvent::simple(
                        "spec_question_opened",
                        json!({"question_id": qid, "question": format!("Plan translation failed: {err}")}),
                    ),
                    ndjson_log,
                )?;
                pause_for_question(store, run_id, qid, ndjson_log)?;
                bail!("run paused due to translation failure")
            }
        };
        fs::write(&run.spl_plan_path, &translated.spl)
            .with_context(|| format!("write translated SPL {}", run.spl_plan_path))?;
        translator::save_translated_plan(&translated_path, &translated)?;
        let frozen_spec = write_frozen_spec(&run_dir, &markdown)?;
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "plan_translated",
                json!({
                    "spl_path": run.spl_plan_path,
                    "translated_plan_path": translated_path,
                    "frozen_spec_path": frozen_spec,
                    "task_count": translated.tasks.len(),
                    "source": "resume_translated",
                    "translator_stdout_path": translation_res.stdout_path,
                    "translator_stderr_path": translation_res.stderr_path
                }),
            ),
            ndjson_log,
        )?;
        (markdown, translated, true)
    };

    if !Path::new(&run.spl_plan_path).exists() {
        fs::write(&run.spl_plan_path, &translated.spl)
            .with_context(|| format!("write regenerated SPL {}", run.spl_plan_path))?;
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "plan_translated",
                json!({
                    "spl_path": run.spl_plan_path,
                    "translated_plan_path": translated_path,
                    "task_count": translated.tasks.len(),
                    "source": "resume_regenerated_from_frozen"
                }),
            ),
            ndjson_log,
        )?;
    } else {
        // Ensure in-memory object and on-disk SPL remain aligned with frozen JSON.
        let on_disk = fs::read_to_string(&run.spl_plan_path)
            .with_context(|| format!("read SPL plan {}", run.spl_plan_path))?;
        if on_disk != translated.spl {
            fs::write(&run.spl_plan_path, &translated.spl)
                .with_context(|| format!("rewrite SPL from frozen plan {}", run.spl_plan_path))?;
            append_event(
                store,
                run_id,
                &NewEvent::simple(
                    "plan_translated",
                    json!({
                        "spl_path": run.spl_plan_path,
                        "translated_plan_path": translated_path,
                        "task_count": translated.tasks.len(),
                        "source": "resume_reconciled_from_frozen"
                    }),
                ),
                ndjson_log,
            )?;
        }
    }

    if let Err(err) =
        validate::validate_spl(&translated.spl).and_then(|_| sanity::run_sanity_checks(&translated))
    {
        let qid = "spec-q-validate";
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "spec_question_opened",
                json!({"question_id": qid, "question": format!("Plan generation failed: {err}")}),
            ),
            ndjson_log,
        )?;
        pause_for_question(store, run_id, qid, ndjson_log)?;
        bail!("run paused due to invalid translated plan")
    }

    if translated_now {
        append_event(
            store,
            run_id,
            &NewEvent::simple(
                "plan_validated",
                json!({"ok": true, "source": "resume_translated"}),
            ),
            ndjson_log,
        )?;
    }

    Ok((markdown, translated))
}

fn rerun_spec_gate_on_resume(
    store: &EventStore,
    run_id: &str,
    run: &RunRow,
    cfg: &RunConfig,
    repo_root: &Path,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let (markdown, translated) =
        load_or_translate_plan_for_run(store, run_id, run, cfg, repo_root, ndjson_log)?;

    match review_loop::review_spec(&markdown, &translated) {
        review_loop::SpecReviewOutcome::Approved => {
            append_event(
                store,
                run_id,
                &NewEvent::simple(
                    "spec_approved",
                    json!({"approved": true, "source": "resume_spec_gate"}),
                ),
                ndjson_log,
            )?;
        }
        review_loop::SpecReviewOutcome::Question {
            question_id,
            question,
        } => {
            append_event(
                store,
                run_id,
                &NewEvent::simple(
                    "spec_question_opened",
                    json!({"question_id": question_id, "question": question}),
                ),
                ndjson_log,
            )?;
            pause_for_question(store, run_id, &question_id, ndjson_log)?;
            bail!("run paused awaiting spec clarification")
        }
    }

    Ok(())
}

fn resolve_checks_configuration_on_resume(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    ensure_checks_configured(&cfg.checks)?;
    let source = if cfg.checks_from_cli {
        "cli_resume"
    } else {
        "config_resume"
    };
    append_event(
        store,
        run_id,
        &NewEvent::simple(
            "checks_approved",
            json!({"commands": cfg.checks, "source": source}),
        ),
        ndjson_log,
    )?;
    Ok(())
}

fn regenerate_plan_spl_if_missing(
    store: &EventStore,
    run_id: &str,
    cfg: &RunConfig,
    repo_root: &Path,
    run: &RunRow,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let _ = load_or_translate_plan_for_run(store, run_id, run, cfg, repo_root, ndjson_log)?;
    Ok(())
}

fn ensure_tasks_registered_on_resume(
    store: &EventStore,
    run_id: &str,
    run: &RunRow,
    cfg: &RunConfig,
    repo_root: &Path,
    ndjson_log: Option<&Path>,
) -> Result<()> {
    let events = store.list_events(run_id)?;
    if events.iter().any(|ev| ev.event_type == "task_registered") {
        return Ok(());
    }

    let run_dir = run_artifact_dir(&repo_root, run_id);
    let translated_path = translated_plan_path(&run_dir);
    let translated = if translated_path.exists() {
        translator::load_translated_plan(&translated_path)
            .context("load translated plan during resume task registration")?
    } else {
        // Backward-compatible fallback for runs without translated_plan.json.
        let (_, translated) =
            load_or_translate_plan_for_run(store, run_id, run, cfg, repo_root, ndjson_log)?;
        translated
    };
    register_translated_tasks(store, run_id, cfg, &translated, ndjson_log)?;
    Ok(())
}

fn read_optional_file(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn repo_root_for_plan(plan_file: &Path) -> Result<PathBuf> {
    let p = plan_file
        .canonicalize()
        .with_context(|| format!("resolve plan path {}", plan_file.display()))?;
    p.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("cannot derive repo root from {}", p.display()))
}

pub(crate) fn default_checks() -> Vec<String> {
    vec!["true".to_string()]
}

pub(crate) fn parse_checks(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
}

fn ensure_checks_configured(commands: &[String]) -> Result<()> {
    if commands.is_empty() {
        bail!(NO_CHECKS_CONFIGURED_ERROR);
    }
    if commands.iter().any(|c| c.trim().is_empty()) {
        bail!(NO_CHECKS_CONFIGURED_ERROR);
    }
    Ok(())
}

pub(crate) fn run_artifact_dir(base: &Path, run_id: &str) -> PathBuf {
    base.join(".thence").join("runs").join(run_id)
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

fn is_spec_review_question_id(question_id: &str) -> bool {
    question_id.starts_with("spec-q-")
        && question_id != "spec-q-translate"
        && question_id != "spec-q-validate"
}

fn discover_attempt_artifacts(
    run_dir: &Path,
    task_id: &str,
    attempt: i64,
    role: &str,
) -> Result<Vec<PathBuf>> {
    let root = run_dir
        .join("worktrees")
        .join("thence")
        .join(task_id)
        .join(format!("v{attempt}"));
    if !root.exists() {
        return Ok(Vec::new());
    }
    let prefix = format!("{role}_attempt{attempt}");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

pub(crate) fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}
