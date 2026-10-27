// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { CommandFailure, setInvoke } from "../ipc/commands";
import frameViewFixture from "../ipc/__fixtures__/frame_view.json";
import rollViewFixture from "../ipc/__fixtures__/roll_view.json";
import { type FrameOverrides, type FrameView, decodeFrameView, decodeRollView, emptyOverrides } from "../ipc/types";
import { hasOwnArea, withRollArea } from "../sliders";
import presetListFixture from "../ipc/__fixtures__/preset_list.json";
import { EditQueue, cropToRollFlow, errorText, presetActions, sameGeometry } from "./store";
import type { Action } from "./reducer";

describe("EditQueue", () => {
  it("runs jobs in order and collapses queued previews to the latest", async () => {
    const q = new EditQueue();
    const log: string[] = [];
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    q.push(async () => {
      await gate;
      log.push("preview 1");
    }, true);
    q.push(async () => void log.push("preview 2"), true);
    q.push(async () => void log.push("preview 3"), true);
    q.push(async () => void log.push("commit"), false);
    q.push(async () => void log.push("preview 4"), true);
    release();
    await q.idle();
    // preview 1 was already running; 2 and 3 were dropped by the commit.
    expect(log).toEqual(["preview 1", "commit", "preview 4"]);
  });

  it("never drops a commit", async () => {
    const q = new EditQueue();
    const log: string[] = [];
    q.push(async () => void log.push("a"), false);
    q.push(async () => void log.push("b"), false);
    q.push(async () => void log.push("p"), true);
    await q.idle();
    expect(log).toEqual(["a", "b", "p"]);
  });
});

describe("errorText", () => {
  it("shows the backend message", () => {
    expect(errorText(new CommandFailure("commit_frame", "read_only", "frame 3 is read-only"))).toBe(
      "frame 3 is read-only",
    );
    expect(errorText(new CommandFailure("roll_view", "transport", "down"))).toBe("roll_view: down");
    expect(errorText("plain")).toBe("plain");
  });
});

describe("sameGeometry", () => {
  it("compares every field", () => {
    const g = { crop: { x: 0.1, y: 0.1, width: 0.8, height: 0.8 }, analysis_buffer: 0.05, rotate_degrees: 0 as const, flip_horizontal: false };
    expect(sameGeometry(g, structuredClone(g))).toBe(true);
    expect(sameGeometry(g, { ...g, crop: null })).toBe(false);
    expect(sameGeometry({ ...g, crop: null }, { ...g, crop: null })).toBe(true);
    expect(sameGeometry(g, { ...g, rotate_degrees: 90 })).toBe(false);
    expect(sameGeometry(g, { ...g, crop: { ...g.crop, width: 0.7 } })).toBe(false);
    // The analysis area counts; null and absent are the same.
    expect(sameGeometry(g, { ...g, analysis_area: null })).toBe(true);
    expect(sameGeometry(g, { ...g, analysis_area: { x: 0, y: 0, width: 1, height: 0.9 } })).toBe(false);
  });
});

describe("the frame's analysis area", () => {
  const roll = {
    crop: null,
    analysis_area: { x: 0.05, y: 0.05, width: 0.9, height: 0.9 },
    analysis_buffer: 0.05,
    rotate_degrees: 0 as const,
    flip_horizontal: false,
  };
  const own = { x: 0.1, y: 0.1, width: 0.8, height: 0.8 };

  it("is its own only when its geometry carries another area", () => {
    const o = emptyOverrides();
    expect(hasOwnArea(o, roll)).toBe(false);
    expect(hasOwnArea({ ...o, geometry: { ...roll } }, roll)).toBe(false);
    expect(hasOwnArea({ ...o, geometry: { ...roll, analysis_area: own } }, roll)).toBe(true);
    expect(hasOwnArea({ ...o, geometry: { ...roll, analysis_area: null } }, roll)).toBe(true);
  });

  it("Use roll analysis area drops the override when nothing else is the frame's own", () => {
    const o = emptyOverrides();
    expect(withRollArea(o, roll)).toBe(o);
    expect(withRollArea({ ...o, geometry: { ...roll, analysis_area: own } }, roll).geometry).toBeNull();
    // A crop of its own stays; only the area follows the roll.
    const crop = { x: 0.2, y: 0.2, width: 0.5, height: 0.5 };
    const kept = withRollArea({ ...o, geometry: { ...roll, crop, analysis_area: own } }, roll);
    expect(kept.geometry).toEqual({ ...roll, crop });
  });
});

