// SPDX-License-Identifier: Apache-2.0
// Thumbnail cache: one blob URL per frame, kept across screen changes and
// refetched only when `negg://thumbs` marks the frame stale at a newer
// generation. At most `limit` fetches run at once; a replaced URL is revoked.

export interface ThumbCacheDeps {
  fetch: (frame: number) => Promise<ArrayBuffer>;
  makeUrl: (bytes: ArrayBuffer) => string;
  revoke: (url: string) => void;
  /** A frame's thumbnail changed: its new URL, or null when the fetch failed. */
  onChange: (frame: number, url: string | null) => void;
}

export const THUMB_CONCURRENCY = 3;

export class ThumbCache {
  private readonly deps: ThumbCacheDeps;
  private readonly limit: number;
  /** URL and the staleness generation it was fetched for. */
  private readonly entries = new Map<number, { url: string; stale: number }>();
  /** Highest staleness asked for, per frame. */
  private readonly wanted = new Map<number, number>();
  /** Staleness at which a fetch failed (not retried until it rises). */
  private readonly failed = new Map<number, number>();
  private readonly inflight = new Set<number>();
  private queue: number[] = [];
  private epoch = 0;

  constructor(deps: ThumbCacheDeps, limit = THUMB_CONCURRENCY) {
    this.deps = deps;
    this.limit = limit;
  }

  /** Asks for a frame's thumbnail at least as new as `stale`; a no-op when cached. */
  request(frame: number, stale: number): void {
    const have = this.entries.get(frame);
    if (have && have.stale >= stale) return;
    if ((this.failed.get(frame) ?? -1) >= stale) return;
    if ((this.wanted.get(frame) ?? -1) >= stale) return;
    this.wanted.set(frame, stale);
    if (!this.inflight.has(frame) && !this.queue.includes(frame)) this.queue.push(frame);
    this.pump();
  }

  /** Forgets everything (a roll was opened) and revokes every URL. */
  reset(): void {
    this.epoch += 1;
    for (const e of this.entries.values()) this.deps.revoke(e.url);
    this.entries.clear();
    this.wanted.clear();
    this.failed.clear();
    this.queue = [];
  }

  /** Number of fetches running (for tests). */
  get running(): number {
    return this.inflight.size;
  }

  private pump(): void {
    while (this.inflight.size < this.limit && this.queue.length > 0) {
      const frame = this.queue.shift();
      if (frame === undefined) break;
      const stale = this.wanted.get(frame) ?? 0;
      const epoch = this.epoch;
      this.inflight.add(frame);
      this.deps
        .fetch(frame)
        .then((bytes) => {
          if (epoch !== this.epoch) return;
          const url = this.deps.makeUrl(bytes);
          const old = this.entries.get(frame);
          this.entries.set(frame, { url, stale });
          this.failed.delete(frame);
          this.deps.onChange(frame, url);
          if (old) this.deps.revoke(old.url);
        })
        .catch(() => {
          if (epoch !== this.epoch) return;
          this.failed.set(frame, stale);
          if (!this.entries.has(frame)) this.deps.onChange(frame, null);
        })
        .finally(() => {
          this.inflight.delete(frame);
          // Asked for again while this fetch ran (marked stale at a newer
          // generation, or asked for after a reset): fetch once more.
          const w = this.wanted.get(frame);
          if (w !== undefined && (epoch !== this.epoch || w > stale) && !this.queue.includes(frame)) {
            this.queue.push(frame);
          }
          this.pump();
        });
    }
  }
}
