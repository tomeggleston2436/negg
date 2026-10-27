// SPDX-License-Identifier: Apache-2.0
// Tile placement for the preview (Ruling 73): which pyramid level to show and
// which tiles cover the viewport. Positions and sizes only; no pixel values
// are computed here.

export type Rotation = 0 | 90 | 180 | 270;

/** L + 1, where L is the smallest n with max(w_n, h_n) <= tile (as in Rust). */
export function levelCount(w: number, h: number, tile: number): number {
  let [lw, lh] = [w, h];
  let n = 0;
  while (Math.max(lw, lh) > tile) {
    lw = Math.ceil(lw / 2);
    lh = Math.ceil(lh / 2);
    n += 1;
  }
  return n + 1;
}

/** Size of `level` (0 = full size): ceil halving, one level at a time. */
export function levelSize(w: number, h: number, level: number): [number, number] {
  let [lw, lh] = [w, h];
  for (let i = 0; i < level; i += 1) {
    lw = Math.ceil(lw / 2);
    lh = Math.ceil(lh / 2);
  }
  return [lw, lh];
}

/** Number of tile columns and rows at `level`. */
export function tileGrid(w: number, h: number, level: number, tile: number): [number, number] {
  const [lw, lh] = levelSize(w, h, level);
  return [Math.ceil(lw / tile), Math.ceil(lh / tile)];
}

/** Size in pixels of tile (tx, ty) at `level` (edge tiles are smaller). */
export function tileSize(w: number, h: number, level: number, tile: number, tx: number, ty: number): [number, number] {
  const [lw, lh] = levelSize(w, h, level);
  return [Math.min(tile, lw - tx * tile), Math.min(tile, lh - ty * tile)];
}

/**
 * Tiles of `level` that intersect `view`, a rectangle in level 0 (source)
 * pixels. Row-major, nearest the top-left first. Empty when the view misses
 * the image.
 */
export function visibleTiles(
  view: { x: number; y: number; width: number; height: number },
  level: number,
  w: number,
  h: number,
  tile: number,
): Array<[number, number]> {
  const [cols, rows] = tileGrid(w, h, level, tile);
  const span = tile * 2 ** level; // level 0 pixels covered by one tile side
  const x0 = Math.max(0, Math.floor(view.x / span));
  const y0 = Math.max(0, Math.floor(view.y / span));
  const x1 = Math.min(cols, Math.ceil((view.x + view.width) / span));
  const y1 = Math.min(rows, Math.ceil((view.y + view.height) / span));
  const out: Array<[number, number]> = [];
  for (let ty = y0; ty < y1; ty += 1) {
    for (let tx = x0; tx < x1; tx += 1) out.push([tx, ty]);
  }
  return out;
}

/**
 * The smallest level whose size fits a viewport given in device pixels
 * (CSS size times devicePixelRatio). Clamped to the pyramid the session
 * builds: the last level (levelCount(w, h, tile) - 1) when none fits.
 */
export function fitLevel(w: number, h: number, viewportW: number, viewportH: number, tile: number): number {
  const last = levelCount(w, h, tile) - 1;
  for (let l = 0; l <= last; l += 1) {
    const [lw, lh] = levelSize(w, h, l);
    if (lw <= viewportW && lh <= viewportH) return l;
  }
  return last;
}

/**
 * The level to draw at a display scale (CSS pixels per source pixel): the
 * coarsest level that still has at least one level pixel per device pixel,
 * clamped to [0, levels - 1].
 */
export function levelForScale(scale: number, dpr: number, levels: number): number {
  const devicePerSource = scale * dpr;
  if (!(devicePerSource > 0)) return levels - 1;
  const l = Math.floor(Math.log2(1 / devicePerSource) + 1e-9);
  return Math.max(0, Math.min(levels - 1, l));
}

/**
 * The level drawn while a slider is dragged: one coarser than `fine` (a
 * quarter of its pixels), so the preview keeps up with the slider. Null
 * when that is the backdrop `top` (or past it), which is always drawn.
 */
export function dragLevel(fine: number, top: number): number | null {
  return fine + 1 < top ? fine + 1 : null;
}

/**
 * The pyramid levels a tiled preview mounts and asks for, besides the
 * backdrop `top` (always both), coarsest first. `fine` is the level for
 * the scale ({@link levelForScale}); `kept` is the coarser level left
 * mounted after the last slider drag (null for none). While `dragging`
 * only the {@link dragLevel} is asked for; `fine` stays mounted (hidden
 * by the view) and is asked for again on release, drawn over `kept`,
 * which is not asked for again, so the picture never falls back to the
 * backdrop while the fine tiles arrive.
 */
