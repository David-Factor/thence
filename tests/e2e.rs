use std::fs;
use tempfile::tempdir;
use thence::events::NewEvent;
use thence::events::store::{EventStore, RunRow};
use thence::run::{RunCommand, answer_question, execute_run, list_questions, resume_run};

fn test_run_id(prefix: &str) -> String {
    format!("{}-{}", prefix, uuid::Uuid::new_v4())
}

fn write_repo_config(repo_root: &std::path::Path, body: &str) {
    let path = repo_root.join(".thence").join("config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

#[test]
fn end_to_end_happy_path_completes() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(
        &plan_path,
        "- [ ] task-a: implement feature\n- [ ] task-b: verify behavior | deps=task-a",
    )
    .unwrap();

    let run_id = test_run_id("happy");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "task_closed")
            .count(),
        2
    );
    assert!(!events.iter().any(|e| e.event_type == "run_failed"));
}

#[test]
fn prose_spec_translates_and_completes() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("spec.md");
    let db_path = tmp.path().join("state.db");
    fs::write(
        &plan_path,
        "# Feature Spec\nImplement a tiny auth flow with validation.\nInclude implementation and review quality checks.",
    )
    .unwrap();

    let run_id = test_run_id("prose");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "plan_translated"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
}

#[test]
fn config_only_checks_resolves_without_question_pause() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();
    write_repo_config(
        tmp.path(),
        r#"
version = 1
[checks]
commands = ["true"]
"#,
    );

    let run_id = test_run_id("config-checks");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 1,
        reviewers: 1,
        checks: None,
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let events = EventStore::open(&db_path)
        .unwrap()
        .list_events(&run_id)
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "checks_approved"));
    assert!(
        events
            .iter()
            .all(|e| e.event_type != "checks_question_opened")
    );
    assert!(events.iter().all(|e| e.event_type != "checks_proposed"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
}

#[test]
fn cli_checks_override_config_checks() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();
    write_repo_config(
        tmp.path(),
        r#"
version = 1
[checks]
commands = ["false"]
"#,
    );

    let run_id = test_run_id("cli-over-config");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 1,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let events = EventStore::open(&db_path)
        .unwrap()
        .list_events(&run_id)
        .unwrap();
    let checks_event = events
        .iter()
        .find(|e| e.event_type == "checks_approved")
        .expect("missing checks_approved");
    let commands = checks_event
        .payload_json
        .get("commands")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(commands, vec![serde_json::json!("true")]);
}

#[test]
fn non_codex_agent_is_rejected() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();

    let err = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "claude".to_string(),
        workers: 1,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(test_run_id("bad-agent")),
        state_db: Some(tmp.path().join("state.db")),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("only `codex` supported in this version"));
}

#[test]
fn reviewer_prompt_override_is_written_to_reviewer_capsule() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();
    write_repo_config(
        tmp.path(),
        r#"
version = 1
[checks]
commands = ["true"]
[prompts]
reviewer = "Return strict JSON with approved/findings only."
"#,
    );

    let run_id = test_run_id("reviewer-prompt");
    execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 1,
        reviewers: 1,
        checks: None,
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let events = EventStore::open(&plan_path.parent().unwrap().join("state.db"))
        .unwrap()
        .list_events(&run_id)
        .unwrap();
    let capsule_path = events
        .iter()
        .find(|e| e.event_type == "review_requested")
        .and_then(|e| e.payload_json.get("capsule_path"))
        .and_then(|v| v.as_str())
        .expect("missing reviewer capsule path");
    let raw = fs::read_to_string(capsule_path).unwrap();
    assert!(raw.contains("Return strict JSON with approved/findings only."));
}

