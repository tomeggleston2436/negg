// SPDX-License-Identifier: Apache-2.0
// TypeScript mirrors of the serde shapes the Tauri commands send and accept
// (snake_case keys, as in the sidecars), plus runtime decoders so a response
// that drifts from the contract fails loudly at the IPC boundary instead of
// deep inside a component. Decoders accept extra keys (additive changes on
// the Rust side do not break the app) but reject missing or mistyped ones.

import { WireError } from "./tile";

export type Rgb = [number, number, number];

export type ChannelEstimate = { mode: "auto" } | { mode: "fixed"; value: Rgb };

export type CastEstimate =
  | { mode: "auto" }
  | { mode: "fixed"; slope_r: number; offset_r: number; slope_b: number; offset_b: number };

export type OutputSpace = "srgb" | "adobe_rgb" | "display_p3" | "pro_photo" | "rec2020";

export const OUTPUT_SPACES: readonly OutputSpace[] = [
  "srgb",
  "adobe_rgb",
  "display_p3",
  "pro_photo",
  "rec2020",
];

export interface CropRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Geometry {
  crop: CropRect | null;
  /**
   * The analysis area (Ruling 134): the part of the source the estimates are
   * made from, separate from the output crop, which it never changes. Omitted
   * on the wire when there is none (the crop, else the whole frame, is
   * analysed); null and absent both mean none.
   */
  analysis_area?: CropRect | null;
  analysis_buffer: number;
  rotate_degrees: 0 | 90 | 180 | 270;
  flip_horizontal: boolean;
}

export interface FilmSettings {
  film_base: ChannelEstimate;
  dmax: ChannelEstimate;
  gamma: ChannelEstimate;
}

export interface PrintSettings {
  grade: number;
  exposure: number;
  cmy: Rgb;
  paper_dmin: number;
  paper_dmax: number;
  shoulder: number;
  toe: number;
  black_point_compensation: boolean;
}

export interface ColourSettings {
  auto_colour: boolean;
  cast: CastEstimate;
  output_space: OutputSpace;
}

export interface ProfileRefs {
  rig: string | null;
  film: string | null;
}

export interface Settings {
  schema_version: number;
  film: FilmSettings;
  print: PrintSettings;
  colour: ColourSettings;
  profiles: ProfileRefs;
  geometry: Geometry;
}

export interface RollSettings {
  schema_version: number;
  shared: Settings;
  pool_film_base: boolean;
  pool_gamma: boolean;
  pool_colour: boolean;
}

export interface FilmOverrides {
  film_base: ChannelEstimate | null;
  dmax: ChannelEstimate | null;
  gamma: ChannelEstimate | null;
}

export interface PrintOverrides {
  grade: number | null;
  exposure: number | null;
  cmy: Rgb | null;
  paper_dmin: number | null;
  paper_dmax: number | null;
  shoulder: number | null;
  toe: number | null;
  black_point_compensation: boolean | null;
}

export interface ColourOverrides {
  auto_colour: boolean | null;
  cast: CastEstimate | null;
  output_space: OutputSpace | null;
}

/** `geometry` is omitted on the wire when the frame has no geometry override. */
export interface FrameOverrides {
  film: FilmOverrides;
  print: PrintOverrides;
  colour: ColourOverrides;
  geometry?: Geometry | null;
}

export type DminSource = "border" | "percentile" | "fixed" | "roll" | "profile" | "roll_min";
export type GammaSource = "range" | "fixed" | "neutral_fit" | "roll" | "profile";
export type CastSource = "fit" | "fixed" | "roll" | "identity";
export type PoolRole = "inlier" | "rejected" | "excluded" | "unused";

export interface CastCorrection {
  slope_r: number;
  offset_r: number;
  slope_b: number;
  offset_b: number;
  cells_used: number;
}

export interface Estimates {
  dmin: Rgb;
  dmin_source: DminSource;
  dmax: Rgb;
  gamma: Rgb;
  gamma_source: GammaSource;
  neutral_offset: Rgb;
  anchor: number;
  x0: number;
  cast: CastCorrection;
  cast_source: CastSource;
}

export interface PoolFrame {
  source: string;
  dmin: Rgb;
  dmin_source: DminSource;
  dmax: Rgb;
  gamma: Rgb;
  cast: CastCorrection;
  dmin_role: PoolRole;
  gamma_role: PoolRole;
  cast_role: PoolRole;
  /** Dmax pinned at the density cap (Ruling 133); omitted when false. */
  needs_analysis_area?: boolean;
}

export interface HistoryView {
  can_undo: boolean;
  can_redo: boolean;
  len: number;
  cursor: number;
  /**
   * A re-pool is pending (Rulings 124 and 126): the backend makes it in the
   * background and announces it with `negg://analysis`; until then estimates
   * are on the previous pool. Absent in older JSON, read as false.
   */
  pool_pending?: boolean;
}