export function previewLevels(
  fine: number,
  top: number,
  dragging: boolean,
  kept: number | null,
): { mount: number[]; request: number[] } {
  const drag = dragging ? dragLevel(fine, top) : null;
  const mount = [kept, drag, fine]
    .filter((l): l is number => l !== null && l < top)
    .filter((l, i, all) => all.indexOf(l) === i)
    .sort((a, b) => b - a);
  const request = dragging ? (drag === null ? [] : [drag]) : fine < top ? [fine] : [];
  return { mount, request };
}

/** Image size after a quarter turn (crop is not applied in the preview). */
export function orientedSize(w: number, h: number, rotate: Rotation): [number, number] {
  return rotate === 90 || rotate === 270 ? [h, w] : [w, h];
}

/** Scale that fits the oriented image inside the viewport. */
export function fitScale(w: number, h: number, rotate: Rotation, vw: number, vh: number): number {
  const [ow, oh] = orientedSize(w, h, rotate);
  if (ow <= 0 || oh <= 0 || vw <= 0 || vh <= 0) return 1;
  return Math.min(vw / ow, vh / oh);
}

/** How the source is placed on screen: centre (source px), scale, orientation. */
export interface ViewPlacement {
  cx: number;
  cy: number;
  scale: number;
  rotate: Rotation;
  flip: boolean;
  vw: number;
  vh: number;
}

/**
 * CSS transform for a stage in source pixels (transform-origin 0 0): move the
 * centre to the origin, rotate clockwise, mirror left-right (rotate then flip,
 * as Ruling 70 orders export), scale, then centre in the viewport.
 */
export function stageTransform(p: ViewPlacement): string {
  const flip = p.flip ? " scaleX(-1)" : "";
  return `translate(${p.vw / 2}px, ${p.vh / 2}px) scale(${p.scale})${flip} rotate(${p.rotate}deg) translate(${-p.cx}px, ${-p.cy}px)`;
}

function rotatePoint(x: number, y: number, deg: number): [number, number] {
  switch (((deg % 360) + 360) % 360) {
    case 90:
      return [-y, x];
    case 180:
      return [-x, -y];
    case 270:
      return [y, -x];
    default:
      return [x, y];
  }
}

/** Source pixel position of a viewport point (inverse of stageTransform). */
export function screenToSource(p: ViewPlacement, sx: number, sy: number): [number, number] {
  let x = (sx - p.vw / 2) / p.scale;
  const y = (sy - p.vh / 2) / p.scale;
  if (p.flip) x = -x;
  const [rx, ry] = rotatePoint(x, y, -p.rotate);
  return [rx + p.cx, ry + p.cy];
}

/** Viewport position of a source pixel (forward stageTransform). */
export function sourceToScreen(p: ViewPlacement, x: number, y: number): [number, number] {
  let [rx, ry] = rotatePoint(x - p.cx, y - p.cy, p.rotate);
  if (p.flip) rx = -rx;
  return [rx * p.scale + p.vw / 2, ry * p.scale + p.vh / 2];
}

/** The source rectangle the viewport shows (bounding box of its corners). */
export function visibleSourceRect(p: ViewPlacement): { x: number; y: number; width: number; height: number } {
  const corners = [
    screenToSource(p, 0, 0),
    screenToSource(p, p.vw, 0),
    screenToSource(p, 0, p.vh),
    screenToSource(p, p.vw, p.vh),
  ];
  const xs = corners.map((c) => c[0]);
  const ys = corners.map((c) => c[1]);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, width: Math.max(...xs) - x, height: Math.max(...ys) - y };
}

export interface NormRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Smallest crop side (CropRect::validate). */
export const MIN_CROP = 0.01;

function round4(v: number): number {
  return Math.round(v * 10000) / 10000;
}

/**
 * Rounds down to 4 decimals, for upper bounds: a bound rounded up could put
 * x + width past 1 + 1e-6, which CropRect::validate rejects. The tiny epsilon
 * only absorbs binary noise such as 0.8 * 10000 = 7999.999...; any excess it
 * allows is far below that slack.
 */
