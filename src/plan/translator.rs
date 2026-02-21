use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spindle_parser::parse_spl;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

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

#[derive(Debug, Clone, Deserialize)]
struct RawTranslatedPlan {
    spl: String,
    tasks: Vec<RawTask>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTask {
    id: String,
    objective: String,
    #[serde(default)]
    acceptance: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    checks: Vec<String>,
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
        let body = if let Some(rest) = trimmed.strip_prefix("- [ ]") {
            rest.trim()
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            rest.trim()
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            rest.trim()
        } else {
            continue;
        };
        if body.is_empty() {
            continue;
        }
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
        let objective = markdown
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .map(ToString::to_string);
        let objective = objective.ok_or_else(|| {
            anyhow!("translation failed: specification is empty; add concrete requirements")
        })?;
        tasks.push(PlanTask {
            id: "task1".to_string(),
            objective: objective.clone(),
            acceptance: format!("Complete objective: {objective}"),
            dependencies: Vec::new(),
            checks: default_checks.to_vec(),
        });
    }

    let mut spl = String::from("; generated plan.spl\n");
    for t in &tasks {
        spl.push_str(&format!("(given (task {}))\n", t.id));
        spl.push_str(&format!("(given (has-objective {}))\n", t.id));
        spl.push_str(&format!("(given (has-acceptance {}))\n", t.id));
        for dep in &t.dependencies {
            spl.push_str(&format!("(given (depends-on {} {}))\n", t.id, dep));
        }
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

pub fn parse_translated_plan_output(
    output: &Value,
    default_checks: &[String],
) -> Result<TranslatedPlan> {
    let raw: RawTranslatedPlan = serde_json::from_value(output.clone()).context(
        "translator output must be a JSON object with keys 'spl' (string) and 'tasks' (array)",
    )?;
    let spl = raw.spl.trim().to_string();
    if spl.is_empty() {
        bail!("translator output has empty 'spl'")
    }
    validate_no_import_directives(&spl)?;

    let mut seen_ids = HashSet::<String>::new();
    let mut tasks = Vec::<PlanTask>::with_capacity(raw.tasks.len());
    for task in raw.tasks {
        let id = task.id.trim().to_string();
        if !is_valid_task_id(&id) {
            bail!("invalid task id '{id}'; allowed chars: [A-Za-z0-9_-]");
        }
        if !seen_ids.insert(id.clone()) {
            bail!("duplicate task id '{id}' in translator output");
        }

        let objective = task.objective.trim().to_string();
        let acceptance = task
            .acceptance
            .unwrap_or_else(|| format!("Complete objective: {objective}"))
            .trim()
            .to_string();

        let mut deps_seen = HashSet::<String>::new();
        let mut dependencies = Vec::<String>::new();
        for dep in task.dependencies {
            let dep = dep.trim().to_string();
            if dep.is_empty() {
                continue;
            }
            if !is_valid_task_id(&dep) {
                bail!("task '{id}' has invalid dependency id '{dep}'");
            }
            if dep == id {
                bail!("task '{id}' cannot depend on itself");
            }
            if deps_seen.insert(dep.clone()) {
                dependencies.push(dep);
            }
        }

        let checks = task
            .checks
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect::<Vec<_>>();
        let checks = if checks.is_empty() {
            default_checks.to_vec()
        } else {
            checks
        };

        tasks.push(PlanTask {
            id,
            objective,
            acceptance,
            dependencies,
            checks,
        });
    }

    if tasks.is_empty() {
        bail!("translator output has empty 'tasks'");
    }

    let ids = tasks.iter().map(|t| t.id.clone()).collect::<HashSet<_>>();
    for task in &tasks {
        for dep in &task.dependencies {
            if !ids.contains(dep) {
                bail!(
                    "task '{}' depends on unknown task '{}'; all dependencies must reference known task ids",
                    task.id,
                    dep
                );
            }
        }
    }

    let translated = TranslatedPlan { tasks, spl };
    validate_canonical_facts(&translated)?;
    Ok(translated)
}

pub fn save_translated_plan(path: &Path, translated: &TranslatedPlan) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create translated plan dir {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(translated)?)
        .with_context(|| format!("write translated plan {}", path.display()))?;
    Ok(())
}

pub fn load_translated_plan(path: &Path) -> Result<TranslatedPlan> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read translated plan {}", path.display()))?;
    let parsed = serde_json::from_str::<TranslatedPlan>(&raw)
        .with_context(|| format!("parse translated plan {}", path.display()))?;
    Ok(parsed)
}