#[test]
fn ambiguity_pauses_and_can_resume() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: This spec is ambiguous ???").unwrap();

    let run_id = test_run_id("paused");
    let err = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("paused"));

    list_questions(&run_id, Some(db_path.clone())).unwrap();
    answer_question(&run_id, "spec-q-1", "Clarified", Some(db_path.clone())).unwrap();
    resume_run(&run_id, Some(db_path.clone())).unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "run_paused"));
    assert!(events.iter().any(|e| e.event_type == "run_resumed"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
}

#[test]
fn dedupe_key_prevents_duplicate_event() {
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("state.db");
    let store = EventStore::open(&db_path).unwrap();

    let run_id = test_run_id("dedupe");
    store
        .create_run(&RunRow {
            id: run_id.clone(),
            plan_path: "plan.md".to_string(),
            plan_sha256: "abc".to_string(),
            spl_plan_path: "plan.spl".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "running".to_string(),
            config_json: serde_json::json!({}),
        })
        .unwrap();

    let ev = NewEvent {
        event_type: "task_registered".to_string(),
        task_id: Some("t1".to_string()),
        actor_role: None,
        actor_id: None,
        attempt: None,
        payload_json: serde_json::json!({"task_id": "t1"}),
        dedupe_key: Some("task_registered:t1".to_string()),
    };

    let first = store.append_event(&run_id, &ev).unwrap();
    let second = store.append_event(&run_id, &ev).unwrap();
    assert!(first.is_some());
    assert!(second.is_none());
}

#[test]
fn review_question_uses_returned_question_id() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: ").unwrap();

    let run_id = test_run_id("question-id");
    let err = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("paused"));

    answer_question(&run_id, "spec-q-2", "filled objective", Some(db_path)).unwrap();
}

#[test]
fn implementer_nonzero_exit_blocks_review_and_close() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: break build [impl-fail]").unwrap();

    let run_id = test_run_id("impl-fail");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "run_failed"));
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "task_failed_terminal")
    );
    assert!(events.iter().all(|e| e.event_type != "review_requested"));
    assert!(events.iter().all(|e| e.event_type != "task_closed"));
}

#[test]
fn reviewer_missing_output_fails_closed() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(
        &plan_path,
        "- [ ] task-a: reviewer output absent [missing-review-output]",
    )
    .unwrap();

    let run_id = test_run_id("review-missing");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "review_requested"));
    assert!(events.iter().any(|e| e.event_type == "review_found_issues"));
    let invalid_reviewer = events
        .iter()
        .find(|e| e.event_type == "review_found_issues")
        .expect("missing review_found_issues");
    assert!(
        invalid_reviewer
            .payload_json
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("invalid reviewer output")
    );
    assert!(events.iter().all(|e| e.event_type != "review_approved"));
    assert!(events.iter().all(|e| e.event_type != "task_closed"));
}

#[test]
fn reviewer_findings_persist_and_reach_next_implementer_attempt() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    let agent_path = tmp.path().join("agent.sh");
    fs::write(
        &plan_path,
        "- [ ] task-a: implement feature with rework loop",
    )
    .unwrap();
    fs::write(
        &agent_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
case "${THENCE_ROLE:-}" in
  plan-translator)
    cat > "${THENCE_RESULT_FILE}" <<'JSON'
{"spl":"(given (task task-a))\n(given (ready task-a))\n","tasks":[{"id":"task-a","objective":"implement feature with rework loop","acceptance":"Complete objective: implement feature with rework loop","dependencies":[],"checks":["true"]}]}
JSON
    ;;
  implementer)
    if [ "${THENCE_ATTEMPT:-1}" = "1" ]; then
      echo '{"submitted":true}' > "${THENCE_RESULT_FILE}"
    else
      if grep -q "must-handle-edge-case" "${THENCE_CAPSULE_FILE}"; then
        echo '{"submitted":true}' > "${THENCE_RESULT_FILE}"
      else
        echo '{"submitted":false}' > "${THENCE_RESULT_FILE}"
      fi
    fi
    ;;
  reviewer)
    if [ "${THENCE_ATTEMPT:-1}" = "1" ]; then
      cat > "${THENCE_RESULT_FILE}" <<'JSON'
{"approved":false,"findings":["must-handle-edge-case","add-regression-test"]}
JSON
    else
      echo '{"approved":true,"findings":[]}' > "${THENCE_RESULT_FILE}"
    fi
    ;;
  checks-proposer) echo '{"commands":["true"],"rationale":"ok"}' > "${THENCE_RESULT_FILE}" ;;
  *) echo '{"submitted":true}' > "${THENCE_RESULT_FILE}" ;;
