mod plans;
mod state;

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::{Parser, Subcommand};
use plans::{PlanGraph, Task, load_plans};
use serde_json::Value;
use state::{ClaimStore, mark_task_done};

#[derive(Debug, Parser)]
#[command(name = "plantool")]
#[command(about = "Lightweight PlanTree orchestrator for plans/*.txt")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Validate,
    List {
        #[arg(long, default_value_t = false)]
        ready: bool,
    },
    Claim {
        task_id: String,
        #[arg(long)]
        owner: String,
    },
    Complete {
        task_id: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    Run {
        #[arg(long, default_value = "agent:cursor-agent")]
        owner: String,
        #[arg(long, default_value_t = false)]
        watch: bool,
        #[arg(long, default_value_t = 100)]
        max_steps: usize,
        #[arg(long, default_value_t = 60)]
        max_minutes: u64,
        #[arg(long, default_value_t = 5)]
        sleep_seconds: u64,
        #[arg(long, default_value_t = 600)]
        idle_timeout_seconds: u64,
        #[arg(
            long,
            default_value = "cursor-agent --print --force --output-format stream-json --stream-partial-output 'You are executing plan {plan_id} from {plan_path}.\n\nComplete as much of this plan as you can in this single run.\nIf you finish items, update checklist markers in the plan file.\nIf blocked, leave clear notes in the plan file.\n\nOpen checklist items ({pending_count}):\n{open_tasks}\n\nFull plan text:\n{plan_text}'"
        )]
        exec: String,
        #[arg(long, default_value_t = false)]
        auto_complete_on_success: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = discover_workspace_root()?;

    match cli.command {
        Commands::Validate => cmd_validate(&root),
        Commands::List { ready } => cmd_list(&root, ready),
        Commands::Claim { task_id, owner } => cmd_claim(&root, &task_id, &owner),
        Commands::Complete {
            task_id,
            owner,
            note,
        } => cmd_complete(&root, &task_id, owner.as_deref(), note.as_deref()),
        Commands::Run {
            owner,
            watch,
            max_steps,
            max_minutes,
            sleep_seconds,
            idle_timeout_seconds,
            exec,
            auto_complete_on_success,
        } => cmd_run(
            &root,
            &owner,
            watch,
            max_steps,
            max_minutes,
            sleep_seconds,
            idle_timeout_seconds,
            &exec,
            auto_complete_on_success,
        ),
    }
}

fn cmd_validate(root: &Path) -> Result<()> {
    let graph = load_plans(root)?;
    if assert_graph_valid(&graph).is_err() {
        bail!("Validation failed");
    }

    let total_tasks: usize = graph.plans.iter().map(|p| p.tasks.len()).sum();
    println!(
        "OK: {} plans, {} tasks, dependency graph valid",
        graph.plans.len(),
        total_tasks
    );
    Ok(())
}

fn cmd_list(root: &Path, ready_only: bool) -> Result<()> {
    let (graph, excluded_plan_ids) = load_actionable_graph(root)?;
    warn_excluded_plans(&excluded_plan_ids);
    let claims = ClaimStore::load(root)?;
    let now = Utc::now();

    for plan in &graph.plans {
        let plan_claimed = claims
            .active_claim(&plan_claim_key(&plan.id), now)
            .is_some();
        for task in &plan.tasks {
            if task.done {
                if !ready_only {
                    println!("[done] {}  {}", task.id, task.text);
                }
                continue;
            }
            let claimed = claims.active_claim(&task.id, now).map(|c| c.owner.clone());
            let deps_ok = graph.dependencies_completed(&task.plan_id);
            let is_ready = deps_ok && !plan_claimed && claimed.is_none();
            if ready_only && !is_ready {
                continue;
            }
            let status = if let Some(owner) = claimed {
                format!("claimed:{owner}")
            } else if plan_claimed {
                "plan_claimed".to_string()
            } else if deps_ok {
                "ready".to_string()
            } else {
                "blocked".to_string()
            };
            println!("[{}] {}  {}", status, task.id, task.text);
        }
    }
    Ok(())
}

