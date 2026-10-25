// SPDX-License-Identifier: Apache-2.0
// The frame screen's controls: the slider table, black point compensation,
// quarter turns, flip, crop, lock and reset. Every control edits this frame's
// FrameOverrides (Ruling 79); toggles and buttons commit at once.

import { useState } from "react";
import type { FrameOverrides, Settings } from "../ipc/types";
import { SLIDERS, effectiveGeometry, turn, withGeometry } from "../sliders";
import { Slider } from "./Slider";
import { CropFields } from "./CropFields";

export interface SliderPanelProps {
  overrides: FrameOverrides;
  settings: Settings;
  disabled: boolean;
  cropMode: boolean;
  onCropMode: (on: boolean) => void;
  onDrag: (o: FrameOverrides) => void;
  onCommit: (o: FrameOverrides) => void;
  onLock: (dmin: boolean, gamma: boolean, cast: boolean) => void;
  /** Makes this frame's crop the roll's crop. */
  onCropToRoll: () => void;
}

export function SliderPanel(props: SliderPanelProps) {
  const { overrides: o, settings: s, disabled } = props;
  const geometry = effectiveGeometry(o, s);
  const bpc = o.print.black_point_compensation ?? s.print.black_point_compensation;
  const [lockDmin, setLockDmin] = useState(false);
  const [lockGamma, setLockGamma] = useState(false);
  const [lockCast, setLockCast] = useState(false);

  const resetPrint = () => {
    const print = Object.fromEntries(Object.keys(o.print).map((k) => [k, null])) as unknown as FrameOverrides["print"];
    props.onCommit({ ...o, print });
  };

  return (
    <div className="panel-body">
      <section aria-labelledby="print-heading">
        <h2 id="print-heading">Print</h2>
        {SLIDERS.filter((sp) => sp.id !== "analysis_buffer").map((spec) => (
          <Slider
            key={spec.id}
            spec={spec}
            value={spec.get(o, s)}
            disabled={disabled}
            onDrag={(v) => props.onDrag(spec.set(o, v, s))}
            onCommit={(v) => props.onCommit(spec.set(o, v, s))}
          />
        ))}
        <label className="check">
          <input
            type="checkbox"
            checked={bpc}
            disabled={disabled}
            onChange={(e) =>
              props.onCommit({ ...o, print: { ...o.print, black_point_compensation: e.currentTarget.checked } })
            }
          />
          Black point compensation
        </label>
        <button type="button" disabled={disabled} onClick={resetPrint}>
          Reset print to roll
        </button>
      </section>

      <section aria-labelledby="geometry-heading">
        <h2 id="geometry-heading">Geometry</h2>
        <div className="button-row">
          <button
            type="button"
            disabled={disabled}
            onClick={() => props.onCommit(withGeometry(o, { rotate_degrees: turn(geometry.rotate_degrees, -1) }, s))}
          >
            Rotate left
          </button>
          <button
            type="button"
            disabled={disabled}
            onClick={() => props.onCommit(withGeometry(o, { rotate_degrees: turn(geometry.rotate_degrees, 1) }, s))}
          >
            Rotate right
          </button>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={geometry.flip_horizontal}
            disabled={disabled}
            onChange={(e) => props.onCommit(withGeometry(o, { flip_horizontal: e.currentTarget.checked }, s))}
          />
          Flip horizontally
        </label>
        <div className="button-row">
          <button
            type="button"
            aria-pressed={props.cropMode}
            disabled={disabled}
            onClick={() => props.onCropMode(!props.cropMode)}
            title="Drag on the picture to draw, move or resize the crop"
          >
            {props.cropMode ? "Finish crop" : "Crop"}
          </button>
          <button
            type="button"
            disabled={disabled || geometry.crop === null}
            onClick={() => props.onCommit(withGeometry(o, { crop: null }, s))}
            title="No crop for this frame"
          >
            Clear crop
          </button>
        </div>
        <CropFields
          crop={geometry.crop}
          disabled={disabled}
          label="Frame"
          onCommit={(crop) => props.onCommit(withGeometry(o, { crop }, s))}
        />
        <p className="note">
          {geometry.crop
            ? `Crop ${pct(geometry.crop.width)} by ${pct(geometry.crop.height)} of the frame`
            : "No crop: the whole frame"}
          {o.geometry ? " (this frame's own geometry)" : " (from the roll)"}
        </p>
        <div className="button-row wrap">
          <button
            type="button"
            disabled={disabled || !o.geometry}
            onClick={() => props.onCommit({ ...o, geometry: null })}
            title="Remove this frame's geometry so it follows the roll's crop, turn and flip"
          >
            Use roll geometry
          </button>
          <button
            type="button"
            disabled={disabled}
            onClick={props.onCropToRoll}
            title="Make this frame's crop and analysis buffer the roll's"
          >
            Use this frame's crop for the roll
          </button>
        </div>
        {SLIDERS.filter((sp) => sp.id === "analysis_buffer").map((spec) => (
          <Slider
            key={spec.id}
            spec={spec}
            value={spec.get(o, s)}
            disabled={disabled || (geometry.analysis_area ?? geometry.crop) === null}
            onCommit={(v) => props.onCommit(spec.set(o, v, s))}
          />
        ))}
      </section>

      <section aria-labelledby="lock-heading">
        <h2 id="lock-heading">Lock estimates</h2>
        <p className="note">Fixes this frame's estimates as overrides. With nothing ticked, all three are locked.</p>
        <label className="check">
          <input type="checkbox" checked={lockDmin} disabled={disabled} onChange={(e) => setLockDmin(e.currentTarget.checked)} />
          Film base
        </label>
        <label className="check">
          <input type="checkbox" checked={lockGamma} disabled={disabled} onChange={(e) => setLockGamma(e.currentTarget.checked)} />
          Gamma
        </label>
        <label className="check">
          <input type="checkbox" checked={lockCast} disabled={disabled} onChange={(e) => setLockCast(e.currentTarget.checked)} />
          Colour cast
        </label>
        <button type="button" disabled={disabled} onClick={() => props.onLock(lockDmin, lockGamma, lockCast)}>
          Lock
        </button>
      </section>
    </div>
  );
}

function pct(v: number): string {
  return `${Math.round(v * 1000) / 10}%`;
}
