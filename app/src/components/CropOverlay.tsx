// SPDX-License-Identifier: Apache-2.0
// The crop overlay. The crop lives in source coordinates (normalised, before
// any quarter turn or flip, Ruling 70) and is mapped through the same
// placement as the tiles, so it turns and mirrors with the picture. In crop
// mode a drag outside the crop draws a new one, a drag inside moves it, and
// the eight handles resize it. Each drag step is reported with `onDrag`
// (preview) and the release with `onCommit` (Ruling 79). A release that
// leaves no valid crop (a click, or a resize below the minimum side) and a
// cancelled pointer commit nothing: `onCancel` reverts any preview the drag
// sent, so no uncommitted crop lingers in the store.

import { useEffect, useRef, useState, type PointerEvent } from "react";
import {
  type CropHandle,
  HANDLE_POSITIONS,
  type NormRect,
  type ViewPlacement,
  cropFromPoints,
  cropScreenRect,
  handleEdges,
  moveCrop,
  resizeCrop,
  screenToSource,
} from "../tiles";

export interface CropOverlayProps {
  placement: ViewPlacement;
  width: number;
  height: number;
  crop: NormRect | null;
  active: boolean;
  onDrag: (crop: NormRect) => void;
  onCommit: (crop: NormRect) => void;
  /** The drag ended without a commit after previewing: revert the preview. */
  onCancel: () => void;
  /**
   * "crop" (the output crop, dimming what it leaves out) or "area" (the
   * analysis area, Ruling 134: its own colour, nothing dimmed, as it does not
   * crop the photo).
   */
  variant?: "crop" | "area";
  /** Label drawn on the rectangle. */
  label?: string;
}

type Drag =
  | { kind: "draw"; start: [number, number] }
  | { kind: "move"; start: [number, number]; from: NormRect }
  | { kind: "resize"; handle: CropHandle; from: NormRect };

export function CropOverlay({
  placement,
  width,
  height,
  crop,
  active,
  onDrag,
  onCommit,
  onCancel,
  variant = "crop",
  label,
}: CropOverlayProps) {
  const [draft, setDraft] = useState<NormRect | null>(null);
  const drag = useRef<Drag | null>(null);
  const previewed = useRef(false);
  const layer = useRef<HTMLDivElement>(null);
  useEffect(() => setDraft(null), [crop]);

  const shown = draft ?? crop;
  const local = (e: PointerEvent): [number, number] => {
    const box = layer.current?.getBoundingClientRect();
    return screenToSource(placement, e.clientX - (box?.left ?? 0), e.clientY - (box?.top ?? 0));
  };

  const begin = (e: PointerEvent<HTMLElement>, d: Drag) => {
    drag.current = d;
    previewed.current = false;
    layer.current?.setPointerCapture(e.pointerId);
    e.preventDefault();
    e.stopPropagation();
  };

  const down = (e: PointerEvent<HTMLDivElement>) => {
    if (!active || e.button !== 0) return;
    const p = local(e);
    const inside =
      crop !== null &&
      p[0] >= crop.x * width &&
      p[0] <= (crop.x + crop.width) * width &&
      p[1] >= crop.y * height &&
      p[1] <= (crop.y + crop.height) * height;
    begin(e, inside && crop ? { kind: "move", start: p, from: crop } : { kind: "draw", start: p });
  };

  const handleDown = (e: PointerEvent<HTMLElement>, sx: number, sy: number) => {
    if (!active || e.button !== 0 || !shown) return;
    begin(e, { kind: "resize", handle: handleEdges(placement, width, height, shown, sx, sy), from: shown });
  };

  const moveTo = (e: PointerEvent): NormRect | null => {
    const d = drag.current;
    if (!d) return null;
    const p = local(e);
    if (d.kind === "draw") return cropFromPoints(width, height, d.start, p);
    if (d.kind === "move") return moveCrop(d.from, (p[0] - d.start[0]) / width, (p[1] - d.start[1]) / height);
    return resizeCrop(d.from, width, height, d.handle, p[0], p[1]);
  };

  const move = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    const r = moveTo(e);
    if (r) {
      setDraft(r);
      previewed.current = true;
      onDrag(r);
    }
  };

  const abandon = () => {
    drag.current = null;
    setDraft(null);
    if (previewed.current) onCancel();
    previewed.current = false;
  };

  const up = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    const r = moveTo(e);
    if (!r) {
      abandon();
      return;
    }
    drag.current = null;
    previewed.current = false;
    setDraft(r);
    onCommit(r);
  };

  const rect = shown ? cropScreenRect(placement, width, height, shown) : null;
  return (
    <div
      ref={layer}
      className={`crop-layer ${variant}-layer${active ? " active" : ""}`}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={() => {
        if (drag.current) abandon();
      }}
      aria-hidden="true"
    >
      {rect && (
        <div
          className={`crop-rect ${variant}${active ? " editing" : ""}`}
          style={{ left: rect.x, top: rect.y, width: rect.width, height: rect.height }}
        >
          {label && <span className="rect-label">{label}</span>}
        </div>
      )}
      {rect &&
        active &&
        HANDLE_POSITIONS.map(([fx, fy]) => {
          const sx = rect.x + fx * rect.width;
          const sy = rect.y + fy * rect.height;
          return (
            <div
              key={`${fx}-${fy}`}
              className={`crop-handle ${variant}`}
              style={{ left: sx, top: sy, cursor: handleCursor(fx, fy) }}
              onPointerDown={(e) => handleDown(e, sx, sy)}
            />
          );
        })}
    </div>
  );
}

function handleCursor(fx: number, fy: number): string {
  if (fx === 0.5) return "ns-resize";
  if (fy === 0.5) return "ew-resize";
  return (fx === 0) === (fy === 0) ? "nwse-resize" : "nesw-resize";
}