fn cmd_claim(root: &Path, task_id: &str, owner: &str) -> Result<()> {
    let (graph, excluded_plan_ids) = load_actionable_graph(root)?;
    warn_excluded_plans(&excluded_plan_ids);
    let task = graph
        .tasks_by_id
        .get(task_id)
        .with_context(|| format!("Unknown task id {}", task_id))?;
    if task.done {
        bail!("Task {} is already done", task_id);
    }
    if !graph.dependencies_completed(&task.plan_id) {
        bail!("Task {} is blocked by incomplete dependencies", task_id);
    }

    let mut claims = ClaimStore::load(root)?;
    claims.claim(task_id, owner, Utc::now())?;
    claims.save(root)?;
    println!("Claimed {} for {}", task_id, owner);
    Ok(())
}

fn cmd_complete(root: &Path, task_id: &str, owner: Option<&str>, note: Option<&str>) -> Result<()> {
    let (graph, excluded_plan_ids) = load_actionable_graph(root)?;
    warn_excluded_plans(&excluded_plan_ids);
    let task = graph
        .tasks_by_id
        .get(task_id)
        .with_context(|| format!("Unknown task id {}", task_id))?;
    if task.done {
        println!("Task {} already complete", task_id);
        return Ok(());
    }

    let mut claims = ClaimStore::load(root)?;
    if let Some(active) = claims.active_claim(task_id, Utc::now()) {
        if let Some(owner_name) = owner {
            if active.owner != owner_name {
                bail!(
                    "Task {} is claimed by {} (not {})",
                    task_id,
                    active.owner,
                    owner_name
                );
            }
        }
    }

    mark_task_done(task, note)?;
    claims.release(task_id);
    claims.save(root)?;
    println!("Completed {}", task_id);
    if let Some(archived_path) = maybe_archive_completed_plan(root, &task.plan_id)? {
        println!(
            "Archived completed plan {} to {}",
            task.plan_id,
            archived_path.display()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_run(
    root: &Path,
    owner: &str,
    watch: bool,
    max_steps: usize,
    max_minutes: u64,
    sleep_seconds: u64,
    idle_timeout_seconds: u64,
    exec: &str,
    auto_complete_on_success: bool,
) -> Result<()> {
    let started = Instant::now();
    let mut steps = 0usize;
    let mut consecutive_failures = 0usize;
    let mut last_excluded_signature = String::new();

    loop {
        if steps >= max_steps {
            println!("Stopping: reached max steps ({max_steps})");
            break;
        }
        if started.elapsed() > StdDuration::from_secs(max_minutes * 60) {
            println!("Stopping: reached max runtime ({max_minutes} minutes)");
            break;
        }

        let (graph, excluded_plan_ids) = load_actionable_graph(root)?;
        let excluded_signature = excluded_plan_ids.join(",");
        if !excluded_plan_ids.is_empty() && excluded_signature != last_excluded_signature {
            warn_excluded_plans(&excluded_plan_ids);
            last_excluded_signature = excluded_signature;
        }
        let mut claims = ClaimStore::load(root)?;
        let now = Utc::now();
        let Some(plan_work) = select_next_ready_plan(&graph, &claims, now, owner) else {
            if watch {
                println!("No ready tasks. Sleeping {}s...", sleep_seconds);
                thread::sleep(StdDuration::from_secs(sleep_seconds));
                continue;
            }
            let diagnostics = compute_ready_diagnostics(&graph, &claims, now, owner);
            println!("No ready tasks. Exiting.");
            print_no_ready_guidance(&diagnostics);
            break;
        };

        let claim_id = plan_claim_key(&plan_work.plan_id);
        claims.claim(&claim_id, owner, now)?;
        claims.save(root)?;
        steps += 1;
        println!(
            "Step {}: claimed plan {} ({} open items)",
            steps, plan_work.plan_id, plan_work.pending_count
        );

        let cmd = render_exec_command(exec, &plan_work);
        println!("Executing: {}", cmd);
        println!("==============================");
        println!("Plan Output: {}", plan_work.plan_id);
        println!("==============================");
        let exec_result = run_shell(&cmd, idle_timeout_seconds)?;
        println!("==============================");
        let ok = execution_succeeded(&exec_result);
        println!("Command exit code: {}", exec_result.exit_code);

        if ok && auto_complete_on_success {
            claims.release(&claim_id);
            claims.save(root)?;
            println!("Run succeeded for {}", plan_work.plan_id);
            consecutive_failures = 0;
        } else if ok {
            claims.release(&claim_id);
            claims.save(root)?;
            println!("Execution finished for {}", plan_work.plan_id);
            consecutive_failures = 0;
        } else {
            claims.release(&claim_id);
            claims.save(root)?;
            consecutive_failures += 1;
            println!(
                "Plan {} failed (failure count: {})",
                plan_work.plan_id, consecutive_failures
            );
            if let Some(archived_path) = maybe_archive_completed_plan(root, &plan_work.plan_id)? {
                println!(
                    "Archived completed plan {} to {}",
                    plan_work.plan_id,
                    archived_path.display()
                );
            }
            if consecutive_failures >= 3 {
                println!("Circuit breaker: 3 consecutive failures.");
                break;
            }
            continue;
        }
        if let Some(archived_path) = maybe_archive_completed_plan(root, &plan_work.plan_id)? {
            println!(
                "Archived completed plan {} to {}",
                plan_work.plan_id,
                archived_path.display()
            );
        }
    }

    Ok(())
}

fn render_exec_command(template: &str, plan_work: &PlanWorkItem) -> String {
    let safe_plan_id = sanitize_prompt_fragment(&plan_work.plan_id);
    let safe_plan_path = sanitize_prompt_fragment(&plan_work.plan_path);
    let safe_plan_text = sanitize_prompt_fragment(&plan_work.plan_text);
    let safe_pending_count = plan_work.pending_count.to_string();
    let safe_open_tasks = sanitize_prompt_fragment(&plan_work.open_tasks);
    let safe_task_id = sanitize_prompt_fragment(&plan_work.first_task_id);
    let safe_task_text = sanitize_prompt_fragment(&plan_work.first_task_text);
    template
        .replace("{plan_id}", &safe_plan_id)
        .replace("{plan_path}", &safe_plan_path)
        .replace("{plan_text}", &safe_plan_text)
        .replace("{pending_count}", &safe_pending_count)
        .replace("{open_tasks}", &safe_open_tasks)
        .replace("{task_id}", &safe_task_id)
        .replace("{task_text}", &safe_task_text)
}

struct ExecResult {
    exit_code: i32,
    stream_success: bool,
    stream_error: bool,
}

struct StreamLine {
    is_stderr: bool,
    line: String,
}

#[derive(Default)]
struct StreamFormatter {
    thinking_buffer: String,
    assistant_delta_buffer: String,
}

impl StreamFormatter {
    fn render_line(&mut self, line: &str) -> Vec<String> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return vec![];
        }

        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            return vec![trimmed.to_string()];
        };
        let Some(obj) = value.as_object() else {
            return vec![trimmed.to_string()];
        };
        let Some(event_type) = obj.get("type").and_then(|v| v.as_str()) else {
            return vec![trimmed.to_string()];
        };

        match event_type {
            "system" => self.render_system(obj),
            "user" => self.render_user(obj),
            "thinking" => self.render_thinking(obj),
            "assistant" => self.render_assistant(obj),
            "tool_call" => self.render_tool_call(obj),
            "result" => self.render_result(obj),
            other => vec![format!("[event:{other}]")],
        }
    }

    fn flush(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.thinking_buffer.trim().is_empty() {
            out.push(format!(
                "[thinking] {}",
                truncate_text(self.thinking_buffer.trim(), 240)
            ));
            self.thinking_buffer.clear();
        }
        if !self.assistant_delta_buffer.trim().is_empty() {
            out.push(String::from("----- assistant -----"));
            out.push(self.assistant_delta_buffer.trim().to_string());
            out.push(String::from("---------------------"));
            self.assistant_delta_buffer.clear();
        }
        out
    }

    fn render_system(&self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let subtype = obj.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        let model = obj.get("model").and_then(|v| v.as_str()).unwrap_or("");
        if subtype == "init" && !model.is_empty() {
            vec![format!("[system:init] model={model}")]
        } else {
            vec![format!("[system:{subtype}]")]
        }
    }

    fn render_user(&self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let text = extract_nested_message_text(obj).unwrap_or_default();
        if text.trim().is_empty() {
            vec![String::from("[user]")]
        } else {
            vec![format!("> prompt: {}", truncate_text(text.trim(), 220))]
        }
    }

    fn render_thinking(&mut self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let subtype = obj.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
        match subtype {
            "delta" => {
                if let Some(fragment) = obj.get("text").and_then(|v| v.as_str()) {
                    self.thinking_buffer.push_str(fragment);
                }
                vec![]
            }
            "completed" => {
                if self.thinking_buffer.trim().is_empty() {
                    vec![String::from("[thinking] completed")]
                } else {
                    let text = truncate_text(self.thinking_buffer.trim(), 240);
                    self.thinking_buffer.clear();
                    vec![format!("[thinking] {text}")]
                }
            }
            _ => vec![format!("[thinking:{subtype}]")],
        }
    }

    fn render_assistant(&mut self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let text = extract_nested_message_text(obj).unwrap_or_default();
        if text.trim().is_empty() {
            return vec![];
        }

        let has_model_call_id = obj.contains_key("model_call_id");
        let has_timestamp = obj.contains_key("timestamp_ms");

        if has_timestamp && !has_model_call_id {
            self.assistant_delta_buffer.push_str(text.trim());
            return vec![];
        }

        let rendered = if has_model_call_id && !text.trim().is_empty() {
            text.trim().to_string()
        } else if !self.assistant_delta_buffer.trim().is_empty() {
            self.assistant_delta_buffer.trim().to_string()
        } else {
            text.trim().to_string()
        };
        self.assistant_delta_buffer.clear();

        vec![
            String::from("----- assistant -----"),
            rendered,
            String::from("---------------------"),
        ]
    }

    fn render_tool_call(&self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let subtype = obj
            .get("subtype")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        vec![format!("[tool:{subtype}] {}", summarize_tool_call(obj))]
    }

    fn render_result(&self, obj: &serde_json::Map<String, Value>) -> Vec<String> {
        let is_error = obj
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = if is_error { "error" } else { "success" };
        let result = obj
            .get("result")
            .and_then(|v| v.as_str())
            .map(|s| truncate_text(s, 240))
            .unwrap_or_default();
        if result.is_empty() {
            vec![format!("[result:{status}]")]
        } else {
            vec![format!("[result:{status}] {result}")]
        }
    }
}

