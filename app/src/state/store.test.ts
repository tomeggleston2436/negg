// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { CommandFailure } from "../ipc/commands";
import frameViewFixture from "../ipc/__fixtures__/frame_view.json";
import rollViewFixture from "../ipc/__fixtures__/roll_view.json";
import { type FrameOverrides, type FrameView, decodeFrameView, decodeRollView, emptyOverrides } from "../ipc/types";
import { hasOwnArea, withRollArea } from "../sliders";
import { EditQueue, cropToRollFlow, errorText, sameGeometry } from "./store";

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
