// SPDX-License-Identifier: Apache-2.0
// The single app store (Ruling 84): one reducer, no library. Server state
// (roll, frame view, history) is whatever the last command returned; the
// only client state on top is the draft overrides during a drag, the open
// screen, the selection and progress counters.

import type { ScopesData } from "../ipc/scopes";
import type {
  AnalysisEvent,
  ExportEvent,
  ExportReport,
  ExportRequest,
  FrameOverrides,
  FrameView,
  HistoryView,
  NegativeView,
  Preset,
  PresetList,
  RollView,
} from "../ipc/types";

export type Screen = "roll" | "frame" | "batch";

export interface ExportProgress {
  request: ExportRequest;
  done: number;
  total: number;
  items: ExportEvent[];
  report: ExportReport | null;
  error: string | null;
  running: boolean;
}

export interface State {
  screen: Screen;
  roll: RollView | null;
  /**
   * open_roll was sent and has not answered. The open roll is kept as it is
   * meanwhile (a refusal leaves it), so a view of it arriving now is treated
   * as any refresh of it, never as a fresh roll.
   */
  requesting: boolean;
  /** open_roll was accepted: the next roll view starts the new session afresh. */
  opening: boolean;
  analysis: { done: number; total: number; finished: boolean };
  /** Index of the frame shown on the frame screen. */
  current: number | null;
  /** Last FrameView returned for `current`. */
  frame: FrameView | null;
  /**
   * The negative of `current` (Ruling 134), shown while pass 1 has not
   * reached the frame.
   */
  negative: NegativeView | null;
  /** Overrides being dragged (sent with preview_frame); null when none. */
  draft: FrameOverrides | null;
  /** The draft when it came from a drag (preview_frame), not a commit. */
  dragged: FrameOverrides | null;
  /** Highest session generation seen in any response or event. */
  generation: number;
  history: HistoryView;
  scopes: ScopesData | null;
  /** Generation from which each frame's thumbnail must be refetched. */
  thumbs: Record<number, number>;
  /** Cached thumbnail blob URL per frame; null when its fetch failed. */
  thumbUrls: Record<number, string | null>;
  /** Cached negative thumbnail blob URL per frame (before analysis); null when its fetch failed. */
  negativeUrls: Record<number, string | null>;
  selected: number[];
  exportDialog: boolean;
  exportProgress: ExportProgress | null;
  error: string | null;
  /** The tone presets (built-in, then the user's); kept across rolls. */
  presets: Preset[];
  /** How many of `presets` are built-ins (they come first). */
  presetBuiltins: number;
  /** A message per user preset file that could not be read. */
  presetWarnings: string[];
  /** Preset preview URL per `previewKey(frame, name)`; null when its fetch failed. */
  presetUrls: Record<string, string | null>;
}

export type Action =
  /** open_roll was sent; the open roll and its state stay until it is accepted. */
  | { type: "roll/requesting" }
  | { type: "roll/opening" }
  | { type: "roll/loaded"; roll: RollView }
  | { type: "roll/failed"; error: string }
  | { type: "analysis/progress"; event: AnalysisEvent }
  | { type: "frame/open"; index: number }
  /** `committed`: the overrides this view answers; a newer draft survives it. */
  | { type: "frame/loaded"; frame: FrameView; committed?: FrameOverrides }
  | { type: "negative/loaded"; negative: NegativeView }
  /** `drag`: a slider or crop drag (previewed), not a commit on its way. */
  | { type: "frame/draft"; overrides: FrameOverrides; drag?: boolean }
  | { type: "frame/discard-draft" }
  | { type: "frame/previewed"; generation: number }
  | { type: "history/changed"; history: HistoryView }
  | { type: "scopes/loaded"; scopes: ScopesData }
  | { type: "thumbs/stale"; frames: number[]; generation: number }
  | { type: "thumbs/loaded"; frame: number; url: string | null }
  | { type: "negatives/loaded"; frame: number; url: string | null }
  | { type: "select/toggle"; index: number }
  | { type: "select/clear" }
  | { type: "screen/show"; screen: Screen }
  | { type: "export/dialog"; open: boolean }
  | { type: "export/started"; request: ExportRequest; total: number }
  | { type: "export/progress"; event: ExportEvent }
  | { type: "export/finished"; report: ExportReport }
  | { type: "export/failed"; error: string }
  | { type: "error/set"; error: string }
  | { type: "error/clear" }
  | { type: "presets/loaded"; list: PresetList }
  | { type: "presets/preview"; frame: number; name: string; url: string | null };

