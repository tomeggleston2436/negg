# negg-io

TIFF and JPEG writers plus the `*.negg.json` sidecar. Linear DNG output is
phase 4 and is not here yet.

## Owns

- `write_tiff16`: 16-bit RGB TIFF in strips of 64 rows, each compressed
  on its own in parallel with rayon (ADR 0013). Deflate at the fast level
  by default (also none, LZW, PackBits), Orientation (274) = 1, XResolution/YResolution (282/283) =
  300/1 with ResolutionUnit (296) = inch (ADR 0010), and the ICC profile in
  tag 34675 as UNDEFINED bytes when one is given.
- `write_jpeg8`: 8-bit RGB JPEG through the `image` crate's encoder at
  quality 92 with the encoder's default chroma subsampling (ADR 0010); the
  ICC profile goes into APP2 segments and is split across segments when it
  is larger than one.
- `Sidecar` and `sidecar_path`, `read_sidecar`, `write_sidecar`, `migrate`:
  the sidecar beside each source frame. `frame.CR2` gets
  `frame.negg.json`; `roll.01.frame.CR2` gets `roll.01.frame.negg.json`
  (only the last extension is replaced).

- `RollSidecar`, `roll_sidecar_path`, `read_roll_sidecar`,
  `write_roll_sidecar`: `roll.negg.json` (`ROLL_SIDECAR_NAME`), roll
  settings, hash, pooled estimates and frame list, own schema version 1.
- `read_rig_profile`, `write_rig_profile`, `read_film_profile`,
  `write_film_profile`: `*.negg.toml` profiles (docs/PROFILES.md); both
  directions call `validate`, so a `TODO(measure)` source is rejected.
- `parse_wedge_csv`, `read_wedge_csv`, `parse_patches_csv`,
  `read_patches_csv`: calibration inputs; errors are `IoError::Csv` with the
  line number.

## Pixel data

Both writers take interleaved RGB that is already encoded and quantised by
negg-cms (`u16` for TIFF, `u8` for JPEG). Ground rule 4 of the plan:
quantisation happens exactly once, in the exporter, so nothing here
converts floats. The buffer length must be exactly `width * height * 3`.

## Sidecar

Fields: `schema_version` (2), `source` (file name only), `settings`,
`settings_hash` (`Settings::content_hash`), `estimates`
(`Option<Estimates>`), `overrides` (`FrameOverrides`, omitted when empty),
`roll` (`Option<String>`, omitted when none), `negg_version` (filled by the caller from its
`CARGO_PKG_VERSION`; this crate does not read it).

Writing is atomic: pretty JSON with a trailing newline goes to
`<path>.tmp` in the same directory and is renamed over the target, so a
crash never leaves a half-written sidecar. `write_sidecar` refuses a
`schema_version` other than `SIDECAR_SCHEMA_VERSION` and a `source` that
contains a directory separator.

Reading always goes through `migrate`, which reads `schema_version` from
the raw JSON before deserialising anything else. A missing, non-integer or
unknown version is `IoError::Schema`; a version newer than this build
understands says so and asks for an upgrade. Migration is a ladder of
`upgrade_vN_to_vN+1` steps on the raw `serde_json::Value` ending at the
current version; `upgrade_v1_to_v2` (Ruling 51) sets both schema versions to 2 and, when
`estimates` is present, adds `neutral_offset = [0, 0, 0]` and a
`cast_source` of `fit` (cast `cells_used > 0`) or `identity`. Adding
version 3 means bumping the constant, writing `upgrade_v2_to_v3`, and
adding it to the ladder in `migrate`. Within one
version a missing field takes its default and an unknown field is ignored.

## Must not

- Decode input files (tests use the `tiff` and `image` decoders to read
  back what was written; the library does not)
- Contain rendering or colour maths
- Depend on the CLI or app

## Dependencies

`tiff` (writer), `image` with only the `jpeg` feature (JPEG encoder with
ICC support), `serde`, `serde_json`, `toml` (profiles), `negg-types`.

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
