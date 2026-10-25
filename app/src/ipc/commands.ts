// SPDX-License-Identifier: Apache-2.0
// One wrapper per Tauri command ("### Tauri backend (P3-T)"), same names and
// argument keys as the Rust functions. JSON results go through the decoders in
// types.ts; binary results (tiles, thumbnails, scopes) come back as an
// ArrayBuffer. Tests swap the transport with `setInvoke`.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import {
  type CommandError,
  type ExportReport,
  type ExportRequest,
  type FrameOverrides,
  type FrameView,
  type CropRect,
  type HistoryView,
  type NegativeView,
  type RollSettings,
  type RollView,
  decodeExportReport,
  decodeFrameView,
  decodeGeneration,
  decodeHistoryView,
  decodeNegativeView,
  decodeRollView,
} from "./types";
import { WireError } from "./tile";

export type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

let transport: InvokeFn = (cmd, args) => tauriInvoke(cmd, args);

/** Replaces the transport (tests, or a mock backend); returns the previous one. */
export function setInvoke(fn: InvokeFn): InvokeFn {
  const previous = transport;
  transport = fn;
  return previous;
}

/** A command that the backend rejected (its `CommandError`), or a transport failure. */
export class CommandFailure extends Error {
  readonly kind: string;
  readonly command: string;
  constructor(command: string, kind: string, message: string) {
    super(message);
    this.name = "CommandFailure";
    this.kind = kind;
    this.command = command;
  }
}

function isCommandError(e: unknown): e is CommandError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as CommandError).kind === "string" &&
    typeof (e as CommandError).message === "string"
  );
}

async function call(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  try {
    return await transport(cmd, args);
  } catch (e) {
    if (isCommandError(e)) throw new CommandFailure(cmd, e.kind, e.message);
    if (e instanceof Error) throw new CommandFailure(cmd, "transport", e.message);
    throw new CommandFailure(cmd, "transport", String(e));
  }
}

/** Normalises a binary command result to an ArrayBuffer. */
export function toArrayBuffer(cmd: string, v: unknown): ArrayBuffer {
  if (v instanceof ArrayBuffer) return v;
  if (ArrayBuffer.isView(v)) {
    const copy = new Uint8Array(v.byteLength);
    copy.set(new Uint8Array(v.buffer, v.byteOffset, v.byteLength));
    return copy.buffer;
  }
  if (Array.isArray(v) && v.every((b) => typeof b === "number")) {
    return Uint8Array.from(v as number[]).buffer;
  }
  throw new WireError(`${cmd}: expected binary data`);
}

async function binary(cmd: string, args: Record<string, unknown>): Promise<ArrayBuffer> {
  return toArrayBuffer(cmd, await call(cmd, args));
}

export async function openRoll(dir: string): Promise<RollView> {
  return decodeRollView(await call("open_roll", { dir }));
}

/** Starts the analysis of a roll that awaits it (the Analyse button, Ruling 136). */
export async function startAnalysis(): Promise<RollView> {
  return decodeRollView(await call("start_analysis"));
}

/** Opens a frame for its negative (before analysis too). */
export async function negativeView(frame: number): Promise<NegativeView> {
  return decodeNegativeView(await call("negative_view", { frame }));
}

/**
 * One tile of the frame's negative, in the tile wire format. Its header is
 * the same as a rendered tile's, so the two are never cached together.
 */
export function negativeTile(frame: number, level: number, tileX: number, tileY: number): Promise<ArrayBuffer> {
  return binary("negative_tile", { frame, level, tile_x: tileX, tile_y: tileY });
}

/** The frame's negative as a JPEG thumbnail (before analysis too). */
export function negativeThumbnail(frame: number): Promise<ArrayBuffer> {
  return binary("negative_thumbnail", { frame });
}

/** Sets (or with null removes) the roll's analysis area, before or after analysis. */
export async function setRollAnalysisArea(area: CropRect | null): Promise<RollView> {
  return decodeRollView(await call("set_roll_analysis_area", { area }));
}

export async function rollView(): Promise<RollView> {
  return decodeRollView(await call("roll_view"));
}

export async function frameView(frame: number): Promise<FrameView> {
  return decodeFrameView(await call("frame_view", { frame }));
}

/**
 * Argument keys are the Rust names (`tile_x`, `tile_y`): the backend
 * declares its commands with `rename_all = "snake_case"`, per the contract's
 * "snake_case args".
 */
export function renderTile(frame: number, level: number, tileX: number, tileY: number): Promise<ArrayBuffer> {
  return binary("render_tile", { frame, level, tile_x: tileX, tile_y: tileY });
}

export function thumbnail(frame: number): Promise<ArrayBuffer> {
  return binary("thumbnail", { frame });
}

export function scopes(frame: number): Promise<ArrayBuffer> {
  return binary("scopes", { frame });
}

export async function previewFrame(frame: number, overrides: FrameOverrides): Promise<number> {
  return decodeGeneration(await call("preview_frame", { frame, overrides }));
}

export async function commitFrame(frame: number, overrides: FrameOverrides): Promise<FrameView> {
  return decodeFrameView(await call("commit_frame", { frame, overrides }));
}

export async function commitRoll(roll: RollSettings): Promise<RollView> {
  return decodeRollView(await call("commit_roll", { roll }));
}

/** All three false locks all three (as `negg lock` with no flags). */
export async function lockFrame(frame: number, dmin: boolean, gamma: boolean, cast: boolean): Promise<FrameView> {
  return decodeFrameView(await call("lock_frame", { frame, dmin, gamma, cast }));
}

export async function undo(): Promise<HistoryView> {
  return decodeHistoryView(await call("undo"));
}

export async function redo(): Promise<HistoryView> {
  return decodeHistoryView(await call("redo"));
}

export async function flush(): Promise<void> {
  await call("flush");
}

/**
 * Answers `negg://closing`: every edit the webview had pending has been
 * sent, so the backend may flush and close.
 */
export async function closeReady(): Promise<void> {
  await call("close_ready");
}

export async function exportFrames(request: ExportRequest): Promise<ExportReport> {
  return decodeExportReport(await call("export", { request }));
}
