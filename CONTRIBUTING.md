# Contributing to ClauseFree

Thank you for helping. A few rules keep the project legally safe and technically coherent.

## Clean-room rules (mandatory)

ClauseFree reimplements an engine. To stay on the right side of copyright and licence terms:

- **Allowed sources:** the game's data files in *your own legally purchased copy*; the documentation shipped with the
  game; publicly available wikis and forum posts; and **black-box observation** of the real game (running it,
  logging, saving as text, taking screenshots, writing test mods).
- **Forbidden:** decompiled or disassembled code of the game or of the Clausewitz engine; leaked source code; any
  code or text copied from them; contributions derived from such material. If you have seen leaked or decompiled
  source code, do not contribute to the subsystem it concerns.
- **No assets.** Never commit game files, textures, fonts, sounds, localisation text, maps or extracted data.
  Tests that need the game must read it from `HOI4_DIR` and **skip** when it is absent.
- **No circumvention.** Do not add or discuss tools that bypass licence or DLC checks.
- **Do not use the *Hearts of Iron* name or logos** as the name of the product or in its branding.

## Licensing

By contributing you agree that your contribution is licensed under GPL-3.0-only, the licence of this project.
Only submit code you have the right to submit.

## Conventions

- Language: **English** for code, comments, identifiers, logs, commit messages and docs.
- No inline player-facing text: use localisation keys.
- No hardcoded content ids or balance numbers in engine code (see `docs/ARCHITECTURE.md`, principle 1).
- No `f32`/`f64` in simulation code. Use the fixed-point type. Define iteration order explicitly.
- No `unwrap`/`expect` on data coming from game or mod files; return errors with context.
- Every mechanic that is not data-defined gets an entry in `docs/CANON.md` with its source.
- Every change has tests. Tests that need the game skip cleanly when `HOI4_DIR` is not set.
- `cargo fmt`, `cargo clippy` and `cargo test` must pass with no warnings.
- Keep commits focused; explain the *why* in the message.

## Getting started

1. Read `docs/ARCHITECTURE.md` and `docs/ROADMAP.md`.
2. Set `HOI4_DIR` to your game directory.
3. Pick an item from the current milestone, or open an issue to discuss first.