fn run_shell(command: &str, idle_timeout_seconds: u64) -> Result<ExecResult> {
    let mut stream_success = false;
    let mut stream_error = false;
    let mut attempt = 0usize;
    let mut current_command = command.to_string();

    loop {
        attempt += 1;
        if attempt > 1 {
            println!(
                "... idle timeout reached; restarting command with --continue (attempt {})",
                attempt
            );
        }

        // Execute plan commands through bash so WSL runs natively without PowerShell routing.
        let mut platform_command = Command::new("bash");
        platform_command.arg("-lc").arg(&current_command);

        let mut child = platform_command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| "Failed to spawn bash for exec command")?;

        let stdout = child
            .stdout
            .take()
            .with_context(|| "Failed to capture stdout")?;
        let stderr = child
            .stderr
            .take()
            .with_context(|| "Failed to capture stderr")?;

        let (tx, rx) = mpsc::channel::<StreamLine>();
        let tx_out = tx.clone();
        let stdout_handle = thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let _ = tx_out.send(StreamLine {
                    is_stderr: false,
                    line,
                });
            }
        });
        let tx_err = tx.clone();
        let stderr_handle = thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let _ = tx_err.send(StreamLine {
                    is_stderr: true,
                    line,
                });
            }
        });
        drop(tx);

        let started = Instant::now();
        let mut last_output_at = Instant::now();
        let mut next_heartbeat_secs = 10u64;
        let mut formatter = StreamFormatter::default();
        let mut idled_out = false;

        let status = loop {
            while let Ok(stream_line) = rx.try_recv() {
                last_output_at = Instant::now();
                if stream_line.is_stderr {
                    eprintln!("[stderr] {}", stream_line.line);
                } else {
                    for rendered in formatter.render_line(&stream_line.line) {
                        println!("{}", rendered);
                    }
                    update_stream_result_flags(
                        &stream_line.line,
                        &mut stream_success,
                        &mut stream_error,
                    );
                }
            }

            if let Some(status) = child
                .try_wait()
                .with_context(|| "Failed while checking exec command status")?
            {
                break status;
            }

            if last_output_at.elapsed() >= StdDuration::from_secs(idle_timeout_seconds) {
                idled_out = true;
                println!(
                    "... idle timeout reached (no output for {}s)",
                    idle_timeout_seconds
                );
                terminate_process_tree(child.id());
                let status = child
                    .wait()
                    .with_context(|| "Failed collecting exec command status after idle kill")?;
                break status;
            }

            thread::sleep(StdDuration::from_millis(200));
            let elapsed_secs = started.elapsed().as_secs();
            if elapsed_secs >= next_heartbeat_secs {
                println!("... task command still running ({}s elapsed)", elapsed_secs);
                next_heartbeat_secs += 10;
            }
        };

        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        while let Ok(stream_line) = rx.try_recv() {
            if stream_line.is_stderr {
                eprintln!("[stderr] {}", stream_line.line);
            } else {
                for rendered in formatter.render_line(&stream_line.line) {
                    println!("{}", rendered);
                }
                update_stream_result_flags(
                    &stream_line.line,
                    &mut stream_success,
                    &mut stream_error,
                );
            }
        }
        for rendered in formatter.flush() {
            println!("{}", rendered);
        }

        if idled_out && current_command.contains("cursor-agent") {
            current_command =
                with_continue_diagnostic_prompt(&current_command, idle_timeout_seconds);
            continue;
        } else if idled_out {
            println!("... command idled out and was terminated");
        }

        let exit_code = status.code().unwrap_or(1);
        return Ok(ExecResult {
            exit_code,
            stream_success,
            stream_error,
        });
    }
}

