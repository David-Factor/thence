use crate::plan::translator::TranslatedPlan;
use anyhow::{bail, Result};

pub fn run_sanity_checks(plan: &TranslatedPlan) -> Result<()> {
    if plan.tasks.is_empty() {
        bail!("sanity failed: plan has zero tasks")
    }
    if !plan.tasks.iter().any(|t| t.dependencies.is_empty()) {
        bail!("sanity failed: no initially ready task")
    }
    Ok(())
}
