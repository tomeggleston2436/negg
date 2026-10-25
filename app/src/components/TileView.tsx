// SPDX-License-Identifier: Apache-2.0
// The tiled preview (Ruling 73). Tiles arrive in source orientation and are
// drawn with putImageData; the stage holding them is placed, turned and
// mirrored with one CSS transform. The coarsest level stays loaded underneath
// as a backdrop while finer tiles arrive. A tile is drawn only when its
// generation is at least the one already drawn in its slot, and each new
// generation re-requests the visible tiles. The view shows either the
// rendered positive or the frame's negative (Ruling 134); both use the same
// tile header, so each mode has its own slots and a tile of the other mode
// is never drawn.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent, type WheelEvent } from "react";
import { CommandFailure, negativeTile, renderTile } from "../ipc/commands";
import { parseTile } from "../ipc/tile";
import type { Geometry } from "../ipc/types";
import {
  type NormRect,
  TileRequests,
  type Rotation,
  type ViewPlacement,
  fitScale,
  levelForScale,
  screenToSource,
  stageTransform,
  tileGrid,
  tileSize,
  visibleSourceRect,
  visibleTiles,
} from "../tiles";
import { CropOverlay } from "./CropOverlay";

/** "fit" or a scale in CSS pixels per source pixel. */
export type Zoom = "fit" | number;

/** What the view tiles: a frame's size and pyramid (FrameView or NegativeView). */
export interface TileSource {
  index: number;
  width: number;
  height: number;
  levels: number;
  tile_size: number;
}

/** The rendered positive, or the negative before (or instead of) the analysis. */
export type TileMode = "positive" | "negative";

/** The drawing tool on the picture, if any. */
export type Tool = "crop" | "area" | null;

const MAX_INFLIGHT = 2;
const MIN_SCALE = 1 / 64;
const MAX_SCALE = 8;

interface Slot {
  key: string;
  level: number;
  tx: number;
  ty: number;
}

export interface TileViewProps {
  source: TileSource;
  mode: TileMode;
  generation: number;
  /** Turn and flip of the view, and the crop drawn. */
  geometry: Geometry;
  /** The analysis area drawn (source coordinates); null for none. */
  area: NormRect | null;
  /** Its label: whose area it is (the roll's or the frame's). */
  areaLabel?: string;
  zoom: Zoom;
  onZoom: (z: Zoom) => void;
  tool: Tool;
  onCropDrag: (crop: NormRect) => void;
  onCrop: (crop: NormRect) => void;
  onCropCancel: () => void;
  /** A drawn analysis area (on release; a drag only moves the outline). */
  onArea: (area: NormRect) => void;
  onError: (e: unknown) => void;
}

const noop = () => undefined;