export type FrameStatus = "pending" | "ready" | "failed" | "read_only";

export interface FrameInfo {
  index: number;
  source: string;
  status: FrameStatus;
  message: string | null;
  /**
   * The frame's Dmax is pinned at the density cap (Ruling 133): the film
   * holder is in its analysed area, so it needs an analysis area. From its
   * last completed analysis. Absent in older JSON, read as false.
   */
  needs_analysis_area?: boolean;
}

export interface RollView {
  dir: string;
  frames: FrameInfo[];
  roll: RollSettings;
  analysed: boolean;
  generation: number;
  history: HistoryView;
  /**
   * A re-pool is pending (Rulings 124 and 126): the backend makes it in the
   * background and announces it with `negg://analysis`; until then estimates
   * are on the previous pool. Absent in older JSON, read as false.
   */
  pool_pending?: boolean;
  /**
   * The roll was opened for the first time (no roll.negg.json) and the user
   * has not pressed Analyse yet (Ruling 136): the negatives are shown and the
   * analysis area can be set first. Sent by the app's commands only.
   */
  awaiting_analysis?: boolean;
  /**
   * Set when the last analysis area or crop change took the roll's film
   * base pool off the clear film edge (Ruling 138): the text to show. Absent
   * when unset.
   */
  pool_warning?: string | null;
}

/** A frame's negative before analysis (Ruling 134): its size and pyramid. */
export interface NegativeView {
  index: number;
  source: string;
  /** Source orientation, uncropped. */
  width: number;
  height: number;
  levels: number;
  tile_size: number;
}

export interface FrameView {
  index: number;
  source: string;
  /** Source orientation, before any quarter turn or flip. */
  width: number;
  height: number;
  /** L + 1 pyramid levels. */
  levels: number;
  tile_size: number;
  overrides: FrameOverrides;
  /** Resolved settings (roll shared settings plus this frame's overrides). */
  settings: Settings;
  estimates: Estimates | null;
  pool: PoolFrame | null;
  generation: number;
  read_only: boolean;
  history: HistoryView;
  /**
   * A re-pool is pending (Rulings 124 and 126): the backend makes it in the
   * background and announces it with `negg://analysis`; until then estimates
   * are on the previous pool. Absent in older JSON, read as false.
   */
  pool_pending?: boolean;
  /** As FrameInfo.needs_analysis_area. */
  needs_analysis_area?: boolean;
}

export type ExportScope =
  | { kind: "frame"; frame: number }
  | { kind: "frames"; frames: number[] }
  | { kind: "roll" };

export type ExportFormat = "tiff16" | "jpeg";

export interface ExportRequest {
  scope: ExportScope;
  format: ExportFormat;
  /** null = beside the sources, as the CLI does. */
  out_dir: string | null;
}

export interface ExportReport {
  converted: number;
  failed: number;
  outputs: string[];
  errors: string[];
}

export interface CommandError {
  kind: string;
  message: string;
}

/** Payload of `negg://analysis`. */
export interface AnalysisEvent {
  done: number;
  total: number;
  frame: number | null;
  finished: boolean;
}

/** Payload of `negg://thumbs` (these frames' thumbnails are stale). */
export interface ThumbsEvent {
  frames: number[];
  generation: number;
}

/** Payload of `negg://export`. */
export interface ExportEvent {
  done: number;
  total: number;
  input: string;
  ok: boolean;
  message: string | null;
}

/** Name of the backend's error event (see `BackendErrorEvent`). */
export const ERROR_EVENT = "negg://error";

/**
 * Payload of `negg://error`: an error no command returned (a debounced
 * sidecar flush that refused to overwrite a changed file, a pool that could
 * not be made). The app shows it and fetches its views again.
 */
export interface BackendErrorEvent {
  message: string;
}

/** Empty overrides: every field inherits the roll. */
export function emptyOverrides(): FrameOverrides {
  return {
    film: { film_base: null, dmax: null, gamma: null },
    print: {
      grade: null,
      exposure: null,
      cmy: null,
      paper_dmin: null,
      paper_dmax: null,
      shoulder: null,
      toe: null,
      black_point_compensation: null,
    },
    colour: { auto_colour: null, cast: null, output_space: null },
  };
}

/**
 * The TypeScript copy of `negg_types::resolve`: field copies only (Some
 * replaces), used by the roll panel to turn a slider edit into new shared
 * settings. No value is computed.
 */