esac
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&agent_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&agent_path, perms).unwrap();
    }
    write_repo_config(
        tmp.path(),
        &format!(
            "version = 1\n[agent]\nprovider = \"codex\"\ncommand = \"bash {}\"\n[checks]\ncommands = [\"true\"]\n",
            agent_path.display()
        ),
    );

    let run_id = test_run_id("findings-forward");
    execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 1,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: false,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();

    let findings_event = events
        .iter()
        .find(|e| e.event_type == "review_found_issues" && e.attempt == Some(1))
        .expect("missing review_found_issues for attempt 1");
    let findings = findings_event
        .payload_json
        .get("findings")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        findings
            .iter()
            .any(|v| v.as_str() == Some("must-handle-edge-case"))
    );

    assert!(
        events
            .iter()
            .any(|e| e.event_type == "task_claimed" && e.attempt == Some(2))
    );
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "review_approved" && e.attempt == Some(2))
    );
    assert!(events.iter().any(|e| e.event_type == "task_closed"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));

    let capsule = plan_path
        .parent()
        .unwrap()
        .join(".thence")
        .join("runs")
        .join(&run_id)
        .join("capsules")
        .join("task-a")
        .join("attempt2")
        .join("implementer.json");
    let capsule_raw = fs::read_to_string(capsule).unwrap();
    assert!(capsule_raw.contains("must-handle-edge-case"));
}

#[test]
fn duplicate_sanitized_task_ids_pause_translation() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: one\n- [ ] task_a: two").unwrap();

    let run_id = test_run_id("dup-id");
    let err = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("translation failure"));

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.event_type == "spec_question_opened")
    );
    assert!(events.iter().any(|e| {
        e.event_type == "human_input_requested"
            && e.payload_json.get("question_id").and_then(|v| v.as_str())
                == Some("spec-q-translate")
    }));
}

#[test]
fn resume_with_open_question_uses_real_question_id() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: ").unwrap();

    let run_id = test_run_id("resume-qid");
    let _ = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    });

    let err = resume_run(&run_id, Some(db_path.clone())).unwrap_err();
    assert!(format!("{err}").contains("paused"));

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    let latest_human_input_requested = events
        .iter()
        .rev()
        .find(|e| e.event_type == "human_input_requested")
        .expect("expected human_input_requested");
    assert_eq!(
        latest_human_input_requested
            .payload_json
            .get("question_id")
            .and_then(|v| v.as_str()),
        Some("spec-q-2")
    );
}

#[test]
fn missing_checks_fails_fast() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();

    let err = execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: None,
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(test_run_id("checks-gate")),
        state_db: Some(db_path),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("No checks configured"));
}

#[test]
fn translation_pause_resume_regenerates_spl_and_completes() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: one\n- [ ] task_a: two").unwrap();

    let run_id = test_run_id("translate-resume");
    let err = execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("translation failure"));

    // Fix plan after pause and resume same run.
    fs::write(
        &plan_path,
        "- [ ] task-a: one\n- [ ] task-b: two | deps=task-a",
    )
    .unwrap();
    answer_question(
        &run_id,
        "spec-q-translate",
        "fixed plan",
        Some(db_path.clone()),
    )
    .unwrap();
    resume_run(&run_id, Some(db_path.clone())).unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let run = store.get_run(&run_id).unwrap().expect("run row");
    let events = store.list_events(&run_id).unwrap();
    assert!(std::path::Path::new(&run.spl_plan_path).exists());
    assert!(events.iter().any(|e| e.event_type == "plan_translated"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
}

#[test]
fn resume_retranslates_when_translated_plan_missing() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: clarify behavior ???").unwrap();

    let run_id = test_run_id("resume-missing-translated");
    let err = execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("paused"));
    answer_question(&run_id, "spec-q-1", "clarified", Some(db_path.clone())).unwrap();

    let translated_path = plan_path
        .parent()
        .unwrap()
        .join(".thence")
        .join("runs")
        .join(&run_id)
        .join("translated_plan.json");
    if translated_path.exists() {
        fs::remove_file(translated_path).unwrap();
    }

    resume_run(&run_id, Some(db_path.clone())).unwrap();
    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "plan_translated"));
    assert!(events.iter().any(|e| e.event_type == "task_registered"));
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
}