describe("cropToRollFlow", () => {
  const frameA: FrameView = decodeFrameView(frameViewFixture); // index 0, own geometry override
  const roll = decodeRollView(rollViewFixture).roll;

  it("commits to the captured frame even when the user navigates during the awaits", async () => {
    let shown = 0;
    const commits: Array<[FrameOverrides, number]> = [];
    const asked: number[] = [];
    await cropToRollFlow(
      { frame: frameA, roll, overrides: frameA.overrides },
      {
        commitRoll: async (next) => {
          shown = 2; // the user moved to frame 2 while the roll re-pooled
          expect(next.shared.geometry.crop).toEqual(frameA.overrides.geometry?.crop);
          return true;
        },
        frameView: async (i) => {
          asked.push(i);
          // The session now has the roll crop; the frame's override still
          // carries the same crop and buffer but its own turn and flip.
          return { ...frameA, overrides: { ...frameA.overrides, geometry: { ...roll.shared.geometry, crop: frameA.overrides.geometry?.crop ?? null } } };
        },
        commit: (o, i) => commits.push([o, i]),
        fail: (e) => {
          throw e;
        },
      },
    );
    expect(shown).toBe(2);
    expect(asked).toEqual([0]);
    expect(commits).toHaveLength(1);
    expect(commits[0]?.[1]).toBe(0);
    expect(commits[0]?.[0].geometry).toBeNull();
  });

  it("keeps a frame override that differs from the roll geometry", async () => {
    const commits: number[] = [];
    await cropToRollFlow(
      { frame: frameA, roll, overrides: frameA.overrides },
      {
        commitRoll: async () => true,
        frameView: async () => frameA, // rotate 90 and flip: not the roll's geometry
        commit: (_o, i) => commits.push(i),
        fail: () => undefined,
      },
    );
    expect(commits).toEqual([]);
  });

  it("stops when the roll commit is refused", async () => {
    let asked = false;
    await cropToRollFlow(
      { frame: frameA, roll, overrides: frameA.overrides },
      {
        commitRoll: async () => false,
        frameView: async () => {
          asked = true;
          return frameA;
        },
        commit: () => undefined,
        fail: () => undefined,
      },
    );
    expect(asked).toBe(false);
  });
});

