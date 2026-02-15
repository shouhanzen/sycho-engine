#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Game (default)
GAME_PID_FILE="${ROLLOUT_GO_GAME_PID_FILE:-$ROOT_DIR/.go.pid}"
GAME_LOG_FILE="${ROLLOUT_GO_GAME_LOG_FILE:-$ROOT_DIR/.go.log}"

# Editor (engine editor API + Tauri app)
EDITOR_API_PID_FILE="${ROLLOUT_GO_EDITOR_API_PID_FILE:-$ROOT_DIR/.go.editor-api.pid}"
EDITOR_API_LOG_FILE="${ROLLOUT_GO_EDITOR_API_LOG_FILE:-$ROOT_DIR/.go.editor-api.log}"
EDITOR_APP_PID_FILE="${ROLLOUT_GO_EDITOR_APP_PID_FILE:-$ROOT_DIR/.go.editor-app.pid}"
EDITOR_APP_LOG_FILE="${ROLLOUT_GO_EDITOR_APP_LOG_FILE:-$ROOT_DIR/.go.editor-app.log}"

usage() {
  cat <<'EOF'
go.sh - rollout_engine control surface

Usage:
  ./go.sh --start [--game|--editor] [--headful|--headless] [--record [PATH]] [--replay PATH] [--detach] [--release]
  ./go.sh --stop [--game|--editor]
  ./go.sh --restart [--game|--editor] [--headful|--headless] [--record [PATH]] [--replay PATH] [--detach] [--release]
  ./go.sh --status [--game|--editor]
  ./go.sh --test [--release]
  ./go.sh --e2e [--video] [--ffmpeg /path/to/ffmpeg] [--release]
  ./go.sh --profile [--release]
  ./go.sh --help

Targets:
  --game        Control the game process (default).
  --editor      Control the engine editor API + Tauri editor app pair.

Modes:
  --headful      Run the windowed game (default). Uses: cargo run -p game --bin headful
  --headless     Run headless playtests (non-interactive). Uses: cargo test -p game --test e2e_playtest_tests

Recording / Replay (headful game only):
  --record [PATH]   Save a frame-by-frame state recording (JSON) on exit.
                   If PATH is omitted, the game chooses a default under `target/recordings/`.
  --replay PATH      Replay a previously saved state recording (JSON).

Build:
  --release      Use release profile for cargo commands (faster runtime, slower compile).

Build cache (multi-worktree):
  sccache          If `sccache` is on PATH, go.sh auto-enables it for cargo builds/tests.
                  Disable with: ROLLOUT_DISABLE_SCCACHE=1
  shared target/   Set ROLLOUT_SHARED_TARGET_DIR=1 to set CARGO_TARGET_DIR to a shared per-repo dir
                  (reuses build artifacts across worktrees; concurrent cargo invocations will block on a file lock).
                  Override with: CARGO_TARGET_DIR=/path/to/target

Editor:
  API:  cargo run -p game --bin editor_api   (default http://127.0.0.1:4000; override via ROLLOUT_EDITOR_API_ADDR / ROLLOUT_EDITOR_API_PORT)
  App:  cd editor/frontend && npm run tauri dev
  Note: the app may require a one-time: cd editor/frontend && npm install

Ports (multi-worktree safe defaults):
  go.sh derives a stable per-worktree port offset from the worktree path.
  Override any of these to force a specific port assignment:
    ROLLOUT_EDITOR_PORT_OFFSET  (integer; used to derive both ports)
    ROLLOUT_EDITOR_API_PORT     (default: 4000 + offset)
    ROLLOUT_EDITOR_DEV_PORT     (default: 5173 + offset)

Lifecycle:
  --start        Starts the selected target/mode. Default is foreground (attached to the terminal).
  --detach       Run in background (writes pid/log files).
                If a background process exits immediately, go.sh will print recent logs to the terminal.
  --foreground   (default) Runs attached to the current terminal (no pid/log management).
  --stop         Stops the background process started by --start --detach.
  --restart      Equivalent to --stop then --start.
  --status       Prints whether a background process is running (--start --detach).

Testing:
  --test         cargo test (workspace)
  --e2e          cargo test -p game --test e2e_playtest_tests
  --video        (e2e) record an mp4 per test via ffmpeg (requires `ffmpeg` on PATH)
  --ffmpeg PATH  (e2e) path to ffmpeg binary (sets ROLLOUT_FFMPEG_BIN)
  --profile      run the headless profiler (cargo run -p game --bin profile)
                Env knobs:
                  ROLLOUT_PROFILE_FRAMES=10000
                  ROLLOUT_PROFILE_WARMUP=200
                  ROLLOUT_PROFILE_WIDTH=960
                  ROLLOUT_PROFILE_HEIGHT=720
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

is_windows() {
  if is_wsl; then
    return 1
  fi
  case "$(uname -s 2>/dev/null || echo "")" in
    MINGW*|MSYS*|CYGWIN*) return 0 ;;
  esac
  [[ "${OS:-}" == "Windows_NT" ]]
}

is_wsl() {
  if [[ -n "${WSL_INTEROP:-}" ]] || [[ -n "${WSL_DISTRO_NAME:-}" ]]; then
    return 0
  fi

  local kernel_release
  kernel_release="$(uname -r 2>/dev/null || echo "")"
  kernel_release="$(printf "%s" "$kernel_release" | tr '[:upper:]' '[:lower:]')"
  case "$kernel_release" in
    *microsoft*|*wsl*) return 0 ;;
  esac

  return 1
}