#[test]
fn resume_refreshes_agent_command_before_initial_translation() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    let agent_path = tmp.path().join("agent.sh");
    fs::write(&plan_path, "- [ ] task-a: implement feature").unwrap();
    write_repo_config(
        tmp.path(),
        r#"
version = 1
[agent]
provider = "codex"
command = "missing-codex-command"
[checks]
commands = ["true"]
"#,
    );

    let run_id = test_run_id("refresh-agent-command");
    let err = execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 1,
        reviewers: 1,
        checks: None,
        simulate: false,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("paused"));

    fs::write(
        &agent_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
case "${THENCE_ROLE:-}" in
  plan-translator)
    cat > "${THENCE_RESULT_FILE}" <<'JSON'
{"spl":"(given (task task-a))\n(given (ready task-a))\n","tasks":[{"id":"task-a","objective":"implement feature","acceptance":"Complete objective: implement feature","dependencies":[],"checks":["true"]}]}
JSON
    ;;
  implementer) echo '{"submitted":true}' > "${THENCE_RESULT_FILE}" ;;
  reviewer) echo '{"approved":true,"findings":[]}' > "${THENCE_RESULT_FILE}" ;;
  *) echo '{"submitted":true}' > "${THENCE_RESULT_FILE}" ;;
esac
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&agent_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&agent_path, perms).unwrap();
    }
    write_repo_config(
        tmp.path(),
        &format!(
            "version = 1\n[agent]\nprovider = \"codex\"\ncommand = \"bash {}\"\n[checks]\ncommands = [\"true\"]\n",
            agent_path.display()
        ),
    );

    answer_question(&run_id, "spec-q-translate", "retry", Some(db_path.clone())).unwrap();
    resume_run(&run_id, Some(db_path.clone())).unwrap();

    let events = EventStore::open(&db_path)
        .unwrap()
        .list_events(&run_id)
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "run_completed"));
    let translate_question_count = events
        .iter()
        .filter(|e| {
            e.event_type == "spec_question_opened"
                && e.payload_json.get("question_id").and_then(|v| v.as_str())
                    == Some("spec-q-translate")
        })
        .count();
    assert_eq!(translate_question_count, 1);
}

#[test]
fn translate_answer_does_not_bypass_spec_review_gate() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "- [ ] task-a: one\n- [ ] task_a: two").unwrap();

    let run_id = test_run_id("translate-no-bypass");
    let err = execute_run(RunCommand {
        plan_file: plan_path.clone(),
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: true,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap_err();
    assert!(format!("{err}").contains("translation failure"));

    // Fix translation issue, but keep ambiguity marker that should be caught by review gate.
    fs::write(
        &plan_path,
        "- [ ] task-a: unclear behavior ???\n- [ ] task-b: follow up | deps=task-a",
    )
    .unwrap();
    answer_question(
        &run_id,
        "spec-q-translate",
        "retry translation",
        Some(db_path.clone()),
    )
    .unwrap();
    let err = resume_run(&run_id, Some(db_path.clone())).unwrap_err();
    assert!(format!("{err}").contains("paused"));

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| {
        e.event_type == "spec_question_opened"
            && e.payload_json.get("question_id").and_then(|v| v.as_str()) == Some("spec-q-1")
    }));
    assert!(!events.iter().any(|e| e.event_type == "spec_approved"));
    assert!(!events.iter().any(|e| e.event_type == "checks_approved"));
    assert!(!events.iter().any(|e| e.event_type == "task_registered"));
}

#[test]
fn subprocess_invalid_reviewer_output_fails_closed() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    let agent_path = tmp.path().join("agent.sh");
    fs::write(&plan_path, "- [ ] task-a: run reviewer invalid output").unwrap();
    fs::write(
        &agent_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
case "${THENCE_ROLE:-}" in
  plan-translator)
    cat > "${THENCE_RESULT_FILE}" <<'JSON'
{"spl":"(given (task task-a))\n(given (ready task-a))\n","tasks":[{"id":"task-a","objective":"run reviewer invalid output","acceptance":"Complete objective: run reviewer invalid output","dependencies":[],"checks":["true"]}]}
JSON
    ;;
  implementer) echo '{"submitted":true}' > "${THENCE_RESULT_FILE}" ;;
  reviewer) echo '{' > "${THENCE_RESULT_FILE}" ;;
  checks-proposer) echo '{"commands":["true"],"rationale":"ok"}' > "${THENCE_RESULT_FILE}" ;;
  *) echo '{"submitted":true}' > "${THENCE_RESULT_FILE}" ;;
