// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import {
  TileRequests,
  type Rotation,
  type ViewPlacement,
  HANDLE_POSITIONS,
  cropFromPoints,
  cropWithField,
  sameRect,
  fitCrop,
  formatPercent,
  handleEdges,
  parseCropText,
  resizeCrop,
  cropScreenRect,
  dragLevel,
  fitLevel,
  fitScale,
  levelCount,
  levelForScale,
  levelSize,
  moveCrop,
  previewLevels,
  screenToSource,
  sourceToScreen,
  stageTransform,
  tileGrid,
  tileSize,
  visibleSourceRect,
  visibleTiles,
} from "./tiles";

const W = 6000;
const H = 4000;
const T = 512;

describe("pyramid levels", () => {
  it("counts L + 1 levels as Rust does", () => {
    expect(levelCount(W, H, T)).toBe(5); // L = 4
    expect(levelCount(512, 300, T)).toBe(1); // L = 0
    expect(levelCount(513, 10, T)).toBe(2);
    expect(levelCount(1, 1, T)).toBe(1);
  });

  it("halves with ceil at each level", () => {
    expect([0, 1, 2, 3, 4].map((l) => levelSize(W, H, l))).toEqual([
      [6000, 4000],
      [3000, 2000],
      [1500, 1000],
      [750, 500],
      [375, 250],
    ]);
    expect(levelSize(7, 5, 1)).toEqual([4, 3]);
    expect(levelSize(7, 5, 2)).toEqual([2, 2]);
  });

  it("lays out the tile grid for 6000 x 4000", () => {
    expect([0, 1, 2, 3, 4].map((l) => tileGrid(W, H, l, T))).toEqual([
      [12, 8],
      [6, 4],
      [3, 2],
      [2, 1],
      [1, 1],
    ]);
    expect(tileSize(W, H, 0, T, 11, 7)).toEqual([368, 416]);
    expect(tileSize(W, H, 0, T, 0, 0)).toEqual([512, 512]);
    expect(tileSize(W, H, 3, T, 1, 0)).toEqual([238, 500]);
  });
});

describe("visibleTiles", () => {
  it("covers the whole frame", () => {
    expect(visibleTiles({ x: 0, y: 0, width: W, height: H }, 0, W, H, T)).toHaveLength(96);
    expect(visibleTiles({ x: 0, y: 0, width: W, height: H }, 4, W, H, T)).toEqual([[0, 0]]);
  });

  it("picks the tiles a view intersects, row-major", () => {
    const view = { x: 1000, y: 600, width: 1100, height: 500 };
    expect(visibleTiles(view, 0, W, H, T)).toEqual([
      [1, 1],
      [2, 1],
      [3, 1],
      [4, 1],
      [1, 2],
      [2, 2],
      [3, 2],
      [4, 2],
    ]);
    expect(visibleTiles(view, 1, W, H, T)).toEqual([
      [0, 0],
      [1, 0],
      [2, 0],
      [0, 1],
      [1, 1],
      [2, 1],
    ]);
  });

  it("clamps at the edges and is empty off the image", () => {
    expect(visibleTiles({ x: 5900, y: 3900, width: 1000, height: 1000 }, 0, W, H, T)).toEqual([[11, 7]]);
    expect(visibleTiles({ x: -500, y: -500, width: 600, height: 600 }, 0, W, H, T)).toEqual([[0, 0]]);
    expect(visibleTiles({ x: 7000, y: 0, width: 100, height: 100 }, 0, W, H, T)).toEqual([]);
  });
});