resolve_powershell() {
  if command -v pwsh >/dev/null 2>&1; then
    echo "pwsh"
    return 0
  fi
  if command -v powershell.exe >/dev/null 2>&1; then
    echo "powershell.exe"
    return 0
  fi
  if command -v powershell >/dev/null 2>&1; then
    echo "powershell"
    return 0
  fi
  return 1
}

start_detached() {
  local log_file="$1"
  shift
  local exe="$1"
  shift
  local args=("$@")

  if is_windows; then
    local ps_bin
    ps_bin="$(resolve_powershell 2>/dev/null || true)"
    if [[ -n "$ps_bin" ]]; then
      local log_native
      log_native="$(to_native_path "$log_file")"

      local ps_args=""
      for arg in "${args[@]}"; do
        local escaped="${arg//\'/\'\'}"
        if [[ -n "$ps_args" ]]; then
          ps_args+=", "
        fi
        ps_args+="'$escaped'"
      done

      local ps_cmd
      ps_cmd="\$p = Start-Process -FilePath '$exe' -ArgumentList $ps_args -RedirectStandardOutput '$log_native' -RedirectStandardError '$log_native' -PassThru; Write-Output \$p.Id"
      local pid
      pid="$("$ps_bin" -NoProfile -Command "$ps_cmd" 2>/dev/null | tr -d '\r' | awk 'NF{last=$0} END{print last}')"
      if [[ -n "$pid" ]]; then
        echo "$pid"
        return 0
      fi
    fi
  fi

  nohup "$exe" "${args[@]}" >"$log_file" 2>&1 &
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

kill_listeners_on_port() {
  local port="$1"

  if ! is_uint "$port" || ((port < 1 || port > 65535)); then
    return 0
  fi

  # macOS / Linux
  if command -v lsof >/dev/null 2>&1; then
    local pids
    pids="$(lsof -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null || true)"
    if [[ -n "$pids" ]]; then
      for pid in $pids; do
        kill "$pid" 2>/dev/null || true
      done
    fi
    return 0
  fi

  # Windows Git Bash fallback
  if command -v netstat >/dev/null 2>&1 && command -v taskkill >/dev/null 2>&1; then
    local pids
    # netstat -ano columns: proto local foreign state pid
    pids="$(netstat -ano 2>/dev/null | awk -v p=":${port}$" '$2 ~ p && $4 == "LISTENING" { print $5 }' | sort -u)"
    if [[ -n "$pids" ]]; then
      for pid in $pids; do
        taskkill //PID "$pid" //F >/dev/null 2>&1 || true
      done
    fi
    return 0
  fi
}

worktree_port_offset() {
  # Stable per-worktree offset derived from the worktree root directory path.
  # Use a wide enough range to make collisions very unlikely across a handful of worktrees.
  local crc
  crc="$(printf "%s" "$ROOT_DIR" | cksum | awk '{print $1}')"
  echo $((crc % 10000))
}

compute_editor_ports() {
  local offset
  offset="${ROLLOUT_EDITOR_PORT_OFFSET:-$(worktree_port_offset)}"
  if ! is_uint "$offset"; then
    echo "invalid ROLLOUT_EDITOR_PORT_OFFSET: $offset" >&2
    exit 2
  fi

  EDITOR_API_PORT="${ROLLOUT_EDITOR_API_PORT:-$((4000 + offset))}"
  EDITOR_DEV_PORT="${ROLLOUT_EDITOR_DEV_PORT:-$((5173 + offset))}"

  if ! is_uint "$EDITOR_API_PORT" || ((EDITOR_API_PORT < 1 || EDITOR_API_PORT > 65535)); then
    echo "invalid editor api port: $EDITOR_API_PORT" >&2
    exit 2
  fi
  if ! is_uint "$EDITOR_DEV_PORT" || ((EDITOR_DEV_PORT < 1 || EDITOR_DEV_PORT > 65535)); then
    echo "invalid editor dev port: $EDITOR_DEV_PORT" >&2
    exit 2
  fi

  EDITOR_API_URL="http://127.0.0.1:${EDITOR_API_PORT}"
  # Use 127.0.0.1 rather than localhost to avoid Windows IPv4/IPv6 resolution mismatches
  # that can prevent the Tauri dev server proxy from reaching Vite.
  EDITOR_DEV_URL="http://127.0.0.1:${EDITOR_DEV_PORT}"
}

generate_tauri_worktree_config() {
  local api_port="$1"
  local dev_port="$2"

  local base_conf="$ROOT_DIR/editor/frontend/src-tauri/tauri.conf.json"
  local gen_conf="$ROOT_DIR/editor/frontend/src-tauri/gen/tauri.conf.worktree.json"

  mkdir -p "$(dirname "$gen_conf")"

  # Replace the dev server URL + CSP API endpoints so multiple worktrees can run side-by-side.
  sed \
    -e "s|http://127.0.0.1:5173|http://127.0.0.1:${dev_port}|g" \
    -e "s|http://127.0.0.1:4000|http://127.0.0.1:${api_port}|g" \
    -e "s|http://localhost:4000|http://localhost:${api_port}|g" \
    "$base_conf" >"$gen_conf"

  echo "$gen_conf"
}

TARGET="game"
MODE="headful"
FOREGROUND="true"
ACTION=""
VIDEO="false"
RECORD_FLAG="false"
RECORD_PATH=""
REPLAY_PATH=""
FFMPEG_BIN=""
RELEASE="false"
EDITOR_API_PORT=""
EDITOR_DEV_PORT=""
EDITOR_API_URL=""
EDITOR_DEV_URL=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --start|--stop|--restart|--status|--test|--e2e|--profile)
      ACTION="$1"
      shift
      ;;
    --game)
      TARGET="game"
      shift
      ;;
    --editor)
      TARGET="editor"
      shift
      ;;
    --headful)
      MODE="headful"
      shift
      ;;
    --headless)
      MODE="headless"
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
    --record)
      RECORD_FLAG="true"
      # Optional path argument (only meaningful for headful state recordings).
      if [[ $# -ge 2 ]] && ! [[ "$2" == --* ]]; then
        RECORD_PATH="$2"
        shift 2
      else
        shift
      fi
      ;;
    --replay)
      if [[ $# -lt 2 ]]; then
        echo "--replay requires a path" >&2
        exit 2
      fi
      REPLAY_PATH="$2"
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

if [[ -z "$ACTION" ]]; then
  usage
  exit 2
fi

cd "$ROOT_DIR"

# Interpret `--record`:
# - headful mode: state recording (passed through to the headful binary)
# - headless / e2e mode: legacy alias for `--video` (mp4 capture)
STATE_RECORD="false"
STATE_RECORD_PATH=""
if [[ "$RECORD_FLAG" == "true" ]]; then
  if [[ "$ACTION" == "--e2e" || "$MODE" == "headless" ]]; then
    if [[ -n "$RECORD_PATH" ]]; then
      echo "--record PATH is only supported for headful state recordings; use --video for e2e mp4 capture" >&2
      exit 2
    fi
    if [[ "$VIDEO" == "true" ]]; then
      echo "cannot combine --record and --video" >&2
      exit 2
    fi
    VIDEO="true"
    echo "warning: --record in headless/e2e mode is deprecated; use --video" >&2
  else
    STATE_RECORD="true"
    STATE_RECORD_PATH="$RECORD_PATH"
  fi
fi

if [[ -n "$REPLAY_PATH" ]]; then
  if [[ "$TARGET" != "game" ]]; then
    echo "--replay is a game-only flag; remove --editor" >&2
    exit 2
  fi
  if [[ "$MODE" != "headful" ]]; then
    echo "--replay is headful-only; remove --headless" >&2
    exit 2
  fi
  if [[ "$STATE_RECORD" == "true" ]]; then
    echo "cannot combine --record and --replay" >&2
    exit 2
  fi
  if [[ "$ACTION" != "--start" && "$ACTION" != "--restart" ]]; then
    echo "--replay requires --start (or --restart)" >&2
    exit 2
  fi
fi

if [[ "$STATE_RECORD" == "true" ]]; then
  if [[ "$TARGET" != "game" ]]; then
    echo "--record is a game-only flag; remove --editor" >&2
    exit 2
  fi
  if [[ "$MODE" != "headful" ]]; then
    echo "--record (state recording) is headful-only; remove --headless" >&2
    exit 2
  fi
  if [[ "$ACTION" != "--start" && "$ACTION" != "--restart" ]]; then
    echo "--record requires --start (or --restart)" >&2
    exit 2
  fi
fi

# Auto-detect ffmpeg for `--video` runs (useful on Windows Git Bash where PATH may not include Scoop shims).
if [[ "$VIDEO" == "true" ]] && [[ -z "$FFMPEG_BIN" ]]; then
  if command -v ffmpeg >/dev/null 2>&1; then
    FFMPEG_BIN=""
  else
    # Scoop default install location (per-user)
    if [[ -f "$HOME/scoop/shims/ffmpeg.exe" ]]; then
      FFMPEG_BIN="$(cygpath -w "$HOME/scoop/shims/ffmpeg.exe")"
    # Chocolatey shim location (machine-wide; may require admin to install)
    elif [[ -f "/c/ProgramData/chocolatey/bin/ffmpeg.exe" ]]; then
      FFMPEG_BIN="$(cygpath -w "/c/ProgramData/chocolatey/bin/ffmpeg.exe")"
    fi
  fi
fi

CARGO_PROFILE_ARGS=()
if [[ "$RELEASE" == "true" ]]; then
  CARGO_PROFILE_ARGS+=(--release)
fi

case "$ACTION" in
  --status)
    if [[ "$TARGET" == "editor" ]]; then
      local_api="stopped"
      local_app="stopped"

      if is_running_pidfile "$EDITOR_API_PID_FILE"; then
        local_api="running (pid $(cat "$EDITOR_API_PID_FILE"))"
      fi
      if is_running_pidfile "$EDITOR_APP_PID_FILE"; then
        local_app="running (pid $(cat "$EDITOR_APP_PID_FILE"))"
      fi

      echo "editor api: $local_api"
      echo "editor app: $local_app"

      if [[ "$local_api" == running* ]] && [[ "$local_app" == running* ]]; then
        exit 0
      fi
      exit 1
    fi

    if is_running_pidfile "$GAME_PID_FILE"; then
      echo "running (pid $(cat "$GAME_PID_FILE"))"
      exit 0
    fi
    echo "not running"
    exit 1
    ;;

  --stop)
    if [[ "$TARGET" == "editor" ]]; then
      stop_pidfile "$EDITOR_APP_PID_FILE"
      stop_pidfile "$EDITOR_API_PID_FILE"

      # Best-effort cleanup for stray processes (e.g. Vite dev server) if the editor crashed
      # or the PID file went stale.
      compute_editor_ports
      kill_listeners_on_port "$EDITOR_DEV_PORT"
      kill_listeners_on_port "$EDITOR_API_PORT"

      # Backward compatibility: stop old pidfiles if they exist.
      stop_pidfile "$ROOT_DIR/.go.editor-frontend.pid" || true
      stop_pidfile "$ROOT_DIR/.go.editor-backend.pid" || true
      exit 0
    fi
    stop_pidfile "$GAME_PID_FILE"
    ;;

  --restart)
    if [[ "$TARGET" == "editor" ]]; then
      stop_pidfile "$EDITOR_APP_PID_FILE"
      stop_pidfile "$EDITOR_API_PID_FILE"

      compute_editor_ports
      kill_listeners_on_port "$EDITOR_DEV_PORT"
      kill_listeners_on_port "$EDITOR_API_PORT"

      # Backward compatibility: stop old pidfiles if they exist.
      stop_pidfile "$ROOT_DIR/.go.editor-frontend.pid" || true
      stop_pidfile "$ROOT_DIR/.go.editor-backend.pid" || true
    else
      stop_pidfile "$GAME_PID_FILE"
    fi
    ACTION="--start"
    ;&

  --start)
    if [[ "$TARGET" == "editor" ]]; then
      if [[ "$MODE" == "headless" ]]; then
        echo "--headless is a game-only mode; remove it when using --editor" >&2
        exit 2
      fi

      compute_editor_ports
      tauri_conf="$(generate_tauri_worktree_config "$EDITOR_API_PORT" "$EDITOR_DEV_PORT")"

      api_running="false"
      app_running="false"
      if is_running_pidfile "$EDITOR_API_PID_FILE"; then
        api_running="true"
      fi
      if is_running_pidfile "$EDITOR_APP_PID_FILE"; then
        app_running="true"
      fi

      if [[ "$FOREGROUND" == "true" ]]; then
        if [[ "$api_running" == "true" ]] || [[ "$app_running" == "true" ]]; then
          if [[ "$api_running" == "true" ]]; then
            echo "already running editor api (pid $(cat "$EDITOR_API_PID_FILE"))"
            echo "api logs: $EDITOR_API_LOG_FILE"
          fi
          if [[ "$app_running" == "true" ]]; then
            echo "already running editor app (pid $(cat "$EDITOR_APP_PID_FILE"))"
            echo "app logs: $EDITOR_APP_LOG_FILE"
          fi
          echo "stop with: ./go.sh --stop --editor"
          exit 0
        fi

        echo "starting editor api ($EDITOR_API_URL) + tauri app ($EDITOR_DEV_URL) in foreground"
        env ROLLOUT_EDITOR_API_ADDR="127.0.0.1:$EDITOR_API_PORT" cargo run -p game --bin editor_api "${CARGO_PROFILE_ARGS[@]}" &
        api_pid=$!

        (
          cd "$ROOT_DIR/editor/frontend"
          export ROLLOUT_EDITOR_DEV_PORT="$EDITOR_DEV_PORT"
          export ROLLOUT_EDITOR_API_URL="$EDITOR_API_URL"
          export VITE_EDITOR_API="$EDITOR_API_URL"
          npm run tauri -- dev --config "$tauri_conf"
        ) &
        app_pid=$!

        trap 'kill "$api_pid" "$app_pid" 2>/dev/null || true' INT TERM EXIT
        wait
        exit 0
      fi

      if [[ "$api_running" == "true" ]] && [[ "$app_running" == "true" ]]; then
        echo "already running editor api (pid $(cat "$EDITOR_API_PID_FILE"))"
        echo "already running editor app (pid $(cat "$EDITOR_APP_PID_FILE"))"
        echo "api logs: $EDITOR_API_LOG_FILE"
        echo "app logs: $EDITOR_APP_LOG_FILE"
        exit 0
      fi

      if [[ "$app_running" == "false" ]] && ! [[ -d "$ROOT_DIR/editor/frontend/node_modules/@tauri-apps/cli" ]]; then
        echo "editor app dependencies not installed (missing @tauri-apps/cli)." >&2
        echo "run: cd editor/frontend && npm install" >&2
        exit 2
      fi

      if [[ "$api_running" == "false" ]]; then
        nohup env ROLLOUT_EDITOR_API_ADDR="127.0.0.1:$EDITOR_API_PORT" cargo run -p game --bin editor_api "${CARGO_PROFILE_ARGS[@]}" >"$EDITOR_API_LOG_FILE" 2>&1 &
        echo $! >"$EDITOR_API_PID_FILE"
        ensure_pid_alive_or_dump_logs "$EDITOR_API_PID_FILE" "$EDITOR_API_LOG_FILE" "editor api"
      fi

      if [[ "$app_running" == "false" ]]; then
        (
          cd "$ROOT_DIR/editor/frontend"
          export ROLLOUT_EDITOR_DEV_PORT="$EDITOR_DEV_PORT"
          export ROLLOUT_EDITOR_API_URL="$EDITOR_API_URL"
          export VITE_EDITOR_API="$EDITOR_API_URL"
          nohup npm run tauri -- dev --config "$tauri_conf" >"$EDITOR_APP_LOG_FILE" 2>&1 &
          echo $! >"$EDITOR_APP_PID_FILE"
          ensure_pid_alive_or_dump_logs "$EDITOR_APP_PID_FILE" "$EDITOR_APP_LOG_FILE" "editor app"
        )
      fi

      echo "started editor api (pid $(cat "$EDITOR_API_PID_FILE" 2>/dev/null || echo "?"))"
      echo "started editor app (pid $(cat "$EDITOR_APP_PID_FILE" 2>/dev/null || echo "?"))"
      echo "api: $EDITOR_API_URL"
      echo "ui:  $EDITOR_DEV_URL"
      echo "api logs: $EDITOR_API_LOG_FILE"
      echo "app logs: $EDITOR_APP_LOG_FILE"
      exit 0
    fi

    if is_running_pidfile "$GAME_PID_FILE"; then
      echo "already running (pid $(cat "$GAME_PID_FILE"))"
      exit 0
    fi

    if [[ "$MODE" == "headless" ]]; then
      if [[ "$FOREGROUND" == "true" ]]; then
        if [[ "$VIDEO" == "true" ]]; then
          if [[ -n "$FFMPEG_BIN" ]]; then
            exec env ROLLOUT_E2E_RECORD_MP4=1 ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN" cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
          fi
          exec env ROLLOUT_E2E_RECORD_MP4=1 cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
        fi
        exec cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
      fi

      # Background headless run (still useful for CI-ish usage).
      if [[ "$VIDEO" == "true" ]]; then
        if [[ -n "$FFMPEG_BIN" ]]; then
          nohup env ROLLOUT_E2E_RECORD_MP4=1 ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN" cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture >"$GAME_LOG_FILE" 2>&1 &
        else
          nohup env ROLLOUT_E2E_RECORD_MP4=1 cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture >"$GAME_LOG_FILE" 2>&1 &
        fi
      else
        nohup cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture >"$GAME_LOG_FILE" 2>&1 &
      fi
      echo $! >"$GAME_PID_FILE"
      ensure_pid_alive_or_dump_logs "$GAME_PID_FILE" "$GAME_LOG_FILE" "headless tests"
      echo "started headless tests (pid $(cat "$GAME_PID_FILE"))"
      echo "logs: $GAME_LOG_FILE"
      exit 0
    fi

    # Headful (default)
    HEADFUL_ARGS=()
    if [[ -n "$REPLAY_PATH" ]]; then
      HEADFUL_ARGS+=(--replay "$REPLAY_PATH")
    elif [[ "$STATE_RECORD" == "true" ]]; then
      HEADFUL_ARGS+=(--record)
      if [[ -n "$STATE_RECORD_PATH" ]]; then
        HEADFUL_ARGS+=("$STATE_RECORD_PATH")
      fi
    fi

    if [[ "$FOREGROUND" == "true" ]]; then
      if [[ ${#HEADFUL_ARGS[@]} -gt 0 ]]; then
        exec cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}" -- "${HEADFUL_ARGS[@]}"
      fi
      exec cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}"
    fi

    if [[ ${#HEADFUL_ARGS[@]} -gt 0 ]]; then
      pid="$(start_detached "$GAME_LOG_FILE" cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}" -- "${HEADFUL_ARGS[@]}")"
    else
      pid="$(start_detached "$GAME_LOG_FILE" cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}")"
    fi
    echo "$pid" >"$GAME_PID_FILE"
    ensure_pid_alive_or_dump_logs "$GAME_PID_FILE" "$GAME_LOG_FILE" "headful game"
    echo "started headful game (pid $(cat "$GAME_PID_FILE"))"
    echo "logs: $GAME_LOG_FILE"
    ;;

  --test)
    exec cargo test --workspace "${CARGO_PROFILE_ARGS[@]}"
    ;;

  --e2e)
    if [[ "$VIDEO" == "true" ]]; then
      if [[ -n "$FFMPEG_BIN" ]]; then
        exec env ROLLOUT_E2E_RECORD_MP4=1 ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN" cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
      fi
      exec env ROLLOUT_E2E_RECORD_MP4=1 cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
    fi
    exec cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
    ;;

  --profile)
    if [[ "$TARGET" != "game" ]]; then
      echo "--profile is a game-only action; remove --editor" >&2
      exit 2
    fi
    exec cargo run -p game --bin profile "${CARGO_PROFILE_ARGS[@]}"
    ;;
esac