fn execution_succeeded(exec: &ExecResult) -> bool {
    if exec.exit_code == 0 {
        return true;
    }
    exec.stream_success && !exec.stream_error
}

struct PlanWorkItem {
    plan_id: String,
    plan_path: String,
    plan_text: String,
    pending_count: usize,
    open_tasks: String,
    first_task_id: String,
    first_task_text: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ReadyDiagnostics {
    open_plans: usize,
    open_tasks: usize,
    ready_plans: usize,
    blocked_by_dependencies: usize,
    claimed_by_other_owner: usize,
    claimed_by_self: usize,
}

fn compute_ready_diagnostics(
    graph: &PlanGraph,
    claims: &ClaimStore,
    now: chrono::DateTime<Utc>,
    owner: &str,
) -> ReadyDiagnostics {
    let mut diagnostics = ReadyDiagnostics::default();

    for plan in &graph.plans {
        let open_tasks = plan.tasks.iter().filter(|task| !task.done).count();
        if open_tasks == 0 {
            continue;
        }

        diagnostics.open_plans += 1;
        diagnostics.open_tasks += open_tasks;
        let deps_ok = graph.dependencies_completed(&plan.id);
        let claim_id = plan_claim_key(&plan.id);

        match claims.active_claim(&claim_id, now) {
            Some(claim) if claim.owner == owner => diagnostics.claimed_by_self += 1,
            Some(_) => diagnostics.claimed_by_other_owner += 1,
            None if deps_ok => diagnostics.ready_plans += 1,
            None => diagnostics.blocked_by_dependencies += 1,
        }
    }

    diagnostics
}

fn print_no_ready_guidance(diagnostics: &ReadyDiagnostics) {
    if diagnostics.open_plans == 0 {
        println!("All checklist items are complete in plans/*.txt and plans/*.md.");
    } else {
        println!(
            "Open tasks: {} across {} plan(s). Ready plans: {}. Blocked by dependencies: {}. Claimed by other owners: {}. Claimed by this owner: {}.",
            diagnostics.open_tasks,
            diagnostics.open_plans,
            diagnostics.ready_plans,
            diagnostics.blocked_by_dependencies,
            diagnostics.claimed_by_other_owner,
            diagnostics.claimed_by_self
        );
    }
    println!(
        "Hint: run `plan list --ready` to inspect ready work, or `plan run --watch` to wait for tasks."
    );
}

fn plan_claim_key(plan_id: &str) -> String {
    format!("PLAN::{plan_id}")
}

fn maybe_archive_completed_plan(root: &Path, plan_id: &str) -> Result<Option<PathBuf>> {
    let graph = load_plans(root)?;
    let Some(plan) = graph.plans_by_id.get(plan_id) else {
        return Ok(None);
    };
    if plan.tasks.is_empty() || plan.tasks.iter().any(|task| !task.done) {
        return Ok(None);
    }

    let done_dir = root.join("plans").join("done");
    if plan.path.starts_with(&done_dir) {
        return Ok(None);
    }
    if !plan.path.exists() {
        return Ok(None);
    }

    let archived_path = archive_plan_file(root, &plan.path)?;
    Ok(Some(archived_path))
}

fn archive_plan_file(root: &Path, plan_path: &Path) -> Result<PathBuf> {
    let done_dir = root.join("plans").join("done");
    fs::create_dir_all(&done_dir)
        .with_context(|| format!("Failed to create {}", done_dir.display()))?;

    let file_name = plan_path
        .file_name()
        .with_context(|| format!("Plan path {} has no file name", plan_path.display()))?;
    let mut archived_path = done_dir.join(file_name);
    if archived_path.exists() {
        let stem = plan_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("plan");
        let ext = plan_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|v| format!(".{v}"))
            .unwrap_or_default();
        let mut suffix = 1usize;
        loop {
            let candidate = done_dir.join(format!("{stem}_{suffix}{ext}"));
            if !candidate.exists() {
                archived_path = candidate;
                break;
            }
            suffix += 1;
        }
    }

