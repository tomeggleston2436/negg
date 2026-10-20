# negg-ffi

C ABI over the negg engine for the desktop app (phase 3).

## Owns

- The C ABI boundary over the engine crates
- Memory ownership rules across that boundary

## Must not

- Contain engine maths of its own
- Be depended on by any engine crate

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
