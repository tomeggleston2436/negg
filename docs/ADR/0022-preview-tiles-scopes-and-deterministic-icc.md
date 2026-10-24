# 0022: preview pyramid, tiles, scopes, thumbnails and a deterministic ICC header

## Status

Proposed, 2026-10-24 (Rulings 73, 74, 75, 76 and 77).

## Context

The app shows a live preview of a 24 MP frame while sliders move. Export remains the reference (ADR 0024), so the preview may approximate it at reduced scale but must never change what an export is. Scopes and thumbnails are derived from the same render. Separately, Gate 3 requires byte-equal exports, and every export so far embedded the wall-clock date in the ICC header (tests masked it with `without_icc_date`). `docs/MATHS.md` step 8 gains the preview definitions; this ADR records the decisions.

## Decision

**Pyramid (Ruling 73).** The open frame keeps its full linear image (after decode, prepare and rig) as level 0, and a pyramid of levels 1 to L built by `negg_render::downscale_half`: a 2 x 2 box mean in linear light, where an odd edge row or column averages the pixels that exist, giving size `ceil(w/2) x ceil(h/2)`. L is the smallest n with `max(w_n, h_n) <= TILE_SIZE`, with `TILE_SIZE = 512`.

**Tiles.** A tile (level `l`, column `tx`, row `ty`) covers `[tx*512, min((tx+1)*512, w_l)) x [ty*512, ...)` of level `l`. It is `render_region` of that level with the frame's `FramePlan` (estimates and print coefficients from the analysis grid restricted per ADR 0021, exactly as export), the rig density model and the camera matrix, then `working_to_output(Srgb, Perceptual).apply_u8`, with alpha 255. Display is always sRGB in phase 3 (monitor profiles are deferred).

**Consistency.** Level 0 tiles assembled are byte-equal to `apply_u8` of `render_frame_with(...).image` for the same frame and settings. Levels above 0 are a preview approximation (a render of a downscale, not a downscale of a render). Export never reads the preview caches.

**Tile wire format (Ruling 74).** A 48-byte little-endian header followed by `width * height * 4` bytes of RGBA8, row-major from the top-left, stride `width * 4`. Fields: magic `NEGGTILE` (0), version u16 = 1 (8), pixel_format u16 = 1 (10), width u32 (12), height u32 (16), tile_x u32 (20), tile_y u32 (24), level u32 (28), frame u32 (32), generation u32 (36), byte_offset u32 = 48 (40), reserved u32 = 0 (44). Readers reject a wrong magic, a version other than 1, a byte_offset other than 48, or a total length other than `48 + w*h*4`.

**Scopes (Ruling 75).** Computed by `negg_render::scopes` from the 8-bit sRGB render of the whole frame at the first pyramid level with long side at most 1024 (level 0 if already smaller), with geometry applied, so they describe the exported framing.

- Histogram: 256 bins for R, G, B and luma `round(0.2126 R + 0.7152 G + 0.0722 B)` on code values.
- Parade: per channel, 256 columns (`col = floor(x * 256 / width)`) by 256 code values; u16 counts, saturating.
- Vectorscope: BT.709 `Y' = 0.2126 R' + 0.7152 G' + 0.0722 B'`, `Cb = (B' - Y') / 1.8556`, `Cr = (R' - Y') / 1.5748` on codes divided by 255; `col = min(255, floor((Cb + 0.5) * 256))`, `row = min(255, floor((0.5 - Cr) * 256))`; u16 counts, saturating.
- Sent in a binary layout, never JSON: magic `NEGGSCOP`, version u16 = 1, reserved u16 = 0, frame u32, generation u32, byte_offset u32 = 32, width u32 and height u32 of the measured image, then histogram 4 x 256 u32, parade 3 x 256 x 256 u16 and vectorscope 256 x 256 u16; 528416 bytes in all.

**Thumbnails (Ruling 76).** `render_full` of the frame's cached `AnalysisGrid::median` (about 1024 px long side) with its current `FramePlan`, geometry applied, `working_to_output(Srgb, Perceptual).apply_u8`, encoded by `negg_io::encode_jpeg8` at quality 85 with the sRGB ICC. Raw JPEG bytes, no header, in memory only (no disk cache in phase 3).

**Deterministic ICC bytes (Ruling 77).** `negg_cms::profile_bytes` is deterministic: after serialisation it overwrites the ICC header creation date (bytes 24 to 35, six big-endian u16) with 2026-01-01 00:00:00 and zeroes the profile ID (bytes 84 to 99, the "not computed" value).

## Consequences

- **Preview only.** A level above 0 can differ from the export; the cost of that approximation is paid in the preview, never the export.
- **Tile versioning.** A format change is a version 2 header; readers refuse unknown versions.
- **Memory.** Only the open frame keeps level 0 and its pyramid (ADR 0024).
- **Thumbnails cost CPU on open.** A disk cache can be added later without changing any byte format.
- **Byte-equal exports (Ruling 77).** The creation date was the only varying byte range in an export. The profile ID was already all zero in the old file, so zeroing it changed nothing. Pixel data is unchanged. The phase 1 golden `crates/negg-cli/tests/data/golden.phase1.negg.tif` is re-baselined once (Ruling 117) because of the fixed date: the difference is 5 bytes at ICC-relative offsets 27, 29, 31, 33 and 35 (the creation-date field), pixels are identical and the file size is unchanged. The golden test still masks the creation date (ADR 0027's rule: a deliberate engine change re-baselines the golden with an ADR; this is one).
- **Cost of choosing wrongly:** one constant (the date), or a scopes version 2.

## Eval delta

None, no pixel change. Only embedded ICC header bytes change.
