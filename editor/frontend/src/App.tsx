import { useCallback, useEffect, useMemo, useState } from "react";

import {
  fetchHealth,
  fetchManifest,
  fetchState,
  fetchTimeline,
  forward,
  getApiBase,
  reset,
  rewind,
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

export default function App() {
  const envApiBase = import.meta.env.VITE_EDITOR_API;
  const apiBaseLocked = Boolean(envApiBase && envApiBase.trim().length);

  const [apiBaseInput, setApiBaseInput] = useState(() => getApiBase());
  const [manifest, setManifest] = useState<EditorManifest | null>(null);
  const [snapshot, setSnapshot] = useState<EditorSnapshot | null>(null);
  const [timeline, setTimeline] = useState<EditorTimeline | null>(null);
  const [health, setHealth] = useState<string>("checking");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const boardText = useMemo(() => formatGrid(snapshot), [snapshot]);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const [healthResponse, mf, state, tl] = await Promise.all([
        fetchHealth(),
        fetchManifest(),
        fetchState(),
        fetchTimeline(),
      ]);
      setHealth(healthResponse.status);
      setManifest(mf);
      setSnapshot(state);
      setTimeline(tl);
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
    } catch (err) {
      setError(err instanceof Error ? err.message : "Unknown error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app">
      <header className="header">
        <div>
          <h1>{manifest?.title ?? "Rollout Editor"}</h1>
          <p>Agent interface status: {health}</p>
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
          <button onClick={refresh} disabled={busy}>
            Refresh
          </button>
          <button onClick={handleReset} disabled={busy}>
            Reset
          </button>
        </div>
      </header>

      {error ? <div className="error">{error}</div> : null}

      <section className="panel">
        <div className="panel-details">
          <h2>Snapshot</h2>
          <dl>
            <div>
              <dt>Frame</dt>
              <dd>{snapshot?.frame ?? "-"}</dd>
            </div>
            {(snapshot?.stats ?? []).map((stat) => (
              <div key={stat.label}>
                <dt>{stat.label}</dt>
                <dd>{stat.value}</dd>
              </div>
            ))}
          </dl>

          <h2>Timeline</h2>
          <dl>
            <div>
              <dt>History Len</dt>
              <dd>{timeline?.historyLen ?? "-"}</dd>
            </div>
            <div>
              <dt>Cursor</dt>
              <dd>
                {timeline ? `${timeline.frame} / ${timeline.historyLen - 1}` : "-"}
              </dd>
            </div>
          </dl>

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
        </div>

        <div className="panel-board">
          <h2>Grid</h2>
          <pre>{boardText || "No data yet."}</pre>
        </div>
      </section>

      <section className="actions">
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
  );
}
