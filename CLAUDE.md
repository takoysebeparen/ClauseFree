# Instructions for AI assistants working on ClauseFree

Read this file first in every session. The assistant has no memory between sessions; these rules are the memory.

## What the project is

A free, open-source Rust engine that runs *Hearts of Iron IV* data files, indistinguishable to the eye from the
original but faster. Read `README.md`, `docs/ARCHITECTURE.md`, `docs/CANON.md`, `docs/ROADMAP.md`,
`CONTRIBUTING.md`. The maintainer is not a professional programmer; explain decisions plainly, in the maintainer's
language (Russian) when talking to them. **Code, comments, logs, commit messages and docs are in English.**

## Non-negotiable rules

1. **Truth over speed.** Never invent a game number, formula or behaviour. Find it in, in this order: game data files,
   `documentation/`, an oracle measurement, the wiki (as a hypothesis). If none exists, say so and add a `guess` entry to
   `docs/CANON.md` instead of silently coding it.
2. **No hardcoded content.** No tags (`"GER"`), ids, unit names or balance numbers in engine code. Read from data.
3. **Deterministic simulation:** fixed-point only, defined iteration order, seeded RNG, no wall-clock, no `HashMap`
   iteration influencing results, headless (no Bevy in `sim`).
4. **Respect the layers** in `docs/ARCHITECTURE.md`. No upward dependencies.
5. **Clean room:** never read, request or reproduce decompiled/leaked Clausewitz or game code. Black-box observation only.
6. **Do not help circumvent DLC/licence checks.** Reading whatever files are installed is fine.
7. **No panics on game/mod data.** No `unwrap`/`expect` on external data. Return typed errors.
8. **Tests are not proof of canon.** A passing test only shows code matches our intent. Canon is proven by oracle
   scenarios and recorded in `docs/CANON.md`.
9. **Do not claim things work if you only compiled them.** Say what was verified and what was not (for example: "not
   checked visually").

## Workflow

- The game path comes from the `HOI4_DIR` environment variable. Never hardcode a path. Tests that need the game skip
  when it is absent.
- `legacy/` is a frozen prototype (tag `skeleton-v0`). Do not extend it. Port ideas out of it into new crates, rewriting
  and translating them to English, and verifying against the sources of truth.
- Before finishing a task: `cargo fmt`, `cargo clippy` (no warnings), `cargo test`. State results honestly.
- Commit only when asked. Keep commits focused.
- When unsure about a product decision, ask the maintainer; when unsure about a game fact, measure or cite.