function floor4(v: number): number {
  return Math.floor(v * 10000 + 1e-7) / 10000;
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

/**
 * A normalised crop from two source points (any order) on a w x h source,
 * clamped to the frame and rounded to 4 decimals; null when either side is
 * under MIN_CROP (a click rather than a drag).
 */
export function cropFromPoints(w: number, h: number, a: [number, number], b: [number, number]): NormRect | null {
  const x0 = clamp01(Math.min(a[0], b[0]) / w);
  const x1 = clamp01(Math.max(a[0], b[0]) / w);
  const y0 = clamp01(Math.min(a[1], b[1]) / h);
  const y1 = clamp01(Math.max(a[1], b[1]) / h);
  const rect = { x: round4(x0), y: round4(y0), width: round4(x1 - x0), height: round4(y1 - y0) };
  if (rect.width < MIN_CROP || rect.height < MIN_CROP) return null;
  return fitCrop(rect);
}

/** Keeps a crop inside the frame (x + width <= 1, y + height <= 1) after rounding. */
export function fitCrop(r: NormRect): NormRect {
  const width = Math.min(1, Math.max(MIN_CROP, r.width));
  const height = Math.min(1, Math.max(MIN_CROP, r.height));
  const x = Math.max(0, Math.min(round4(Math.max(0, r.x)), floor4(1 - width)));
  const y = Math.max(0, Math.min(round4(Math.max(0, r.y)), floor4(1 - height)));
  return { x, y, width, height };
}

/** The crop moved by (dx, dy) normalised units, kept inside the frame. */
export function moveCrop(r: NormRect, dx: number, dy: number): NormRect {
  return fitCrop({ ...r, x: r.x + dx, y: r.y + dy });
}

/** Screen rectangle (viewport pixels) of a normalised crop under a placement. */
export function cropScreenRect(p: ViewPlacement, w: number, h: number, r: NormRect): NormRect {
  const [ax, ay] = sourceToScreen(p, r.x * w, r.y * h);
  const [bx, by] = sourceToScreen(p, (r.x + r.width) * w, (r.y + r.height) * h);
  return { x: Math.min(ax, bx), y: Math.min(ay, by), width: Math.abs(bx - ax), height: Math.abs(by - ay) };
}

/** Which source edge a crop handle moves on each axis (null: that axis stays). */
export interface CropHandle {
  x: "min" | "max" | null;
  y: "min" | "max" | null;
}

/** The eight handles of a crop, in screen terms: corners and edge midpoints. */
export const HANDLE_POSITIONS: ReadonlyArray<[number, number]> = [
  [0, 0],
  [0.5, 0],
  [1, 0],
  [1, 0.5],
  [1, 1],
  [0.5, 1],
  [0, 1],
  [0, 0.5],
];

/**
 * Which source edges a handle at screen point (sx, sy) moves. The handle is
 * mapped back into source pixels, so a quarter turn or flip of the view is
 * taken into account: an edge midpoint lies on the crop centre line of one
 * source axis and leaves that axis alone.
 */
export function handleEdges(p: ViewPlacement, w: number, h: number, r: NormRect, sx: number, sy: number): CropHandle {
  const [hx, hy] = screenToSource(p, sx, sy);
  const cx = (r.x + r.width / 2) * w;
  const cy = (r.y + r.height / 2) * h;
  const tolX = (r.width * w) / 4;
  const tolY = (r.height * h) / 4;
  return {
    x: Math.abs(hx - cx) < tolX ? null : hx < cx ? "min" : "max",
    y: Math.abs(hy - cy) < tolY ? null : hy < cy ? "min" : "max",
  };
}

/**
 * The crop with the handle's edges moved to source point (px, py). Dragging
 * past the opposite edge swaps them; null when a side would fall under
 * MIN_CROP (the caller keeps the last valid rect).
 */
export function resizeCrop(r: NormRect, w: number, h: number, handle: CropHandle, px: number, py: number): NormRect | null {
  let x0 = r.x * w;
  let x1 = (r.x + r.width) * w;
  let y0 = r.y * h;
  let y1 = (r.y + r.height) * h;
  if (handle.x === "min") x0 = px;
  if (handle.x === "max") x1 = px;
  if (handle.y === "min") y0 = py;
  if (handle.y === "max") y1 = py;
  return cropFromPoints(w, h, [x0, y0], [x1, y1]);
}

export type CropField = "x" | "y" | "width" | "height";

/** True when `b` is the rect `a` (none is never equal to a rect). */
export function sameRect(a: NormRect | null, b: NormRect): boolean {
  return a !== null && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/** The whole frame as a rect (what "no crop" means). */
export const WHOLE_FRAME: NormRect = { x: 0, y: 0, width: 1, height: 1 };

/**
 * The crop with one field set to `value` (a fraction of the source side),
 * kept valid: a position is clamped so the crop stays inside the frame, a
 * size to [MIN_CROP, 1 - position]. No crop counts as the whole frame. A
 * position typed on a side the rect spans whole (no crop, or a full width
 * or height) moves its near edge and keeps the far edge at the frame's,
 * so the size shrinks instead of the position being clamped back to 0.
 */
export function cropWithField(r: NormRect | null, field: CropField, value: number): NormRect {
  const base = r ?? WHOLE_FRAME;
  const v = Number.isFinite(value) ? value : 0;
  const position = (size: number) => Math.max(0, Math.min(round4(Math.max(0, v)), floor4(1 - size)));
  // A side spanned whole: the near edge moves, the far edge stays.
  const fromEdge = (): [number, number] => {
    const p = Math.max(0, Math.min(round4(Math.max(0, v)), floor4(1 - MIN_CROP)));
    return [p, floor4(1 - p)];
  };
  // A size keeps its position when it fits; at the far edge the minimum side
  // wins and the position moves back.
  const size = (pos: number): [number, number] => {
    const s = Math.min(round4(Math.max(MIN_CROP, v)), floor4(1 - pos));
    return s >= MIN_CROP ? [pos, s] : [floor4(1 - MIN_CROP), MIN_CROP];
  };
  switch (field) {
    case "x": {
      if (base.x <= 0 && base.width >= 1) {
        const [x, width] = fromEdge();
        return { ...base, x, width };
      }
      return { ...base, x: position(base.width) };
    }
    case "y": {
      if (base.y <= 0 && base.height >= 1) {
        const [y, height] = fromEdge();
        return { ...base, y, height };
      }
      return { ...base, y: position(base.height) };
    }
    case "width": {
      const [x, width] = size(base.x);
      return { ...base, x, width };
    }
    case "height": {
      const [y, height] = size(base.y);
      return { ...base, y, height };
    }
  }
}

/** Percentage text for a crop fraction (two decimals). */
export function formatPercent(fraction: number): string {
  return (fraction * 100).toFixed(2);
}

/**
 * Parses typed crop text, a percentage of the side ("25", "25%" or "25.5"),
 * into a fraction. Null when it is not a number.
 */
export function parseCropText(text: string): number | null {
  const t = text.trim().replace(/\u2212/g, "-");
  const body = t.endsWith("%") ? t.slice(0, -1).trim() : t;
  if (body === "") return null;
  const v = Number(body);
  return Number.isFinite(v) ? v / 100 : null;
}

/** One tile slot of the preview: a pyramid level and tile position. */
export interface TileSlot {
  key: string;
  level: number;
  tx: number;
  ty: number;
}

/**
 * Which tiles the preview asks for (Ruling 73): each visible slot once per
 * generation, the coarsest level first, in a queue rebuilt on every pass so
 * tiles scrolled away are never fetched. The requests belong to one view (a
 * frame in one mode, positive or negative, whose tiles share a header): a
 * pass for another view forgets every request first, so a mode switch at the
 * same size and generation asks for every visible tile again.
 */
export class TileRequests<S extends TileSlot = TileSlot> {
  private view: string | null = null;
  private readonly requested = new Map<string, number>();
  /** Slots waiting to be fetched, in order. */
  queue: S[] = [];

  /** The view the requests belong to, or null before the first pass. */
  get current(): string | null {
    return this.view;
  }

  /** Plans a pass for `view` at `generation`; returns the slots newly asked for. */
  plan(view: string, slots: readonly S[], generation: number): S[] {
    if (view !== this.view) this.reset(view);
    const want = slots.filter((s) => (this.requested.get(s.key) ?? -1) < generation);
    want.sort((a, b) => b.level - a.level); // backdrop first
    for (const s of want) this.requested.set(s.key, generation);
    const keep = new Set(slots.map((s) => s.key));
    const wanted = new Set(want.map((s) => s.key));
    this.queue = [...this.queue.filter((q) => keep.has(q.key) && !wanted.has(q.key)), ...want];
    return want;
  }

  /** Lets a slot be asked for again (its fetch failed, or its canvas went away). */
  forget(key: string): void {
    this.requested.delete(key);
  }

  /** Forgets every request and empties the queue, for `view`. */
  reset(view: string | null): void {
    this.view = view;
    this.requested.clear();
    this.queue = [];
  }
}