    fs::rename(plan_path, &archived_path)
        .or_else(|_| copy_then_remove(plan_path, &archived_path))
        .with_context(|| {
            format!(
                "Failed to move completed plan {} to {}",
                plan_path.display(),
                archived_path.display()
            )
        })?;
    Ok(archived_path)
}

fn copy_then_remove(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst).with_context(|| {
        format!(
            "Failed to copy completed plan from {} to {}",
            src.display(),
            dst.display()
        )
    })?;
    fs::remove_file(src)
        .with_context(|| format!("Failed to remove original plan {}", src.display()))?;
    Ok(())
}

fn sanitize_prompt_fragment(value: &str) -> String {
    value.replace('\r', "").replace('\'', "''")
}

fn update_stream_result_flags(line: &str, stream_success: &mut bool, stream_error: &mut bool) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let Some(obj) = value.as_object() else {
        return;
    };
    if obj.get("type").and_then(|v| v.as_str()) != Some("result") {
        return;
    }
    let is_error = obj
        .get("is_error")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if is_error {
        *stream_error = true;
    } else {
        *stream_success = true;
    }
}

fn extract_nested_message_text(obj: &serde_json::Map<String, Value>) -> Option<String> {
    let message = obj.get("message")?.as_object()?;
    let content = message.get("content")?.as_array()?;
    let mut out = String::new();
    for item in content {
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn summarize_tool_call(obj: &serde_json::Map<String, Value>) -> String {
    let tool_call = match obj.get("tool_call").and_then(|v| v.as_object()) {
        Some(v) => v,
        None => return String::from("tool invocation"),
    };
    let Some((tool_name, tool_payload)) = tool_call.iter().next() else {
        return String::from("tool invocation");
    };

    let mut suffix = String::new();
    if let Some(args) = tool_payload.get("args").and_then(|v| v.as_object()) {
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            suffix = format!(" path={}", truncate_text(path, 120));
        }
    }
    format!("{tool_name}{suffix}")
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = String::with_capacity(max_chars + 1);
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    out.push('…');
    out
}

fn ensure_continue_flag(command: &str) -> String {
    if command.contains("--continue") {
        return command.to_string();
    }
    format!("{command} --continue")
}

fn with_continue_diagnostic_prompt(command: &str, idle_timeout_seconds: u64) -> String {
    let base = extract_continue_base(command);
    let resume_prompt = format!(
        "Session resumed after {}s of no output. First, identify why the previous run timed out or stalled. \
Then fix the root cause if possible (for example: hung command, missing timeout wrapper, blocked tool call, or bad test command). \
After that, continue from the current state. Do not restart the plan from scratch.",
        idle_timeout_seconds
    );
    let escaped = resume_prompt.replace('\'', "''");
    format!("{base} '{escaped}'")
}

fn extract_continue_base(command: &str) -> String {
    let mut base = command.to_string();
    if let Some(idx) = base.find(" --continue") {
        base.truncate(idx);
    }

    let trimmed = base.trim_end().to_string();
    if trimmed.ends_with('\'') {
        if let Some(first_quote) = trimmed.find('\'') {
            base = trimmed[..first_quote].trim_end().to_string();
        } else {
            base = trimmed;
        }
    } else {
        base = trimmed;
    }

    ensure_continue_flag(&base)
}

fn terminate_process_tree(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    } else {
        let _ = Command::new("pkill")
            .args(["-TERM", "-P", &pid.to_string()])
            .status();
    }
}

