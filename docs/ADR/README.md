# Architecture decision records

An ADR records one decision that shapes `negg`: why it was made, what it rules out, and what it costs. The plan (`IMPLEMENTATION_PLAN.md`) says that any deviation from `docs/MATHS.md`, any change to a default constant, and any reopening of a decision in its "Decisions already made" list needs an ADR. This directory is where those live.

## When to write one

- The code is about to deviate from `docs/MATHS.md`.
- A default in a `docs/MATHS.md` constants table is about to change.
- A `TODO(spec)` in `docs/MATHS.md` or `docs/PROFILES.md` is being resolved.
- One of the accepted decisions (0001 onwards) is being reopened.
- A dependency with an unusual licence is being added (for example, LibRaw; see 0003).

Do not write an ADR for routine implementation choices that do not touch the model, a default, a licence or a decision.

## Numbering and naming

Files are `NNNN-short-title.md`, four digits, zero-padded, allocated in order. `0000` is the template. Numbers are never reused; a superseded ADR keeps its number and its status changes to "Superseded by NNNN".

## Index

The directory listing is the index. Notable entries:

- 0020: the app links the engine through `negg-session`; `negg-ffi` is a separate C ABI; the app is its own cargo workspace.
- 0021: one geometry field (crop, analysis buffer, quarter turn, flip) serves analysis and export; the Dmin border search runs inside the crop; schema version 3 (amends 0007).
- 0022: preview pyramid, tile and scopes wire formats, thumbnails, and a deterministic ICC header.
- 0023: slider mapping, edits and linear history, sidecar writes with data safety, and pool refresh.
- 0024: app exports are the CLI's own functions; the automated Gate 3 parity test; packaging; memory budget.
- 0025: frontend state and tests, and the npm licence check.
- 0026: print output is read as Rec.709 primaries for RAW input (supersedes the step 7 matrix part of 0012).
- 0027: step 6 cast fit classifies cells after a provisional correction, slopes bounded to [0.5, 2] (supersedes the cell selection and slope clamp of 0009).
- 0028: `negg eval` registers hand-cropped, mirrored or turned references (Negative Lab Pro) onto the render, takes a per-frame geometry from the manifest and reports tone and colour descriptors.
- 0029: roll mode never lifts a percentile film base above the frame's own estimate and pools the cast only from a Border base (amends Ruling 59, supersedes in part 0017).
- 0030: the print stage floors `x_0` so that the film base prints at least at `P = 2.0`, a black point (reopens and supersedes in part 0008).
- 0031: the black point of 0030 moves to the knee of the toe, `P_black = Dmax_p - ln(2) / a_sh` (2.1845 with the defaults), derived from the paper constants; narrows the shadow and midtone gap to reference renders (reopens and supersedes in part 0030).

## Status values

`Proposed`, `Accepted`, `Rejected`, `Superseded by NNNN`.

## Template

Copy `0000-template.md`. The sections are:

- **Status**: one of the values above, with the date.
- **Context**: what question is being answered and which plan rule or `docs/MATHS.md` step it touches.
- **Decision**: what was decided, in one or two sentences, in the imperative.
- **Consequences**: what this rules in, rules out, and costs.
- **Eval delta**: the `negg-cli eval` metrics before and after, for any change that affects a render. The plan says changed defaults must show the eval delta; "none, no render change" is an acceptable entry for decisions that do not touch pixels.

Keep each ADR short. British English, no em dashes (ground rule 9). ADRs are Apache-2.0 like the rest of `docs/`.
