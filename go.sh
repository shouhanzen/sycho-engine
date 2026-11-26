#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

GAME_PID_FILE="${ROLLOUT_GO_GAME_PID_FILE:-$ROOT_DIR/.go.pid}"
GAME_LOG_FILE="${ROLLOUT_GO_GAME_LOG_FILE:-$ROOT_DIR/.go.log}"

usage() {
  cat <<'EOF'
go.sh - Sycho launcher

Usage:
  ./go.sh --editor [--timeline DIR] [--fresh] [--detach] [--release]
  ./go.sh --start [--watch] [--no-recording | --timeline DIR] [--fresh|--playback] [--detach] [--release]
  ./go.sh --stop
  ./go.sh --restart [--watch|--editor] [--no-recording | --timeline DIR] [--fresh|--playback] [--detach] [--release]
  ./go.sh --status
  ./go.sh --test [--release]
  ./go.sh --test-light [--release]
  ./go.sh --e2e [--video] [--ffmpeg /path/to/ffmpeg] [--release]
  ./go.sh --export-video PATH [--timeline DIR] [--ffmpeg PATH] [--release]
  ./go.sh --profile [--release]
  ./go.sh --help

Live timeline (windowed game):
  --watch         Rebuild on source edits and relaunch at the last live checkpoint.
  --editor        Launch the game's timeline editor with live reload and recording.
  --no-recording  Run standalone without opening a timeline.
  --timeline DIR  Open or create a timeline (default: games/tetree/.sycho/timelines/headful).
  --fresh         Start a separate timeline. If DIR is given, it must be new.
  --playback      Play a saved timeline from its first snapshot.
  Timeline shortcuts work while the editor Game panel has focus.
  Option+P (F5)     Pause live play or explicitly resume it from the cursor.
  Option+Left (F6)  Step backward one snapshot.
  Option+Right (F7) Step forward one snapshot.
  Option+Space (F8) Play / pause the recorded timeline. Playback stops at the end.
  Use Alt in place of Option on other keyboards.

  --export-video PATH  Render a closed timeline to MP4 without a game window.

Build:
  --release      Use release profile for cargo commands (faster runtime, slower compile).

Build cache (multi-worktree):
  sccache          If `sccache` is on PATH, go.sh auto-enables it for cargo builds/tests.
                  Disable with: ROLLOUT_DISABLE_SCCACHE=1
  shared target/   Set ROLLOUT_SHARED_TARGET_DIR=1 to set CARGO_TARGET_DIR to a shared per-repo dir
                  (reuses build artifacts across worktrees; concurrent cargo invocations will block on a file lock).
                  Override with: CARGO_TARGET_DIR=/path/to/target

Lifecycle:
  --start        Starts the windowed game. Default is foreground (attached to the terminal).
  --detach       Run in background (writes pid/log files).
                If a background process exits immediately, go.sh will print recent logs to the terminal.
  --foreground   (default) Runs attached to the current terminal (no pid/log management).
  --stop         Stops the background process started by --start --detach.
  --restart      Equivalent to --stop then --start.
  --status       Prints whether a background process is running (--start --detach).

Testing:
  --test         cargo test (engine and game workspace)
  --test-light   workspace tests with reduced local CPU pressure
                Env knobs:
                  ROLLOUT_TEST_LIGHT_BUILD_JOBS=4
                  ROLLOUT_TEST_LIGHT_THREADS=1
  --e2e          cargo test -p tetree --test e2e_playtest_tests
  --video        (e2e) record an mp4 per test via ffmpeg (requires `ffmpeg` on PATH)
  --ffmpeg PATH  (e2e or export) path to ffmpeg binary (sets ROLLOUT_FFMPEG_BIN)
  --profile      run the standalone game and write a trace to target/perf_traces/
                Env knobs:
                  ROLLOUT_PROFILE_FRAMES=600
                  ROLLOUT_PROFILE_WARMUP=200
                  ROLLOUT_PROFILE_WIDTH=1920
                  ROLLOUT_PROFILE_HEIGHT=1080
EOF
}

is_running_pidfile() {
  local pid_file="$1"

  [[ -f "$pid_file" ]] || return 1
  local pid
  pid="$(cat "$pid_file" 2>/dev/null || true)"
  [[ -n "$pid" ]] || return 1
  kill -0 "$pid" 2>/dev/null
}

