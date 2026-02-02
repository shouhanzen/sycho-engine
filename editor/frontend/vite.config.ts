import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

function readPort(env: Record<string, string | undefined>): number {
  const raw = env.ROLLOUT_EDITOR_DEV_PORT ?? env.VITE_PORT;
  if (!raw) {
    return 5173;
  }
  const port = Number(raw);
  if (!Number.isFinite(port) || port <= 0) {
    return 5173;
  }
  return port;
}

function readApiBase(env: Record<string, string | undefined>): string {
  return env.ROLLOUT_EDITOR_API_URL ?? env.VITE_EDITOR_API ?? "http://127.0.0.1:4000";
}

export default defineConfig({
  plugins: [react()],
  server: {
    port: readPort(process.env),
    strictPort: true,
    proxy: {
      "/api": readApiBase(process.env),
    },
  },
});