fn select_next_ready_plan(
    graph: &PlanGraph,
    claims: &ClaimStore,
    now: chrono::DateTime<Utc>,
    owner: &str,
) -> Option<PlanWorkItem> {
    let mut plans: Vec<&plans::Plan> = graph
        .plans
        .iter()
        .filter(|plan| graph.dependencies_completed(&plan.id))
        .filter(|plan| {
            let claim_id = plan_claim_key(&plan.id);
            match claims.active_claim(&claim_id, now) {
                None => true,
                Some(claim) => claim.owner == owner,
            }
        })
        .filter(|plan| plan.tasks.iter().any(|t| !t.done))
        .collect();

    plans.sort_by(|a, b| a.id.cmp(&b.id));
    let plan = plans.into_iter().next()?;

    let pending_tasks: Vec<&Task> = plan.tasks.iter().filter(|t| !t.done).collect();
    let first_task = pending_tasks.first()?;
    let open_tasks = pending_tasks
        .iter()
        .map(|t| format!("- [ ] {}", t.text))
        .collect::<Vec<String>>()
        .join("\n");
    let plan_text = std::fs::read_to_string(&plan.path).unwrap_or_default();

    Some(PlanWorkItem {
        plan_id: plan.id.clone(),
        plan_path: plan.path.to_string_lossy().into_owned(),
        plan_text,
        pending_count: pending_tasks.len(),
        open_tasks,
        first_task_id: first_task.id.clone(),
        first_task_text: first_task.text.clone(),
    })
}

