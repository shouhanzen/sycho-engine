# plantool

`plantool` is a lightweight planner orchestrator for `plans/*.txt` and `plans/*.md`.

It treats checklist items (`- [ ]` / `- [x]`) as executable task nodes and supports:

- plan graph validation (`Depends-On:` support)
- ready task listing
- task claiming with lease state
- marking checklist items complete in-place
- looped execution with guardrails
- live passthrough streaming of agent output per task
- stream-json output is rendered into concise readable event lines

## Plan file conventions

Optional metadata lines in any plan file:

- `Plan-ID: MY_PLAN_ID`
- `Depends-On: OTHER_PLAN_ID, ANOTHER_PLAN_ID`

If `Plan-ID` is omitted, an ID is inferred from the file name.

Checklist lines become task IDs like:

- `MY_PLAN_ID#1`
- `MY_PLAN_ID#2`

## Commands

From the workspace root:

```bash
cargo run -p plantool -- validate
cargo run -p plantool -- list --ready
```

If you add this shell function to your config:

```bash
plan() { cargo run -p plantool -- "$@"; }
```

you can use:

```bash
plan validate
plan list --ready
```

Claim and complete tasks:

```bash
cargo run -p plantool -- claim MY_PLAN_ID#1 --owner agent:cursor
cargo run -p plantool -- complete MY_PLAN_ID#1 --owner agent:cursor --note "verified with cargo test"
```

Run the planner loop (full form):

```bash
cargo run -p plantool -- run \
  --owner agent:cursor \
  --watch \
  --max-steps 100 \
  --max-minutes 60 \
  --sleep-seconds 5 \
  --idle-timeout-seconds 300 \
  --exec "cursor-agent --print --output-format stream-json --stream-partial-output 'You are executing plan {plan_id} from {plan_path}.\n\nComplete as much of this plan as you can in this single run.\nIf you finish items, update checklist markers in the plan file.\nIf blocked, leave clear notes in the plan file.\n\nOpen checklist items ({pending_count}):\n{open_tasks}\n\nFull plan text:\n{plan_text}'" \
  --auto-complete-on-success
```

Minimal loop command (defaults to `cursor-agent`):

```bash
plan run --watch --auto-complete-on-success
```

Defaults for `plan run`:

- `--owner agent:cursor-agent`
- `--max-steps 100`
- `--max-minutes 60`
- `--sleep-seconds 5`
- `--idle-timeout-seconds 300`
- `--exec "cursor-agent --print --output-format stream-json --stream-partial-output 'You are executing plan {plan_id} from {plan_path}.\n\nComplete as much of this plan as you can in this single run.\nIf you finish items, update checklist markers in the plan file.\nIf blocked, leave clear notes in the plan file.\n\nOpen checklist items ({pending_count}):\n{open_tasks}\n\nFull plan text:\n{plan_text}'"`

On Windows, `plan run` executes task commands through native PowerShell.

If output is idle for `--idle-timeout-seconds`, `plan run` restarts `cursor-agent` with `--continue` and a short resume prompt asking it to diagnose the stall and continue from current state (it does not resend the full plan prompt on resume).

Template variables for `--exec`:

- `{task_id}`
- `{task_text}`
- `{plan_id}`
- `{plan_path}`
- `{plan_text}` (full contents of the selected plan file)
- `{pending_count}` (number of open checklist items in the selected plan)
- `{open_tasks}` (newline list of unchecked checklist items)

## State files

Runtime claim state is stored at:

- `orca/plantool/state/claims.json`

Claims use a lease window and can be reclaimed once stale.