fn is_valid_task_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn validate_no_import_directives(spl: &str) -> Result<()> {
    let mut chars = spl.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            match ch {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            ';' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '(' => {
                while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
                    let _ = chars.next();
                }
                let mut head = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '(' || c == ')' || c == ';' || c == '"' {
                        break;
                    }
                    head.push(c);
                    let _ = chars.next();
                }
                if head.eq_ignore_ascii_case("import") {
                    bail!(
                        "translated SPL may not contain '(import ...)'; plan must be self-contained"
                    );
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_canonical_facts(translated: &TranslatedPlan) -> Result<()> {
    let theory = parse_spl(&translated.spl).context("SPL parse failed during canonical checks")?;
    let mut task_facts = HashSet::<String>::new();
    let mut dep_facts = HashSet::<(String, String)>::new();

    for rule in theory.facts() {
        let lit = rule.head_literal();
        if lit.is_negated() {
            continue;
        }
        let args = lit
            .predicates()
            .into_iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        match (lit.name(), args.as_slice()) {
            ("task", [task_id]) => {
                task_facts.insert(task_id.clone());
            }
            ("depends-on", [task_id, dep_id]) => {
                dep_facts.insert((task_id.clone(), dep_id.clone()));
            }
            _ => {}
        }
    }

    let task_ids = translated
        .tasks
        .iter()
        .map(|t| t.id.clone())
        .collect::<HashSet<_>>();
    if task_ids != task_facts {
        let expected = task_ids.into_iter().collect::<BTreeSet<_>>();
        let actual = task_facts.into_iter().collect::<BTreeSet<_>>();
        bail!(
            "canonical task facts mismatch between tasks[] and SPL facts; expected {:?}, found {:?}",
            expected,
            actual
        );
    }

    let mut deps = HashSet::<(String, String)>::new();
    for task in &translated.tasks {
        for dep in &task.dependencies {
            deps.insert((task.id.clone(), dep.clone()));
        }
    }
    if deps != dep_facts {
        let expected = deps.into_iter().collect::<BTreeSet<_>>();
        let actual = dep_facts.into_iter().collect::<BTreeSet<_>>();
        bail!(
            "canonical depends-on facts mismatch between tasks[] and SPL facts; expected {:?}, found {:?}",
            expected,
            actual
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_translated_plan_output;
    use serde_json::json;

    #[test]
    fn parses_valid_translated_output() {
        let out = json!({
            "spl": "(given (task task-a))\n(given (task task-b))\n(given (depends-on task-b task-a))\n(given (ready task-a))\n(always r-ready-task-b (closed task-a) (ready task-b))\n",
            "tasks": [
                {
                    "id": "task-a",
                    "objective": "first",
                    "acceptance": "done first",
                    "dependencies": [],
                    "checks": ["cargo check"]
                },
                {
                    "id": "task-b",
                    "objective": "second",
                    "acceptance": "done second",
                    "dependencies": ["task-a"],
                    "checks": ["cargo test"]
                }
            ]
        });
        let translated = parse_translated_plan_output(&out, &["true".to_string()]).unwrap();
        assert_eq!(translated.tasks.len(), 2);
    }

    #[test]
    fn rejects_mismatched_canonical_task_facts() {
        let out = json!({
            "spl": "(given (task task-a))\n(given (ready task-a))\n",
            "tasks": [
                {
                    "id": "task-b",
                    "objective": "only",
                    "acceptance": "done",
                    "dependencies": [],
                    "checks": ["true"]
                }
            ]
        });
        let err = parse_translated_plan_output(&out, &["true".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("canonical task facts mismatch"));
    }

    #[test]
    fn rejects_import_directive() {
        let out = json!({
            "spl": "(import \"other.spl\")\n(given (task task-a))\n",
            "tasks": [
                {
                    "id": "task-a",
                    "objective": "only",
                    "acceptance": "done",
                    "dependencies": [],
                    "checks": ["true"]
                }
            ]
        });
        let err = parse_translated_plan_output(&out, &["true".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("may not contain '(import"));
    }

    #[test]
    fn allows_predicates_containing_import_substring() {
        let out = json!({
            "spl": "(given (task task-a))\n(given (important task-a))\n(given (ready task-a))\n",
            "tasks": [
                {
                    "id": "task-a",
                    "objective": "only",
                    "acceptance": "done",
                    "dependencies": [],
                    "checks": ["true"]
                }
            ]
        });
        let translated = parse_translated_plan_output(&out, &["true".to_string()]).unwrap();
        assert_eq!(translated.tasks.len(), 1);
    }
}