describe("preset actions", () => {
  const view = decodeFrameView(frameViewFixture);
  const rollView = decodeRollView(rollViewFixture);
  const look = { grade: 130, toe: 8, shoulder: 3.5, black_point_compensation: true, saturation: 1.1 };

  function setup(reply: (cmd: string, args: Record<string, unknown> | undefined) => unknown) {
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    setInvoke(async (name, args) => {
      calls.push([name, args]);
      return reply(name, args);
    });
    const dispatched: Action[] = [];
    const failures: unknown[] = [];
    const reloaded: number[] = [];
    const queue = new EditQueue();
    const inflight = { n: 0 };
    const p = presetActions({
      queue,
      dispatch: (a) => dispatched.push(a),
      fail: (e) => failures.push(e),
      current: () => 0,
      onFrameScreen: () => true,
      reloadFrame: async (i) => void reloaded.push(i),
      inflight,
    });
    return { p, calls, dispatched, failures, reloaded, queue, inflight };
  }

  it("applies a frame preset after the edits already queued, and shows the frame it returns", async () => {
    const { p, calls, dispatched, queue } = setup(() => frameViewFixture);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    queue.push(async () => {
      await gate;
      calls.push(["pending slider commit", undefined]);
    }, false);
    p.applyFramePreset("Rich");
    release();
    await queue.idle();
    expect(calls.map((c) => c[0])).toEqual(["pending slider commit", "apply_frame_preset"]);
    expect(calls[1]?.[1]).toEqual({ frame: 0, name: "Rich" });
    expect(dispatched).toEqual([{ type: "frame/loaded", frame: view }]);
  });

  it("applies to the frame named, not the one shown later", async () => {
    const { p, calls, queue } = setup(() => frameViewFixture);
    p.applyFramePreset("Soft", 3);
    await queue.idle();
    expect(calls[0]).toEqual(["apply_frame_preset", { frame: 3, name: "Soft" }]);
  });

  it("reports a refused frame preset and shows the session's frame again", async () => {
    const { p, failures, reloaded, queue } = setup(() => {
      throw { kind: "read_only", message: "frame 1 is read-only" };
    });
    p.applyFramePreset("Rich");
    await queue.idle();
    expect(failures).toHaveLength(1);
    expect(reloaded).toEqual([0]);
  });

  it("asks for the frames with their own look once queued edits are sent", async () => {
    const { p, calls, queue } = setup((c) => (c === "frames_with_own_look" ? [2, 5] : null));
    queue.push(async () => void calls.push(["queued", undefined]), false);
    expect(await p.framesWithOwnLook()).toEqual([2, 5]);
    expect(calls.map((c) => c[0])).toEqual(["queued", "frames_with_own_look"]);
  });

  it("applies a roll preset, then shows the new roll and the frame on screen", async () => {
    const { p, calls, dispatched, reloaded } = setup(() => rollViewFixture);
    expect(await p.applyRollPreset("Rich", "all_frames")).toBe(true);
    expect(calls).toEqual([["apply_roll_preset", { name: "Rich", scope: "all_frames" }]]);
    expect(dispatched).toEqual([{ type: "roll/loaded", roll: rollView }]);
    expect(reloaded).toEqual([0]);
  });

  it("a refused roll preset reports and returns false", async () => {
    const { p, failures, inflight } = setup(() => {
      throw { kind: "convert", message: "no preset" };
    });
    expect(await p.applyRollPreset("Gone", "frames_without_own_look")).toBe(false);
    expect(failures).toHaveLength(1);
    expect(inflight.n).toBe(0);
  });

  it("loads the presets and saves one", async () => {
    const { p, calls, dispatched, queue } = setup((c) => (c === "apply_frame_preset" ? frameViewFixture : presetListFixture));
    await p.loadPresets();
    expect(await p.savePreset("Mine", look)).toBeNull();
    await queue.idle();
    expect(calls).toEqual([
      ["list_presets", undefined],
      ["save_preset", { name: "Mine", look }],
      // The saved preset is applied to the frame, so its heading names it.
      ["apply_frame_preset", { frame: 0, name: "Mine" }],
    ]);
    expect(dispatched.map((a) => a.type)).toEqual(["presets/loaded", "presets/loaded", "frame/loaded"]);
  });

  it("applies a saved preset to the frame it was saved from, not the one shown later", async () => {
    let shown = 4;
    let finishSave: (v: unknown) => void = () => undefined;
    const calls: Array<[string, Record<string, unknown> | undefined]> = [];
    setInvoke(async (name, args) => {
      calls.push([name, args]);
      if (name === "save_preset") return new Promise((r) => (finishSave = r));
      return frameViewFixture;
    });
    const queue = new EditQueue();
    const p = presetActions({
      queue,
      dispatch: () => undefined,
      fail: () => undefined,
      current: () => shown,
      onFrameScreen: () => true,
      reloadFrame: async () => undefined,
      inflight: { n: 0 },
    });
    const saved = p.savePreset("Mine", look);
    shown = 5;
    finishSave(presetListFixture);
    expect(await saved).toBeNull();
    await queue.idle();
    expect(calls[1]).toEqual(["apply_frame_preset", { frame: 4, name: "Mine" }]);
  });

  it("returns why a save was refused", async () => {
    const { p, dispatched, calls, queue } = setup(() => {
      throw { kind: "convert", message: "\"Rich\" is a built-in preset name; choose another" };
    });
    expect(await p.savePreset("Rich", look)).toBe('"Rich" is a built-in preset name; choose another');
    expect(dispatched).toEqual([]);
    // A refused save applies nothing.
    await queue.idle();
    expect(calls.map((c) => c[0])).toEqual(["save_preset"]);
  });
});
