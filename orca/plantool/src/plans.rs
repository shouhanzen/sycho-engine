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
    pub human_only: bool,
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

    pub fn plans_with_missing_dependencies(&self) -> HashSet<String> {
        let mut broken = HashSet::new();
        for plan in &self.plans {
            if plan
                .depends_on
                .iter()
                .any(|dep| !self.plans_by_id.contains_key(dep))
            {
                broken.insert(plan.id.clone());
            }
        }
        broken
    }

    pub fn plans_in_cycles(&self) -> HashSet<String> {
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
            cycle_ids: &mut HashSet<String>,
        ) {
            if let Some(mark) = marks.get(id) {
                if *mark == Mark::Perm {
                    return;
                }
                if *mark == Mark::Temp {
                    if let Some(idx) = stack.iter().position(|x| x == id) {
                        for plan_id in &stack[idx..] {
                            cycle_ids.insert(plan_id.clone());
                        }
                        cycle_ids.insert(id.to_string());
                    }
                    return;
                }
            }
            marks.insert(id.to_string(), Mark::Temp);
            stack.push(id.to_string());

            if let Some(plan) = graph.plans_by_id.get(id) {
                for dep in &plan.depends_on {
                    if graph.plans_by_id.contains_key(dep) {
                        visit(dep, graph, marks, stack, cycle_ids);
                    }
                }
            }

            stack.pop();
            marks.insert(id.to_string(), Mark::Perm);
        }

        let mut marks = HashMap::new();
        let mut stack = Vec::new();
        let mut cycle_ids = HashSet::new();
        for id in self.plans_by_id.keys() {
            visit(id, self, &mut marks, &mut stack, &mut cycle_ids);
        }
        cycle_ids
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

    pub fn without_plans(&self, removed_plan_ids: &HashSet<String>) -> PlanGraph {
        let plans: Vec<Plan> = self
            .plans
            .iter()
            .filter(|plan| !removed_plan_ids.contains(&plan.id))
            .cloned()
            .collect();

        let mut plans_by_id = HashMap::new();
        let mut tasks_by_id = HashMap::new();
        for plan in &plans {
            plans_by_id.insert(plan.id.clone(), plan.clone());
            for task in &plan.tasks {
                tasks_by_id.insert(task.id.clone(), task.clone());
            }
        }

        PlanGraph {
            plans,
            plans_by_id,
            tasks_by_id,
        }
    }
}

pub fn load_plans(root: &Path) -> Result<PlanGraph> {
    let plans_dir = root.join("plans");
    let mut paths = Vec::new();
    if plans_dir.exists() {
        collect_top_level_plan_files(&plans_dir, &mut paths)?;
        let archived_dir = plans_dir.join("done");
        if archived_dir.exists() {
            collect_plan_files_recursive(&archived_dir, &mut paths)?;
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

fn collect_top_level_plan_files(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("Failed to read plans directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_plan_file(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn collect_plan_files_recursive(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("Failed to read archived plans directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_plan_files_recursive(&path, paths)?;
            continue;
        }
        if path.is_file() && is_plan_file(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn is_plan_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|x| x.to_str()),
        Some("txt" | "md")
    )
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
                .filter(|s| !s.is_empty() && !is_no_dependency_marker(s))
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
                human_only: task.human_only,
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
    human_only: bool,
}

fn parse_task_line(line: &str) -> Option<ParsedTaskLine> {
    if !line.starts_with("- [") {
        return None;
    }
    if let Some(rest) = line.strip_prefix("- [x]") {
        let (text, human_only) = parse_task_text_metadata(rest.trim());
        return Some(ParsedTaskLine {
            done: true,
            text,
            human_only,
        });
    }
    if let Some(rest) = line.strip_prefix("- [X]") {
        let (text, human_only) = parse_task_text_metadata(rest.trim());
        return Some(ParsedTaskLine {
            done: true,
            text,
            human_only,
        });
    }
    if let Some(rest) = line.strip_prefix("- [ ]") {
        let (text, human_only) = parse_task_text_metadata(rest.trim());
        return Some(ParsedTaskLine {
            done: false,
            text,
            human_only,
        });
    }
    None
}

fn parse_task_text_metadata(raw: &str) -> (String, bool) {
    let mut text = raw.trim();
    let mut human_only = false;

    loop {
        if let Some(rest) = strip_task_label_prefix(text, "[human]") {
            human_only = true;
            text = rest;
            continue;
        }
        if let Some(rest) = strip_task_label_prefix(text, "[manual]") {
            human_only = true;
            text = rest;
            continue;
        }
        if let Some(rest) = strip_task_label_prefix(text, "[agent]") {
            text = rest;
            continue;
        }
        break;
    }

    (text.trim().to_string(), human_only)
}

fn strip_task_label_prefix<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    if text.len() < label.len() {
        return None;
    }
    let (prefix, rest) = text.split_at(label.len());
    if prefix.eq_ignore_ascii_case(label) {
        Some(rest.trim_start())
    } else {
        None
    }
}

fn infer_plan_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("UNNAMED_PLAN");
    sanitize_id(stem)
}

