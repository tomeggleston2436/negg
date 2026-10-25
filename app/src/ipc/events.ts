// SPDX-License-Identifier: Apache-2.0
// Typed listeners for the backend events ("### Tauri backend (P3-T)").
// A payload that does not decode is reported through `onError` and dropped.

import { listen as tauriListen } from "@tauri-apps/api/event";
import {
  type AnalysisEvent,
  type ExportEvent,
  type ThumbsEvent,
  decodeAnalysisEvent,
  decodeExportEvent,
  decodeThumbsEvent,
} from "./types";

export const ANALYSIS_EVENT = "negg://analysis";
export const THUMBS_EVENT = "negg://thumbs";
export const EXPORT_EVENT = "negg://export";
/** The macOS Edit menu's Undo or Redo: `{ action: "undo" | "redo" }`. */
export const MENU_EVENT = "negg://menu";
/** A close or quit is about to flush; answered with the `close_ready` command. */
export const CLOSING_EVENT = "negg://closing";

export type Unlisten = () => void;
export type ListenFn = (event: string, handler: (payload: unknown) => void) => Promise<Unlisten>;

let transport: ListenFn = async (event, handler) => tauriListen(event, (e) => handler(e.payload));

/** Replaces the event transport (tests, or a mock backend); returns the previous one. */
export function setListen(fn: ListenFn): ListenFn {
  const previous = transport;
  transport = fn;
  return previous;
}

function typed<T>(event: string, decode: (v: unknown) => T) {
  return (handler: (payload: T) => void, onError: (e: unknown) => void): Promise<Unlisten> =>
    transport(event, (payload) => {
      let value: T;
      try {
        value = decode(payload);
      } catch (e) {
        onError(e);
        return;
      }
      handler(value);
    });
}

export const onAnalysis = typed<AnalysisEvent>(ANALYSIS_EVENT, decodeAnalysisEvent);
export const onThumbs = typed<ThumbsEvent>(THUMBS_EVENT, decodeThumbsEvent);
export const onExport = typed<ExportEvent>(EXPORT_EVENT, decodeExportEvent);