export function resolve(shared: Settings, o: FrameOverrides): Settings {
  const s: Settings = structuredClone(shared);
  if (o.film.film_base !== null) s.film.film_base = o.film.film_base;
  if (o.film.dmax !== null) s.film.dmax = o.film.dmax;
  if (o.film.gamma !== null) s.film.gamma = o.film.gamma;
  const p = o.print;
  if (p.grade !== null) s.print.grade = p.grade;
  if (p.exposure !== null) s.print.exposure = p.exposure;
  if (p.cmy !== null) s.print.cmy = [...p.cmy];
  if (p.paper_dmin !== null) s.print.paper_dmin = p.paper_dmin;
  if (p.paper_dmax !== null) s.print.paper_dmax = p.paper_dmax;
  if (p.shoulder !== null) s.print.shoulder = p.shoulder;
  if (p.toe !== null) s.print.toe = p.toe;
  if (p.black_point_compensation !== null) {
    s.print.black_point_compensation = p.black_point_compensation;
  }
  const c = o.colour;
  if (c.auto_colour !== null) s.colour.auto_colour = c.auto_colour;
  if (c.cast !== null) s.colour.cast = c.cast;
  if (c.output_space !== null) s.colour.output_space = c.output_space;
  if (o.geometry) s.geometry = { ...o.geometry };
  return s;
}

// ---------------------------------------------------------------------------
// Runtime decoders

type Check = (v: unknown, path: string) => void;

function fail(path: string, want: string, v: unknown): never {
  const got = v === null ? "null" : Array.isArray(v) ? "array" : typeof v;
  throw new WireError(`${path}: expected ${want}, got ${got}`);
}

const num: Check = (v, p) => {
  if (typeof v !== "number" || !Number.isFinite(v)) fail(p, "a finite number", v);
};
const u32: Check = (v, p) => {
  if (typeof v !== "number" || !Number.isInteger(v) || v < 0 || v > 0xffffffff) {
    fail(p, "a u32", v);
  }
};
const bool: Check = (v, p) => {
  if (typeof v !== "boolean") fail(p, "a boolean", v);
};
const str: Check = (v, p) => {
  if (typeof v !== "string") fail(p, "a string", v);
};
const nullable =
  (c: Check): Check =>
  (v, p) => {
    if (v !== null && v !== undefined) c(v, p);
  };
const oneOf =
  (...values: readonly (string | number)[]): Check =>
  (v, p) => {
    if (!values.includes(v as string | number)) fail(p, `one of ${values.join(", ")}`, v);
  };
const arr =
  (c: Check, len?: number): Check =>
  (v, p) => {
    if (!Array.isArray(v)) fail(p, "an array", v);
    if (len !== undefined && v.length !== len) fail(p, `an array of ${len}`, v);
    v.forEach((x, i) => c(x, `${p}[${i}]`));
  };
const obj =
  (fields: Record<string, Check>): Check =>
  (v, p) => {
    if (typeof v !== "object" || v === null || Array.isArray(v)) fail(p, "an object", v);
    const o = v as Record<string, unknown>;
    for (const [k, c] of Object.entries(fields)) c(o[k], `${p}.${k}`);
  };
const tagged =
  (tag: string, variants: Record<string, Record<string, Check>>): Check =>
  (v, p) => {
    if (typeof v !== "object" || v === null) fail(p, "an object", v);
    const t = (v as Record<string, unknown>)[tag];
    const fields = typeof t === "string" ? variants[t] : undefined;
    if (!fields) fail(`${p}.${tag}`, `one of ${Object.keys(variants).join(", ")}`, t);
    obj(fields)(v, p);
  };