export function TileView({
  source,
  mode,
  generation,
  geometry,
  area,
  areaLabel = "Analysis area",
  zoom,
  onZoom,
  tool,
  onCropDrag,
  onCrop,
  onCropCancel,
  onArea,
  onError,
}: TileViewProps) {
  const cropMode = tool !== null;
  const viewportRef = useRef<HTMLDivElement>(null);
  const [vp, setVp] = useState({ w: 0, h: 0 });
  const [centre, setCentre] = useState<[number, number]>([source.width / 2, source.height / 2]);
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const { width: W, height: H, tile_size: tile, levels, index } = source;
  const rotate = geometry.rotate_degrees as Rotation;

  useLayoutEffect(() => {
    const el = viewportRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setVp({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setVp({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  // A new frame starts centred.
  useEffect(() => setCentre([W / 2, H / 2]), [index, W, H]);

  const scale = zoom === "fit" ? fitScale(W, H, rotate, vp.w, vp.h) : zoom;
  const placement: ViewPlacement = useMemo(
    () => ({
      cx: zoom === "fit" ? W / 2 : centre[0],
      cy: zoom === "fit" ? H / 2 : centre[1],
      scale,
      rotate,
      flip: geometry.flip_horizontal,
      vw: vp.w,
      vh: vp.h,
    }),
    [zoom, W, H, centre, scale, rotate, geometry.flip_horizontal, vp.w, vp.h],
  );
  const level = levelForScale(scale, dpr, levels);
  const top = levels - 1;

  const slots: Slot[] = useMemo(() => {
    if (vp.w === 0 || vp.h === 0) return [];
    const out: Slot[] = [];
    const [cols, rows] = tileGrid(W, H, top, tile);
    for (let ty = 0; ty < rows; ty += 1) {
      for (let tx = 0; tx < cols; tx += 1) out.push({ key: `${top}/${tx}/${ty}`, level: top, tx, ty });
    }
    if (level !== top) {
      for (const [tx, ty] of visibleTiles(visibleSourceRect(placement), level, W, H, tile)) {
        out.push({ key: `${level}/${tx}/${ty}`, level, tx, ty });
      }
    }
    return out;
  }, [placement, level, top, W, H, tile, vp.w, vp.h]);

  // Per-slot canvases and the generation drawn and requested in each.
  const canvases = useRef(new Map<string, HTMLCanvasElement>());
  const drawn = useRef(new Map<string, number>());
  const requests = useRef(new TileRequests<Slot>());
  // The view the tiles belong to: the same header for both modes.
  const view = `${mode}:${index}`;
  const inflight = useRef(0);
  const frameRef = useRef(index);
  frameRef.current = index;
  const modeRef = useRef(mode);
  modeRef.current = mode;

  // Tile failures are reported once per frame and kind, not once per tile.
  const reported = useRef(new Set<string>());
  const report = useCallback(
    (e: unknown) => {
      const key = `${frameRef.current}:${errorKind(e)}`;
      if (reported.current.has(key)) return;
      reported.current.add(key);
      onError(e);
    },
    [onError],
  );

  // Switching frames, or between positive and negative, forgets every slot
  // (the requests forget theirs on the next pass for the new view).
  useEffect(() => {
    drawn.current.clear();
    reported.current.clear();
  }, [view]);

  const pump = useCallback(() => {
    const queue = requests.current;
    while (inflight.current < MAX_INFLIGHT && queue.queue.length > 0) {
      const s = queue.queue.shift();
      if (!s) break;
      const forFrame = frameRef.current;
      const forMode = modeRef.current;
      const fetchTile = forMode === "negative" ? negativeTile : renderTile;
      inflight.current += 1;
      fetchTile(forFrame, s.level, s.tx, s.ty)
        .then((buf) => {
          const { header, pixels } = parseTile(buf);
          // The header does not say which kind of tile it is: one asked for
          // in the other mode is dropped here.
          if (forMode !== modeRef.current) return;
          if (header.frame !== frameRef.current || header.level !== s.level) return;
          const key = `${header.level}/${header.tileX}/${header.tileY}`;
          const canvas = canvases.current.get(key);
          if (!canvas) return;
          if (header.generation < (drawn.current.get(key) ?? -1)) return;
          canvas.width = header.width;
          canvas.height = header.height;
          canvas.getContext("2d")?.putImageData(new ImageData(pixels, header.width, header.height), 0, 0);
          drawn.current.set(key, header.generation);
        })
        .catch((e: unknown) => {
          if (forMode !== modeRef.current) return;
          // Let the slot be asked for again on the next pass.
          requests.current.forget(s.key);
          report(e);
        })
        .finally(() => {
          inflight.current -= 1;
          pump();
        });
    }
  }, [report]);

  // Ask for every visible slot not yet requested at this generation. The
  // queue is rebuilt each time, so tiles scrolled away are never fetched.
  // A new view (frame or mode) runs a pass even at the same size and
  // generation, and asks for every visible tile again.
  useEffect(() => {
    requests.current.plan(view, slots, generation);
    pump();
  }, [view, slots, generation, pump]);

  // One stable ref callback per slot key, so React only calls it on mount
  // and unmount (a new function each render would detach and reattach).
  const refFns = useRef(new Map<string, (el: HTMLCanvasElement | null) => void>());
  const slotRef = (key: string) => {
    let fn = refFns.current.get(key);
    if (!fn) {
      fn = (el) => {
        if (el) {
          canvases.current.set(key, el);
        } else {
          canvases.current.delete(key);
          drawn.current.delete(key);
          requests.current.forget(key);
        }
      };
      refFns.current.set(key, fn);
    }
    return fn;
  };

  // Pan with a drag (outside crop mode) and zoom with the wheel about the pointer.
  const pan = useRef<{ x: number; y: number; cx: number; cy: number } | null>(null);
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (cropMode || e.button !== 0) return;
    pan.current = { x: e.clientX, y: e.clientY, cx: placement.cx, cy: placement.cy };
    if (zoom === "fit") onZoom(scale);
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const p = pan.current;
    if (!p) return;
    const moved = { ...placement, cx: p.cx, cy: p.cy };
    const [ax, ay] = screenToSource(moved, vp.w / 2, vp.h / 2);
    const [bx, by] = screenToSource(moved, vp.w / 2 + e.clientX - p.x, vp.h / 2 + e.clientY - p.y);
    setCentre([clamp(p.cx - (bx - ax), 0, W), clamp(p.cy - (by - ay), 0, H)]);
  };
  const onPointerUp = () => {
    pan.current = null;
  };
  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    const sx = e.clientX - box.left;
    const sy = e.clientY - box.top;
    const factor = Math.exp(-e.deltaY * 0.002);
    const next = clamp(scale * factor, MIN_SCALE, MAX_SCALE);
    // Keep the source point under the pointer where it is.
    const [px, py] = screenToSource(placement, sx, sy);
    const after = { ...placement, scale: next };
    const [qx, qy] = screenToSource(after, sx, sy);
    setCentre([clamp(placement.cx + px - qx, 0, W), clamp(placement.cy + py - qy, 0, H)]);
    onZoom(next);
  };

  const pixelated = scale * dpr >= 2;
  return (
    <div
      ref={viewportRef}
      className={`tile-viewport${cropMode ? " crop-mode" : ""}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onWheel={onWheel}
      role="img"
      aria-label={`${mode === "negative" ? "Negative" : "Preview"} of frame ${index + 1} at ${Math.round(scale * 100)}%`}
    >
      <div className="tile-stage" style={{ width: W, height: H, transform: stageTransform(placement) }}>
        {slots.map((s) => {
          const f = 2 ** s.level;
          const [tw, th] = tileSize(W, H, s.level, tile, s.tx, s.ty);
          return (
            <canvas
              key={`${mode}:${index}:${s.key}`}
              ref={slotRef(s.key)}
              className={pixelated && s.level === 0 ? "tile pixelated" : "tile"}
              style={{
                left: s.tx * tile * f,
                top: s.ty * tile * f,
                width: tw * f,
                height: th * f,
                zIndex: levels - s.level,
              }}
            />
          );
        })}
      </div>
      <CropOverlay
        placement={placement}
        width={W}
        height={H}
        crop={geometry.crop}
        active={tool === "crop"}
        onDrag={onCropDrag}
        onCommit={onCrop}
        onCancel={onCropCancel}
        label={tool === "crop" || area !== null ? "Crop" : undefined}
      />
      <CropOverlay
        variant="area"
        placement={placement}
        width={W}
        height={H}
        crop={area}
        active={tool === "area"}
        onDrag={noop}
        onCommit={onArea}
        onCancel={noop}
        label={areaLabel}
      />
    </div>
  );
}

function errorKind(e: unknown): string {
  if (e instanceof CommandFailure) return `command:${e.kind}`;
  if (e instanceof Error) return e.name;
  return "unknown";
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}
