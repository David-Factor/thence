use thence::events::EventRow;
use thence::events::projector::RunProjection;

#[test]
fn attempt_interrupted_clears_claimed_flag() {
    let events = vec![
        EventRow {
            seq: 1,
            run_id: "r1".to_string(),
            ts: "2026-02-20T00:00:00Z".to_string(),
            event_type: "task_registered".to_string(),
            task_id: Some("t1".to_string()),
            actor_role: None,
            actor_id: None,
            attempt: None,
            payload_json: serde_json::json!({"task_id":"t1","objective":"obj","dependencies":[],"checks":[]}),
            dedupe_key: None,
        },
        EventRow {
            seq: 2,
            run_id: "r1".to_string(),
            ts: "2026-02-20T00:00:01Z".to_string(),
            event_type: "task_claimed".to_string(),
            task_id: Some("t1".to_string()),
            actor_role: Some("implementer".to_string()),
            actor_id: Some("impl-1".to_string()),
            attempt: Some(1),
            payload_json: serde_json::json!({}),
            dedupe_key: None,
        },
        EventRow {
            seq: 3,
            run_id: "r1".to_string(),
            ts: "2026-02-20T00:00:02Z".to_string(),
            event_type: "attempt_interrupted".to_string(),
            task_id: Some("t1".to_string()),
            actor_role: Some("supervisor".to_string()),
            actor_id: Some("supervisor-recovery".to_string()),
            attempt: Some(1),
            payload_json: serde_json::json!({}),
            dedupe_key: None,
        },
    ];

    let state = RunProjection::replay(&events);
    assert!(!state.tasks.get("t1").unwrap().claimed);
}

#[test]
fn checks_question_events_do_not_open_projected_questions() {
    let events = vec![EventRow {
        seq: 1,
        run_id: "r1".to_string(),
        ts: "2026-02-20T00:00:00Z".to_string(),
        event_type: "checks_question_opened".to_string(),
        task_id: None,
        actor_role: None,
        actor_id: None,
        attempt: None,
        payload_json: serde_json::json!({"question_id":"checks-q-1","question":"legacy"}),
        dedupe_key: None,
    }];

    let state = RunProjection::replay(&events);
    assert!(state.open_questions.is_empty());
}
