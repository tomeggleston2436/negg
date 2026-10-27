# 0036: GPU preview kernel

## Status

Accepted, 2026-10-27 (phase 4).

## Context

Phase 4 asks for a wgpu preview kernel for the visible tiles, compared with the CPU path on every CI run with a tolerance of 1/4096 per channel (ground rule 3: one pipeline, a GPU copy only when it is tested against the CPU).

Measured on a 24 MP synthetic frame (release build, Apple M3 Max) before this change, a 512 px tile took about 30 ms: about 16 ms in the Little-CMS 8-bit sRGB transform, which ran on one core because a tile is exactly one 262,144-pixel band of `negg-cms`'s parallel split, and the rest in the fused kernel. A fit-to-window redraw of 24 tiles took about 0.73 s after every slider step.

## Decision

- **CPU first.** `negg-cms` splits an image smaller than one full band into about one band per rayon thread, so a tile's sRGB transform runs on every core. Each pixel is still converted on its own, so the bytes are unchanged. A tile now takes about 9 ms.
- **While a slider is dragged** the app draws the pyramid level one coarser than the zoom needs (a quarter of the pixels) and hides the finer level. On release it asks for the finer level again, drawn over the coarser tiles. This is display only; no render changes.
- **The GPU kernel** is `negg-render/src/render.wgsl` behind the `gpu` cargo feature (`negg_render::gpu::Gpu`, wgpu 30). It runs steps 3 to 7 in the same order as the fused CPU kernel `render_in_place`: density (with or without the rig density model), step 4, the neutral offset, step 6, step 5, the step 7 entry matrix and saturation. The plan named steps 4 to 7; density and the matrix run on the GPU too, because otherwise the CPU would make an extra pass over the tile. The CPU kernel stays the reference. Exports, thumbnails, scopes and negative tiles never use the GPU.
- **The display encoding is on the GPU too:** `negg_cms::working_to_output_matrix(Srgb)` (the linear part of the Little-CMS transform, from the two profiles' colorant tags), the IEC 61966-2-1 curve, then rounding to 16 bits and from 16 to 8 bits as Little-CMS packs a float pipeline (`FROM_16_TO_8`). A tile comes back as RGBA8 with no CPU pass.
- **Tolerances**, tested on every run where an adapter exists (`negg-render/tests/gpu.rs`, and a session tile test):
  - the GPU positive is within 1/4096 of the CPU positive per channel (measured worst on Metal: 2e-6);
  - the GPU display tile is within one 8-bit step of the CPU tile, with at most 1% of values differing;
  - `negg-cms` checks the matrix, curve and rounding model itself against `OutputTransform::apply_u8`: within one step, at most 0.1% of values differing.
- **Where it runs.** `Session::set_gpu` turns the GPU on for `render_tile`. A failed GPU render falls back to the CPU for that tile and for every later one. Input errors are reported exactly as the CPU path reports them. Only the desktop app turns the GPU on: it looks for an adapter on a background thread at start-up, and `NEGG_GPU=0` keeps it off. The CLI and the C ABI do not build wgpu.
- **CI** installs Mesa's software Vulkan driver (lavapipe) on Linux and sets `NEGG_REQUIRE_GPU=1` there, so the comparison runs on every CI run. macOS and Windows runners test the GPU when they expose an adapter and skip it otherwise.

## Consequences

- A preview tile takes about 1 ms on the GPU (about 9 ms on the CPU, 30 ms before): a fit-to-window redraw takes about 25 ms, and about 6 ms while dragging at the coarser level.
- GPU tiles may differ from CPU tiles by one 8-bit step on a few values. The preview is not an export (step 8), so app/CLI export parity is unaffected.
- The per-pixel maths now exists in Rust and in WGSL, which is the copy ground rule 3 permits only with the tests above. Any change to the fused kernel must change `render.wgsl` too, or the tests fail.
- Adds `wgpu`, `pollster` and `bytemuck` (MIT or Apache-2.0) to the app build only.

## Eval delta

None. No CPU render changes (the `negg-cms` band split is byte-identical), and the GPU path is preview only.
