// SPDX-License-Identifier: Apache-2.0
// Reader for the binary scopes response:
// a 32-byte little-endian header, then the histogram (4 x 256 u32: R, G, B,
// luma), the parade (3 x 256 x 256 u16, [channel][value][column]) and the
// vectorscope (256 x 256 u16, [row][column]); 528416 bytes in all.

import { WireError, hasMagic } from "./tile";

export const SCOPES_MAGIC = "NEGGSCOP";
export const SCOPES_VERSION = 1;
export const SCOPES_HEADER_LEN = 32;
export const SCOPE_BINS = 256;
const HISTOGRAM_BYTES = 4 * SCOPE_BINS * 4;
const PARADE_LEN = 3 * SCOPE_BINS * SCOPE_BINS;
const VECTORSCOPE_LEN = SCOPE_BINS * SCOPE_BINS;
export const SCOPES_TOTAL_LEN = SCOPES_HEADER_LEN + HISTOGRAM_BYTES + 2 * PARADE_LEN + 2 * VECTORSCOPE_LEN;

export interface ScopesData {
  frame: number;
  generation: number;
  /** Size of the measured image (geometry applied). */
  width: number;
  height: number;
  /** Four arrays of 256 counts: R, G, B, luma. */
  histogram: Uint32Array[];
  /** [channel][value][column], 3 * 256 * 256. */
  parade: Uint16Array;
  /** [row][column], 256 * 256; row 0 is the top (Cr = +0.5). */
  vectorscope: Uint16Array;
}

/** Parses a scopes response; throws WireError when it breaks the format. */
export function parseScopes(buf: ArrayBuffer): ScopesData {
  if (buf.byteLength !== SCOPES_TOTAL_LEN) {
    throw new WireError(`scopes: length ${buf.byteLength} is not ${SCOPES_TOTAL_LEN}`);
  }
  const v = new DataView(buf);
  if (!hasMagic(v, SCOPES_MAGIC)) throw new WireError("scopes: wrong magic");
  const version = v.getUint16(8, true);
  if (version !== SCOPES_VERSION) throw new WireError(`scopes: version ${version} is not 1`);
  const byteOffset = v.getUint32(20, true);
  if (byteOffset !== SCOPES_HEADER_LEN) {
    throw new WireError(`scopes: byte_offset ${byteOffset} is not 32`);
  }
  // Copies: the arrays start at offsets that need not suit typed-array
  // alignment in every engine, and the little-endian order is explicit.
  let at = SCOPES_HEADER_LEN;
  const histogram: Uint32Array[] = [];
  for (let c = 0; c < 4; c += 1) {
    const h = new Uint32Array(SCOPE_BINS);
    for (let i = 0; i < SCOPE_BINS; i += 1) {
      h[i] = v.getUint32(at, true);
      at += 4;
    }
    histogram.push(h);
  }
  const parade = new Uint16Array(PARADE_LEN);
  for (let i = 0; i < PARADE_LEN; i += 1) {
    parade[i] = v.getUint16(at, true);
    at += 2;
  }
  const vectorscope = new Uint16Array(VECTORSCOPE_LEN);
  for (let i = 0; i < VECTORSCOPE_LEN; i += 1) {
    vectorscope[i] = v.getUint16(at, true);
    at += 2;
  }
  return {
    frame: v.getUint32(12, true),
    generation: v.getUint32(16, true),
    width: v.getUint32(24, true),
    height: v.getUint32(28, true),
    histogram,
    parade,
    vectorscope,
  };
}
