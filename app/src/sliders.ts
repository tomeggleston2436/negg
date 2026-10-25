// SPDX-License-Identifier: Apache-2.0
// The slider table (Ruling 78), in order. Ranges are UI limits (Controller
// choice), not model defaults. Each slider maps to an existing settings field
// (geometry.analysis_buffer is the one new field). `get` reads the effective
// value: the frame's override when set, else the resolved setting. `set`
// returns new overrides with that field set; the frontend always sends whole
// values, never deltas (Ruling 79).

import type { CropRect, FrameOverrides, Geometry, Rgb, Settings } from "./ipc/types";

export interface SliderSpec {
  id: string;
  label: string;
  /** Short help text shown as the control's description. */
  hint: string;
  min: number;
  max: number;
  step: number;
  /** The model default (the slider table's Default column). */
  defaultValue: number;
  get(o: FrameOverrides, s: Settings): number;
  /**
   * New overrides with this field set to `v`. `s` (the resolved settings) is
   * needed by fields that live in a group the override replaces whole (the
   * CMY triple, geometry); without it those fall back to the defaults.
   */
  set(o: FrameOverrides, v: number, s?: Settings): FrameOverrides;
}

function withPrint(o: FrameOverrides, patch: Partial<FrameOverrides["print"]>): FrameOverrides {
  return { ...o, print: { ...o.print, ...patch } };
}

function cmySlider(i: 0 | 1 | 2, id: string, label: string): SliderSpec {
  return {
    id,
    label,
    hint: `Positive adds ${label.toLowerCase()} dye to the print`,
    min: -0.3,
    max: 0.3,
    step: 0.005,
    defaultValue: 0,
    get: (o, s) => (o.print.cmy ?? s.print.cmy)[i],
    set: (o, v, s) => {
      const cmy: Rgb = [...(o.print.cmy ?? s?.print.cmy ?? [0, 0, 0])];
      cmy[i] = v;
      return withPrint(o, { cmy });
    },
  };
}

export const DEFAULT_GEOMETRY: Geometry = {
  crop: null,
  analysis_buffer: 0.05,
  rotate_degrees: 0,
  flip_horizontal: false,
};

/** The geometry the frame currently has: its override, else the resolved one. */
export function effectiveGeometry(o: FrameOverrides, s?: Settings): Geometry {
  return o.geometry ?? s?.geometry ?? DEFAULT_GEOMETRY;
}

/** Overrides with geometry replaced by `patch` applied to the effective geometry. */
export function withGeometry(o: FrameOverrides, patch: Partial<Geometry>, s?: Settings): FrameOverrides {
  return { ...o, geometry: { ...effectiveGeometry(o, s), ...patch } };
}

/** Field-wise equality of two rects; null and absent both mean none. */
export function sameRect(a: CropRect | null | undefined, b: CropRect | null | undefined): boolean {
  if (!a || !b) return !a && !b;
  return a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
}

/** Field-wise equality of two geometries, the analysis area included. */
export function sameGeometry(a: Geometry, b: Geometry): boolean {
  return (
    sameRect(a.crop, b.crop) &&
    sameRect(a.analysis_area, b.analysis_area) &&
    a.analysis_buffer === b.analysis_buffer &&
    a.rotate_degrees === b.rotate_degrees &&
    a.flip_horizontal === b.flip_horizontal
  );
}

/** The analysis area in effect for a geometry (Ruling 134); null when none. */
export function analysisArea(g: Geometry): CropRect | null {
  return g.analysis_area ?? null;
}

/**
 * True when the frame has an analysis area of its own: its geometry override
 * carries an area other than the roll's.
 */
export function hasOwnArea(o: FrameOverrides, roll: Geometry): boolean {
  return o.geometry != null && !sameRect(o.geometry.analysis_area, roll.analysis_area);
}

/**
 * "Use roll analysis area": the frame's own area replaced by the roll's.
 * When its geometry override then equals the roll geometry, the override is
 * removed, so the frame follows later roll changes; otherwise its crop, turn
 * and flip stay its own.
 */
export function withRollArea(o: FrameOverrides, roll: Geometry): FrameOverrides {
  if (!o.geometry) return o;
  const next: Geometry = { ...o.geometry, analysis_area: roll.analysis_area ?? null };
  return { ...o, geometry: sameGeometry(next, roll) ? null : next };
}

