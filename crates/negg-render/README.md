# negg-render

Inversion, print curve, adjustments and scopes as pure per-pixel functions.

## Owns

- Every per-pixel operation, as pure functions on a buffer plus settings
- Inversion, the print curve and user adjustments
- Scopes (histograms and waveforms) computed from a buffer

## Must not

- Do any file or device I/O
- Decode RAW or TIFF input
- Hold a second copy of maths that lives elsewhere

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