describe("level choice", () => {
  it("fitLevel is the first level that fits the device viewport", () => {
    expect(fitLevel(W, H, 1600, 1000, T)).toBe(2);
    expect(fitLevel(W, H, 6000, 4000, T)).toBe(0);
    expect(fitLevel(W, H, 3000, 1999, T)).toBe(2);
  });

  it("fitLevel never goes past the last pyramid level", () => {
    expect(fitLevel(W, H, 100, 100, T)).toBe(4);
    expect(fitLevel(512, 300, 10, 10, T)).toBe(0);
  });

  it("dragLevel is one level coarser, short of the backdrop", () => {
    expect(dragLevel(1, 5)).toBe(2);
    expect(dragLevel(0, 5)).toBe(1);
    expect(dragLevel(3, 4)).toBeNull();
    expect(dragLevel(4, 4)).toBeNull();
  });

  it("previewLevels asks only for the coarser level while dragging", () => {
    expect(previewLevels(1, 5, true, null)).toEqual({ mount: [2, 1], request: [2] });
    expect(previewLevels(1, 5, true, 2)).toEqual({ mount: [2, 1], request: [2] });
    // Next to the backdrop the backdrop alone keeps up.
    expect(previewLevels(3, 4, true, null)).toEqual({ mount: [3], request: [] });
    expect(previewLevels(4, 4, true, null)).toEqual({ mount: [], request: [] });
  });

  it("previewLevels asks for the fine level on release and keeps the coarser one under it", () => {
    expect(previewLevels(1, 5, false, 2)).toEqual({ mount: [2, 1], request: [1] });
    expect(previewLevels(1, 5, false, null)).toEqual({ mount: [1], request: [1] });
    expect(previewLevels(4, 4, false, null)).toEqual({ mount: [], request: [] });
  });

  it("levelForScale keeps at least one level pixel per device pixel", () => {
    expect(levelForScale(0.25, 2, 5)).toBe(1);
    expect(levelForScale(0.25, 1, 5)).toBe(2);
    expect(levelForScale(0.3, 1, 5)).toBe(1);
    expect(levelForScale(1, 2, 5)).toBe(0);
    expect(levelForScale(4, 1, 5)).toBe(0);
    expect(levelForScale(0.001, 1, 5)).toBe(4);
  });

  it("fitScale uses the oriented size", () => {
    expect(fitScale(W, H, 0, 1200, 800)).toBeCloseTo(0.2);
    expect(fitScale(W, H, 90, 1200, 800)).toBeCloseTo(0.1333, 3);
  });
});

describe("placement", () => {
  const base: ViewPlacement = { cx: 50, cy: 30, scale: 1, rotate: 0, flip: false, vw: 100, vh: 100 };

  it("turns clockwise, then mirrors", () => {
    const right: [number, number] = [60, 30]; // 10 right of centre
    const below: [number, number] = [50, 40]; // 10 below centre
    expect(sourceToScreen({ ...base, rotate: 90 }, ...right)).toEqual([50, 60]);
    expect(sourceToScreen({ ...base, rotate: 90 }, ...below)).toEqual([40, 50]);
    expect(sourceToScreen({ ...base, rotate: 90, flip: true }, ...below)).toEqual([60, 50]);
    expect(sourceToScreen({ ...base, rotate: 180 }, ...right)).toEqual([40, 50]);
    expect(sourceToScreen({ ...base, rotate: 270 }, ...right)).toEqual([50, 40]);
    expect(sourceToScreen({ ...base, flip: true }, ...right)).toEqual([40, 50]);
  });

  it("screenToSource inverts sourceToScreen for every orientation", () => {
    for (const rotate of [0, 90, 180, 270] as Rotation[]) {
      for (const flip of [false, true]) {
        const p = { ...base, rotate, flip, scale: 0.37, cx: 1234, cy: 777, vw: 640, vh: 480 };
        const [sx, sy] = sourceToScreen(p, 100, 2000);
        const [x, y] = screenToSource(p, sx, sy);
        expect(x).toBeCloseTo(100, 9);
        expect(y).toBeCloseTo(2000, 9);
      }
    }
  });

  it("visibleSourceRect swaps axes under a quarter turn", () => {
    const r = visibleSourceRect({ ...base, rotate: 90, vw: 200, vh: 100 });
    expect(r).toEqual({ x: 0, y: -70, width: 100, height: 200 });
  });

  it("builds the stage transform", () => {
    expect(stageTransform({ ...base, rotate: 90, flip: true, scale: 0.5 })).toBe(
      "translate(50px, 50px) scale(0.5) scaleX(-1) rotate(90deg) translate(-50px, -30px)",
    );
  });
});

describe("crop", () => {
  it("normalises a drag in any direction", () => {
    expect(cropFromPoints(W, H, [600, 400], [300, 3600])).toEqual({ x: 0.05, y: 0.1, width: 0.05, height: 0.8 });
  });

  it("clamps to the frame and ignores a click", () => {
    expect(cropFromPoints(W, H, [-100, -100], [7000, 5000])).toEqual({ x: 0, y: 0, width: 1, height: 1 });
    expect(cropFromPoints(W, H, [100, 100], [130, 2000])).toBeNull();
  });

  it("moves inside the frame", () => {
    const r = { x: 0.3, y: 0.2, width: 0.5, height: 0.5 };
    expect(moveCrop(r, 0.4, -0.5)).toEqual({ x: 0.5, y: 0, width: 0.5, height: 0.5 });
    expect(moveCrop(r, 0.1, 0.1)).toEqual({ x: 0.4, y: 0.3, width: 0.5, height: 0.5 });
  });

  it("maps to the screen through the placement", () => {
    const p: ViewPlacement = { cx: 50, cy: 50, scale: 1, rotate: 90, flip: false, vw: 100, vh: 100 };
    // Top-left quarter of a 100 x 100 source lands top-right after a clockwise turn.
    expect(cropScreenRect(p, 100, 100, { x: 0, y: 0, width: 0.5, height: 0.5 })).toEqual({
      x: 50,
      y: 0,
      width: 50,
      height: 50,
    });
  });
});