export const SLIDERS: readonly SliderSpec[] = [
  {
    id: "density",
    label: "Density",
    hint: "Print exposure in log density; positive is darker",
    min: -1,
    max: 1,
    step: 0.01,
    defaultValue: 0,
    get: (o, s) => o.print.exposure ?? s.print.exposure,
    set: (o, v) => withPrint(o, { exposure: v }),
  },
  {
    id: "grade",
    label: "Grade",
    hint: "Print contrast",
    min: 50,
    max: 180,
    step: 1,
    defaultValue: 115,
    get: (o, s) => o.print.grade ?? s.print.grade,
    set: (o, v) => withPrint(o, { grade: v }),
  },
  cmySlider(0, "cyan", "Cyan"),
  cmySlider(1, "magenta", "Magenta"),
  cmySlider(2, "yellow", "Yellow"),
  {
    id: "toe",
    label: "Toe",
    hint: "Shadow roll-off of the print curve",
    min: 1,
    max: 20,
    step: 0.1,
    defaultValue: 6,
    get: (o, s) => o.print.toe ?? s.print.toe,
    set: (o, v) => withPrint(o, { toe: v }),
  },
  {
    id: "shoulder",
    label: "Shoulder",
    hint: "Highlight roll-off of the print curve",
    min: 0.5,
    max: 10,
    step: 0.1,
    defaultValue: 3,
    get: (o, s) => o.print.shoulder ?? s.print.shoulder,
    set: (o, v) => withPrint(o, { shoulder: v }),
  },
  {
    id: "analysis_buffer",
    label: "Analysis buffer",
    hint: "Fraction of the analysis area's (or crop's) short side left out of the analysis on every edge",
    min: 0,
    max: 0.2,
    step: 0.01,
    defaultValue: 0.05,
    get: (o, s) => effectiveGeometry(o, s).analysis_buffer,
    set: (o, v, s) => withGeometry(o, { analysis_buffer: v }, s),
  },
];

/** Number of decimals a step needs (0.005 gives 3). */
export function stepDecimals(step: number): number {
  const text = String(step);
  const dot = text.indexOf(".");
  return dot < 0 ? 0 : text.length - dot - 1;
}

/** Snaps to the step grid from `min`, clamps to the range, and trims float noise. */
export function clampToSpec(spec: Pick<SliderSpec, "min" | "max" | "step">, v: number): number {
  if (!Number.isFinite(v)) return spec.min;
  const steps = Math.round((v - spec.min) / spec.step);
  const snapped = spec.min + steps * spec.step;
  const clamped = Math.min(spec.max, Math.max(spec.min, snapped));
  return Number(clamped.toFixed(stepDecimals(spec.step)));
}

/** Where the slider thumb sits: the value clamped to the UI range, not rewritten. */
export function sliderPosition(spec: Pick<SliderSpec, "min" | "max">, v: number): number {
  return Math.min(spec.max, Math.max(spec.min, v));
}

/** True when a value (from a sidecar) lies outside the UI range. */
export function outOfRange(spec: Pick<SliderSpec, "min" | "max">, v: number): boolean {
  return v < spec.min || v > spec.max;
}

/** Text for the numeric field. */
export function formatValue(spec: Pick<SliderSpec, "step">, v: number): string {
  return v.toFixed(stepDecimals(spec.step));
}

/** Parses typed text; null when it is not a number. */
export function parseValue(text: string): number | null {
  const t = text.trim().replace(/−/g, "-");
  if (t === "") return null;
  const v = Number(t);
  return Number.isFinite(v) ? v : null;
}

/** Next quarter turn clockwise (+1) or anticlockwise (-1). */
export function turn(rotate: Geometry["rotate_degrees"], dir: 1 | -1): Geometry["rotate_degrees"] {
  return (((rotate + dir * 90) % 360) + 360) % 360 as Geometry["rotate_degrees"];
}

/**
 * What typed entry commits when the field is left or Enter is pressed: null
 * when the text is unchanged from what the field showed at focus time (so
 * tabbing through an out-of-range or off-step value from a sidecar never
 * rewrites it, Ruling 78), when it is not a number, or when the clamped value
 * equals the current one; otherwise the value snapped and clamped to the spec.
 */
export function enteredValue(
  spec: Pick<SliderSpec, "min" | "max" | "step">,
  text: string,
  focusText: string,
  value: number,
): number | null {
  if (text.trim() === focusText.trim()) return null;
  const parsed = parseValue(text);
  if (parsed === null) return null;
  const v = clampToSpec(spec, parsed);
  return v === value ? null : v;
}
