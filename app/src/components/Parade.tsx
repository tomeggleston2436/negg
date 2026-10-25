// SPDX-License-Identifier: Apache-2.0
// Draws the RGB parade: three 256 x 256 panels side by side, column = image
// column bucket, row = code value (255 at the top), brightness from the count.

import { useEffect, useRef } from "react";
import { SCOPE_BINS, type ScopesData } from "../ipc/scopes";

const N = SCOPE_BINS;

/** Display brightness 0 to 255 for a scope count, on a log scale against `peak`. */
export function scopeBrightness(count: number, peak: number): number {
  if (count <= 0 || peak <= 0) return 0;
  return Math.min(255, Math.round(40 + 215 * (Math.log1p(count) / Math.log1p(peak))));
}

export function arrayPeak(a: ArrayLike<number>): number {
  let peak = 0;
  for (let i = 0; i < a.length; i += 1) peak = Math.max(peak, a[i] ?? 0);
  return peak;
}

export function Parade({ scopes }: { scopes: ScopesData | null }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const ctx = ref.current?.getContext("2d");
    if (!ctx) return;
    const img = ctx.createImageData(3 * N, N);
    const d = img.data;
    for (let i = 3; i < d.length; i += 4) d[i] = 255;
    if (scopes) {
      const peak = arrayPeak(scopes.parade);
      for (let c = 0; c < 3; c += 1) {
        for (let v = 0; v < N; v += 1) {
          const row = N - 1 - v;
          for (let col = 0; col < N; col += 1) {
            const b = scopeBrightness(scopes.parade[(c * N + v) * N + col] ?? 0, peak);
            if (b === 0) continue;
            const o = (row * 3 * N + c * N + col) * 4;
            d[o + c] = b;
            // A little of the other two channels keeps dim traces visible.
            d[o + ((c + 1) % 3)] = b >> 2;
            d[o + ((c + 2) % 3)] = b >> 2;
          }
        }
      }
    }
    ctx.putImageData(img, 0, 0);
    ctx.fillStyle = "rgba(255, 255, 255, 0.12)";
    for (const v of [64, 128, 192]) ctx.fillRect(0, N - 1 - v, 3 * N, 1);
    ctx.fillStyle = "rgba(255, 255, 255, 0.25)";
    ctx.fillRect(N, 0, 1, N);
    ctx.fillRect(2 * N, 0, 1, N);
  }, [scopes]);
  return (
    <figure className="scope">
      <canvas ref={ref} width={3 * N} height={N} role="img" aria-label="RGB parade" />
      <figcaption>RGB parade</figcaption>
    </figure>
  );
}
