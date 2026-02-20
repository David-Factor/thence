use crate::events::projector::RunProjection;
use crate::policy::spindle_bridge::PolicySnapshot;

pub fn next_claimable_task(
    run: &RunProjection,
    policy: &PolicySnapshot,
    max_attempts: i64,
) -> Option<String> {
    let mut ids = run.tasks.keys().cloned().collect::<Vec<_>>();
    ids.sort();
    ids.into_iter().find(|id| {
        run.tasks
            .get(id)
            .map(|t| policy.claimable.contains(id) && t.attempts < max_attempts)
            .unwrap_or(false)
    })
}
