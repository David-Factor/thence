pub fn attempt_merge(task_objective: &str, attempt: i64) -> bool {
    !(task_objective.contains("[conflict]") && attempt == 1)
}
