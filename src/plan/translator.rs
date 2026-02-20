use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub objective: String,
    pub acceptance: String,
    pub dependencies: Vec<String>,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatedPlan {
    pub tasks: Vec<PlanTask>,
    pub spl: String,
}

fn sanitize_ident(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "task".to_string()
    } else {
        out
    }
}

pub fn translate_markdown_to_spl(
    markdown: &str,
    default_checks: &[String],
) -> Result<TranslatedPlan> {
    let mut tasks = Vec::new();
    let mut seen_ids: HashMap<String, String> = HashMap::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- [ ]") {
            continue;
        }
        let body = trimmed.trim_start_matches("- [ ]").trim();
        // Format: task-id: objective | deps=a,b | checks=cmd1,cmd2
        let mut parts = body.split('|').map(str::trim);
        let first = parts.next().unwrap_or("");
        let (id, objective, source_id) = if let Some((id, obj)) = first.split_once(':') {
            (
                sanitize_ident(id.trim()),
                obj.trim().to_string(),
                id.trim().to_string(),
            )
        } else {
            let generated = format!("task{}", tasks.len() + 1);
            (generated.clone(), first.to_string(), generated)
        };
        if let Some(prev) = seen_ids.insert(id.clone(), source_id.clone()) {
            bail!(
                "translation failed: duplicate task ID after sanitization: '{}' (from '{}' and '{}')",
                id,
                prev,
                source_id
            );
        }

        let mut deps = Vec::new();
        let mut checks = default_checks.to_vec();
        for p in parts {
            if let Some(d) = p.strip_prefix("deps=") {
                deps = d
                    .split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(sanitize_ident)
                    .collect();
            }
            if let Some(c) = p.strip_prefix("checks=") {
                checks = c
                    .split(',')
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(ToString::to_string)
                    .collect();
            }
        }

        tasks.push(PlanTask {
            id,
            objective: objective.clone(),
            acceptance: format!("Complete objective: {objective}"),
            dependencies: deps,
            checks,
        });
    }

    if tasks.is_empty() {
        bail!(
            "translation failed: no tasks found in markdown. Use '- [ ] task-id: objective' lines"
        )
    }

    let mut spl = String::from("; generated plan.spl\n");
    for t in &tasks {
        spl.push_str(&format!("(given (task {}))\n", t.id));
        spl.push_str(&format!("(given (has-objective {}))\n", t.id));
        spl.push_str(&format!("(given (has-acceptance {}))\n", t.id));
        if t.dependencies.is_empty() {
            spl.push_str(&format!("(given (ready {}))\n", t.id));
        } else {
            let label = format!("r-ready-{}", t.id);
            let deps = t
                .dependencies
                .iter()
                .map(|d| format!("(closed {})", d))
                .collect::<Vec<_>>();
            let body = if deps.len() == 1 {
                deps[0].clone()
            } else {
                format!("(and {})", deps.join(" "))
            };
            spl.push_str(&format!("(always {} {} (ready {}))\n", label, body, t.id));
        }
    }

    Ok(TranslatedPlan { tasks, spl })
}
