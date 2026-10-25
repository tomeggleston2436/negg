// SPDX-License-Identifier: Apache-2.0
// One roll browser cell: the frame's JPEG thumbnail from the store's cache
// (fetched once the frame is analysed, and again only when `negg://thumbs`
// marks it stale), its file name and a status badge. Before a frame is
// analysed it shows the frame's negative (Ruling 136), and a frame whose
// Dmax is pinned at the density cap carries a "Needs analysis area" badge
// (Ruling 133).

import { useEffect } from "react";
import type { FrameInfo, FrameStatus } from "../ipc/types";
import { basename } from "./RollSettingsPanel";

export const STATUS_LABELS: Record<FrameStatus, string> = {
  pending: "Analysing",
  ready: "Ready",
  failed: "Failed",
  read_only: "Read-only",
};

export interface ThumbnailProps {
  info: FrameInfo;
  /** Generation from which the thumbnail must be refetched. */
  stale: number;
  /** Cached blob URL; undefined when not fetched yet, null when the fetch failed. */
  url: string | null | undefined;
  /** Asks the cache for a thumbnail at least as fresh as `stale`. */
  onNeed: (stale: number) => void;
  /** The negative thumbnail's blob URL, from its own cache. */
  negativeUrl: string | null | undefined;
  /** The roll waits for Analyse: the negative is asked for and shown. */
  awaiting: boolean;
  onNeedNegative: () => void;
  selected: boolean;
  current: boolean;
  onOpen: () => void;
  onSelect: () => void;
}

export function Thumbnail({
  info,
  stale,
  url,
  onNeed,
  negativeUrl,
  awaiting,
  onNeedNegative,
  selected,
  current,
  onOpen,
  onSelect,
}: ThumbnailProps) {
  const canFetch = info.status === "ready" || info.status === "read_only";

  useEffect(() => {
    if (canFetch) onNeed(stale);
  }, [canFetch, stale, onNeed]);

  useEffect(() => {
    if (awaiting && info.status !== "failed") onNeedNegative();
  }, [awaiting, info.status, onNeedNegative]);

  const name = basename(info.source);
  // The positive once analysed; until then (or while it loads) the negative.
  const positive = canFetch ? url : undefined;
  const shown = positive || negativeUrl || undefined;
  const negative = !positive && Boolean(negativeUrl);
  const statusLabel = awaiting && info.status === "pending" ? "Not analysed" : STATUS_LABELS[info.status];
  const needsArea = info.needs_analysis_area === true;
  return (
    <li className={`thumb${selected ? " selected" : ""}${current ? " current" : ""}`}>
      <button
        type="button"
        className="thumb-open"
        onClick={onOpen}
        aria-label={`Open frame ${info.index + 1}, ${name}, ${statusLabel}${needsArea ? ", needs analysis area" : ""}`}
      >
        <span className={`thumb-image${negative ? " negative" : ""}`}>
          {shown ? (
            <img src={shown} alt="" draggable={false} />
          ) : (
            <span className="thumb-placeholder">
              {info.status === "pending" ? (awaiting ? "Loading the negative" : "Analysing") : "No preview"}
            </span>
          )}
        </span>
        <span className="thumb-caption">
          <span className="thumb-number">{info.index + 1}</span>
          <span className="thumb-name" title={info.source}>
            {name}
          </span>
        </span>
      </button>
      <span className="thumb-badges">
        <span className={`badge badge-${info.status}`} title={info.message ?? undefined}>
          {statusLabel}
        </span>
        {needsArea && (
          <span
            className="badge badge-area"
            title="The film holder is in the analysed area (Dmax at the density cap). Set an analysis area over the whole film including its clear edge (rebate), leaving out only the holder."
          >
            Needs analysis area
          </span>
        )}
      </span>
      <label className="thumb-select">
        <input type="checkbox" checked={selected} onChange={onSelect} />
        <span className="visually-hidden">Select frame {info.index + 1} for export</span>
      </label>
    </li>
  );
}
