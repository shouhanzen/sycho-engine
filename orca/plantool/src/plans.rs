use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub plan_id: String,
    pub plan_path: PathBuf,
    pub line_index: usize,
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub id: String,
    pub path: PathBuf,
    pub depends_on: Vec<String>,
    pub tasks: Vec<Task>,
}

#[derive(Debug)]
pub struct PlanGraph {
    pub plans: Vec<Plan>,
    pub plans_by_id: HashMap<String, Plan>,
    pub tasks_by_id: HashMap<String, Task>,
}

impl PlanGraph {
    pub fn dependency_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for plan in &self.plans {
            for dep in &plan.depends_on {
                if !self.plans_by_id.contains_key(dep) {
                    errors.push(format!(
                        "Plan {} depends on missing plan {} ({})",
                        plan.id,
                        dep,
                        plan.path.display()
                    ));
                }
            }
        }
        errors
    }

    pub fn cycle_errors(&self) -> Vec<String> {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Mark {
            Temp,
            Perm,
        }

        fn visit(
            id: &str,
            graph: &PlanGraph,
            marks: &mut HashMap<String, Mark>,
            stack: &mut Vec<String>,
            cycles: &mut Vec<String>,
        ) {
            if let Some(mark) = marks.get(id) {
                if *mark == Mark::Perm {
                    return;
                }
                if *mark == Mark::Temp {
                    if let Some(idx) = stack.iter().position(|x| x == id) {
                        let mut cycle = stack[idx..].to_vec();
                        cycle.push(id.to_string());
                        cycles.push(format!("Cycle: {}", cycle.join(" -> ")));
                    }
                    return;
                }
            }
            marks.insert(id.to_string(), Mark::Temp);
            stack.push(id.to_string());

            if let Some(plan) = graph.plans_by_id.get(id) {
                for dep in &plan.depends_on {
                    if graph.plans_by_id.contains_key(dep) {
                        visit(dep, graph, marks, stack, cycles);
                    }
                }
            }

            stack.pop();
            marks.insert(id.to_string(), Mark::Perm);
        }

        let mut marks = HashMap::new();
        let mut stack = Vec::new();
        let mut cycles = Vec::new();
        for id in self.plans_by_id.keys() {
            visit(id, self, &mut marks, &mut stack, &mut cycles);
        }
        cycles
    }

    pub fn plan_completed(&self, plan_id: &str) -> bool {
        self.plans_by_id
            .get(plan_id)
            .map(|p| p.tasks.iter().all(|t| t.done))
            .unwrap_or(false)
    }

    pub fn dependencies_completed(&self, plan_id: &str) -> bool {
        self.plans_by_id
            .get(plan_id)
            .map(|p| p.depends_on.iter().all(|dep| self.plan_completed(dep)))
            .unwrap_or(false)
    }
}

pub fn load_plans(root: &Path) -> Result<PlanGraph> {
    let plans_dir = root.join("plans");
    let mut paths = Vec::new();
    if plans_dir.exists() {
        for entry in fs::read_dir(&plans_dir).with_context(|| "Failed to read plans directory")? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|x| x.to_str()) else {
                continue;
            };
            if matches!(ext, "txt" | "md") {
                paths.push(path);
            }
        }
    }
    paths.sort();

    let mut plans = Vec::new();
    let mut ids = HashSet::new();
    for path in paths {
        let plan = parse_plan_file(&path)?;
        if !ids.insert(plan.id.clone()) {
            bail!(
                "Duplicate plan id {} detected (from {})",
                plan.id,
                plan.path.display()
            );
        }
        plans.push(plan);
    }

    let mut plans_by_id = HashMap::new();
    let mut tasks_by_id = HashMap::new();
    for plan in &plans {
        plans_by_id.insert(plan.id.clone(), plan.clone());
        for task in &plan.tasks {
            if tasks_by_id.contains_key(&task.id) {
                bail!("Duplicate task id detected: {}", task.id);
            }
            tasks_by_id.insert(task.id.clone(), task.clone());
        }
    }

    Ok(PlanGraph {
        plans,
        plans_by_id,
        tasks_by_id,
    })
}

fn parse_plan_file(path: &Path) -> Result<Plan> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed reading plan file {}", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();

    let mut plan_id = None;
    let mut depends_on: Vec<String> = Vec::new();
    let mut tasks = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Plan-ID:") {
            let value = value.trim();
            if !value.is_empty() {
                plan_id = Some(value.to_string());
            }
        } else if let Some(value) = trimmed.strip_prefix("Depends-On:") {
            depends_on = value
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
        } else if let Some(task) = parse_task_line(trimmed) {
            let task_id = format!(
                "{}#{}",
                plan_id
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| infer_plan_id(path)),
                tasks.len() + 1
            );
            tasks.push(Task {
                id: task_id,
                plan_id: String::new(),
                plan_path: path.to_path_buf(),
                line_index: idx,
                text: task.text,
                done: task.done,
            });
        }
    }

    let id = plan_id.unwrap_or_else(|| infer_plan_id(path));
    for t in &mut tasks {
        t.plan_id = id.clone();
    }

    Ok(Plan {
        id,
        path: path.to_path_buf(),
        depends_on,
        tasks,
    })
}

struct ParsedTaskLine {
    done: bool,
    text: String,
}

fn parse_task_line(line: &str) -> Option<ParsedTaskLine> {
    if !line.starts_with("- [") {
        return None;
    }
    if let Some(rest) = line.strip_prefix("- [x]") {
        return Some(ParsedTaskLine {
            done: true,
            text: rest.trim().to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix("- [X]") {
        return Some(ParsedTaskLine {
            done: true,
            text: rest.trim().to_string(),
        });
    }
    if let Some(rest) = line.strip_prefix("- [ ]") {
        return Some(ParsedTaskLine {
            done: false,
            text: rest.trim().to_string(),
        });
    }
    None
}

fn infer_plan_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("UNNAMED_PLAN");
    sanitize_id(stem)
}

fn sanitize_id(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}
