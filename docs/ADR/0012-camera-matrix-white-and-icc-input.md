# 0012: white-normalised camera matrix and ICC input in its own primaries

## Status

Proposed, 2026-10-21 (controller ruling pending owner review).

Superseded in part by 0026: the step 7 matrix for RAW input is no longer the camera matrix; the ICC input path stands.

## Context

`docs/MATHS.md` step 7 enters the working space through the camera matrix recorded in step 1, "camera RGB -> XYZ -> Rec.2020 with a Bradford chromatic adaptation to D65", applied after inversion. The phase 1 final review found two problems with how that entry was built.

First (I2, M5), `negg_cms::camera_to_rec2020` was `XYZ_TO_REC2020 * Bradford(W -> D65) * inverse(X2C)`, with `X2C` the XYZ-to-camera matrix and `W` its illuminant. After steps 4 to 6 a neutral is equal in every camera channel, but `inverse(X2C)` maps camera `(1, 1, 1)` to whatever XYZ the camera happens to read as equal channels, not to the illuminant's white. A camera whose D65 neutral is `(0.5, 1, 0.7)`, which is typical of raw sensors, therefore rendered every grey with a strong cast. The decoder also did not say which illuminant a fallback matrix was measured under, so `W` was always D65 (M5).

Second (I3), a TIFF with an embedded ICC profile was converted by Little-CMS straight into linear Rec.2020 before step 3. Density, Dmin, gamma and the cast fit then ran on Rec.2020 channels, a mix of the scan's own channels, rather than on the channels the scanner captured, which is what step 1 ("linearise with its TRC") and the rest of the model assume. The camera path keeps camera channels until step 7; the ICC path did not.

## Decision

Build the step 7 matrix as `M = XYZ_TO_REC2020 * Bradford(W -> D65) * inverse(X2C) * diag(X2C * W)`, with `W` the XYZ white of the matrix's illuminant carried in `Decoded::matrix_white` (Ruling 28). Linearise a TIFF with a matrix/TRC ICC profile per channel through its own TRC, in its own primaries, and use `M = XYZ_TO_REC2020 * Bradford(D50 -> D65) * C` as its step 7 matrix, with `C` the colorant matrix; refuse LUT-based profiles as unsupported (Ruling 29).

## Consequences

- `diag(X2C * W)` scales camera `(1, 1, 1)` to the camera's response to `W`; `inverse(X2C)` takes that to `W`, Bradford to D65 and `XYZ_TO_REC2020` to Rec.2020 `(1, 1, 1)`. Any camera neutral stays neutral whatever the matrix. The test: a camera whose D65 neutral is `(0.5, 1, 0.7)` maps camera `(0.18, 0.18, 0.18)` to Rec.2020 grey within 1e-4 (the unnormalised product misses by more than 0.05). A matrix whose response to `W` is not positive in every channel cannot carry a neutral and is rejected as singular.
- `Decoded::matrix_white` is `Some` exactly when `xyz_to_camera` is. The whites are the CIE 2-degree tabulated values: D65 `[0.95047, 1, 1.08883]`, D50 `[0.96422, 1, 0.82521]`, D55 `[0.95682, 1, 0.92149]`, D75 `[0.94972, 1, 1.22638]`, A `[1.09850, 1, 0.35585]`. Any other illuminant, and the catalogue matrix with no recorded illuminant, is taken as D65 with a warning.
- With the identity matrix the entry is now `XYZ_TO_REC2020 * diag(D65)` rather than `XYZ_TO_REC2020`; the phase 1 contract test that equated the two is replaced by this one. The XYZ-to-sRGB "camera", whose D65 neutral is `(1, 1, 1)`, still gives the BT.2087 sRGB to Rec.2020 matrix.
- `negg_cms::icc_linearise_native(icc, encoded)` returns the linear image and the step 7 matrix. It reads the profile's red, green and blue TRC and colorant tags with Little-CMS and evaluates each TRC in floating point per channel, so the image stays in the profile's primaries through steps 3 to 6. The colorants are relative to the D50 profile connection space and sum to its white, so `M` maps `(1, 1, 1)` to Rec.2020 `(1, 1, 1)` by construction; the Bradford source is the ICC PCS white `[0.9642, 1, 0.8249]`. Tests: sRGB grey linearises to 0.18 within 1e-3, the matrix maps `(1, 1, 1)` to `(1, 1, 1)` within 1e-3 and equals BT.2087, and an sRGB-tagged neutral negative converts to a neutral print end to end.
- A profile is matrix/TRC when Little-CMS reports it as a matrix shaper and it has no `AToB` tag. A profile that has both a LUT and matrix/TRC tags is refused rather than read through its matrix, because colour-managed software would render it through the LUT. Non-RGB profiles are refused as before. `linearise_with_icc` had no other caller and is removed.
- `negg convert` and `negg eval` both go through `prepare_input`, so both pass `matrix_white` and use the native ICC path. An ICC TIFF now has a step 7 matrix, so `--dump-stats` reports it as applied.
- Cost if wrong: one matrix formula (Ruling 28) and the ICC input path only (Ruling 29).

## Eval delta

None measured; the corpus has no frames yet. On the phase 1 review's synthetic scenes (linear float TIFFs, no matrix) the change does not apply.
