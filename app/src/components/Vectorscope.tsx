// SPDX-License-Identifier: Apache-2.0
// Draws the BT.709 vectorscope (256 x 256, Cb across, Cr up). Neutral greys
// land in the centre cell (128, 128), marked by the cross.

import { useEffect, useRef } from "react";
import { SCOPE_BINS, type ScopesData } from "../ipc/scopes";
import { arrayPeak, scopeBrightness } from "./Parade";

const N = SCOPE_BINS;

export function Vectorscope({ scopes }: { scopes: ScopesData | null }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const ctx = ref.current?.getContext("2d");
    if (!ctx) return;
    const img = ctx.createImageData(N, N);
    const d = img.data;
    for (let i = 3; i < d.length; i += 4) d[i] = 255;
    if (scopes) {
      const peak = arrayPeak(scopes.vectorscope);
      for (let i = 0; i < N * N; i += 1) {
        const b = scopeBrightness(scopes.vectorscope[i] ?? 0, peak);
        d[i * 4] = b;
        d[i * 4 + 1] = b;
        d[i * 4 + 2] = b;
      }
    }
    ctx.putImageData(img, 0, 0);
    ctx.strokeStyle = "rgba(255, 255, 255, 0.25)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.arc(N / 2, N / 2, N / 2 - 1, 0, 2 * Math.PI);
    ctx.moveTo(N / 2 + 0.5, 0);
    ctx.lineTo(N / 2 + 0.5, N);
    ctx.moveTo(0, N / 2 + 0.5);
    ctx.lineTo(N, N / 2 + 0.5);
    ctx.stroke();
  }, [scopes]);
  return (
    <figure className="scope">
      <canvas ref={ref} width={N} height={N} role="img" aria-label="Vectorscope" />
      <figcaption>Vectorscope</figcaption>
    </figure>
  );
}
