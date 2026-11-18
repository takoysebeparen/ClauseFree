# ClauseFree

A free, open-source, non-commercial game engine written in Rust that runs the data files of
*Hearts of Iron IV* — without the original Clausewitz engine and its single-core simulation limit.

**Goal:** a player installs ClauseFree next to their own copy of the game, presses *Play*, and sees a game that
looks and feels like the original, only faster and smoother — with room for features the original engine cannot do.
This is the same idea as [OpenMW](https://openmw.org) for *Morrowind*.

> **Status: early development.** Nothing here is playable yet. The repository is being re-founded on a clean
> architecture; see [docs/ROADMAP.md](docs/ROADMAP.md).

## Requirements

- A legally owned copy of *Hearts of Iron IV* (ClauseFree ships **no** game assets and never will).
- Rust (stable, edition 2024).
- Set `HOI4_DIR` to the game directory, for example:
  `HOI4_DIR="D:\SteamLibrary\steamapps\common\Hearts of Iron IV"`.

## Repository layout

| Path | What it is |
|------|------------|
| `docs/` | Architecture, canon ledger, roadmap. Start with `docs/ARCHITECTURE.md`. |
| `legacy/` | The frozen prototype (tag `skeleton-v0`). A reference and a source of ideas, **not** the foundation. Parts are extracted into new crates as they are rewritten. |
| `crates/` | The new engine, one crate per layer (created as milestones are reached). |

## Building the legacy prototype

```bash
cargo run -p clausefree-legacy
cargo test -p clausefree-legacy
```

## Contributing

Please read [CONTRIBUTING.md](CONTRIBUTING.md) first — in particular the clean-room rules.

## Legal

ClauseFree is an independent fan project. It is not affiliated with or endorsed by Paradox Interactive.
*Hearts of Iron* is a trademark of Paradox Interactive AB. No game assets or proprietary code are included.
