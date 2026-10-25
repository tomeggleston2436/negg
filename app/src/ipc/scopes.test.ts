// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { SCOPES_HEADER_LEN, SCOPES_TOTAL_LEN, parseScopes } from "./scopes";
import { WireError } from "./tile";

function scopesBytes(opts: { magic?: string; version?: number; byteOffset?: number; length?: number } = {}): ArrayBuffer {
  const buf = new ArrayBuffer(opts.length ?? SCOPES_TOTAL_LEN);
  const v = new DataView(buf);
  const magic = opts.magic ?? "NEGGSCOP";
  for (let i = 0; i < 8; i += 1) v.setUint8(i, magic.charCodeAt(i));
  v.setUint16(8, opts.version ?? 1, true);
  v.setUint16(10, 0, true);
  v.setUint32(12, 3, true); // frame
  v.setUint32(16, 99, true); // generation
  v.setUint32(20, opts.byteOffset ?? 32, true);
  v.setUint32(24, 1024, true);
  v.setUint32(28, 683, true);
  if (buf.byteLength !== SCOPES_TOTAL_LEN) return buf;
  let at = SCOPES_HEADER_LEN;
  // histogram[c][i] = c * 1000 + i, luma bin 255 = 2^32 - 1
  for (let c = 0; c < 4; c += 1) {
    for (let i = 0; i < 256; i += 1) {
      v.setUint32(at, c === 3 && i === 255 ? 0xffffffff : c * 1000 + i, true);
      at += 4;
    }
  }
  // parade[c][value][col]: mark three cells
  const paradeAt = at;
  v.setUint16(paradeAt + 2 * ((0 * 256 + 255) * 256 + 0), 7, true);
  v.setUint16(paradeAt + 2 * ((1 * 256 + 128) * 256 + 200), 65535, true);
  v.setUint16(paradeAt + 2 * ((2 * 256 + 0) * 256 + 255), 1, true);
  at += 2 * 3 * 256 * 256;
  // vectorscope[row][col]: grey lands in (128, 128)
  v.setUint16(at + 2 * (128 * 256 + 128), 40000, true);
  v.setUint16(at + 2 * (255 * 256 + 255), 2, true);
  return buf;
}

describe("parseScopes", () => {
  it("has the contract's total length", () => {
    expect(SCOPES_TOTAL_LEN).toBe(528416);
  });

  it("round trips the header and arrays", () => {
    const s = parseScopes(scopesBytes());
    expect([s.frame, s.generation, s.width, s.height]).toEqual([3, 99, 1024, 683]);
    expect(s.histogram).toHaveLength(4);
    expect(s.histogram[0]?.[0]).toBe(0);
    expect(s.histogram[2]?.[17]).toBe(2017);
    expect(s.histogram[3]?.[255]).toBe(0xffffffff);
    expect(s.parade.length).toBe(3 * 256 * 256);
    expect(s.parade[(0 * 256 + 255) * 256 + 0]).toBe(7);
    expect(s.parade[(1 * 256 + 128) * 256 + 200]).toBe(65535);
    expect(s.parade[(2 * 256 + 0) * 256 + 255]).toBe(1);
    expect(s.vectorscope.length).toBe(256 * 256);
    expect(s.vectorscope[128 * 256 + 128]).toBe(40000);
    expect(s.vectorscope[255 * 256 + 255]).toBe(2);
    expect(s.vectorscope.reduce((a, b) => a + b, 0)).toBe(40002);
  });

  it("rejects a wrong magic, version, offset or length", () => {
    expect(() => parseScopes(scopesBytes({ magic: "NEGGTILE" }))).toThrow(WireError);
    expect(() => parseScopes(scopesBytes({ version: 2 }))).toThrow(WireError);
    expect(() => parseScopes(scopesBytes({ byteOffset: 48 }))).toThrow(WireError);
    expect(() => parseScopes(scopesBytes({ length: SCOPES_TOTAL_LEN - 2 }))).toThrow(WireError);
    expect(() => parseScopes(scopesBytes({ length: SCOPES_TOTAL_LEN + 2 }))).toThrow(WireError);
  });
});
