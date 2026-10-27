// SPDX-License-Identifier: Apache-2.0
// React context around the reducer, plus the command-calling actions. Frame
// edits run through one serial queue so a preview can never land after the
// commit that followed it, and queued previews collapse to the latest one.

import { createContext, createElement, useContext, useMemo, useReducer, useRef, type ReactNode } from "react";
import * as cmd from "../ipc/commands";
import type {
  CropRect,
  ExportRequest,
  FrameOverrides,
  FrameView,
  PrintLook,
  RollPresetScope,
  RollSettings,
} from "../ipc/types";
import { PresetPreviewCache } from "../presets";
import { sameGeometry, withRollArea } from "../sliders";
import { ThumbCache } from "./thumbs";
import { type Action, type State, initialState, reducer } from "./reducer";

export function errorText(e: unknown): string {
  if (e instanceof cmd.CommandFailure) {
    return e.kind === "transport" ? `${e.command}: ${e.message}` : e.message;
  }
  if (e instanceof Error) return e.message;
  return String(e);
}

/** Collapsing serial queue: runs jobs one at a time; a queued preview is dropped when a newer job arrives. */
export class EditQueue {
  private running = false;
  private queue: Array<{ job: () => Promise<void>; droppable: boolean }> = [];

  push(job: () => Promise<void>, droppable: boolean): void {
    this.queue = this.queue.filter((q) => !q.droppable);
    this.queue.push({ job, droppable });
    void this.pump();
  }

  /** Resolves when every queued job has run. */
  async idle(): Promise<void> {
    while (this.running || this.queue.length > 0) {
      await new Promise((r) => setTimeout(r, 5));
    }
  }

  private async pump(): Promise<void> {
    if (this.running) return;
    this.running = true;
    try {
      for (let next = this.queue.shift(); next; next = this.queue.shift()) {
        await next.job();
      }
    } finally {
      this.running = false;
    }
  }
}

export interface Actions {
  openRoll(dir: string): Promise<void>;
  refreshRoll(): Promise<void>;
  /** Fetches the roll view and, on the frame screen, the frame view (a drag keeps its draft). */
  refreshViews(): Promise<void>;
  openFrame(index: number): Promise<void>;
  /** A drag step: preview only (no history, no sidecar). */
  drag(overrides: FrameOverrides): void;
  /** Release, keyboard entry or a toggle: one history entry. */
  /**
   * Commits overrides to `frame` (default: the frame shown now). Code that
   * commits after an await passes the frame it captured before the await, so
   * navigating meanwhile can never send one frame's overrides to another.
   */
  commit(overrides: FrameOverrides, frame?: number): void;
  /**
   * Drops an uncommitted drag: the shown overrides return to the committed
   * ones and the session preview is reset to them.
   */
  revertDraft(): void;
  commitRoll(roll: RollSettings): Promise<boolean>;
  lock(dmin: boolean, gamma: boolean, cast: boolean): void;
  undo(): void;
  redo(): void;
  runExport(request: ExportRequest, total: number): Promise<void>;
  /**
   * Before a close or quit (`negg://closing`): the field being typed in is
   * left (so its value commits), and every queued edit is sent.
   */
  prepareClose(): Promise<void>;
  /** Fetches a frame's thumbnail unless the cache already holds one at least this fresh. */
  requestThumb(frame: number, stale: number): void;
  /** Fetches a frame's negative thumbnail (before analysis too) unless cached. */
  requestNegativeThumb(frame: number): void;
  /** Fetches the negative's size and pyramid of `frame` (the Negative toggle). */
  loadNegative(frame: number): Promise<void>;
  /** The Analyse button: starts the analysis of a roll that awaits it (Ruling 136). */
  startAnalysis(): Promise<void>;
  /**
   * Sets (or with null removes) the roll's analysis area, before or after
   * analysis; after it the roll is analysed again in the background.
   */
  setRollArea(area: CropRect | null): Promise<boolean>;
  /** "Use roll analysis area": the frame on screen drops its own area. */
  useRollArea(): void;
  /**
   * Makes this frame's crop and analysis buffer the roll's (commit_roll);
   * when the frame's geometry override then equals the roll geometry, the
   * override is removed so the frame follows later roll changes.
   */
  frameCropToRoll(): Promise<void>;
  /** Fetches a preset preview of `frame` unless one at this version is cached. */
  requestPresetPreview(frame: number, name: string, version: string): void;
}