fn assert_graph_valid(graph: &PlanGraph) -> Result<()> {
    let dep_errors = graph.dependency_errors();
    let cycle_errors = graph.cycle_errors();
    if !dep_errors.is_empty() || !cycle_errors.is_empty() {
        for e in dep_errors {
            eprintln!("ERROR: {e}");
        }
        for e in cycle_errors {
            eprintln!("ERROR: {e}");
        }
        bail!("Plan graph is invalid");
    }
    Ok(())
}

fn load_actionable_graph(root: &Path) -> Result<(PlanGraph, Vec<String>)> {
    let graph = load_plans(root)?;
    Ok(prune_invalid_plans(graph))
}

fn prune_invalid_plans(mut graph: PlanGraph) -> (PlanGraph, Vec<String>) {
    let mut removed_plan_ids = HashSet::new();
    loop {
        let mut invalid_plan_ids = graph.plans_with_missing_dependencies();
        invalid_plan_ids.extend(graph.plans_in_cycles());
        if invalid_plan_ids.is_empty() {
            break;
        }
        removed_plan_ids.extend(invalid_plan_ids.iter().cloned());
        graph = graph.without_plans(&invalid_plan_ids);
        if graph.plans.is_empty() {
            break;
        }
    }

    let mut removed: Vec<String> = removed_plan_ids.into_iter().collect();
    removed.sort();
    (graph, removed)
}

fn warn_excluded_plans(excluded_plan_ids: &[String]) {
    if excluded_plan_ids.is_empty() {
        return;
    }
    eprintln!(
        "WARNING: Excluding invalid plan node(s): {}",
        excluded_plan_ids.join(", ")
    );
    eprintln!("WARNING: Run `plan validate` for detailed graph errors.");
}

