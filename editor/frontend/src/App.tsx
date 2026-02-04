import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  fetchGameStatus,
  fetchHealth,
  fetchManifest,
  fetchState,
  fetchTimeline,
  forward,
  getApiBase,
  launchGame,
  reset,
  rewind,
  seek,
  setApiBase,
  step,
} from "./api";
import { EditorManifest, EditorSnapshot, EditorTimeline } from "./types";

function formatGrid(snapshot: EditorSnapshot | null): string {
  const grid = snapshot?.grid;
  if (!grid) {
    return "";
  }

  const rows = grid.origin === "bottomLeft" ? grid.cells.slice().reverse() : grid.cells;
  return rows.map((row) => row.map((cell) => (cell ? "#" : ".")).join(" ")).join("\n");
}

function formatJson(value: unknown): string {
  if (value === null || value === undefined) {
    return "";
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export default function App() {
  const envApiBase = import.meta.env.VITE_EDITOR_API;
  const apiBaseLocked = Boolean(envApiBase && envApiBase.trim().length);

  const [apiBaseInput, setApiBaseInput] = useState(() => getApiBase());
  const [manifest, setManifest] = useState<EditorManifest | null>(null);
  const [snapshot, setSnapshot] = useState<EditorSnapshot | null>(null);
  const [timeline, setTimeline] = useState<EditorTimeline | null>(null);
  const [health, setHealth] = useState<string>("checking");
  const [gameStatus, setGameStatus] = useState<{ running: boolean; detail: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [seekFrame, setSeekFrame] = useState<number>(0);

  const autoRetryAttempt = useRef(0);
  const autoRetryTimer = useRef<number | null>(null);
  const stepInFlight = useRef(false);
  const pollInFlight = useRef(false);
  const scrubbingRef = useRef(false);

  const boardText = useMemo(() => formatGrid(snapshot), [snapshot]);
  const stateText = useMemo(() => formatJson(snapshot?.state), [snapshot]);

  const gameRunning = gameStatus?.running === true;

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const [healthResponse, mf, gs] = await Promise.all([
        fetchHealth(),
        fetchManifest(),
        fetchGameStatus().catch(() => null),
      ]);
      setHealth(healthResponse.status);
      setManifest(mf);
      if (gs) {
        setGameStatus(gs);
      }

      if (gs?.running) {
        const [stateRes, tlRes] = await Promise.allSettled([fetchState(), fetchTimeline()]);
        if (stateRes.status === "fulfilled") {
          setSnapshot(stateRes.value);
        }
        if (tlRes.status === "fulfilled") {
          setTimeline(tlRes.value);
          if (!scrubbingRef.current) {
            setSeekFrame(tlRes.value.frame);
          }
        }
      } else {
        setSnapshot(null);
        setTimeline(null);
        if (!scrubbingRef.current) {
          setSeekFrame(0);
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
      setHealth("error");
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!gameRunning) {
      return;
    }

    const pollMs = 200;
    const timer = window.setInterval(() => {
      if (pollInFlight.current || scrubbingRef.current) {
        return;
      }
      pollInFlight.current = true;
      void (async () => {
        const [stateRes, tlRes] = await Promise.allSettled([fetchState(), fetchTimeline()]);
        if (stateRes.status === "fulfilled") {
          setSnapshot(stateRes.value);
        }
        if (tlRes.status === "fulfilled") {
          setTimeline(tlRes.value);
          setSeekFrame(tlRes.value.frame);
        }
      })().finally(() => {
        pollInFlight.current = false;
      });
    }, pollMs);

    return () => {
      window.clearInterval(timer);
    };
  }, [gameRunning]);

  useEffect(() => {
    if (health === "ok") {
      autoRetryAttempt.current = 0;
      if (autoRetryTimer.current !== null) {
        window.clearTimeout(autoRetryTimer.current);
        autoRetryTimer.current = null;
      }
      return;
    }

    // Only auto-retry when we couldn't reach the API (common on cold start while the backend compiles).
    if (busy || health !== "error") {
      return;
    }

    if (autoRetryAttempt.current >= 20) {
      return;
    }

    const attempt = autoRetryAttempt.current;
    const delayMs = Math.min(2000, 200 * 2 ** attempt);

    autoRetryTimer.current = window.setTimeout(() => {
      autoRetryAttempt.current = attempt + 1;
      void refresh();
    }, delayMs);

    return () => {
      if (autoRetryTimer.current !== null) {
        window.clearTimeout(autoRetryTimer.current);
        autoRetryTimer.current = null;
      }
    };
  }, [busy, health, refresh]);

  const handleConnect = async () => {
    setApiBase(apiBaseInput.trim());
    await refresh();
  };

  const handleStep = useCallback(async (actionId: string) => {
    if (stepInFlight.current) {
      return;
    }
    stepInFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      const next = await step(actionId);
      setSnapshot(next);
      const tl = await fetchTimeline();
      setTimeline(tl);
      setSeekFrame(tl.frame);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      stepInFlight.current = false;
      setBusy(false);
    }
  }, []);

  const handleSeek = async (frame: number) => {
    setBusy(true);
    setError(null);
    try {
      const next = await seek(frame);
      setSnapshot(next);
      const tl = await fetchTimeline();
      setTimeline(tl);
      setSeekFrame(tl.frame);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  const handleRewind = async (frames: number) => {
    setBusy(true);
    setError(null);
    try {
      const next = await rewind(frames);
      setSnapshot(next);
      const tl = await fetchTimeline();
      setTimeline(tl);
      setSeekFrame(tl.frame);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  const handleForward = async (frames: number) => {
    setBusy(true);
    setError(null);
    try {
      const next = await forward(frames);
      setSnapshot(next);
      const tl = await fetchTimeline();
      setTimeline(tl);
      setSeekFrame(tl.frame);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  const handleReset = async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await reset();
      setSnapshot(next);
      const tl = await fetchTimeline();
      setTimeline(tl);
      setSeekFrame(tl.frame);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  const handleLaunchGame = async () => {
    setBusy(true);
    setError(null);
    try {
      const launched = await launchGame();
      await refresh();
      if (!launched.ok) {
        setError(launched.detail || "Launch failed");
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  const maxFrame = Math.max(0, (timeline?.historyLen ?? 1) - 1);
  const seekFrameClamped = Math.min(Math.max(0, seekFrame), maxFrame);

  return (
    <div className="app">
      <header className="header dock">
        <div className="header-left">
          <h1>{manifest?.title ?? "Rollout Editor"}</h1>
          <div className="status-line">
            <span>API: {health}</span>
            <span>
              Game:{" "}
              {gameStatus ? (gameStatus.running ? "running" : "stopped") : "unknown"}
            </span>
          </div>

          <div className="api-config">
            <label>
              API
              <input
                value={apiBaseInput}
                onChange={(e) => setApiBaseInput(e.target.value)}
                placeholder="http://127.0.0.1:4000"
                disabled={busy || apiBaseLocked}
              />
            </label>
            <button onClick={handleConnect} disabled={busy || apiBaseLocked}>
              Connect
            </button>
          </div>
        </div>

        <div className="header-actions">
          <button onClick={handleLaunchGame} disabled={busy}>
            Launch Headful Game
          </button>
          <button onClick={refresh} disabled={busy}>
            Refresh
          </button>
          <button onClick={handleReset} disabled={busy || !gameRunning}>
            Reset
          </button>
        </div>
      </header>

      {error ? <div className="error">{error}</div> : null}

      <div className="workspace">
        <div className="left">
          <section className="dock viewport">
            <h2>Viewport</h2>
            <pre>{boardText || "No viewport data yet."}</pre>
          </section>

          <section className="dock actions">
            <h2>Actions</h2>
            <div className="actions-grid">
              {(manifest?.actions ?? []).map((action) => (
                <button
                  key={action.id}
                  onClick={() => handleStep(action.id)}
                  disabled={busy || !gameRunning}
                >
                  {action.label}
                </button>
              ))}
            </div>
          </section>
        </div>

        <aside className="dock inspector">
          <h2>Inspector</h2>

          <div className="inspector-meta">
            <div>
              <div className="meta-label">Frame</div>
              <div className="meta-value">{snapshot?.frame ?? "-"}</div>
            </div>
            <div>
              <div className="meta-label">History</div>
              <div className="meta-value">{timeline?.historyLen ?? "-"}</div>
            </div>
          </div>

          <div className="inspector-section">
            <h3>State</h3>
            <pre>{stateText || "No state yet."}</pre>
          </div>

          <div className="inspector-section">
            <h3>Stats</h3>
            <dl className="stats">
              {(snapshot?.stats ?? []).map((stat) => (
                <div key={stat.label}>
                  <dt>{stat.label}</dt>
                  <dd>{stat.value}</dd>
                </div>
              ))}
            </dl>
          </div>
        </aside>

        <section className="dock timeline">
          <div className="timeline-header">
            <h2>Timeline</h2>
            <div className="timeline-cursor">
              {timeline ? `${timeline.frame} / ${timeline.historyLen - 1}` : "-"}
            </div>
          </div>

          {!gameRunning ? (
            <div className="hint">
              Game not running. Click <strong>Launch Headful Game</strong> to start and attach the
              timeline.
            </div>
          ) : !timeline ? (
            <div className="hint">Waiting for the game timeline (the game may still be starting)...</div>
          ) : timeline.historyLen <= 1 ? (
            <div className="hint">
              No history yet. Play in the game window (or use the <strong>Actions</strong> buttons)
              to record frames.
            </div>
          ) : null}

          <div className="timeline-seek">
            <input
              type="range"
              min={0}
              max={maxFrame}
              value={seekFrameClamped}
              onChange={(e) => setSeekFrame(Number(e.target.value))}
              onPointerDown={() => {
                scrubbingRef.current = true;
              }}
              onPointerUp={() => {
                scrubbingRef.current = false;
                if (!timeline || busy) {
                  return;
                }
                if (seekFrameClamped === timeline.frame) {
                  return;
                }
                void handleSeek(seekFrameClamped);
              }}
              onKeyUp={() => {
                if (!timeline || busy) {
                  return;
                }
                if (seekFrameClamped === timeline.frame) {
                  return;
                }
                void handleSeek(seekFrameClamped);
              }}
              disabled={busy || !timeline || !gameRunning}
            />
            <button
              onClick={() => handleSeek(seekFrameClamped)}
              disabled={busy || !timeline || !gameRunning || seekFrameClamped === timeline?.frame}
            >
              Seek
            </button>
          </div>

          <div className="actions-grid" style={{ marginTop: 12 }}>
            <button
              onClick={() => handleRewind(timeline?.frame ?? 0)}
              disabled={busy || !timeline || !timeline.canRewind}
            >
              Jump Start
            </button>
            <button
              onClick={() => handleRewind(10)}
              disabled={busy || !timeline || !timeline.canRewind}
            >
              Rewind 10
            </button>
            <button
              onClick={() => handleRewind(1)}
              disabled={busy || !timeline || !timeline.canRewind}
            >
              Rewind 1
            </button>
            <button
              onClick={() => handleForward(1)}
              disabled={busy || !timeline || !timeline.canForward}
            >
              Forward 1
            </button>
            <button
              onClick={() => handleForward(10)}
              disabled={busy || !timeline || !timeline.canForward}
            >
              Forward 10
            </button>
            <button
              onClick={() =>
                handleForward(
                  timeline ? Math.max(0, timeline.historyLen - 1 - timeline.frame) : 0,
                )
              }
              disabled={busy || !timeline || !timeline.canForward}
            >
              Jump End
            </button>
          </div>
        </section>
      </div>
    </div>
  );
}