esac
"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&agent_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&agent_path, perms).unwrap();
    }
    write_repo_config(
        tmp.path(),
        &format!(
            "version = 1\n[agent]\nprovider = \"codex\"\ncommand = \"bash {}\"\n[checks]\ncommands = [\"true\"]\n",
            agent_path.display()
        ),
    );

    let run_id = test_run_id("invalid-reviewer-json");
    execute_run(RunCommand {
        plan_file: plan_path,
        agent: "codex".to_string(),
        workers: 2,
        reviewers: 1,
        checks: Some("true".to_string()),
        simulate: false,
        log: None,
        resume: false,
        run_id: Some(run_id.clone()),
        state_db: Some(db_path.clone()),
        allow_partial_completion: false,
        trust_plan_checks: false,
        interactive: false,
        attempt_timeout_secs: None,
        debug_dump_spl: None,
    })
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    let events = store.list_events(&run_id).unwrap();
    assert!(events.iter().any(|e| e.event_type == "review_requested"));
    assert!(events.iter().any(|e| e.event_type == "review_found_issues"));
    assert!(events.iter().all(|e| e.event_type != "review_approved"));
    assert!(events.iter().all(|e| e.event_type != "task_closed"));
}

#[test]
fn resume_blocks_when_orphan_attempt_has_fresh_active_lease() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "Implement a tiny parser with tests.").unwrap();

    let run_id = test_run_id("fresh-lease");
    let run_dir = plan_path
        .parent()
        .unwrap()
        .join(".thence")
        .join("runs")
        .join(&run_id);
    fs::create_dir_all(&run_dir).unwrap();
    let spl_path = run_dir.join("plan.spl");
    fs::write(&spl_path, "(given (task task-a))\n(given (ready task-a))\n").unwrap();
    fs::write(
        run_dir.join("spec.md"),
        "Implement a tiny parser with tests.",
    )
    .unwrap();
    fs::write(
        run_dir.join("translated_plan.json"),
        r#"{
  "tasks": [
    {"id":"task-a","objective":"build parser","acceptance":"done","dependencies":[],"checks":["true"]}
  ],
  "spl": "(given (task task-a))\n(given (ready task-a))\n"
}"#,
    )
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    store
        .create_run(&RunRow {
            id: run_id.clone(),
            plan_path: plan_path.display().to_string(),
            plan_sha256: "abc".to_string(),
            spl_plan_path: spl_path.display().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "running".to_string(),
            config_json: serde_json::json!({
                "agent": "codex",
                "workers": 1,
                "reviewers": 1,
                "checks": ["true"],
                "checks_from_cli": true,
                "simulate": true,
                "allow_partial_completion": false,
                "trust_plan_checks": false,
                "interactive": false,
                "max_attempts": 3,
                "check_timeout_secs": 60,
                "attempt_timeout_secs": 120
            }),
        })
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("run_started", serde_json::json!({})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("spec_approved", serde_json::json!({"approved": true})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("checks_approved", serde_json::json!({"commands": ["true"]})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent {
                event_type: "task_registered".to_string(),
                task_id: Some("task-a".to_string()),
                actor_role: None,
                actor_id: None,
                attempt: None,
                payload_json: serde_json::json!({
                    "task_id": "task-a",
                    "objective": "build parser",
                    "acceptance": "done",
                    "dependencies": [],
                    "checks": ["true"]
                }),
                dedupe_key: Some("task_registered:task-a".to_string()),
            },
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent {
                event_type: "task_claimed".to_string(),
                task_id: Some("task-a".to_string()),
                actor_role: Some("implementer".to_string()),
                actor_id: Some("impl-1".to_string()),
                attempt: Some(1),
                payload_json: serde_json::json!({"attempt": 1}),
                dedupe_key: None,
            },
        )
        .unwrap();

    let lease_path = run_dir
        .join("leases")
        .join("task-a")
        .join("attempt1")
        .join("implementer.json");
    fs::create_dir_all(lease_path.parent().unwrap()).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    fs::write(
        &lease_path,
        serde_json::json!({
            "version": 1,
            "run_id": run_id.clone(),
            "task_id": "task-a",
            "attempt": 1,
            "role": "implementer",
            "owner_pid": std::process::id(),
            "started_at": now,
            "last_seen_at": chrono::Utc::now().to_rfc3339(),
            "state": "active"
        })
        .to_string(),
    )
    .unwrap();

    let err = resume_run(&run_id, Some(db_path)).unwrap_err();
    assert!(format!("{err}").contains("active lease"));
}

#[test]
fn resume_interrupts_stale_orphan_attempt_lease() {
    let tmp = tempdir().unwrap();
    let plan_path = tmp.path().join("plan.md");
    let db_path = tmp.path().join("state.db");
    fs::write(&plan_path, "Implement a tiny parser with tests.").unwrap();

    let run_id = test_run_id("stale-lease");
    let run_dir = plan_path
        .parent()
        .unwrap()
        .join(".thence")
        .join("runs")
        .join(&run_id);
    fs::create_dir_all(&run_dir).unwrap();
    let spl_path = run_dir.join("plan.spl");
    fs::write(&spl_path, "(given (task task-a))\n(given (ready task-a))\n").unwrap();
    fs::write(
        run_dir.join("spec.md"),
        "Implement a tiny parser with tests.",
    )
    .unwrap();
    fs::write(
        run_dir.join("translated_plan.json"),
        r#"{
  "tasks": [
    {"id":"task-a","objective":"build parser","acceptance":"done","dependencies":[],"checks":["true"]}
  ],
  "spl": "(given (task task-a))\n(given (ready task-a))\n"
}"#,
    )
    .unwrap();

    let store = EventStore::open(&db_path).unwrap();
    store
        .create_run(&RunRow {
            id: run_id.clone(),
            plan_path: plan_path.display().to_string(),
            plan_sha256: "abc".to_string(),
            spl_plan_path: spl_path.display().to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            status: "running".to_string(),
            config_json: serde_json::json!({
                "agent": "codex",
                "workers": 1,
                "reviewers": 1,
                "checks": ["true"],
                "checks_from_cli": true,
                "simulate": true,
                "allow_partial_completion": false,
                "trust_plan_checks": false,
                "interactive": false,
                "max_attempts": 3,
                "check_timeout_secs": 60,
                "attempt_timeout_secs": 120
            }),
        })
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("run_started", serde_json::json!({})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("spec_approved", serde_json::json!({"approved": true})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent::simple("checks_approved", serde_json::json!({"commands": ["true"]})),
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent {
                event_type: "task_registered".to_string(),
                task_id: Some("task-a".to_string()),
                actor_role: None,
                actor_id: None,
                attempt: None,
                payload_json: serde_json::json!({
                    "task_id": "task-a",
                    "objective": "build parser",
                    "acceptance": "done",
                    "dependencies": [],
                    "checks": ["true"]
                }),
                dedupe_key: Some("task_registered:task-a".to_string()),
            },
        )
        .unwrap();
    store
        .append_event(
            &run_id,
            &NewEvent {
                event_type: "task_claimed".to_string(),
                task_id: Some("task-a".to_string()),
                actor_role: Some("implementer".to_string()),
                actor_id: Some("impl-1".to_string()),
                attempt: Some(1),
                payload_json: serde_json::json!({"attempt": 1}),
                dedupe_key: None,
            },
        )
        .unwrap();

    let lease_path = run_dir
        .join("leases")
        .join("task-a")
        .join("attempt1")
        .join("implementer.json");
    fs::create_dir_all(lease_path.parent().unwrap()).unwrap();
    let stale = (chrono::Utc::now() - chrono::Duration::seconds(300)).to_rfc3339();
    fs::write(
        &lease_path,
        serde_json::json!({
            "version": 1,
            "run_id": run_id.clone(),
            "task_id": "task-a",
            "attempt": 1,
            "role": "implementer",
            "owner_pid": 999999,
            "started_at": stale,
            "last_seen_at": stale,
            "state": "active"
        })
        .to_string(),
    )
    .unwrap();

    resume_run(&run_id, Some(db_path.clone())).unwrap();
    let events = EventStore::open(&db_path)
        .unwrap()
        .list_events(&run_id)
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "attempt_interrupted"));
}