export interface PresetActions {
  /**
   * Applies a preset to `frame` (default: the frame shown now) as one
   * history entry, after every edit already queued.
   */
  applyFramePreset(name: string, frame?: number): void;
  /** The frames with a look of their own, once queued edits are sent; null when refused (reported). */
  framesWithOwnLook(): Promise<number[] | null>;
  /** Applies a preset to the roll (one history entry); false when refused (reported). */
  applyRollPreset(name: string, scope: RollPresetScope): Promise<boolean>;
  /** Fetches the preset list. */
  loadPresets(): Promise<void>;
  /**
   * Saves `look` as a user preset; resolves to why it was refused, or null.
   * Once saved, the preset is applied (as `applyFramePreset` does) to `frame`
   * (default: the frame shown when the save started), the frame `look` was
   * taken from, so its heading names the new preset; the values are already
   * the frame's, so its render does not change.
   */
  savePreset(name: string, look: PrintLook, frame?: number): Promise<string | null>;
}

export interface PresetContext {
  queue: EditQueue;
  dispatch: (a: Action) => void;
  fail: (e: unknown) => void;
  current: () => number | null;
  onFrameScreen: () => boolean;
  reloadFrame: (index: number) => Promise<void>;
  /** Roll edits sent outside the queue and not answered yet (a close waits for them). */
  inflight: { n: number };
}

/**
 * The preset actions. A frame preset goes through the edit queue as a commit
 * does (so a pending slider edit is sent first and never lost); a roll preset
 * waits for the queue and refreshes the roll and the frame shown, as
 * commitRoll does. Thumbnails follow from the backend's `negg://thumbs`.
 */
export function presetActions(ctx: PresetContext): PresetActions {
  const { queue, dispatch, fail } = ctx;
  const actions: PresetActions = {
    applyFramePreset(name, target) {
      const frame = target ?? ctx.current();
      if (frame === null) return;
      queue.push(async () => {
        try {
          // No `committed`: the view answers every draft before it.
          dispatch({ type: "frame/loaded", frame: await cmd.applyFramePreset(frame, name) });
        } catch (e) {
          fail(e);
          await ctx.reloadFrame(frame).catch(fail);
        }
      }, false);
    },
    async framesWithOwnLook() {
      await queue.idle();
      try {
        return await cmd.framesWithOwnLook();
      } catch (e) {
        fail(e);
        return null;
      }
    },
    async applyRollPreset(name, scope) {
      ctx.inflight.n += 1;
      await queue.idle();
      try {
        dispatch({ type: "roll/loaded", roll: await cmd.applyRollPreset(name, scope) });
        const frame = ctx.current();
        if (frame !== null && ctx.onFrameScreen()) await ctx.reloadFrame(frame);
        return true;
      } catch (e) {
        fail(e);
        return false;
      } finally {
        ctx.inflight.n -= 1;
      }
    },
    async loadPresets() {
      try {
        dispatch({ type: "presets/loaded", list: await cmd.listPresets() });
      } catch (e) {
        fail(e);
      }
    },
    async savePreset(name, look, target) {
      const frame = target ?? ctx.current();
      try {
        dispatch({ type: "presets/loaded", list: await cmd.savePreset(name, look) });
      } catch (e) {
        return errorText(e);
      }
      if (frame !== null) actions.applyFramePreset(name, frame);
      return null;
    },
  };
  return actions;
}

export interface CropToRollDeps {
  commitRoll: (roll: RollSettings) => Promise<boolean>;
  frameView: (frame: number) => Promise<FrameView>;
  commit: (overrides: FrameOverrides, frame: number) => void;
  fail: (e: unknown) => void;
}

/**
 * "Use this frame's crop for the roll": commits the frame's effective crop
 * and analysis buffer as the roll's, then reads the frame back and, when its
 * geometry override now equals the roll geometry, removes the override. The
 * frame index is captured before the first await and every later call names
 * it, so navigating during the (slow, re-pooling) roll commit cannot touch
 * another frame.
 */
export async function cropToRollFlow(
  st: { frame: FrameView; roll: RollSettings; overrides: FrameOverrides },
  deps: CropToRollDeps,
): Promise<void> {
  const index = st.frame.index;
  const g = st.overrides.geometry ?? st.frame.settings.geometry;
  const next = structuredClone(st.roll);
  next.shared.geometry = { ...next.shared.geometry, crop: g.crop, analysis_buffer: g.analysis_buffer };
  if (!(await deps.commitRoll(next))) return;
  try {
    const after = await deps.frameView(index);
    if (after.index !== index) return;
    if (after.overrides.geometry && sameGeometry(after.overrides.geometry, next.shared.geometry)) {
      deps.commit({ ...after.overrides, geometry: null }, index);
    }
  } catch (e) {
    deps.fail(e);
  }
}

export { sameGeometry };

interface StoreValue {
  state: State;
  dispatch: (a: Action) => void;
  actions: Actions & PresetActions;
}

