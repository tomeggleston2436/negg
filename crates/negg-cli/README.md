# negg-cli

The negg command-line tool: convert, batch, eval and profile.

## Owns

- The `negg` binary and its subcommands: convert, batch, eval, profile
- Argument parsing and orchestration of the engine crates

## Must not

- Contain image, calibration or colour maths
- Be depended on by any engine crate

Licence: Apache License 2.0 (Apache-2.0). See `LICENSE` in this directory.
