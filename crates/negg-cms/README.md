# negg-cms

Colour management: working space, ICC profiles and soft proof.

## Owns

- `lab`: sRGB transfer functions (`srgb_eotf`, `srgb_oetf`), the D65
  sRGB-to-XYZ matrix, CIE 1976 L*a*b* (`xyz_to_lab`, `srgb_to_lab`) and
  the CIEDE2000 colour difference (`ciede2000`), verified against the
  34 published Sharma, Wu and Dalal test pairs
- The working colour space definition (phase 1)
- ICC profile loading and transforms via lcms2 (phase 1)
- Soft proofing (phase 1)

## Must not

- Decode or invert images
- Write output files
- Contain film or rig calibration maths

Licence: Mozilla Public License 2.0 (MPL-2.0). See `LICENSE` in this directory.
