import { EditorManifest, EditorSnapshot, EditorTimeline } from "./types";

const API_BASE_STORAGE_KEY = "rollout.editor.apiBase";

function normalizeBase(base: string): string {
  return base.replace(/\/+$/, "");
}

function envApiBase(): string | null {
  const base = import.meta.env.VITE_EDITOR_API;
  if (!base) {
    return null;
  }
  const trimmed = base.trim();
  return trimmed.length ? trimmed : null;
}

export function getApiBase(): string {
  const env = envApiBase();
  if (env) {
    return normalizeBase(env);
  }

  try {
    const stored = localStorage.getItem(API_BASE_STORAGE_KEY);
    if (stored && stored.trim().length) {
      return normalizeBase(stored.trim());
    }
  } catch {
    // ignore (e.g. storage not available)
  }

  return "http://127.0.0.1:4000";
}

export function setApiBase(base: string): void {
  const env = envApiBase();
  if (env) {
    // In dev/prod, env vars are treated as authoritative (do not overwrite).
    return;
  }

  try {
    localStorage.setItem(API_BASE_STORAGE_KEY, base);
  } catch {
    // ignore (e.g. storage not available)
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${getApiBase()}${path}`, {
    headers: {
      "Content-Type": "application/json",
    },
    ...init,
  });

  if (!response.ok) {
    const message = await response.text();
    throw new Error(message || `Request failed: ${response.status}`);
  }

  return response.json() as Promise<T>;
}

export function fetchHealth(): Promise<{ status: string }> {
  return fetch(`${getApiBase()}/api/health`)
    .then(async (response) => {
      if (!response.ok) {
        const message = await response.text();
        throw new Error(message || `Request failed: ${response.status}`);
      }
      const text = await response.text();
      return { status: text || "ok" };
    })
    .catch((err) => {
      throw err instanceof Error ? err : new Error("Unknown error");
    });
}

export function fetchManifest(): Promise<EditorManifest> {
  return request("/api/manifest");
}

export function fetchState(): Promise<EditorSnapshot> {
  return request("/api/agent/state");
}

export function fetchTimeline(): Promise<EditorTimeline> {
  return request("/api/agent/timeline");
}

export function step(actionId: string): Promise<EditorSnapshot> {
  return request("/api/agent/step", {
    method: "POST",
    body: JSON.stringify({ actionId }),
  });
}

export function rewind(frames: number): Promise<EditorSnapshot> {
  return request("/api/agent/rewind", {
    method: "POST",
    body: JSON.stringify({ frames }),
  });
}

export function forward(frames: number): Promise<EditorSnapshot> {
  return request("/api/agent/forward", {
    method: "POST",
    body: JSON.stringify({ frames }),
  });
}

export function reset(): Promise<EditorSnapshot> {
  return request("/api/agent/reset", { method: "POST" });
}