describe("crop handles", () => {
  const r = { x: 0.2, y: 0.2, width: 0.4, height: 0.4 };
  const upright: ViewPlacement = { cx: 50, cy: 50, scale: 1, rotate: 0, flip: false, vw: 100, vh: 100 };

  function edgesAt(p: ViewPlacement, fx: number, fy: number) {
    const s = cropScreenRect(p, 100, 100, r);
    return handleEdges(p, 100, 100, r, s.x + fx * s.width, s.y + fy * s.height);
  }

  it("maps each screen handle to source edges", () => {
    expect(edgesAt(upright, 0, 0)).toEqual({ x: "min", y: "min" });
    expect(edgesAt(upright, 0.5, 0)).toEqual({ x: null, y: "min" });
    expect(edgesAt(upright, 1, 0.5)).toEqual({ x: "max", y: null });
    expect(edgesAt(upright, 1, 1)).toEqual({ x: "max", y: "max" });
    expect(HANDLE_POSITIONS).toHaveLength(8);
  });

  it("follows a quarter turn and a flip", () => {
    // Turned clockwise, the screen's top edge is the source's left edge.
    expect(edgesAt({ ...upright, rotate: 90 }, 0.5, 0)).toEqual({ x: "min", y: null });
    // Mirrored, the screen's left edge is the source's right edge.
    expect(edgesAt({ ...upright, flip: true }, 0, 0.5)).toEqual({ x: "max", y: null });
  });

  it("resizes the moved edges only, swapping past the opposite edge", () => {
    expect(resizeCrop(r, 100, 100, { x: "max", y: null }, 90, 0)).toEqual({ x: 0.2, y: 0.2, width: 0.7, height: 0.4 });
    expect(resizeCrop(r, 100, 100, { x: "min", y: "min" }, 10, 5)).toEqual({ x: 0.1, y: 0.05, width: 0.5, height: 0.55 });
    expect(resizeCrop(r, 100, 100, { x: "max", y: null }, 10, 0)).toEqual({ x: 0.1, y: 0.2, width: 0.1, height: 0.4 });
    expect(resizeCrop(r, 100, 100, { x: "max", y: null }, 20.5, 0)).toBeNull();
  });
});

describe("crop fields", () => {
  it("sets one field and keeps the crop valid", () => {
    expect(cropWithField(null, "width", 0.5)).toEqual({ x: 0, y: 0, width: 0.5, height: 1 });
    const r = { x: 0.1, y: 0.1, width: 0.8, height: 0.8 };
    expect(cropWithField(r, "x", 0.5)).toEqual({ ...r, x: 0.2 });
    expect(cropWithField(r, "width", 2)).toEqual({ ...r, width: 0.9 });
    expect(cropWithField(r, "height", 0)).toEqual({ ...r, height: 0.01 });
    expect(cropWithField(r, "y", -1)).toEqual({ ...r, y: 0 });
  });

  it("compares rects", () => {
    const r = { x: 0.1, y: 0.1, width: 0.8, height: 0.8 };
    expect(sameRect(r, { ...r })).toBe(true);
    expect(sameRect(r, { ...r, x: 0.11 })).toBe(false);
    expect(sameRect(null, r)).toBe(false);
  });

  it("moves the near edge of a side spanned whole", () => {
    expect(cropWithField(null, "x", 0.1)).toEqual({ x: 0.1, y: 0, width: 0.9, height: 1 });
    expect(cropWithField(null, "y", 0.25)).toEqual({ x: 0, y: 0.25, width: 1, height: 0.75 });
    const tall = { x: 0.2, y: 0, width: 0.5, height: 1 };
    expect(cropWithField(tall, "y", 0.05)).toEqual({ ...tall, y: 0.05, height: 0.95 });
    expect(cropWithField(tall, "x", 0.9)).toEqual({ ...tall, x: 0.5 });
    expect(cropWithField(null, "x", 2)).toEqual({ x: 0.99, y: 0, width: 0.01, height: 1 });
  });

  it("reads and shows percentages", () => {
    expect(parseCropText("25")).toBe(0.25);
    expect(parseCropText(" 12.5 % ")).toBe(0.125);
    expect(parseCropText("%")).toBeNull();
    expect(parseCropText("x")).toBeNull();
    expect(formatPercent(0.125)).toBe("12.50");
  });
});

