// SPDX-License-Identifier: Apache-2.0
// Pure helpers for the tone presets in the app: which preset a look shows
// (with "(modified)" once a slider has moved), cycling, the P and
// Shift+P keys, the roll preset question, the export dialog's
// Original and Web/Instagram choices, and the cache of preset previews.

import type { FrameView, Preset, PrintLook, RollPresetScope, Settings, WebOutput } from "./ipc/types";
import { presetLabel } from "./ipc/types";

export interface PresetState {
  name: string;
  /** A look value differs from the preset's (or the preset is not known). */
  modified: boolean;
}

/** The five look values of resolved settings; saturation 1 when absent. */
export function lookOf(s: Settings): PrintLook {
  return {
    grade: s.print.grade,
    toe: s.print.toe,
    shoulder: s.print.shoulder,
    black_point_compensation: s.print.black_point_compensation,
    saturation: s.print.saturation ?? 1,
  };
}

function sameLook(a: PrintLook, b: PrintLook): boolean {
  return (
    a.grade === b.grade &&
    a.toe === b.toe &&
    a.shoulder === b.shoulder &&
    a.black_point_compensation === b.black_point_compensation &&
    a.saturation === b.saturation
  );
}

/**
 * The preset label of `settings` and whether the look has moved from it. An
 * absent, null or "" label is no preset; a label no longer in
 * `presets` is shown as modified.
 */
export function presetState(settings: Settings, presets: readonly Preset[]): PresetState | null {
  const name = presetLabel(settings.print);
  if (name === null) return null;
  const preset = presets.find((p) => p.name === name);
  return { name, modified: !preset || !sameLook(lookOf(settings), preset.look) };
}

/** The heading text: "Rich", "Rich (modified)" or "No preset". */
export function presetHeading(st: PresetState | null): string {
  if (st === null) return "No preset";
  return st.modified ? `${st.name} (modified)` : st.name;
}

/** What "Save as preset" says about a typed name, and whether Save is allowed. */
export interface SaveNameCheck {
  note: string | null;
  /** True when the name must not be saved (empty, or a built-in's). */
  blocked: boolean;
}

/**
 * Checks a typed preset name against the list, whose first `builtins` entries
 * are the built-ins (case-insensitive, as the backend compares names). A
 * built-in name cannot be saved; a user preset's name is replaced.
 */
export function saveNameCheck(name: string, presets: readonly Preset[], builtins: number): SaveNameCheck {
  const trimmed = name.trim();
  if (trimmed === "") return { note: null, blocked: true };
  const i = presets.findIndex((p) => p.name.toLowerCase() === trimmed.toLowerCase());
  if (i < 0) return { note: null, blocked: false };
  if (i < builtins) {
    return { note: `${presets[i]?.name ?? trimmed} is a built-in preset; choose another name`, blocked: true };
  }
  return { note: "Saving replaces your preset of this name.", blocked: false };
}

/**
 * The preset `step` away from `current`, wrapping at either end. From no
 * preset, or one not in the list, it starts at the first. `presets` must not
 * be empty.
 */
export function nextPreset(presets: readonly Preset[], current: string | null, step: 1 | -1): Preset {
  const first = presets[0];
  if (!first) throw new Error("nextPreset: no presets");
  const i = current === null ? -1 : presets.findIndex((p) => p.name === current);
  if (i < 0) return first;
  const n = presets.length;
  return presets[(((i + step) % n) + n) % n] ?? first;
}

export interface KeyFacts {
  key: string;
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
}

/**
 * The preset cycle keys: P for the next preset, Shift+P for the
 * previous one. Null while typing in a text field, with Cmd, Ctrl or Alt
 * held, and for every other key (`[` and `]` move between frames).
 */
export function presetCycleStep(e: KeyFacts, textEntry: boolean): 1 | -1 | null {
  if (textEntry || e.metaKey || e.ctrlKey || e.altKey) return null;
  if (e.key.toLowerCase() !== "p") return null;
  return e.shiftKey ? -1 : 1;
}

/** The roll preset question's answers, in button order. */
export const ROLL_PRESET_CHOICES: ReadonlyArray<{ label: string; scope: RollPresetScope }> = [
  { label: "All frames", scope: "all_frames" },
  { label: "Only frames without their own look", scope: "frames_without_own_look" },
];

/** "3 frames have their own look. Apply Rich to:" */
export function rollPresetQuestion(count: number, name: string): string {
  const own = count === 1 ? "1 frame has its own look" : `${count} frames have their own look`;
  return `${own}. Apply ${name} to:`;
}

export type RollPresetOutcome = { kind: "applied" } | { kind: "ask"; frames: number[] } | { kind: "failed" };

