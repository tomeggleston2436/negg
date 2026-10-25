// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { TILE_HEADER_LEN, WireError, parseTile } from "./tile";

interface Fields {
  magic?: string;
  version?: number;
  format?: number;
  width?: number;
  height?: number;
  tileX?: number;
  tileY?: number;
  level?: number;
  frame?: number;
  generation?: number;
  byteOffset?: number;
  extra?: number;
}

/** Builds a tile response byte by byte, as the Rust encoder lays it out. */
function tileBytes(f: Fields = {}): ArrayBuffer {
  const width = f.width ?? 3;
  const height = f.height ?? 2;
  const pixels = Math.max(0, width * height * 4 + (f.extra ?? 0));
  const buf = new ArrayBuffer(TILE_HEADER_LEN + pixels);
  const v = new DataView(buf);
  const magic = f.magic ?? "NEGGTILE";
  for (let i = 0; i < 8; i += 1) v.setUint8(i, magic.charCodeAt(i));
  v.setUint16(8, f.version ?? 1, true);
  v.setUint16(10, f.format ?? 1, true);
  v.setUint32(12, width, true);
  v.setUint32(16, height, true);
  v.setUint32(20, f.tileX ?? 4, true);
  v.setUint32(24, f.tileY ?? 5, true);
  v.setUint32(28, f.level ?? 2, true);
  v.setUint32(32, f.frame ?? 17, true);
  v.setUint32(36, f.generation ?? 0xfffffffe, true);
  v.setUint32(40, f.byteOffset ?? 48, true);
  v.setUint32(44, 0, true);
  const px = new Uint8Array(buf, TILE_HEADER_LEN);
  for (let i = 0; i < px.length; i += 1) px[i] = i % 251;
  return buf;
}

describe("parseTile", () => {
  it("round trips the header and pixels", () => {
    const { header, pixels } = parseTile(tileBytes());
    expect(header).toEqual({
      width: 3,
      height: 2,
      tileX: 4,
      tileY: 5,
      level: 2,
      frame: 17,
      generation: 0xfffffffe,
      byteOffset: 48,
    });
    expect(pixels.length).toBe(3 * 2 * 4);
    expect(Array.from(pixels.slice(0, 5))).toEqual([0, 1, 2, 3, 4]);
    expect(pixels[23]).toBe(23);
  });

  it("accepts a full 512 x 512 tile", () => {
    const { header, pixels } = parseTile(tileBytes({ width: 512, height: 512 }));
    expect(header.width).toBe(512);
    expect(pixels.length).toBe(512 * 512 * 4);
  });

  const bad: Array<[string, Fields]> = [
    ["wrong magic", { magic: "NEGGTILX" }],
    ["version 2", { version: 2 }],
    ["version 0", { version: 0 }],
    ["pixel format 2", { format: 2 }],
    ["byte_offset 64", { byteOffset: 64 }],
    ["a missing byte", { extra: -1 }],
    ["a trailing byte", { extra: 1 }],
    ["width 0", { width: 0 }],
    ["width 513", { width: 513, height: 1 }],
  ];
  for (const [name, fields] of bad) {
    it(`rejects ${name}`, () => {
      expect(() => parseTile(tileBytes(fields))).toThrow(WireError);
    });
  }

  it("rejects a buffer shorter than the header", () => {
    expect(() => parseTile(new ArrayBuffer(47))).toThrow(WireError);
  });
});
