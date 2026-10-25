// SPDX-License-Identifier: Apache-2.0
// Draws the scopes histogram (R, G, B added, luma as a line). Heights are the
// counts scaled to the tallest bin away from the two ends, so a clipped
// shadow or highlight spike does not flatten the rest.

import { useEffect, useRef } from "react";
import type { ScopesData } from "../ipc/scopes";

const W = 256;
const H = 120;
const COLOURS = ["rgb(230, 70, 70)", "rgb(70, 200, 90)", "rgb(80, 120, 240)"];

/** Tallest bin, ignoring bins 0 and 255 unless they are all there is. */
export function histogramPeak(h: ArrayLike<number>): number {
  let peak = 0;
  for (let i = 1; i < h.length - 1; i += 1) peak = Math.max(peak, h[i] ?? 0);
  if (peak === 0) for (let i = 0; i < h.length; i += 1) peak = Math.max(peak, h[i] ?? 0);
  return peak;
}

export function Histogram({ scopes }: { scopes: ScopesData | null }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const ctx = ref.current?.getContext("2d");
    if (!ctx) return;
    ctx.globalCompositeOperation = "source-over";
    ctx.fillStyle = "#161616";
    ctx.fillRect(0, 0, W, H);
    if (!scopes) return;
    const rgb = scopes.histogram.slice(0, 3);
    const peak = Math.max(1, ...rgb.map(histogramPeak));
    ctx.globalCompositeOperation = "lighter";
    rgb.forEach((h, c) => {
      ctx.fillStyle = COLOURS[c] ?? "white";
      ctx.beginPath();
      ctx.moveTo(0, H);
      for (let i = 0; i < W; i += 1) ctx.lineTo(i + 0.5, H - Math.min(H, ((h[i] ?? 0) / peak) * H));
      ctx.lineTo(W, H);
      ctx.closePath();
      ctx.fill();
    });
    ctx.globalCompositeOperation = "source-over";
    const luma = scopes.histogram[3];
    if (luma) {
      const lp = Math.max(1, histogramPeak(luma));
      ctx.strokeStyle = "rgba(235, 235, 235, 0.9)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      for (let i = 0; i < W; i += 1) {
        const y = H - Math.min(H, ((luma[i] ?? 0) / lp) * H);
        if (i === 0) ctx.moveTo(0.5, y);
        else ctx.lineTo(i + 0.5, y);
      }
      ctx.stroke();
    }
  }, [scopes]);
  return (
    <figure className="scope">
      <canvas ref={ref} width={W} height={H} role="img" aria-label="Histogram of red, green, blue and luma" />
      <figcaption>Histogram</figcaption>
    </figure>
  );
}
