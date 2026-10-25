// SPDX-License-Identifier: Apache-2.0
import { afterEach, describe, expect, it } from "vitest";
import exportRequest from "./__fixtures__/export_request.json";
import frameView from "./__fixtures__/frame_view.json";
import negativeView from "./__fixtures__/negative_view.json";
import rollView from "./__fixtures__/roll_view.json";
import * as cmd from "./commands";
import { onAnalysis, onExport, onThumbs, setListen } from "./events";
import { WireError } from "./tile";
import { decodeExportRequest, decodeRollView, emptyOverrides } from "./types";

type Call = { cmd: string; args: Record<string, unknown> | undefined };

function mock(reply: (c: Call) => unknown): { calls: Call[] } {
  const calls: Call[] = [];
  cmd.setInvoke(async (name, args) => {
    const c = { cmd: name, args };
    calls.push(c);
    return reply(c);
  });
  return { calls };
}

afterEach(() => {
  cmd.setInvoke(async () => {
    throw new Error("no backend in tests");
  });
});

describe("commands", () => {
  it("calls each command by its Rust name with Rust argument keys", async () => {
    const history = { can_undo: true, can_redo: false, len: 1, cursor: 1 };
    const report = { converted: 2, failed: 0, outputs: ["a.tif", "b.tif"], errors: [] };
    const replies: Record<string, unknown> = {
      open_roll: rollView,
      roll_view: rollView,
      frame_view: frameView,
      commit_frame: frameView,
      commit_roll: rollView,
      lock_frame: frameView,
      preview_frame: 42,
      undo: history,
      redo: history,
      flush: null,
      export: report,
      render_tile: new ArrayBuffer(4),
      thumbnail: new Uint8Array([1, 2, 3]),
      scopes: [9, 8],
    };
    const { calls } = mock((c) => replies[c.cmd]);
    const o = emptyOverrides();
    const roll = decodeRollView(rollView).roll;
    const req = decodeExportRequest(exportRequest);

    expect((await cmd.openRoll("/r")).dir).toBe("/rolls/roll-01");
    await cmd.rollView();
    expect((await cmd.frameView(0)).width).toBe(6000);
    expect(await cmd.previewFrame(1, o)).toBe(42);
    await cmd.commitFrame(1, o);
    await cmd.commitRoll(roll);
    await cmd.lockFrame(1, true, false, false);
    expect(await cmd.undo()).toEqual(history);
    await cmd.redo();
    await cmd.flush();
    expect(await cmd.exportFrames(req)).toEqual(report);
    expect((await cmd.renderTile(1, 2, 3, 4)).byteLength).toBe(4);
    expect(Array.from(new Uint8Array(await cmd.thumbnail(5)))).toEqual([1, 2, 3]);
    expect(Array.from(new Uint8Array(await cmd.scopes(6)))).toEqual([9, 8]);

    expect(calls).toEqual([
      { cmd: "open_roll", args: { dir: "/r" } },
      { cmd: "roll_view", args: undefined },
      { cmd: "frame_view", args: { frame: 0 } },
      { cmd: "preview_frame", args: { frame: 1, overrides: o } },
      { cmd: "commit_frame", args: { frame: 1, overrides: o } },
      { cmd: "commit_roll", args: { roll } },
      { cmd: "lock_frame", args: { frame: 1, dmin: true, gamma: false, cast: false } },
      { cmd: "undo", args: undefined },
      { cmd: "redo", args: undefined },
      { cmd: "flush", args: undefined },
      { cmd: "export", args: { request: req } },
      { cmd: "render_tile", args: { frame: 1, level: 2, tile_x: 3, tile_y: 4 } },
      { cmd: "thumbnail", args: { frame: 5 } },
      { cmd: "scopes", args: { frame: 6 } },
    ]);
  });

  it("calls the analysis area commands (Rulings 134 and 136)", async () => {
    const replies: Record<string, unknown> = {
      start_analysis: rollView,
      negative_view: negativeView,
      negative_tile: new ArrayBuffer(8),
      negative_thumbnail: new Uint8Array([4, 5]),
      set_roll_analysis_area: rollView,
    };
    const { calls } = mock((c) => replies[c.cmd]);
    const area = { x: 0.1, y: 0.1, width: 0.8, height: 0.8 };
    expect((await cmd.startAnalysis()).awaiting_analysis).toBe(true);
    expect((await cmd.negativeView(2)).width).toBe(6000);
    expect((await cmd.negativeTile(2, 1, 0, 3)).byteLength).toBe(8);
    expect(Array.from(new Uint8Array(await cmd.negativeThumbnail(2)))).toEqual([4, 5]);
    await cmd.setRollAnalysisArea(area);
    await cmd.setRollAnalysisArea(null);
    expect(calls).toEqual([
      { cmd: "start_analysis", args: undefined },
      { cmd: "negative_view", args: { frame: 2 } },
      { cmd: "negative_tile", args: { frame: 2, level: 1, tile_x: 0, tile_y: 3 } },
      { cmd: "negative_thumbnail", args: { frame: 2 } },
      { cmd: "set_roll_analysis_area", args: { area } },
      { cmd: "set_roll_analysis_area", args: { area: null } },
    ]);
  });

  it("maps a CommandError to CommandFailure with its kind", async () => {
    mock(() => {
      throw { kind: "busy", message: "the roll is still being analysed" };
    });
    await expect(cmd.commitFrame(0, emptyOverrides())).rejects.toMatchObject({
      name: "CommandFailure",
      kind: "busy",
      command: "commit_frame",
      message: "the roll is still being analysed",
    });
  });

  it("maps a transport failure", async () => {
    mock(() => {
      throw new Error("ipc down");
    });
    await expect(cmd.rollView()).rejects.toMatchObject({ kind: "transport", message: "ipc down" });
  });

  it("rejects a response that breaks the contract", async () => {
    mock(() => ({ dir: 1 }));
    await expect(cmd.rollView()).rejects.toThrow(WireError);
    mock(() => "not bytes");
    await expect(cmd.renderTile(0, 0, 0, 0)).rejects.toThrow(WireError);
    mock(() => -1);
    await expect(cmd.previewFrame(0, emptyOverrides())).rejects.toThrow(WireError);
  });
});

describe("events", () => {
  it("decodes payloads and reports bad ones", async () => {
    const handlers = new Map<string, (p: unknown) => void>();
    setListen(async (event, handler) => {
      handlers.set(event, handler);
      return () => handlers.delete(event);
    });
    const got: unknown[] = [];
    const errors: unknown[] = [];
    const un = await onAnalysis((e) => got.push(e), (e) => errors.push(e));
    await onThumbs((e) => got.push(e), (e) => errors.push(e));
    await onExport((e) => got.push(e), (e) => errors.push(e));
    expect([...handlers.keys()]).toEqual(["negg://analysis", "negg://thumbs", "negg://export"]);

    handlers.get("negg://analysis")?.({ done: 1, total: 4, frame: 0, finished: false });
    handlers.get("negg://thumbs")?.({ frames: [0, 2], generation: 9 });
    handlers.get("negg://export")?.({ done: 1, total: 2, input: "/r/a.tif", ok: true, message: null });
    handlers.get("negg://analysis")?.({ done: "1" });
    expect(got).toHaveLength(3);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(WireError);
    un();
    expect(handlers.has("negg://analysis")).toBe(false);
  });
});