stop_pidfile() {
  local pid_file="$1"

  if ! [[ -f "$pid_file" ]]; then
    echo "not running (no pid file)"
    return 0
  fi

  local pid
  pid="$(cat "$pid_file" 2>/dev/null || true)"
  if [[ -z "$pid" ]]; then
    rm -f "$pid_file"
    echo "not running (empty pid file)"
    return 0
  fi

  if kill -0 "$pid" 2>/dev/null; then
    echo "stopping pid $pid"
    kill "$pid" 2>/dev/null || true

    # Best-effort wait (works on bash; no 'wait' for non-child processes).
    for _ in {1..20}; do
      if kill -0 "$pid" 2>/dev/null; then
        sleep 0.1
      else
        break
      fi
    done

    if kill -0 "$pid" 2>/dev/null; then
      echo "process still running; sending SIGKILL"
      kill -9 "$pid" 2>/dev/null || true
    fi
  else
    echo "not running (stale pid $pid)"
  fi

  rm -f "$pid_file"
}

print_log_tail() {
  local log_file="$1"
  local lines="${2:-200}"

  if [[ -z "$log_file" ]]; then
    return 0
  fi

  if ! [[ -f "$log_file" ]]; then
    echo "(no log file at $log_file)" >&2
    return 0
  fi

  echo "" >&2
  echo "---- last ${lines} lines of $log_file ----" >&2
  if command -v tail >/dev/null 2>&1; then
    tail -n "$lines" "$log_file" >&2 || true
  else
    cat "$log_file" >&2 || true
  fi
  echo "---- end log ----" >&2
}

ensure_pid_alive_or_dump_logs() {
  local pid_file="$1"
  local log_file="$2"
  local label="$3"

  local pid
  pid="$(cat "$pid_file" 2>/dev/null || true)"
  if [[ -z "$pid" ]]; then
    echo "failed to start $label (empty pid file: $pid_file)" >&2
    echo "logs: $log_file" >&2
    print_log_tail "$log_file" 200
    rm -f "$pid_file"
    return 1
  fi

  # Give the process a moment to crash early so we can surface useful logs.
  sleep 0.2
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "failed to start $label (pid $pid exited immediately)" >&2
    echo "logs: $log_file" >&2
    print_log_tail "$log_file" 200
    rm -f "$pid_file"
    return 1
  fi

  return 0
}

is_uint() {
  [[ "${1:-}" =~ ^[0-9]+$ ]]
}

is_truthy() {
  local v="${1:-}"
  v="$(printf "%s" "$v" | tr '[:upper:]' '[:lower:]')"
  [[ "$v" == "1" || "$v" == "true" || "$v" == "yes" || "$v" == "on" ]]
}

to_native_path() {
  local p="$1"
  if command -v cygpath >/dev/null 2>&1; then
    # Windows: prefer a native-ish path (C:/...) for child processes like cargo.exe / sccache.exe.
    cygpath -m "$p"
  else
    echo "$p"
  fi
}

start_detached() {
  local log_file="$1"
  shift
  nohup "$@" >"$log_file" 2>&1 &
  echo $!
}

common_repo_root() {
  command -v git >/dev/null 2>&1 || return 1

  local common_dir
  common_dir="$(git -C "$ROOT_DIR" rev-parse --git-common-dir 2>/dev/null)" || return 1

  # `--git-common-dir` may be relative to the current directory.
  local common_abs
  common_abs="$(cd "$ROOT_DIR" && cd "$common_dir" && pwd)" || return 1

  local root
  root="$(cd "$common_abs/.." && pwd)" || return 1
  echo "$root"
}

