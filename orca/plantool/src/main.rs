mod plans;
mod state;

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
        #[arg(long, default_value_t = 300)]
        idle_timeout_seconds: u64,
        #[arg(
            long,
            default_value = "cursor-agent --print --output-format stream-json --stream-partial-output 'You are executing plan {plan_id} from {plan_path}.\n\nComplete as much of this plan as you can in this single run.\nIf you finish items, update checklist markers in the plan file.\nIf blocked, leave clear notes in the plan file.\n\nOpen checklist items ({pending_count}):\n{open_tasks}\n\nFull plan text:\n{plan_text}'"
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
    let dep_errors = graph.dependency_errors();
    let cycle_errors = graph.cycle_errors();
    if !dep_errors.is_empty() || !cycle_errors.is_empty() {
        for e in dep_errors {
            eprintln!("ERROR: {e}");
        }
        for e in cycle_errors {
            eprintln!("ERROR: {e}");
        }
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
    let graph = load_plans(root)?;
    assert_graph_valid(&graph)?;
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
    let graph = load_plans(root)?;
    assert_graph_valid(&graph)?;
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
    let graph = load_plans(root)?;
    assert_graph_valid(&graph)?;
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

    loop {
        if steps >= max_steps {
            println!("Stopping: reached max steps ({max_steps})");
            break;
        }
        if started.elapsed() > StdDuration::from_secs(max_minutes * 60) {
            println!("Stopping: reached max runtime ({max_minutes} minutes)");
            break;
        }

        let graph = load_plans(root)?;
        assert_graph_valid(&graph)?;
        let mut claims = ClaimStore::load(root)?;
        let now = Utc::now();
        let Some(plan_work) = select_next_ready_plan(&graph, &claims, now, owner) else {
            if watch {
                println!("No ready tasks. Sleeping {}s...", sleep_seconds);
                thread::sleep(StdDuration::from_secs(sleep_seconds));
                continue;
            }
            println!("No ready tasks. Exiting.");
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
            if consecutive_failures >= 3 {
                println!("Circuit breaker: 3 consecutive failures.");
                break;
            }
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

        let mut platform_command = if cfg!(windows) {
            let mut cmd = Command::new("powershell.exe");
            cmd.arg("-NoProfile").arg("-Command").arg(&current_command);
            cmd
        } else {
            let mut cmd = Command::new("bash");
            cmd.arg("-lc").arg(&current_command);
            cmd
        };

        let mut child = platform_command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| "Failed to spawn platform shell for exec command")?;

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

fn plan_claim_key(plan_id: &str) -> String {
    format!("PLAN::{plan_id}")
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
    let base = ensure_continue_flag(command);
    let resume_prompt = format!(
        "Session resumed after {}s of no output. First, identify why the previous run timed out or stalled. \
Then fix the root cause if possible (for example: hung command, missing timeout wrapper, blocked tool call, or bad test command). \
After that, continue executing the plan.",
        idle_timeout_seconds
    );
    let escaped = resume_prompt.replace('\'', "''");
    format!("{base} '{escaped}'")
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