export interface RollPresetDeps {
  /** The frames with a look of their own; null when they could not be listed (already reported). */
  framesWithOwnLook: () => Promise<number[] | null>;
  apply: (name: string, scope: RollPresetScope) => Promise<boolean>;
}

/**
 * Choosing a roll preset: with no frame that has its own look it is applied
 * at once (no question); otherwise the caller asks which frames it is for.
 */
export async function chooseRollPreset(name: string, deps: RollPresetDeps): Promise<RollPresetOutcome> {
  const own = await deps.framesWithOwnLook();
  if (own === null) return { kind: "failed" };
  if (own.length > 0) return { kind: "ask", frames: own };
  return (await deps.apply(name, "frames_without_own_look")) ? { kind: "applied" } : { kind: "failed" };
}

/**
 * The export request's `web` from the dialog's two boxes: Original only is
 * "none", both "also", Web/Instagram only "only"; null with neither ticked
 * (nothing to export).
 */
export function webOutput(original: boolean, web: boolean): WebOutput | null {
  if (original) return web ? "also" : "none";
  return web ? "only" : null;
}

/**
 * What a preset preview of this frame depends on: the frame's settings apart
 * from its own look and label (a preset replaces those), its estimates, and
 * the preset's look. A slider move of the look alone keeps every preview.
 */
export function previewVersion(view: FrameView, look: PrintLook): string {
  const print: Partial<Settings["print"]> = { ...view.settings.print };
  for (const k of ["grade", "toe", "shoulder", "black_point_compensation", "saturation", "preset"] as const) {
    delete print[k];
  }
  return JSON.stringify([{ ...view.settings, print }, view.estimates ?? null, look]);
}

export interface PresetPreviewDeps {
  fetch: (frame: number, name: string) => Promise<ArrayBuffer>;
  makeUrl: (bytes: ArrayBuffer) => string;
  revoke: (url: string) => void;
  /** A preview changed: its new URL, or null when the fetch failed. */
  onChange: (frame: number, name: string, url: string | null) => void;
}

/** Previews fetched at once; the rest wait, so the frame screen stays quick. */
export const PREVIEW_CONCURRENCY = 2;

/**
 * Preset previews, one blob URL per frame and preset, fetched lazily and
 * again only when the version (see `previewVersion`) changes. A failed fetch
 * is not retried at the same version.
 */
export class PresetPreviewCache {
  private readonly deps: PresetPreviewDeps;
  private readonly entries = new Map<string, { url: string | null; version: string }>();
  /** Latest version asked for, per key. */
  private readonly wanted = new Map<string, { frame: number; name: string; version: string }>();
  private readonly inflight = new Set<string>();
  private queue: string[] = [];
  private epoch = 0;

  constructor(deps: PresetPreviewDeps) {
    this.deps = deps;
  }

  request(frame: number, name: string, version: string): void {
    const key = JSON.stringify([frame, name]);
    if (this.entries.get(key)?.version === version) return;
    if (this.wanted.get(key)?.version === version) return;
    this.wanted.set(key, { frame, name, version });
    if (!this.inflight.has(key) && !this.queue.includes(key)) this.queue.push(key);
    this.pump();
  }

  /** Forgets everything (a roll was opened) and revokes every URL. */
  reset(): void {
    this.epoch += 1;
    for (const e of this.entries.values()) if (e.url) this.deps.revoke(e.url);
    this.entries.clear();
    this.wanted.clear();
    this.queue = [];
  }

  private pump(): void {
    while (this.inflight.size < PREVIEW_CONCURRENCY && this.queue.length > 0) {
      const key = this.queue.shift();
      const want = key === undefined ? undefined : this.wanted.get(key);
      if (key === undefined || !want) continue;
      const epoch = this.epoch;
      this.inflight.add(key);
      this.deps
        .fetch(want.frame, want.name)
        .then(
          (bytes) => {
            if (epoch !== this.epoch) return;
            const url = this.deps.makeUrl(bytes);
            const old = this.entries.get(key);
            this.entries.set(key, { url, version: want.version });
            this.deps.onChange(want.frame, want.name, url);
            if (old?.url) this.deps.revoke(old.url);
          },
          () => {
            if (epoch !== this.epoch) return;
            const old = this.entries.get(key);
            this.entries.set(key, { url: old?.url ?? null, version: want.version });
            if (!old?.url) this.deps.onChange(want.frame, want.name, null);
          },
        )
        .finally(() => {
          this.inflight.delete(key);
          const now = this.wanted.get(key);
          if (now && now.version !== this.entries.get(key)?.version && !this.queue.includes(key)) {
            this.queue.push(key);
          } else if (now && now.version === this.entries.get(key)?.version) {
            this.wanted.delete(key);
          }
          this.pump();
        });
    }
  }
}