setup_build_cache() {
  # Optional: reuse Cargo artifacts across worktrees by using a shared target dir.
  # Opt-in because Cargo will serialize concurrent builds via a target-dir lock.
  if [[ -z "${CARGO_TARGET_DIR:-}" ]] && is_truthy "${ROLLOUT_SHARED_TARGET_DIR:-0}"; then
    local common_root
    common_root="$(common_repo_root 2>/dev/null || true)"
    if [[ -n "$common_root" ]]; then
      export CARGO_TARGET_DIR
      CARGO_TARGET_DIR="$(to_native_path "$common_root/.cache/cargo-target")"
    else
      export CARGO_TARGET_DIR
      CARGO_TARGET_DIR="$(to_native_path "$ROOT_DIR/.cache/cargo-target")"
    fi
  fi

  # Optional: enable sccache if installed (shared compiler cache across worktrees).
  if is_truthy "${ROLLOUT_DISABLE_SCCACHE:-0}"; then
    return 0
  fi
  if [[ -n "${RUSTC_WRAPPER:-}" || -n "${CARGO_BUILD_RUSTC_WRAPPER:-}" ]]; then
    return 0
  fi
  if command -v sccache >/dev/null 2>&1; then
    export RUSTC_WRAPPER="sccache"
    if [[ -z "${SCCACHE_DIR:-}" ]]; then
      local common_root
      common_root="$(common_repo_root 2>/dev/null || true)"
      if [[ -n "$common_root" ]]; then
        export SCCACHE_DIR
        SCCACHE_DIR="$(to_native_path "$common_root/.cache/sccache")"
      else
        export SCCACHE_DIR
        SCCACHE_DIR="$(to_native_path "$ROOT_DIR/.cache/sccache")"
      fi
    fi
  fi
}

FOREGROUND="true"
ACTION=""
VIDEO="false"
EXPORT_PATH=""
WATCH="false"
EDITOR="false"
TIMELINE_ARGS=()
FFMPEG_BIN=""
RELEASE="false"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --start|--stop|--restart|--status|--test|--test-light|--e2e|--profile)
      ACTION="$1"
      shift
      ;;
    --foreground)
      FOREGROUND="true"
      shift
      ;;
    --detach)
      FOREGROUND="false"
      shift
      ;;
    --video)
      VIDEO="true"
      shift
      ;;
    --no-recording|--fresh|--playback)
      TIMELINE_ARGS+=("$1")
      shift
      ;;
    --export-video)
      if [[ $# -lt 2 || "$2" == --* ]]; then
        echo "--export-video requires an output path" >&2
        exit 2
      fi
      ACTION="$1"
      EXPORT_PATH="$2"
      shift 2
      ;;
    --watch)
      WATCH="true"
      shift
      ;;
    --editor)
      EDITOR="true"
      shift
      ;;
    --timeline)
      if [[ $# -lt 2 || "$2" == --* ]]; then
        echo "--timeline requires a directory" >&2
        exit 2
      fi
      TIMELINE_ARGS+=("$1" "$2")
      shift 2
      ;;
    --ffmpeg)
      if [[ $# -lt 2 ]]; then
        echo "--ffmpeg requires a path" >&2
        exit 2
      fi
      FFMPEG_BIN="$2"
      shift 2
      ;;
    --release)
      RELEASE="true"
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      echo "" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "$EDITOR" == "true" ]]; then
  if [[ -z "$ACTION" ]]; then
    ACTION="--start"
  fi
  if [[ "$ACTION" != "--start" && "$ACTION" != "--restart" ]]; then
    echo "--editor requires a windowed launch" >&2
    exit 2
  fi
  WATCH="true"
fi

if [[ -z "$ACTION" ]]; then
  usage
  exit 2
fi

cd "$ROOT_DIR"
setup_build_cache

if [[ "$WATCH" == "true" ]]; then
  if [[ "$ACTION" != "--start" && "$ACTION" != "--restart" ]]; then
    echo "--watch requires --start or --restart" >&2
    exit 2
  fi
fi

