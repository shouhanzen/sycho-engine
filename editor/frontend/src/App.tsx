import { useCallback, useEffect, useMemo, useState } from "react";

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

  const boardText = useMemo(() => formatGrid(snapshot), [snapshot]);
  const stateText = useMemo(() => formatJson(snapshot?.state), [snapshot]);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const [healthResponse, mf, state, tl, gs] = await Promise.all([
        fetchHealth(),
        fetchManifest(),
        fetchState(),
        fetchTimeline(),
        fetchGameStatus().catch(() => null),
      ]);
      setHealth(healthResponse.status);
      setManifest(mf);
      setSnapshot(state);
      setTimeline(tl);
      if (gs) {
        setGameStatus(gs);
      }
      setSeekFrame(tl.frame);
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

  const handleConnect = async () => {
    setApiBase(apiBaseInput.trim());
    await refresh();
  };

  const handleStep = async (actionId: string) => {
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
      setBusy(false);
    }
  };

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
      const gs = await fetchGameStatus().catch(() => null);
      if (gs) {
        setGameStatus(gs);
      }
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
            Launch Game
          </button>
          <button onClick={refresh} disabled={busy}>
            Refresh
          </button>
          <button onClick={handleReset} disabled={busy}>
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
                  disabled={busy}
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

          <div className="timeline-seek">
            <input
              type="range"
              min={0}
              max={maxFrame}
              value={seekFrameClamped}
              onChange={(e) => setSeekFrame(Number(e.target.value))}
              disabled={busy || !timeline}
            />
            <button
              onClick={() => handleSeek(seekFrameClamped)}
              disabled={busy || !timeline || seekFrameClamped === timeline?.frame}
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

