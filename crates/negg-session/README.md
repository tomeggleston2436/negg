# negg-session

The negg flows, moved out of `negg-cli` so that the CLI, the desktop app
and the C ABI share them: settings-file parsing, decode and prepare, rig and
film loading, roll mode (pass 1 and pooling), `convert_frame_with`,
`convert_one` (`negg convert`), `batch_dir` (`negg batch`) and lock.

## Owns

- Orchestration and I/O of a conversion, a batch and a lock
- Reporting progress and warnings as `FlowEvent`s (nothing is printed here)

## Must not

- Contain image maths; that lives in the engine crates
- Print to stdout or stderr

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
