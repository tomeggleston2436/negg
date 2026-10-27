// SPDX-License-Identifier: Apache-2.0
// The analysis area: the part of the scan the
// estimates are measured from: the whole film including its clear edge
// (rebate, where the film base is measured), leaving out only the film
// holder. It is separate from the output crop and never crops the photo. The panel edits it for the roll
// (set_roll_analysis_area) or for one frame (commit_frame with a geometry
// override), by numeric entry, and on the roll screen by drawing on a
// frame's negative thumbnail. Coordinates are the crop's: normalised, in
// source orientation.

import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { CropRect } from "../ipc/types";
import { type Rotation, type ViewPlacement, orientedSize, sameRect, stageTransform } from "../tiles";
import { CropFields } from "./CropFields";
import { CropOverlay } from "./CropOverlay";

export interface AnalysisAreaPanelProps {
  /** Whose area this is: the roll's, or the frame's on screen. */
  scope: "roll" | "frame";
  /** The area in effect for the scope; null for none. */
  area: CropRect | null;
  /** The crop in effect for the scope: what is analysed when there is no area. */
  crop?: CropRect | null;
  /** Frame scope: the frame has an area of its own (not the roll's). */
  own?: boolean;
  disabled: boolean;
  /** Frame screen: the analysis area drawing tool is on. */
  drawing?: boolean;
  onDraw?: (on: boolean) => void;
  onCommit: (area: CropRect | null) => void;
  /** Frame scope: drop the frame's own area ("Use roll analysis area"). */
  onUseRoll?: () => void;
  /** An extra line under the heading. */
  hint?: string;
  /** A drawing surface (the roll screen's negative sketch). */
  children?: ReactNode;
}

export function AnalysisAreaPanel(props: AnalysisAreaPanelProps) {
  const { scope, area, own, disabled } = props;
  const label = scope === "roll" ? "Roll" : "Frame";
  const where =
    scope === "roll"
      ? "Shared by every frame without an area of its own."
      : own
        ? "This frame's own area."
        : "From the roll.";
  return (
    <section aria-labelledby={`${scope}-area-heading`} className="area-panel">
      <h2 id={`${scope}-area-heading`}>
        <span className="area-swatch" aria-hidden="true" />
        Analysis area
      </h2>
      <p className="note">
        What the estimates are measured from. Cover the whole film including its clear edge (rebate), where the film
        base is measured, and leave out only the film holder. It does not crop the photo.
      </p>
      {props.hint && <p className="note">{props.hint}</p>}
      {props.children}
      <CropFields
        crop={area}
        disabled={disabled}
        label={label}
        noun="analysis area"
        onCommit={(r) => props.onCommit(r)}
      />
      <p className="note">
        {area
          ? `Area ${pct(area.width)} by ${pct(area.height)} of the frame. `
          : props.crop
            ? "No area: the crop is analysed. "
            : "No area: the whole frame is analysed. "}
        {where}
      </p>
      <div className="button-row wrap">
        {props.onDraw && (
          <button
            type="button"
            aria-pressed={props.drawing === true}
            disabled={disabled}
            onClick={() => props.onDraw?.(!props.drawing)}
            title="Drag on the picture to draw, move or resize the analysis area"
          >
            {props.drawing ? "Finish analysis area" : "Draw analysis area"}
          </button>
        )}
        <button
          type="button"
          disabled={disabled || area === null}
          onClick={() => props.onCommit(null)}
          title={scope === "roll" ? "No analysis area for the roll" : "No analysis area for this frame"}
        >
          {scope === "roll" ? "Clear roll analysis area" : "Clear analysis area"}
        </button>
        {props.onUseRoll && (
          <button
            type="button"
            disabled={disabled || !own}
            onClick={props.onUseRoll}
            title="Drop this frame's own area so it follows the roll's"
          >
            Use roll analysis area
          </button>
        )}
      </div>
    </section>
  );
}

export interface AreaSketchProps {
  /** The negative thumbnail's blob URL (source orientation, uncropped). */
  url: string | null | undefined;
  /** Which frame it is, for the caption. */
  caption: string;
  area: CropRect | null;
  /** The roll's quarter turn and flip: the sketch is shown as the photos are. */
  rotate: Rotation;
  flip: boolean;
  disabled: boolean;
  onCommit: (area: CropRect) => void;
}

/**
 * Draws the analysis area on a frame's negative thumbnail. The thumbnail is
 * in source orientation and uncropped, so the normalised area maps straight
 * onto its pixels; it is shown turned and flipped as the roll's photos are,
 * with one CSS transform (as the tiled preview), and the area is still
 * stored in source orientation.
 */
export function AreaSketch({ url, caption, area, rotate, flip, disabled, onCommit }: AreaSketchProps) {
  // A click that leaves the rect as it was commits nothing (no history
  // entry, no analysis again).
  const commit = (r: CropRect) => {
    if (!sameRect(area, r)) onCommit(r);
  };
  const box = useRef<HTMLDivElement>(null);
  const [vp, setVp] = useState({ w: 0, h: 0 });
  const [natural, setNatural] = useState({ w: 0, h: 0 });

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setVp({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setVp({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, []);

  const ready = Boolean(url) && natural.w > 0 && natural.h > 0 && vp.w > 0;
  const [ow, oh] = orientedSize(natural.w, natural.h, rotate);
  const placement: ViewPlacement = {
    cx: natural.w / 2,
    cy: natural.h / 2,
    scale: ow > 0 ? vp.w / ow : 1,
    rotate,
    flip,
    vw: vp.w,
    vh: vp.h,
  };
  return (
    <figure className="area-sketch-figure">
      <div
        ref={box}
        className={`area-sketch${disabled ? " disabled" : ""}`}
        style={ow > 0 ? { aspectRatio: `${ow} / ${oh}` } : undefined}
      >
        {url ? (
          <div
            className="area-sketch-stage"
            style={{
              width: natural.w || 1,
              height: natural.h || 1,
              transform: ready ? stageTransform(placement) : undefined,
              visibility: ready ? "visible" : "hidden",
            }}
          >
            <img
              src={url}
              alt=""
              draggable={false}
              onLoad={(e) => setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
            />
          </div>
        ) : (
          <span className="thumb-placeholder">{url === null ? "No negative" : "Loading the negative"}</span>
        )}
        {ready && (
          <CropOverlay
            variant="area"
            placement={placement}
            width={natural.w}
            height={natural.h}
            crop={area}
            active={!disabled}
            onDrag={() => undefined}
            onCommit={commit}
            onCancel={() => undefined}
            label="Analysis area"
          />
        )}
      </div>
      <figcaption>
        {caption}, shown turned and flipped as the roll's photos. Drag on the negative to draw the area over the whole
        film and its clear edge, without the holder; drag inside it to move it.
      </figcaption>
    </figure>
  );
}

function pct(v: number): string {
  return `${Math.round(v * 1000) / 10}%`;
}