const StoreContext = createContext<StoreValue | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initialState);
  const stateRef = useRef(state);
  stateRef.current = state;
  const queueRef = useRef<EditQueue | null>(null);
  queueRef.current ??= new EditQueue();
  const thumbsRef = useRef<ThumbCache | null>(null);
  thumbsRef.current ??= new ThumbCache({
    fetch: (frame) => cmd.thumbnail(frame),
    makeUrl: (bytes) => URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" })),
    revoke: (url) => URL.revokeObjectURL(url),
    onChange: (frame, url) => dispatch({ type: "thumbs/loaded", frame, url }),
  });
  // Negatives in a cache of their own: they never stand in for a positive.
  const negativesRef = useRef<ThumbCache | null>(null);
  negativesRef.current ??= new ThumbCache({
    fetch: (frame) => cmd.negativeThumbnail(frame),
    makeUrl: (bytes) => URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" })),
    revoke: (url) => URL.revokeObjectURL(url),
    onChange: (frame, url) => dispatch({ type: "negatives/loaded", frame, url }),
  });
  const previewsRef = useRef<PresetPreviewCache | null>(null);
  previewsRef.current ??= new PresetPreviewCache({
    fetch: (frame, name) => cmd.presetThumbnail(frame, name),
    makeUrl: (bytes) => URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" })),
    revoke: (url) => URL.revokeObjectURL(url),
    onChange: (frame, name, url) => dispatch({ type: "presets/preview", frame, name, url }),
  });

  const actions = useMemo<Actions & PresetActions>(() => {
    const queue = queueRef.current as EditQueue;
    const thumbs = thumbsRef.current as ThumbCache;
    const negatives = negativesRef.current as ThumbCache;
    const previews = previewsRef.current as PresetPreviewCache;
    const fail = (e: unknown) => dispatch({ type: "error/set", error: errorText(e) });
    const current = () => stateRef.current.current;
    // Roll edits sent outside the queue (commitRoll, setRollArea) and not
    // answered yet: a close waits for them too.
    const inflight = { n: 0 };

    // A frame pass 1 has not reached shows its negative (Ruling 134); its
    // view is asked for only once it is analysed.
    const reloadFrame = async (index: number) => {
      const info = stateRef.current.roll?.frames[index];
      if (!info || info.status === "pending") {
        dispatch({ type: "negative/loaded", negative: await cmd.negativeView(index) });
        return;
      }
      dispatch({ type: "frame/loaded", frame: await cmd.frameView(index) });
    };

    const a: Actions = {
      async openRoll(dir) {
        // The open roll's state is kept until the backend accepts the new
        // one: a refusal (its edits could not be saved) leaves it as it was.
        dispatch({ type: "roll/requesting" });
        // Every edit of the open roll reaches the backend before it is
        // flushed and replaced.
        await queue.idle();
        try {
          const roll = await cmd.openRoll(dir);
          thumbs.reset();
          negatives.reset();
          previews.reset();
          dispatch({ type: "roll/opening" });
          dispatch({ type: "roll/loaded", roll });
        } catch (e) {
          dispatch({ type: "roll/failed", error: errorText(e) });
        }
      },
      async refreshRoll() {
        try {
          dispatch({ type: "roll/loaded", roll: await cmd.rollView() });
        } catch (e) {
          fail(e);
        }
      },
      async refreshViews() {
        try {
          dispatch({ type: "roll/loaded", roll: await cmd.rollView() });
          const frame = current();
          const info = frame === null ? undefined : stateRef.current.roll?.frames[frame];
          if (frame !== null && stateRef.current.screen === "frame" && info && info.status !== "pending") {
            const view = await cmd.frameView(frame);
            // `committed` names the view's own overrides, so a draft survives.
            dispatch({ type: "frame/loaded", frame: view, committed: view.overrides });
          }
        } catch (e) {
          fail(e);
        }
      },
      async openFrame(index) {
        dispatch({ type: "frame/open", index });
        await queue.idle();
        try {
          await reloadFrame(index);
        } catch (e) {
          fail(e);
        }
      },
      drag(overrides) {
        const frame = current();
        if (frame === null) return;
        dispatch({ type: "frame/draft", overrides, drag: true });
        queue.push(async () => {
          try {
            dispatch({ type: "frame/previewed", generation: await cmd.previewFrame(frame, overrides) });
          } catch (e) {
            fail(e);
          }
        }, true);
      },
      commit(overrides, target) {
        const frame = target ?? current();
        if (frame === null) return;
        // Only the frame on screen shows a draft.
        if (frame === current()) dispatch({ type: "frame/draft", overrides });
        queue.push(async () => {
          try {
            dispatch({
              type: "frame/loaded",
              frame: await cmd.commitFrame(frame, overrides),
              committed: overrides,
            });
          } catch (e) {
            fail(e);
            // Show the session's state again rather than the refused draft.
            await reloadFrame(frame).catch(fail);
          }
        }, false);
      },
      async commitRoll(roll) {
        inflight.n += 1;
        await queue.idle();
        try {
          dispatch({ type: "roll/loaded", roll: await cmd.commitRoll(roll) });
          const frame = current();
          if (frame !== null && stateRef.current.screen === "frame") await reloadFrame(frame);
          return true;
        } catch (e) {
          fail(e);
          return false;
        } finally {
          inflight.n -= 1;
        }
      },
      lock(dmin, gamma, cast) {
        const frame = current();
        if (frame === null) return;
        queue.push(async () => {
          try {
            dispatch({ type: "frame/loaded", frame: await cmd.lockFrame(frame, dmin, gamma, cast) });
          } catch (e) {
            fail(e);
          }
        }, false);
      },
      undo() {
        queue.push(async () => {
          try {
            dispatch({ type: "history/changed", history: await cmd.undo() });
            await afterHistory();
          } catch (e) {
            fail(e);
          }
        }, false);
      },
      redo() {
        queue.push(async () => {
          try {
            dispatch({ type: "history/changed", history: await cmd.redo() });
            await afterHistory();
          } catch (e) {
            fail(e);
          }
        }, false);
      },
      requestThumb(frame, stale) {
        thumbs.request(frame, stale);
      },
      requestPresetPreview(frame, name, version) {
        previews.request(frame, name, version);
      },
      requestNegativeThumb(frame) {
        // The negative never changes within a session: generation 0.
        negatives.request(frame, 0);
      },
      async loadNegative(frame) {
        try {
          dispatch({ type: "negative/loaded", negative: await cmd.negativeView(frame) });
        } catch (e) {
          fail(e);
        }
      },
      async startAnalysis() {
        try {
          dispatch({ type: "roll/loaded", roll: await cmd.startAnalysis() });
        } catch (e) {
          fail(e);
        }
      },
      async setRollArea(area) {
        inflight.n += 1;
        await queue.idle();
        try {
          dispatch({ type: "roll/loaded", roll: await cmd.setRollAnalysisArea(area) });
          const frame = current();
          if (frame !== null && stateRef.current.screen === "frame") await reloadFrame(frame);
          return true;
        } catch (e) {
          fail(e);
          return false;
        } finally {
          inflight.n -= 1;
        }
      },
      useRollArea() {
        const st = stateRef.current;
        if (!st.frame || !st.roll) return;
        a.commit(withRollArea(st.draft ?? st.frame.overrides, st.roll.roll.shared.geometry), st.frame.index);
      },
      revertDraft() {
        const st = stateRef.current;
        const frame = st.current;
        if (frame === null || !st.frame || st.draft === null) return;
        const committed = st.frame.overrides;
        dispatch({ type: "frame/discard-draft" });
        queue.push(async () => {
          try {
            dispatch({ type: "frame/previewed", generation: await cmd.previewFrame(frame, committed) });
          } catch (e) {
            fail(e);
          }
        }, true);
      },
      async frameCropToRoll() {
        await queue.idle();
        const st = stateRef.current;
        if (!st.frame || !st.roll) return;
        await cropToRollFlow(
          { frame: st.frame, roll: st.roll.roll, overrides: st.draft ?? st.frame.overrides },
          {
            commitRoll: (r) => a.commitRoll(r),
            frameView: (i) => cmd.frameView(i),
            commit: (o, i) => a.commit(o, i),
            fail,
          },
        );
      },
      async prepareClose() {
        const active = document.activeElement;
        if (active instanceof HTMLElement) active.blur();
        // The blur's commit is sent from its handler; let it start.
        await new Promise((r) => setTimeout(r, 0));
        while (inflight.n > 0) await new Promise((r) => setTimeout(r, 5));
        await queue.idle();
        while (inflight.n > 0) await new Promise((r) => setTimeout(r, 5));
      },
      async runExport(request, total) {
        await queue.idle();
        dispatch({ type: "export/started", request, total });
        try {
          dispatch({ type: "export/finished", report: await cmd.exportFrames(request) });
        } catch (e) {
          dispatch({ type: "export/failed", error: errorText(e) });
        }
      },
    };

    // Undo and redo may change the roll or any frame: refetch what is shown.
    async function afterHistory() {
      dispatch({ type: "roll/loaded", roll: await cmd.rollView() });
      const frame = current();
      if (frame !== null && stateRef.current.screen === "frame") await reloadFrame(frame);
    }

    const p = presetActions({
      queue,
      dispatch,
      fail,
      current,
      onFrameScreen: () => stateRef.current.screen === "frame",
      reloadFrame,
      inflight,
    });
    return { ...a, ...p };
  }, []);

  const value = useMemo(() => ({ state, dispatch, actions }), [state, actions]);
  return createElement(StoreContext.Provider, { value }, children);
}

export function useStore(): StoreValue {
  const v = useContext(StoreContext);
  if (!v) throw new Error("useStore outside StoreProvider");
  return v;
}