export const EMPTY_HISTORY: HistoryView = { can_undo: false, can_redo: false, len: 0, cursor: 0 };

export const initialState: State = {
  screen: "roll",
  roll: null,
  requesting: false,
  opening: false,
  analysis: { done: 0, total: 0, finished: false },
  current: null,
  frame: null,
  negative: null,
  draft: null,
  dragged: null,
  generation: 0,
  history: EMPTY_HISTORY,
  scopes: null,
  thumbs: {},
  thumbUrls: {},
  negativeUrls: {},
  selected: [],
  exportDialog: false,
  exportProgress: null,
  error: null,
  presets: [],
  presetBuiltins: 0,
  presetWarnings: [],
  presetUrls: {},
};

/** The `presetUrls` key of a frame's preview of a preset. */
export function previewKey(frame: number, name: string): string {
  return JSON.stringify([frame, name]);
}

function frameCount(state: State): number {
  return state.roll?.frames.length ?? 0;
}

export function reducer(state: State, action: Action): State {
  switch (action.type) {
    case "roll/requesting":
      // Not `opening`: until open_roll answers, views of the open roll (from
      // its analysis events) must not reset it.
      return { ...state, requesting: true };
    case "roll/opening":
      // Opening a folder (even the one already open) starts a new session:
      // its analysis, generations, history and frame state start afresh.
      return {
        ...initialState,
        roll: state.roll,
        opening: true,
        presets: state.presets,
        presetBuiltins: state.presetBuiltins,
        presetWarnings: state.presetWarnings,
      };
    case "roll/loaded": {
      const { roll } = action;
      const fresh = state.opening || state.roll?.dir !== roll.dir;
      if (fresh) {
        const done = roll.frames.filter((f) => f.status !== "pending").length;
        return {
          ...initialState,
          presets: state.presets,
          presetBuiltins: state.presetBuiltins,
          presetWarnings: state.presetWarnings,
          roll,
          generation: roll.generation,
          history: roll.history,
          analysis: { done, total: roll.frames.length, finished: roll.analysed },
        };
      }
      return {
        ...state,
        roll,
        generation: Math.max(state.generation, roll.generation),
        history: roll.history,
        analysis: { ...state.analysis, finished: state.analysis.finished || roll.analysed },
      };
    }
    case "roll/failed":
      return { ...state, requesting: false, opening: false, error: action.error };
    case "analysis/progress": {
      const e = action.event;
      return {
        ...state,
        analysis: { done: e.done, total: e.total, finished: e.finished },
      };
    }
    case "frame/open": {
      const n = frameCount(state);
      if (action.index < 0 || action.index >= n) return state;
      const changed = action.index !== state.current;
      return {
        ...state,
        screen: "frame",
        current: action.index,
        frame: changed ? null : state.frame,
        negative: changed ? null : state.negative,
        draft: changed ? null : state.draft,
        scopes: changed ? null : state.scopes,
      };
    }
    case "frame/loaded": {
      const f = action.frame;
      if (f.index !== state.current) return state; // a late reply for a frame left behind
      return {
        ...state,
        frame: f,
        draft: action.committed === undefined || state.draft === action.committed ? null : state.draft,
        generation: Math.max(state.generation, f.generation),
        history: f.history,
      };
    }
    case "negative/loaded":
      if (action.negative.index !== state.current) return state;
      return { ...state, negative: action.negative };
    case "frame/draft":
      return { ...state, draft: action.overrides, dragged: action.drag === true ? action.overrides : null };
    case "frame/discard-draft":
      return { ...state, draft: null };
    case "frame/previewed":
      return { ...state, generation: Math.max(state.generation, action.generation) };
    case "history/changed":
      return { ...state, history: action.history };
    case "scopes/loaded": {
      const s = action.scopes;
      if (s.frame !== state.current) return state;
      if (state.scopes && state.scopes.frame === s.frame && s.generation < state.scopes.generation) {
        return state; // older than what is drawn
      }
      return { ...state, scopes: s, generation: Math.max(state.generation, s.generation) };
    }
    case "thumbs/stale": {
      const thumbs = { ...state.thumbs };
      for (const f of action.frames) thumbs[f] = Math.max(thumbs[f] ?? 0, action.generation);
      return { ...state, thumbs, generation: Math.max(state.generation, action.generation) };
    }
    case "thumbs/loaded":
      return { ...state, thumbUrls: { ...state.thumbUrls, [action.frame]: action.url } };
    case "negatives/loaded":
      return { ...state, negativeUrls: { ...state.negativeUrls, [action.frame]: action.url } };
    case "select/toggle": {
      const has = state.selected.includes(action.index);
      const selected = has
        ? state.selected.filter((i) => i !== action.index)
        : [...state.selected, action.index].sort((a, b) => a - b);
      return { ...state, selected };
    }
    case "select/clear":
      return { ...state, selected: [] };
    case "screen/show":
      return { ...state, screen: action.screen, draft: action.screen === "frame" ? state.draft : null };
    case "export/dialog":
      return { ...state, exportDialog: action.open };
    case "export/started":
      return {
        ...state,
        exportDialog: false,
        exportProgress: {
          request: action.request,
          done: 0,
          total: action.total,
          items: [],
          report: null,
          error: null,
          running: true,
        },
      };
    case "export/progress": {
      const p = state.exportProgress;
      if (!p || !p.running) return state;
      return {
        ...state,
        exportProgress: {
          ...p,
          done: Math.max(p.done, action.event.done),
          total: action.event.total,
          items: [...p.items, action.event],
        },
      };
    }
    case "export/finished": {
      const p = state.exportProgress;
      if (!p) return state;
      return { ...state, exportProgress: { ...p, report: action.report, running: false, done: p.total } };
    }
    case "export/failed": {
      const p = state.exportProgress;
      if (!p) return { ...state, error: action.error };
      return { ...state, exportProgress: { ...p, error: action.error, running: false } };
    }
    case "error/set":
      return { ...state, error: action.error };
    case "error/clear":
      return { ...state, error: null };
    case "presets/loaded":
      return {
        ...state,
        presets: action.list.presets,
        presetBuiltins: action.list.builtins,
        presetWarnings: action.list.warnings,
      };
    case "presets/preview":
      return { ...state, presetUrls: { ...state.presetUrls, [previewKey(action.frame, action.name)]: action.url } };
  }
}

