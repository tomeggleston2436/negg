// SPDX-License-Identifier: Apache-2.0
// Reader for the binary tile response (Ruling 74, "### Wire formats"): a
// 48-byte little-endian header followed by width * height * 4 bytes of RGBA8.

/** A binary response or a JSON shape that does not match the contract. */
export class WireError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WireError";
  }
}

export const TILE_MAGIC = "NEGGTILE";
export const TILE_VERSION = 1;
export const TILE_HEADER_LEN = 48;
export const PIXEL_FORMAT_RGBA8 = 1;
export const MAX_TILE_SIDE = 512;

export interface TileHeader {
  width: number;
  height: number;
  tileX: number;
  tileY: number;
  level: number;
  frame: number;
  generation: number;
  byteOffset: number;
}

/** Checks the eight ASCII magic bytes at the start of `view`. */
export function hasMagic(view: DataView, magic: string): boolean {
  if (view.byteLength < magic.length) return false;
  for (let i = 0; i < magic.length; i += 1) {
    if (view.getUint8(i) !== magic.charCodeAt(i)) return false;
  }
  return true;
}

/**
 * Parses a tile. Throws WireError on a wrong magic, a version other than 1, a
 * pixel format other than RGBA8, a size outside 1 to 512, a byte_offset other
 * than 48, or a total length other than 48 + w * h * 4. The pixels are a view
 * on `buf` (no copy).
 */
export function parseTile(buf: ArrayBuffer): { header: TileHeader; pixels: Uint8ClampedArray<ArrayBuffer> } {
  if (buf.byteLength < TILE_HEADER_LEN) {
    throw new WireError(`tile: ${buf.byteLength} bytes is shorter than the 48-byte header`);
  }
  const v = new DataView(buf);
  if (!hasMagic(v, TILE_MAGIC)) throw new WireError("tile: wrong magic");
  const version = v.getUint16(8, true);
  if (version !== TILE_VERSION) throw new WireError(`tile: version ${version} is not 1`);
  const format = v.getUint16(10, true);
  if (format !== PIXEL_FORMAT_RGBA8) {
    throw new WireError(`tile: pixel format ${format} is not 1 (RGBA8)`);
  }
  const header: TileHeader = {
    width: v.getUint32(12, true),
    height: v.getUint32(16, true),
    tileX: v.getUint32(20, true),
    tileY: v.getUint32(24, true),
    level: v.getUint32(28, true),
    frame: v.getUint32(32, true),
    generation: v.getUint32(36, true),
    byteOffset: v.getUint32(40, true),
  };
  const { width, height, byteOffset } = header;
  if (width < 1 || width > MAX_TILE_SIDE || height < 1 || height > MAX_TILE_SIDE) {
    throw new WireError(`tile: size ${width} x ${height} is outside 1 to 512`);
  }
  if (byteOffset !== TILE_HEADER_LEN) {
    throw new WireError(`tile: byte_offset ${byteOffset} is not 48`);
  }
  const expected = TILE_HEADER_LEN + width * height * 4;
  if (buf.byteLength !== expected) {
    throw new WireError(`tile: length ${buf.byteLength} is not ${expected}`);
  }
  return { header, pixels: new Uint8ClampedArray(buf, TILE_HEADER_LEN, width * height * 4) };
}