if [[ ${#TIMELINE_ARGS[@]} -gt 0 && "$ACTION" != "--start" && "$ACTION" != "--restart" && "$ACTION" != "--export-video" ]]; then
  echo "timeline options require --start, --restart, or --export-video" >&2
  exit 2
fi

# Guard expansions of this array because Bash 3.2 treats an empty array as unset.
CARGO_PROFILE_ARGS=()
if [[ "$RELEASE" == "true" ]]; then
  CARGO_PROFILE_ARGS+=(--release)
fi

case "$ACTION" in
  --status)
    if is_running_pidfile "$GAME_PID_FILE"; then
      echo "running (pid $(cat "$GAME_PID_FILE"))"
      exit 0
    fi
    echo "not running"
    exit 1
    ;;

  --stop)
    stop_pidfile "$GAME_PID_FILE"
    ;;

  --start|--restart)
    if [[ "$WATCH" == "true" ]]; then
      LAUNCH=(cargo run -p engine --bin overseer ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} --)
      if [[ "$EDITOR" == "true" ]]; then
        LAUNCH+=(--editor)
      else
        LAUNCH+=(--manifest-path games/tetree/Cargo.toml --package tetree --bin headful)
      fi
      if [[ "$RELEASE" == "true" ]]; then
        LAUNCH+=(--release)
      fi
    else
      LAUNCH=(cargo run -p tetree --bin headful ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} --)
    fi

    if [[ ${#TIMELINE_ARGS[@]} -gt 0 ]]; then
      LAUNCH+=("${TIMELINE_ARGS[@]}")
      # Validate arguments before a restart can stop the current process.
      if [[ "$ACTION" == "--restart" ]] || is_running_pidfile "$GAME_PID_FILE"; then
        "${LAUNCH[@]}" --check-launch
      fi
    fi

    if [[ "$ACTION" == "--restart" ]]; then
      stop_pidfile "$GAME_PID_FILE"
    elif is_running_pidfile "$GAME_PID_FILE"; then
      echo "already running (pid $(cat "$GAME_PID_FILE"))"
      exit 0
    fi

    if [[ "$FOREGROUND" == "true" ]]; then
      exec "${LAUNCH[@]}"
    fi

    pid="$(start_detached "$GAME_LOG_FILE" "${LAUNCH[@]}")"
    echo "$pid" >"$GAME_PID_FILE"
    ensure_pid_alive_or_dump_logs "$GAME_PID_FILE" "$GAME_LOG_FILE" "headful game"
    echo "started headful game (pid $(cat "$GAME_PID_FILE"))"
    echo "logs: $GAME_LOG_FILE"
    ;;

  --test)
    exec cargo test --workspace ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}
    ;;

  --test-light)
    TEST_LIGHT_BUILD_JOBS="${ROLLOUT_TEST_LIGHT_BUILD_JOBS:-4}"
    TEST_LIGHT_THREADS="${ROLLOUT_TEST_LIGHT_THREADS:-1}"
    if ! is_uint "$TEST_LIGHT_BUILD_JOBS" || [[ "$TEST_LIGHT_BUILD_JOBS" -lt 1 ]]; then
      echo "invalid ROLLOUT_TEST_LIGHT_BUILD_JOBS: $TEST_LIGHT_BUILD_JOBS" >&2
      exit 2
    fi
    if ! is_uint "$TEST_LIGHT_THREADS" || [[ "$TEST_LIGHT_THREADS" -lt 1 ]]; then
      echo "invalid ROLLOUT_TEST_LIGHT_THREADS: $TEST_LIGHT_THREADS" >&2
      exit 2
    fi
    exec env \
      CARGO_BUILD_JOBS="$TEST_LIGHT_BUILD_JOBS" \
      RUST_TEST_THREADS="$TEST_LIGHT_THREADS" \
      cargo test --workspace ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"}
    ;;

  --e2e)
    if [[ "$VIDEO" == "true" ]]; then
      export ROLLOUT_E2E_RECORD_MP4=1
      if [[ -n "$FFMPEG_BIN" ]]; then
        export ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN"
      fi
    fi
    exec cargo test -p tetree --test e2e_playtest_tests ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} -- --nocapture
    ;;

  --export-video)
    EXPORT_ARGS=(--export-video "$EXPORT_PATH")
    if [[ ${#TIMELINE_ARGS[@]} -gt 0 ]]; then
      EXPORT_ARGS+=("${TIMELINE_ARGS[@]}")
    fi
    if [[ -n "$FFMPEG_BIN" ]]; then
      export ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN"
    fi
    exec cargo run -p tetree --bin headful ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} -- "${EXPORT_ARGS[@]}"
    ;;

  --profile)
    exec cargo run -p tetree --bin headful ${CARGO_PROFILE_ARGS[@]+"${CARGO_PROFILE_ARGS[@]}"} -- --profile
    ;;
esac