fn is_no_dependency_marker(raw: &str) -> bool {
    raw.eq_ignore_ascii_case("none")
        || raw.eq_ignore_ascii_case("null")
        || raw.eq_ignore_ascii_case("n/a")
        || raw == "-"
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            let nonce = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be monotonic")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "plantool_load_plans_{}_{}_{}",
                std::process::id(),
                ts,
                nonce
            ));
            fs::create_dir_all(root.join("plans")).expect("create plans directory");
            Self { root }
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn load_plans_includes_done_archive_for_dependency_resolution() {
        let ws = TempWorkspace::new();
        let plans = ws.root.join("plans");
        let archive = plans.join("done").join("2026-02");
        fs::create_dir_all(&archive).expect("create archive directory");

        fs::write(
            plans.join("milestone_depth_wall_plan.txt"),
            "Plan-ID: MILESTONE_DEPTH_WALL_PLAN\nDepends-On: BOTTOMWELL_PLAN\n- [ ] implement wall\n",
        )
        .expect("write active plan");
        fs::write(
            archive.join("bottomwell_plan.txt"),
            "Plan-ID: BOTTOMWELL_PLAN\n- [x] done\n",
        )
        .expect("write archived dependency plan");

        let graph = load_plans(&ws.root).expect("load plans");
        assert!(
            graph.dependency_errors().is_empty(),
            "expected dependencies to resolve across plans/done"
        );
        assert!(graph.plans_by_id.contains_key("MILESTONE_DEPTH_WALL_PLAN"));
        assert!(graph.plans_by_id.contains_key("BOTTOMWELL_PLAN"));
    }

    #[test]
    fn load_plans_treats_none_dependency_marker_as_empty() {
        let ws = TempWorkspace::new();
        let plans = ws.root.join("plans");
        fs::write(
            plans.join("tetris_lock_delay_plan.txt"),
            "Plan-ID: TETRIS_LOCK_DELAY_PLAN\nDepends-On: NONE\n- [ ] lock delay\n",
        )
        .expect("write plan");

        let graph = load_plans(&ws.root).expect("load plans");
        let plan = graph
            .plans_by_id
            .get("TETRIS_LOCK_DELAY_PLAN")
            .expect("expected lock delay plan");
        assert!(
            plan.depends_on.is_empty(),
            "NONE marker should not produce a dependency edge"
        );
        assert!(
            graph.dependency_errors().is_empty(),
            "NONE marker should not trigger dependency errors"
        );
    }

    #[test]
    fn parse_task_line_extracts_human_labels() {
        let parsed = parse_task_line("- [ ] [human] run feel tuning").expect("expected task");
        assert!(!parsed.done);
        assert!(parsed.human_only);
        assert_eq!(parsed.text, "run feel tuning");

        let parsed = parse_task_line("- [x] [manual] verify on target hardware")
            .expect("expected done task");
        assert!(parsed.done);
        assert!(parsed.human_only);
        assert_eq!(parsed.text, "verify on target hardware");
    }

    #[test]
    fn parse_task_line_strips_agent_label_without_marking_human_only() {
        let parsed = parse_task_line("- [ ] [agent] write regression test").expect("expected task");
        assert!(!parsed.done);
        assert!(!parsed.human_only);
        assert_eq!(parsed.text, "write regression test");
    }
}
