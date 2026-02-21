use std::collections::{BTreeMap, HashSet};
use thence::events::projector::{RunProjection, TaskProjection};
use thence::policy::spindle_bridge::derive_policy_state;

#[test]
fn policy_marks_basic_task_claimable() {
    let mut run = RunProjection {
        run_id: "r1".to_string(),
        spec_approved: true,
        checks_approved: true,
        checks_commands: vec!["true".to_string()],
        paused: false,
        terminal: None,
        tasks: BTreeMap::new(),
        open_questions: Default::default(),
    };
    run.tasks.insert(
        "t1".to_string(),
        TaskProjection {
            id: "t1".to_string(),
            objective: "o".to_string(),
            acceptance: "a".to_string(),
            dependencies: vec![],
            required_checks: vec![],
            attempts: 0,
            claimed: false,
            latest_attempt: 0,
            review_approved_attempts: HashSet::new(),
            checks_passed_attempts: HashSet::new(),
            unresolved_findings_attempts: HashSet::new(),
            merged_attempts: HashSet::new(),
            closed: false,
            terminal_failed: false,
        },
    );

    let plan = "(given (task t1))\n(given (ready t1))\n";
    let snap = derive_policy_state(&run, plan).unwrap();
    assert!(snap.claimable.contains("t1"));
}

#[test]
fn policy_marks_dependency_task_claimable_after_dependency_closes() {
    let mut run = RunProjection {
        run_id: "r2".to_string(),
        spec_approved: true,
        checks_approved: true,
        checks_commands: vec!["true".to_string()],
        paused: false,
        terminal: None,
        tasks: BTreeMap::new(),
        open_questions: Default::default(),
    };
    run.tasks.insert(
        "task_a".to_string(),
        TaskProjection {
            id: "task_a".to_string(),
            objective: "a".to_string(),
            acceptance: "a".to_string(),
            dependencies: vec![],
            required_checks: vec![],
            attempts: 1,
            claimed: false,
            latest_attempt: 1,
            review_approved_attempts: HashSet::from([1]),
            checks_passed_attempts: HashSet::from([1]),
            unresolved_findings_attempts: HashSet::new(),
            merged_attempts: HashSet::from([1]),
            closed: true,
            terminal_failed: false,
        },
    );
    run.tasks.insert(
        "task_b".to_string(),
        TaskProjection {
            id: "task_b".to_string(),
            objective: "b".to_string(),
            acceptance: "b".to_string(),
            dependencies: vec!["task_a".to_string()],
            required_checks: vec![],
            attempts: 0,
            claimed: false,
            latest_attempt: 0,
            review_approved_attempts: HashSet::new(),
            checks_passed_attempts: HashSet::new(),
            unresolved_findings_attempts: HashSet::new(),
            merged_attempts: HashSet::new(),
            closed: false,
            terminal_failed: false,
        },
    );

    let plan = r#"
(given (task task_a))
(given (ready task_a))
(given (task task_b))
(always r-ready-task_b (closed task_a) (ready task_b))
"#;
    let snap = derive_policy_state(&run, plan).unwrap();
    assert!(snap.claimable.contains("task_b"));
}