/** True while the roll waits for Analyse (Ruling 136). */
export function awaitingAnalysis(state: State): boolean {
  return state.roll?.awaiting_analysis === true;
}

/** True while pass 1 runs (started and not finished). */
export function analysing(state: State): boolean {
  const { roll } = state;
  return roll !== null && !awaitingAnalysis(state) && !(roll.analysed || state.analysis.finished);
}

/** True while an export runs. */
export function exporting(state: State): boolean {
  return state.exportProgress?.running === true;
}

/** Export waits for the analysis (pass 1) and for an export already running. */
export function exportBlocked(state: State): boolean {
  return awaitingAnalysis(state) || analysing(state) || exporting(state);
}

/**
 * Undo and redo wait for the analysis, as the edits do (the backend refuses
 * them before it), and need a roll.
 */
export function historyBlocked(state: State): boolean {
  return state.roll === null || awaitingAnalysis(state) || analysing(state);
}

/**
 * A drag is in progress: the draft came from a drag, not from a commit (a
 * tick box, a key step or a typed value), which the preview need not draw
 * at a coarser level for.
 */
export function dragging(state: State): boolean {
  return state.draft !== null && state.draft === state.dragged;
}

/** Overrides the sliders show: the draft while dragging, else the committed ones. */
export function shownOverrides(state: State): FrameOverrides | null {
  return state.draft ?? state.frame?.overrides ?? null;
}

/** Index of the frame `step` away from the current one, or null at either end. */
export function neighbour(state: State, step: 1 | -1): number | null {
  if (state.current === null) return null;
  const next = state.current + step;
  return next >= 0 && next < frameCount(state) ? next : null;
}
