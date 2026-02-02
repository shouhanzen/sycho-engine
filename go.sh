#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Game (default)
GAME_PID_FILE="$ROOT_DIR/.go.pid"
GAME_LOG_FILE="$ROOT_DIR/.go.log"

# Editor (engine editor API + Tauri app)
EDITOR_API_PID_FILE="$ROOT_DIR/.go.editor-api.pid"
EDITOR_API_LOG_FILE="$ROOT_DIR/.go.editor-api.log"
EDITOR_APP_PID_FILE="$ROOT_DIR/.go.editor-app.pid"
EDITOR_APP_LOG_FILE="$ROOT_DIR/.go.editor-app.log"

usage() {
  cat <<'EOF'
go.sh - rollout_engine control surface

Usage:
  ./go.sh --start [--game|--editor] [--headful|--headless] [--foreground] [--release]
  ./go.sh --stop [--game|--editor]
  ./go.sh --restart [--game|--editor] [--headful|--headless] [--foreground] [--release]
  ./go.sh --status [--game|--editor]
  ./go.sh --test [--release]
  ./go.sh --e2e [--record] [--ffmpeg /path/to/ffmpeg] [--release]
  ./go.sh --help

Targets:
  --game        Control the game process (default).
  --editor      Control the engine editor API + Tauri editor app pair.

Modes:
  --headful      Run the windowed game (default). Uses: cargo run -p game --bin headful
  --headless     Run headless playtests (non-interactive). Uses: cargo test -p game --test e2e_playtest_tests

Build:
  --release      Use release profile for cargo commands (faster runtime, slower compile).

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
  --start        Starts the selected target/mode. Default is background (writes pid/log files).
                If a background process exits immediately, go.sh will print recent logs to the terminal.
  --foreground   Runs attached to the current terminal (no pid/log management).
  --stop         Stops the background process started by --start.
  --restart      Equivalent to --stop then --start.
  --status       Prints whether a background process is running.

Testing:
  --test         cargo test (workspace)
  --e2e          cargo test -p game --test e2e_playtest_tests
  --record       (e2e) record an mp4 per test via ffmpeg (requires `ffmpeg` on PATH)
  --ffmpeg PATH  (e2e) path to ffmpeg binary (sets ROLLOUT_FFMPEG_BIN)
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
  EDITOR_DEV_URL="http://localhost:${EDITOR_DEV_PORT}"
}

generate_tauri_worktree_config() {
  local api_port="$1"
  local dev_port="$2"

  local base_conf="$ROOT_DIR/editor/frontend/src-tauri/tauri.conf.json"
  local gen_conf="$ROOT_DIR/editor/frontend/src-tauri/gen/tauri.conf.worktree.json"

  mkdir -p "$(dirname "$gen_conf")"

  # Replace the dev server URL + CSP API endpoints so multiple worktrees can run side-by-side.
  sed \
    -e "s|http://localhost:5173|http://localhost:${dev_port}|g" \
    -e "s|http://127.0.0.1:4000|http://127.0.0.1:${api_port}|g" \
    -e "s|http://localhost:4000|http://localhost:${api_port}|g" \
    "$base_conf" >"$gen_conf"

  echo "$gen_conf"
}

TARGET="game"
MODE="headful"
FOREGROUND="false"
ACTION=""
RECORD="false"
FFMPEG_BIN=""
RELEASE="false"
EDITOR_API_PORT=""
EDITOR_DEV_PORT=""
EDITOR_API_URL=""
EDITOR_DEV_URL=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --start|--stop|--restart|--status|--test|--e2e)
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
    --record)
      RECORD="true"
      shift
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

# Auto-detect ffmpeg for `--record` runs (useful on Windows Git Bash where PATH may not include Scoop shims).
if [[ "$RECORD" == "true" ]] && [[ -z "$FFMPEG_BIN" ]]; then
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

      # Backward compatibility: stop old pidfiles if they exist.
      stop_pidfile "$ROOT_DIR/.go.editor-frontend.pid" || true
      stop_pidfile "$ROOT_DIR/.go.editor-backend.pid" || true
    else
      stop_pidfile "$GAME_PID_FILE"
    fi
    ACTION="--start"
    ;;&

  --start)
    if [[ "$TARGET" == "editor" ]]; then
      if [[ "$MODE" == "headless" ]]; then
        echo "--headless is a game-only mode; remove it when using --editor" >&2
        exit 2
      fi

      compute_editor_ports
      tauri_conf="$(generate_tauri_worktree_config "$EDITOR_API_PORT" "$EDITOR_DEV_PORT")"

      if [[ "$FOREGROUND" == "true" ]]; then
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

      api_running="false"
      app_running="false"
      if is_running_pidfile "$EDITOR_API_PID_FILE"; then
        api_running="true"
      fi
      if is_running_pidfile "$EDITOR_APP_PID_FILE"; then
        app_running="true"
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

    if [[ "$FOREGROUND" == "false" ]] && is_running_pidfile "$GAME_PID_FILE"; then
      echo "already running (pid $(cat "$GAME_PID_FILE"))"
      exit 0
    fi

    if [[ "$MODE" == "headless" ]]; then
      if [[ "$FOREGROUND" == "true" ]]; then
        if [[ "$RECORD" == "true" ]]; then
          if [[ -n "$FFMPEG_BIN" ]]; then
            exec env ROLLOUT_E2E_RECORD_MP4=1 ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN" cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
          fi
          exec env ROLLOUT_E2E_RECORD_MP4=1 cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
        fi
        exec cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
      fi

      # Background headless run (still useful for CI-ish usage).
      if [[ "$RECORD" == "true" ]]; then
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
    if [[ "$FOREGROUND" == "true" ]]; then
      exec cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}"
    fi

    nohup cargo run -p game --bin headful "${CARGO_PROFILE_ARGS[@]}" >"$GAME_LOG_FILE" 2>&1 &
    echo $! >"$GAME_PID_FILE"
    ensure_pid_alive_or_dump_logs "$GAME_PID_FILE" "$GAME_LOG_FILE" "headful game"
    echo "started headful game (pid $(cat "$GAME_PID_FILE"))"
    echo "logs: $GAME_LOG_FILE"
    ;;

  --test)
    exec cargo test --workspace "${CARGO_PROFILE_ARGS[@]}"
    ;;

  --e2e)
    if [[ "$RECORD" == "true" ]]; then
      if [[ -n "$FFMPEG_BIN" ]]; then
        exec env ROLLOUT_E2E_RECORD_MP4=1 ROLLOUT_FFMPEG_BIN="$FFMPEG_BIN" cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
      fi
      exec env ROLLOUT_E2E_RECORD_MP4=1 cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
    fi
    exec cargo test -p game --test e2e_playtest_tests "${CARGO_PROFILE_ARGS[@]}" -- --nocapture
    ;;
esac
