// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import frameViewFixture from "../ipc/__fixtures__/frame_view.json";
import rollViewFixture from "../ipc/__fixtures__/roll_view.json";
import type { ScopesData } from "../ipc/scopes";
import { type FrameView, type RollView, decodeFrameView, decodeRollView, emptyOverrides } from "../ipc/types";
import {
  type Action,
  type State,
  analysing,
  awaitingAnalysis,
  exportBlocked,
  historyBlocked,
  initialState,
  neighbour,
  reducer,
  shownOverrides,
} from "./reducer";

const roll: RollView = decodeRollView(rollViewFixture);
const frame: FrameView = decodeFrameView(frameViewFixture);

function run(actions: Action[], from: State = initialState): State {
  return actions.reduce(reducer, from);
}

function scopes(frameIndex: number, generation: number): ScopesData {
  return {
    frame: frameIndex,
    generation,
    width: 1,
    height: 1,
    histogram: [],
    parade: new Uint16Array(0),
    vectorscope: new Uint16Array(0),
  };
}

describe("reducer", () => {
  it("opens a roll and tracks analysis progress", () => {
    let s = run([{ type: "roll/opening" }]);
    expect(s.opening).toBe(true);
    s = reducer(s, { type: "roll/loaded", roll });
    expect(s.opening).toBe(false);
    expect(s.generation).toBe(7);
    expect(s.analysis).toEqual({ done: 3, total: 4, finished: false });
    expect(s.history).toEqual(roll.history);
    s = reducer(s, { type: "analysis/progress", event: { done: 4, total: 4, frame: 3, finished: true } });
    expect(s.analysis).toEqual({ done: 4, total: 4, finished: true });
    // A refreshed view of the same roll keeps progress and screen.
    s = run([{ type: "frame/open", index: 0 }, { type: "roll/loaded", roll }], s);
    expect(s.analysis.finished).toBe(true);
    expect(s.screen).toBe("frame");
  });

  it("a different roll resets frame state", () => {
    const s = run([
      { type: "roll/loaded", roll },
      { type: "select/toggle", index: 2 },
      { type: "frame/open", index: 0 },
      { type: "roll/loaded", roll: { ...roll, dir: "/other" } },
    ]);
    expect(s.screen).toBe("roll");
    expect(s.current).toBeNull();
    expect(s.selected).toEqual([]);
  });

  it("opens frames within the roll only", () => {
    const s = run([{ type: "roll/loaded", roll }, { type: "frame/open", index: 9 }]);
    expect(s.screen).toBe("roll");
    const t = reducer(s, { type: "frame/open", index: 3 });
    expect(t.current).toBe(3);
    expect(neighbour(t, 1)).toBeNull();
    expect(neighbour(t, -1)).toBe(2);
  });

  it("ignores a late frame view for a frame left behind", () => {
    const s = run([
      { type: "roll/loaded", roll },
      { type: "frame/open", index: 1 },
      { type: "frame/loaded", frame },
    ]);
    expect(s.frame).toBeNull();
  });

  it("drafts during a drag and clears the draft on its commit only", () => {
    const a = emptyOverrides();
    a.print.grade = 100;
    const b = emptyOverrides();
    b.print.grade = 101;
    let s = run([
      { type: "roll/loaded", roll },
      { type: "frame/open", index: 0 },
      { type: "frame/loaded", frame },
      { type: "frame/draft", overrides: a },
      { type: "frame/previewed", generation: 12 },
    ]);
    expect(shownOverrides(s)).toBe(a);
    expect(s.generation).toBe(12);
    // A newer drag began before the commit of `a` came back: keep it.
    s = run([{ type: "frame/draft", overrides: b }, { type: "frame/loaded", frame, committed: a }], s);
    expect(s.draft).toBe(b);
    s = reducer(s, { type: "frame/loaded", frame, committed: b });
    expect(s.draft).toBeNull();
    expect(shownOverrides(s)).toBe(frame.overrides);
    // Generations never go backwards.
    expect(s.generation).toBe(12);
  });

  it("keeps the newest scopes for the open frame", () => {
    let s = run([{ type: "roll/loaded", roll }, { type: "frame/open", index: 0 }]);
    s = reducer(s, { type: "scopes/loaded", scopes: scopes(0, 10) });
    s = reducer(s, { type: "scopes/loaded", scopes: scopes(0, 9) });
    expect(s.scopes?.generation).toBe(10);
    s = reducer(s, { type: "scopes/loaded", scopes: scopes(1, 20) });
    expect(s.scopes?.frame).toBe(0);
  });

  it("marks thumbnails stale and toggles the selection in order", () => {
    const s = run([
      { type: "thumbs/stale", frames: [0, 2], generation: 5 },
      { type: "thumbs/stale", frames: [2], generation: 3 },
      { type: "select/toggle", index: 3 },
      { type: "select/toggle", index: 1 },
      { type: "select/toggle", index: 3 },
    ]);
    expect(s.thumbs).toEqual({ 0: 5, 2: 5 });
    expect(s.selected).toEqual([1]);
  });

  it("follows an export from start to report", () => {
    const request = { scope: { kind: "roll" as const }, format: "jpeg" as const, out_dir: null };
    let s = run([
      { type: "export/dialog", open: true },
      { type: "export/started", request, total: 2 },
    ]);
    expect(s.exportDialog).toBe(false);
    s = reducer(s, { type: "export/progress", event: { done: 1, total: 2, input: "a.tif", ok: true, message: null } });
    s = reducer(s, { type: "export/progress", event: { done: 2, total: 2, input: "b.tif", ok: false, message: "x" } });
    expect(s.exportProgress?.items).toHaveLength(2);
    s = reducer(s, { type: "export/finished", report: { converted: 1, failed: 1, outputs: ["a.jpg"], errors: ["x"] } });
    expect(s.exportProgress?.running).toBe(false);
    expect(s.exportProgress?.report?.failed).toBe(1);
    // Events after the end are ignored.
    const t = reducer(s, { type: "export/progress", event: { done: 3, total: 2, input: "c", ok: true, message: null } });
    expect(t).toBe(s);
  });

  it("sets and clears errors", () => {
    const s = run([{ type: "error/set", error: "boom" }]);
    expect(s.error).toBe("boom");
    expect(reducer(s, { type: "error/clear" }).error).toBeNull();
  });
});