fn discover_workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().with_context(|| "Failed to get current directory")?;
    for dir in cwd.ancestors() {
        let cargo = dir.join("Cargo.toml");
        if !cargo.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&cargo).unwrap_or_default();
        if text.contains("[workspace]") {
            return Ok(dir.to_path_buf());
        }
    }
    bail!("Could not locate workspace root from current directory");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
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
                "plantool_main_{}_{}_{}",
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

    fn make_plan(id: &str, depends_on: &[&str], task_done: &[bool]) -> plans::Plan {
        let path = PathBuf::from(format!("plans/{}.md", id.to_ascii_lowercase()));
        let tasks = task_done
            .iter()
            .enumerate()
            .map(|(idx, done)| Task {
                id: format!("{id}#{}", idx + 1),
                plan_id: id.to_string(),
                plan_path: path.clone(),
                line_index: idx,
                text: format!("task {id}#{}", idx + 1),
                done: *done,
            })
            .collect();
        plans::Plan {
            id: id.to_string(),
            path,
            depends_on: depends_on.iter().map(|dep| dep.to_string()).collect(),
            tasks,
        }
    }

    fn make_graph(plans: Vec<plans::Plan>) -> PlanGraph {
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

    #[test]
    fn run_command_default_exec_enables_force_mode() {
        let cli = Cli::try_parse_from(["plantool", "run"]).expect("run args should parse");
        let Commands::Run { exec, .. } = cli.command else {
            panic!("expected run subcommand");
        };
        assert!(exec.contains("--force"));
    }

    #[test]
    fn run_command_default_idle_timeout_is_ten_minutes() {
        let cli = Cli::try_parse_from(["plantool", "run"]).expect("run args should parse");
        let Commands::Run {
            idle_timeout_seconds,
            ..
        } = cli.command
        else {
            panic!("expected run subcommand");
        };
        assert_eq!(idle_timeout_seconds, 600);
    }

    #[test]
    fn diagnostics_explain_why_no_plan_is_ready() {
        let graph = make_graph(vec![
            make_plan("A", &[], &[true]),
            make_plan("B", &["A"], &[false]),
            make_plan("C", &["B"], &[false]),
        ]);
        let now = Utc::now();
        let mut claims = ClaimStore::default();
        claims
            .claim("PLAN::B", "agent:other", now)
            .expect("claim should succeed");

        let diagnostics = compute_ready_diagnostics(&graph, &claims, now, "agent:self");
        assert_eq!(
            diagnostics,
            ReadyDiagnostics {
                open_plans: 2,
                open_tasks: 2,
                ready_plans: 0,
                blocked_by_dependencies: 1,
                claimed_by_other_owner: 1,
                claimed_by_self: 0,
            }
        );
    }

    #[test]
    fn maybe_archive_completed_plan_moves_file_to_done() {
        let ws = TempWorkspace::new();
        let plans_dir = ws.root.join("plans");
        let active_path = plans_dir.join("archive_me_plan.txt");
        fs::write(&active_path, "Plan-ID: ARCHIVE_ME_PLAN\n- [x] done\n")
            .expect("write active plan");

        let archived_path =
            maybe_archive_completed_plan(&ws.root, "ARCHIVE_ME_PLAN").expect("archive result");
        let archived_path = archived_path.expect("completed plan should be archived");

        assert!(
            archived_path.starts_with(plans_dir.join("done")),
            "expected archived path under plans/done"
        );
        assert!(!active_path.exists(), "expected source plan to be moved");
        assert!(archived_path.exists(), "expected archived plan to exist");
    }

    #[test]
    fn maybe_archive_completed_plan_keeps_open_plan_in_place() {
        let ws = TempWorkspace::new();
        let plans_dir = ws.root.join("plans");
        let active_path = plans_dir.join("still_open_plan.txt");
        fs::write(
            &active_path,
            "Plan-ID: STILL_OPEN_PLAN\n- [x] done\n- [ ] still open\n",
        )
        .expect("write active plan");

        let archived_path =
            maybe_archive_completed_plan(&ws.root, "STILL_OPEN_PLAN").expect("archive result");
        assert!(
            archived_path.is_none(),
            "incomplete plan should not be archived"
        );
        assert!(active_path.exists(), "open plan should remain active");
    }

    #[test]
    fn prune_invalid_plans_removes_missing_dependency_chains() {
        let graph = make_graph(vec![
            make_plan("VALID", &[], &[false]),
            make_plan("BROKEN", &["MISSING"], &[false]),
            make_plan("DOWNSTREAM", &["BROKEN"], &[false]),
        ]);

        let (pruned, removed) = prune_invalid_plans(graph);
        assert_eq!(removed, vec!["BROKEN", "DOWNSTREAM"]);
        assert!(pruned.plans_by_id.contains_key("VALID"));
        assert!(!pruned.plans_by_id.contains_key("BROKEN"));
        assert!(!pruned.plans_by_id.contains_key("DOWNSTREAM"));
    }

    #[test]
    fn prune_invalid_plans_removes_cycles_and_keeps_valid_nodes() {
        let graph = make_graph(vec![
            make_plan("A", &["B"], &[false]),
            make_plan("B", &["A"], &[false]),
            make_plan("VALID", &[], &[false]),
        ]);

        let (pruned, removed) = prune_invalid_plans(graph);
        assert_eq!(removed, vec!["A", "B"]);
        assert!(pruned.plans_by_id.contains_key("VALID"));
        assert!(!pruned.plans_by_id.contains_key("A"));
        assert!(!pruned.plans_by_id.contains_key("B"));
    }
}