describe("crops stay inside the frame after rounding (CropRect::validate)", () => {
  const valid = (r: { x: number; y: number; width: number; height: number }) =>
    r.x >= 0 &&
    r.y >= 0 &&
    r.width >= 0.01 &&
    r.height >= 0.01 &&
    r.x + r.width <= 1 + 1e-6 &&
    r.y + r.height <= 1 + 1e-6;

  it("x 0.12344 with width 100% (reported case 1)", () => {
    const r = cropWithField({ x: 0.12344, y: 0.12344, width: 0.5, height: 0.5 }, "width", 1);
    expect(r.width).toBe(0.8765);
    expect(valid(r)).toBe(true);
    expect(valid(cropWithField({ x: 0.12344, y: 0.12344, width: 0.5, height: 0.5 }, "height", 1))).toBe(true);
  });

  it("a position bound that would round up (reported case 2)", () => {
    // 1 - 0.87654 = 0.12346 rounds up to 0.1235, and 0.1235 + 0.87654 > 1.
    const r = cropWithField({ x: 0, y: 0, width: 0.87654, height: 0.87654 }, "x", 0.5);
    expect(r.x).toBe(0.1234);
    expect(valid(r)).toBe(true);
    expect(valid(cropWithField({ x: 0, y: 0, width: 0.87654, height: 0.87654 }, "y", 0.5))).toBe(true);
    expect(valid(fitCrop({ x: 0.5, y: 0.5, width: 0.87654, height: 0.87654 }))).toBe(true);
  });

  it("the minimum side wins at the far edge", () => {
    expect(cropWithField({ x: 0.995, y: 0, width: 0.005, height: 1 }, "width", 0.001)).toEqual({
      x: 0.99,
      y: 0,
      width: 0.01,
      height: 1,
    });
  });

  it("holds over a sweep of awkward values", () => {
    let seed = 12345;
    const rand = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed / 2147483648;
    };
    const fields = ["x", "y", "width", "height"] as const;
    for (let i = 0; i < 5000; i += 1) {
      const base = fitCrop({ x: rand(), y: rand(), width: rand(), height: rand() });
      expect(valid(base)).toBe(true);
      const unrounded = { x: rand() * 0.5, y: rand() * 0.5, width: 0.01 + rand() * 0.49, height: 0.01 + rand() * 0.49 };
      for (const f of fields) {
        expect(valid(cropWithField(unrounded, f, rand() * 1.2 - 0.1))).toBe(true);
        expect(valid(cropWithField(base, f, rand() * 1.2 - 0.1))).toBe(true);
      }
      const a: [number, number] = [rand() * 120 - 10, rand() * 120 - 10];
      const b: [number, number] = [rand() * 120 - 10, rand() * 120 - 10];
      const drawn = cropFromPoints(100, 100, a, b);
      if (drawn) expect(valid(drawn)).toBe(true);
    }
  });
});

describe("TileRequests", () => {
  const slot = (level: number, tx: number, ty: number) => ({ key: `${level}/${tx}/${ty}`, level, tx, ty });
  const slots = [slot(0, 0, 0), slot(0, 1, 0), slot(2, 0, 0)];

  it("asks for each visible slot once per generation, backdrop first", () => {
    const r = new TileRequests();
    expect(r.plan("positive:0", slots, 3).map((s) => s.key)).toEqual(["2/0/0", "0/0/0", "0/1/0"]);
    expect(r.queue.map((s) => s.key)).toEqual(["2/0/0", "0/0/0", "0/1/0"]);
    r.queue = [];
    expect(r.plan("positive:0", slots, 3)).toEqual([]);
    expect(r.plan("positive:0", slots, 4)).toHaveLength(3);
  });

  it("a mode switch at the same size and generation asks for every tile again", () => {
    const r = new TileRequests();
    r.plan("negative:2", slots, 5);
    r.queue = [];
    // Pass 1 reaches the frame shown as a negative: same size, same generation.
    expect(r.plan("positive:2", slots, 5).map((s) => s.key)).toEqual(["2/0/0", "0/0/0", "0/1/0"]);
    expect(r.current).toBe("positive:2");
    r.queue = [];
    // And back again with the Negative toggle.
    expect(r.plan("negative:2", slots, 5)).toHaveLength(3);
  });

  it("drops queued tiles of the old view and of slots scrolled away", () => {
    const r = new TileRequests();
    r.plan("positive:0", slots, 1);
    expect(r.plan("positive:1", [slots[0]!], 1).map((s) => s.key)).toEqual(["0/0/0"]);
    expect(r.queue.map((s) => s.key)).toEqual(["0/0/0"]);
    r.plan("positive:1", [slots[1]!], 1);
    expect(r.queue.map((s) => s.key)).toEqual(["0/1/0"]);
  });

  it("a forgotten slot is asked for again", () => {
    const r = new TileRequests();
    r.plan("positive:0", slots, 1);
    r.forget("0/1/0");
    expect(r.plan("positive:0", slots, 1).map((s) => s.key)).toEqual(["0/1/0"]);
  });
});
