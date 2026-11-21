# Roadmap

Milestones are ordered by dependency. Each one ends with something measurable. There are no dates: this is a
long-term volunteer project, and correctness comes before speed.

| # | Milestone | Done when |
|---|-----------|-----------|
| M0 | Foundation | Workspace, documents, conventions, frozen prototype in `legacy/`. **(done)** |
| M1 | Parser + VFS | `clausewitz` parses **every** file of the base game and installed DLC with zero new errors (a baseline records the 40 files that are malformed in the original); `vfs` layers base/DLC/mod directories correctly. **(done; see crates `clausewitz` and `vfs`)** |
| M2 | Data model | `data` loads core definitions (countries, states, provinces, units, equipment, terrain, ...) with a coverage report; unsupported keys are listed, not dropped. **(in progress: provinces, terrain, countries, states, localisation, resources, unit categories, sub-units, equipment (with archetype inheritance), equipment modules (with slot inheritance), defines (Lua), orders of battle and buildings done with full key coverage; next: technologies, ideas, characters, history/countries)** |
| M3 | Headless core | `sim` runs the clock, a command queue, deterministic RNG and a state hash; a scenario runs N days headless and is bit-reproducible across runs and machines. |
| M4 | First oracle-verified system | Land combat reproduces measured results from the original within documented tolerances. Establishes the oracle workflow. |
| M5 | Script VM | Triggers/effects/modifiers/scopes execute; events and on-actions run; coverage tracked against `documentation/`. |
| M6 | Map parity | Map rendering visually matches the original at all zoom levels. |
| M7 | UI from data | `.gui`/`.gfx` interpreter drives the real interface; hand-written windows removed. |
| M8 | Playable slice | A 1936 start where the player can move units, fight, produce and research, faster than the original on the same machine. |
| M9 | Saves & multiplayer | Command-log replays, snapshots, deterministic lockstep. |
| M10 | Mods | Load order, `replace_path`, popular mods start. |

Performance claims are made only after profiling the original to find where it spends its time.
