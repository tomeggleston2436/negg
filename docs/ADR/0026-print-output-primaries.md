# 0026: print output is read as Rec.709 primaries (RAW input)

## Status

Accepted, 2026-10-24.

## Context

`docs/MATHS.md` step 7 entered the working space from the print output with the camera's scene matrix (camera RGB to XYZ, white-normalised, Bradford to D65), as ADR 0012 built it. That matrix characterises a camera's response to scene light. The three print channels are not that: they are synthetic tone curves of dye densities (step 5), with no relation to the sensor's spectral response once the negative has been inverted.

The first real camera scans showed the consequence. Neutrals stay neutral, because the matrix is white-normalised, which is why every synthetic test (untagged TIFF input, no matrix) passed. Colours, however, came out with roughly double the chroma, and the matrix's negative off-diagonal terms drove the green channel negative on saturated blues, so blue skies and seas clipped to violet.

This choice stands until a measured print characterisation exists (the rig and print work, ADR 0015 and later).

## Decision

For RAW input, treat the three print channels as Rec.709/sRGB primaries with a D65 white and enter linear Rec.2020 with the ITU-R BT.2087 Rec.709-to-Rec.2020 matrix (`negg_cms::REC709_TO_REC2020`). The scene camera matrix is no longer applied to print output. This supersedes the step 7 matrix part of ADR 0012; the ICC input path of ADR 0012 is unchanged.

## Consequences

- RAW output has bounded chroma: a saturated Rec.709 blue maps inside Rec.2020 with no negative channel, so nothing clips to violet. Neutrals still map to neutrals (rows sum to 1).
- `negg_cms::camera_to_rec2020` and the decoder's `xyz_to_camera` and `matrix_white` stay, unused by `prepare_input`, for the rig and print characterisation. `camera_to_rec2020` keeps its tests and its ADR 0012 definition.
- A RAW with no camera matrix now enters Rec.2020 like any other RAW, instead of staying in camera RGB with a warning.
- ICC-tagged TIFF input keeps its colorant matrix. Its channels are the scanner's own primaries, which the profile describes, so the same defect does not apply. An untagged TIFF still skips step 7.
- `--dump-stats` prints the matrix actually applied: `Rec.709 print primaries to Rec.2020 applied (BT.2087)` for RAW.
- Cost if wrong: one matrix choice for RAW input. The look of RAW output changes (less saturated, as intended) until a measured characterisation replaces it.

## Eval delta

None, no render change on the committed corpus (it holds untagged TIFF scenes, which do not use this matrix).
