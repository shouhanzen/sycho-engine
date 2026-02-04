export interface EditorAction {
  id: string;
  label: string;
}

export interface EditorManifest {
  title: string;
  actions: EditorAction[];
}

export type GridOrigin = "bottomLeft" | "topLeft";

export type Rgba = [number, number, number, number];

export interface EditorPaletteEntry {
  value: number;
  rgba: Rgba;
  label: string | null;
}

export interface EditorGrid {
  origin: GridOrigin;
  cells: number[][];
  palette: EditorPaletteEntry[] | null;
}

export interface EditorStat {
  label: string;
  value: string;
}

export interface EditorSnapshot {
  frame: number;
  state: unknown;
  stats: EditorStat[];
  grid: EditorGrid | null;
}

export interface EditorTimeline {
  frame: number;
  historyLen: number;
  canRewind: boolean;
  canForward: boolean;
}