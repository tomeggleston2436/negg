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
