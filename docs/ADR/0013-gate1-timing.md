# 0013: Gate 1 timing is end to end, and how it is met

## Status

Proposed, 2026-10-21.

## Context

Gate 1 in `ROADMAP.md` says "the CLI converts a 24 MP frame in under 2 s on a laptop CPU". Phase 1 measured only `negg_render::render_frame` (analysis and render), which was well inside the gate, and left decode, the export transform and the writer to `--timing`. A review at the end of phase 1 timed `negg convert` end to end on a 6000x4000 frame and found 4.1 s to JPEG and 5.8 s to 16-bit TIFF: the Little-CMS export transform and the Deflate writer were single-threaded, and the full-resolution density image of step 3 was a second full-size allocation. This touches `docs/MATHS.md` steps 3, 7 and 8 (performance only, no change to the maths) and ground rule 4 (one quantisation point, unchanged).

## Decision

Measure Gate 1 end to end, as the time `negg convert` takes on a 24 MP frame to 16-bit TIFF and to JPEG. Meet it by running the export transform over bands in parallel with one Little-CMS transform per rayon worker, by taking the full-resolution density inside `render_full`, and by writing the TIFF in strips of 64 rows compressed in parallel with Deflate at its fast level.

## Consequences

- `negg_cms::OutputTransform::apply` and `apply_u8` split the image into bands of 262,144 pixels and convert them on the rayon pool; each worker builds its own Little-CMS transform for the same space and intent, because Little-CMS transforms are `Send` but not `Sync`. The output equals a single sequential transform exactly (unit test over three and a bit bands).
- `negg_render::render_full` now takes the linear image and computes `D = -log10(max(I, 1e-6))` per pixel with `negg_film::density_value`, the one copy of the step 3 formula, which `negg_film::density` also uses on the grid. `render_frame` no longer allocates a full-size density image, so the output is the only full-size allocation it makes.
- `negg_io::write_tiff16` cuts the image into strips of `ROWS_PER_STRIP = 64` rows, compresses each strip on the rayon pool with the `tiff` crate's own compressors (`Deflate` at `DeflateLevel::Fast`, and LZW or PackBits when asked), and writes the pre-compressed strips and every tag through the crate's low-level directory encoder. The `tiff` crate's image encoder cannot take pre-compressed strips, but its directory encoder can write raw strip data and the strip offset and byte count tags, so no new dependency is needed. PackBits packs each row separately, as the TIFF specification requires. Files read back losslessly through the `tiff` decoder for every compression.
- Fast Deflate trades some file size for speed; the files stay lossless and use the same Deflate compression tag, so every reader that read them before still reads them.
- `negg-cli` gains a library target so that `crates/negg-cli/tests/perf.rs` can time `convert_frame` directly. Its ignored test `converts_a_24mp_frame_end_to_end` writes a synthetic 6000x4000 linear (32-bit float) TIFF, then converts it three times to each format after a warm-up and fails if the worst run of either is 2 s or more. The two perf tests hold a shared lock so their timings do not share the CPU.
- The synthetic frame is a float TIFF with no camera matrix, so step 7's matrix (a few multiply-adds per pixel inside the fused kernel) is not in the timed path; RAW decode and demosaic are not either. A real RAW adds rawler's decode and the bilinear demosaic; that is to be measured on the corpus once frames exist.
- Measured on an Apple M3 Max (14 cores), release build, `cargo test --release -p negg-cli --test perf -- --ignored --nocapture`, best and worst of three runs:

| Output | Before | After | After, per stage (best run) |
| --- | --- | --- | --- |
| 16-bit TIFF | 5897 ms (decode 94, analyse and render 890, transform 2681, write 2233) | best 1218 ms, worst 1260 ms | decode 89, analyse and render 772, transform 260, write 97 |
| JPEG | 3868 ms (decode 85, analyse and render 875, transform 2713, write 196) | best 1367 ms, worst 1450 ms | decode 92, analyse and render 774, transform 280, write 222 |

  `render_frame` alone: best 783 ms, worst 859 ms. Both formats are under the gate, so the fallback (hold the gate for JPEG and report TIFF) is not needed. A laptop with fewer cores than the M3 Max will be slower; the gate should be re-checked on the reference laptop.
- Cost if wrong: a gate wording.

## Eval delta

None, no render change: the parallel transform is bit-identical to the sequential one, the fused density is the same formula per pixel, and the TIFF samples are unchanged.