const rgb = arr(num, 3);
const channelEstimate = tagged("mode", { auto: {}, fixed: { value: rgb } });
const castEstimate = tagged("mode", {
  auto: {},
  fixed: { slope_r: num, offset_r: num, slope_b: num, offset_b: num },
});
const outputSpace = oneOf(...OUTPUT_SPACES);
const cropRect = obj({ x: num, y: num, width: num, height: num });
const geometry = obj({
  crop: nullable(cropRect),
  analysis_area: nullable(cropRect),
  analysis_buffer: num,
  rotate_degrees: oneOf(0, 90, 180, 270),
  flip_horizontal: bool,
});
const settings = obj({
  schema_version: u32,
  film: obj({ film_base: channelEstimate, dmax: channelEstimate, gamma: channelEstimate }),
  print: obj({
    grade: num,
    exposure: num,
    cmy: rgb,
    paper_dmin: num,
    paper_dmax: num,
    shoulder: num,
    toe: num,
    black_point_compensation: bool,
  }),
  colour: obj({ auto_colour: bool, cast: castEstimate, output_space: outputSpace }),
  profiles: obj({ rig: nullable(str), film: nullable(str) }),
  geometry,
});
const rollSettings = obj({
  schema_version: u32,
  shared: settings,
  pool_film_base: bool,
  pool_gamma: bool,
  pool_colour: bool,
});
const frameOverrides = obj({
  film: obj({
    film_base: nullable(channelEstimate),
    dmax: nullable(channelEstimate),
    gamma: nullable(channelEstimate),
  }),
  print: obj({
    grade: nullable(num),
    exposure: nullable(num),
    cmy: nullable(rgb),
    paper_dmin: nullable(num),
    paper_dmax: nullable(num),
    shoulder: nullable(num),
    toe: nullable(num),
    black_point_compensation: nullable(bool),
  }),
  colour: obj({
    auto_colour: nullable(bool),
    cast: nullable(castEstimate),
    output_space: nullable(outputSpace),
  }),
  geometry: nullable(geometry),
});
const dminSource = oneOf("border", "percentile", "fixed", "roll", "profile", "roll_min");
const castCorrection = obj({
  slope_r: num,
  offset_r: num,
  slope_b: num,
  offset_b: num,
  cells_used: u32,
});
const estimates = obj({
  dmin: rgb,
  dmin_source: dminSource,
  dmax: rgb,
  gamma: rgb,
  gamma_source: oneOf("range", "fixed", "neutral_fit", "roll", "profile"),
  neutral_offset: rgb,
  anchor: num,
  x0: num,
  cast: castCorrection,
  cast_source: oneOf("fit", "fixed", "roll", "identity"),
});
const poolRole = oneOf("inlier", "rejected", "excluded", "unused");
const poolFrame = obj({
  source: str,
  dmin: rgb,
  dmin_source: dminSource,
  dmax: rgb,
  gamma: rgb,
  cast: castCorrection,
  dmin_role: poolRole,
  gamma_role: poolRole,
  cast_role: poolRole,
  needs_analysis_area: nullable(bool),
});
const historyView = obj({
  can_undo: bool,
  can_redo: bool,
  len: u32,
  cursor: u32,
  pool_pending: nullable(bool),
});
const frameInfo = obj({
  index: u32,
  source: str,
  status: oneOf("pending", "ready", "failed", "read_only"),
  message: nullable(str),
  needs_analysis_area: nullable(bool),
});
const rollView = obj({
  dir: str,
  frames: arr(frameInfo),
  roll: rollSettings,
  analysed: bool,
  generation: u32,
  history: historyView,
  pool_pending: nullable(bool),
  awaiting_analysis: nullable(bool),
  pool_warning: nullable(str),
});
const negativeView = obj({
  index: u32,
  source: str,
  width: u32,
  height: u32,
  levels: u32,
  tile_size: u32,
});
const frameView = obj({
  index: u32,
  source: str,
  width: u32,
  height: u32,
  levels: u32,
  tile_size: u32,
  overrides: frameOverrides,
  settings,
  estimates: nullable(estimates),
  pool: nullable(poolFrame),
  generation: u32,
  read_only: bool,
  history: historyView,
  pool_pending: nullable(bool),
  needs_analysis_area: nullable(bool),
});
const exportRequest = obj({
  scope: tagged("kind", { frame: { frame: u32 }, frames: { frames: arr(u32) }, roll: {} }),
  format: oneOf("tiff16", "jpeg"),
  out_dir: nullable(str),
});
const exportReport = obj({
  converted: u32,
  failed: u32,
  outputs: arr(str),
  errors: arr(str),
});

function decoder<T>(name: string, check: Check): (v: unknown) => T {
  return (v) => {
    check(v, name);
    return v as T;
  };
}

export const decodeSettings = decoder<Settings>("Settings", settings);
export const decodeRollSettings = decoder<RollSettings>("RollSettings", rollSettings);
export const decodeFrameOverrides = decoder<FrameOverrides>("FrameOverrides", frameOverrides);
export const decodeHistoryView = decoder<HistoryView>("HistoryView", historyView);
export const decodeRollView = decoder<RollView>("RollView", rollView);
export const decodeFrameView = decoder<FrameView>("FrameView", frameView);
export const decodeNegativeView = decoder<NegativeView>("NegativeView", negativeView);
export const decodeExportRequest = decoder<ExportRequest>("ExportRequest", exportRequest);
export const decodeExportReport = decoder<ExportReport>("ExportReport", exportReport);
export const decodeGeneration = decoder<number>("generation", u32);
export const decodeAnalysisEvent = decoder<AnalysisEvent>(
  "AnalysisEvent",
  obj({ done: u32, total: u32, frame: nullable(u32), finished: bool }),
);
export const decodeThumbsEvent = decoder<ThumbsEvent>(
  "ThumbsEvent",
  obj({ frames: arr(u32), generation: u32 }),
);
export const decodeExportEvent = decoder<ExportEvent>(
  "ExportEvent",
  obj({ done: u32, total: u32, input: str, ok: bool, message: nullable(str) }),
);
export const decodeBackendErrorEvent = decoder<BackendErrorEvent>("BackendErrorEvent", obj({ message: str }));