describe("reopening", () => {
  it("a view of the open roll during a pending open keeps its state; a refusal leaves it", () => {
    let s = run([
      { type: "roll/loaded", roll: { ...roll, analysed: true } },
      { type: "select/toggle", index: 2 },
      { type: "thumbs/loaded", frame: 0, url: "blob:a" },
      { type: "scopes/loaded", scopes: scopes(0, 7) },
      { type: "frame/open", index: 0 },
      { type: "scopes/loaded", scopes: scopes(0, 7) },
      { type: "screen/show", screen: "roll" },
      { type: "roll/requesting" },
    ]);
    expect(s.requesting).toBe(true);
    expect(s.opening).toBe(false);
    // The old roll's analysis event refreshes its view meanwhile.
    s = reducer(s, { type: "roll/loaded", roll: { ...roll, analysed: true, generation: 8 } });
    expect(s.selected).toEqual([2]);
    expect(s.thumbUrls).toEqual({ 0: "blob:a" });
    expect(s.scopes?.generation).toBe(7);
    expect(s.current).toBe(0);
    expect(s.requesting).toBe(true);
    // Refused (its edits could not be saved): everything stays.
    s = reducer(s, { type: "roll/failed", error: "could not be saved" });
    expect(s.requesting).toBe(false);
    expect(s.selected).toEqual([2]);
    expect(s.thumbUrls).toEqual({ 0: "blob:a" });
    expect(s.error).toBe("could not be saved");
  });

  it("keeps negatives apart from thumbnails and the current frame's negative", () => {
    let s = run([{ type: "roll/loaded", roll }, { type: "frame/open", index: 3 }]);
    s = reducer(s, { type: "negatives/loaded", frame: 3, url: "blob:n" });
    expect(s.negativeUrls).toEqual({ 3: "blob:n" });
    expect(s.thumbUrls).toEqual({});
    const negative = { index: 3, source: "frame-04.tif", width: 60, height: 40, levels: 1, tile_size: 512 };
    expect(reducer(s, { type: "negative/loaded", negative: { ...negative, index: 2 } }).negative).toBeNull();
    s = reducer(s, { type: "negative/loaded", negative });
    expect(s.negative).toEqual(negative);
    expect(reducer(s, { type: "frame/open", index: 2 }).negative).toBeNull();
  });

  it("tells awaiting Analyse from analysing", () => {
    let s = run([{ type: "roll/loaded", roll }]);
    expect(awaitingAnalysis(s)).toBe(true);
    expect(analysing(s)).toBe(false);
    s = reducer(s, { type: "roll/loaded", roll: { ...roll, awaiting_analysis: false } });
    expect(awaitingAnalysis(s)).toBe(false);
    expect(analysing(s)).toBe(true);
    s = reducer(s, { type: "analysis/progress", event: { done: 4, total: 4, frame: null, finished: true } });
    expect(analysing(s)).toBe(false);
  });

  it("blocks export and history while pass 1 runs, and export while an export runs", () => {
    expect(historyBlocked(initialState)).toBe(true);
    let s = run([{ type: "roll/loaded", roll }]);
    expect(exportBlocked(s)).toBe(true);
    expect(historyBlocked(s)).toBe(true);
    s = reducer(s, { type: "roll/loaded", roll: { ...roll, awaiting_analysis: false } });
    expect(exportBlocked(s)).toBe(true);
    s = reducer(s, { type: "analysis/progress", event: { done: 4, total: 4, frame: null, finished: true } });
    expect(exportBlocked(s)).toBe(false);
    expect(historyBlocked(s)).toBe(false);
    const request = { scope: { kind: "roll" as const }, format: "jpeg" as const, out_dir: null };
    s = reducer(s, { type: "export/started", request, total: 2 });
    expect(exportBlocked(s)).toBe(true);
    s = reducer(s, { type: "export/failed", error: "x" });
    expect(exportBlocked(s)).toBe(false);
  });

  it("roll/opening starts afresh even for the folder already open", () => {
    let s = run([
      { type: "roll/loaded", roll: { ...roll, analysed: true } },
      { type: "analysis/progress", event: { done: 4, total: 4, frame: null, finished: true } },
      { type: "frame/open", index: 0 },
      { type: "frame/loaded", frame },
      { type: "thumbs/loaded", frame: 0, url: "blob:a" },
    ]);
    expect(s.analysis.finished).toBe(true);
    s = reducer(s, { type: "roll/opening" });
    expect(s.opening).toBe(true);
    expect(s.analysis.finished).toBe(false);
    expect(s.current).toBeNull();
    expect(s.frame).toBeNull();
    expect(s.screen).toBe("roll");
    expect(s.thumbUrls).toEqual({});
    expect(s.generation).toBe(0);
    // The same folder answers: progress comes from the new session's view.
    s = reducer(s, { type: "roll/loaded", roll: { ...roll, generation: 1 } });
    expect(s.opening).toBe(false);
    expect(s.analysis).toEqual({ done: 3, total: 4, finished: false });
    expect(s.generation).toBe(1);
  });

  it("caches thumbnail URLs per frame", () => {
    const s = run([
      { type: "thumbs/loaded", frame: 1, url: "blob:a" },
      { type: "thumbs/loaded", frame: 2, url: null },
      { type: "thumbs/loaded", frame: 1, url: "blob:b" },
    ]);
    expect(s.thumbUrls).toEqual({ 1: "blob:b", 2: null });
  });
});
