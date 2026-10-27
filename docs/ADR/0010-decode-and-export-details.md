# 0010: decode and export details for phase 1

## Status

Proposed, 2026-10-20.

## Context

Two `docs/MATHS.md` steps left phase 1 details open that `negg-decode` and `negg-io` cannot be built without.

Step 1 says "use `rawler` for RAW; run demosaic and active-area crop only" and lists the 16-bit TIFF rules, but does not say which demosaic, which colour filter arrays are supported, or where TIFF linearisation happens.

Step 8 lists the TIFF, JPEG and sidecar outputs and carried the line "TODO(spec): the plan does not fix the resolution tag values, the JPEG quality default or chroma subsampling, or the full sidecar field list beyond the items above."

This ADR records both decisions.

## Decision

Decode: the phase 1 demosaic is bilinear for Bayer CFAs, written in `negg-decode`; X-Trans and any other CFA return `DecodeError::Unsupported` until a later ADR. No white balance, no colour matrix, no sRGB conversion, as step 1 already says. For TIFF input: a 16-bit integer TIFF without an ICC profile is treated as gamma 2.2 encoded and flagged with a warning; a float TIFF is linear; a TIFF with an ICC profile is passed up with the ICC bytes and the encoded values untouched, and the caller linearises through `negg-cms`. `Decoded.transfer` records which case applied (`Linear`, `Gamma22Assumed` or `Icc(bytes)`), and `Decoded.image` is linear only when `transfer` is `Linear`.

Export: TIFF resolution tags are 300 pixels per inch with `ResolutionUnit` inch. JPEG quality is 92 with the encoder's default chroma subsampling. The sidecar is the `Sidecar` struct in `negg-io`: `schema_version` (1), `source` (the source file name only, no directories), `settings`, `settings_hash`, `estimates` (optional: `dmin`, `dmin_source`, `dmax`, `gamma`, `gamma_source`, `anchor`, `x0`, `cast`), and `negg_version`.

## Consequences

- Bilinear demosaic is the simplest correct interpolation and is enough for phase 1, whose gates measure colour accuracy on grey cards and roll consistency, not resolution. A better demosaic (and X-Trans support) is a later ADR with an eval delta; it changes pixels, so it cannot slip in silently.
- Writing the demosaic in `negg-decode` rather than taking `rawler`'s keeps the step 1 rule that nothing but demosaic and crop is applied: the decoder hands over the CFA data and the levels, and `negg-decode` owns what happens next.
- Moving TIFF linearisation into `negg-cms` keeps the ICC handling in one crate (ground rule 3: one implementation of each colour operation). The step 1 rule "honour the embedded ICC profile" still holds; it is honoured by the caller through `negg-cms` (`icc_linearise_native` since ADR 0012, which linearises in the profile's own primaries), and the gamma 2.2 fallback by `negg-cms::gamma22_to_linear`. `negg-decode` never guesses a transfer silently: the gamma 2.2 case always carries the warning.
- 300 pixels per inch is the conventional print resolution and is metadata only; it does not change pixels. Quality 92 with default subsampling is a conventional high-quality setting; it is a design choice and has no corpus measurement behind it, but it affects only the lossy JPEG output and never the TIFF or the eval metrics, so it is not marked `TODO(measure)`.
- The sidecar `source` holds a file name and never a directory, so a sidecar is portable with its frame and never leaks a path. `settings_hash` lets a tool detect a sidecar whose settings were edited by hand. `estimates` is optional so that a sidecar written before a render (settings only) is valid.
- The estimate sources are the `DminSource` (`border`, `percentile`, `fixed`, `roll`, `profile`) and `GammaSource` (`range`, `fixed`, `neutral_fit`, `roll`, `profile`) enums in `negg-types`, serialised in snake case; this is the "source of each estimate" that step 8 asks for.
- `schema_version` is 1; `migrate` accepts only version 1 and rejects anything else, so a version 2 can be added with a migration rather than a silent reinterpretation. Adding a field to `Estimates` or `Sidecar` after phase 1 bumps the version.

## Eval delta

None measured; the corpus has no frames yet.
