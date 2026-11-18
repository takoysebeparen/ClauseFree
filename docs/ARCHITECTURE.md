# Architecture

This document is the contract for the new engine. If code and this document disagree, one of them is a bug.

## 1. Principles

1. **Data-driven.** All game content (countries, units, techs, focuses, events, localisation, ...) comes from the
   game's files through a virtual file system. The engine code contains **no** content identifiers or balance numbers
   (no `"GER"`, no `"infantry"`, no `0.3` that means "organisation"). If a number has to exist in code, it is a named
   constant with a documented source (see [CANON.md](CANON.md)).
2. **Deterministic simulation.** The same state and the same commands must produce bit-identical results on every
   machine. This is required for multiplayer, replays, saves and oracle tests.
3. **Headless core.** The simulation runs without a window, a GPU or Bevy. Rendering and UI only *read* simulation
   state and *send commands*.
4. **Layered, with one-way dependencies.** A layer may depend only on layers below it.
5. **Verify against the original.** Behaviour that is not defined by data files is validated against the original
   game as a black-box oracle (section 8), never guessed.
6. **Contributor-friendly.** English everywhere (code, comments, logs, docs). Small crates with clear responsibilities.

## 2. Layers (bottom to top)

```
 app            window, input, main loop (Bevy)
 render         map renderer, shaders, fonts           ─┐ read-only view of sim state
 ui             .gui/.gfx interpreter, widgets          ─┘ sends commands
 sim            deterministic world simulation (headless)
 script         Clausewitz script compiler + VM (triggers, effects, modifiers, scopes)
 data           typed definitions built from parsed files (countries, units, ...)
 vfs            layered virtual file system (base game, DLC, mods)
 clausewitz     lossless lexer/parser for Paradox script and localisation files
```

Crates: `clausewitz` and `vfs` exist (M1). Planned: `data`, `script`, `sim`, `render`, `ui`, `app`, and `common` for
shared small utilities.

## 3. Virtual file system and layering

Content is read through a VFS that stacks sources in order: base game, then DLC, then mods, honouring the original
rules (load order, per-file override by relative path, `replace_path` for whole directories). Application code never
touches the real file system for game content. Layering the original way is what makes DLC and mod support fall out of
the design instead of being bolted on later.

## 4. Parsing

`clausewitz` provides a lossless parser: it preserves key order, duplicate keys, comments (optionally) and source
spans for good error messages. It never panics on malformed input. First measurable milestone: **parse every file of
an installation without errors**.

## 5. Data model

`data` turns parsed files into typed, validated definitions with interned ids (compact integers) and cross-reference
checking. Unknown or unsupported keys are *reported*, never silently dropped, so that coverage is measurable.

## 6. Simulation

- State is plain Rust data (structure-of-arrays where it matters) indexed by stable integer ids. Not Bevy ECS: ECS
  iteration order and component layout must not leak into save formats or network protocol.
- **Numbers:** fixed-point arithmetic only. No `f32` / `f64` in simulation code. (Exact scale to be confirmed against
  `documentation/script_math_functions.md` and oracle measurements.)
- **Randomness:** a single seeded, explicitly stepped generator; never time- or hash-seeded.
- **Ordering:** iteration order must be defined (sorted ids / `BTreeMap`); `HashMap` iteration order must never
  influence results.
- **Time:** discrete hourly ticks; daily/monthly work is triggered by the clock, not by wall time.
- **Parallelism:** data-parallel stages with deterministic partitioning and ordered merges (rayon). A stage may run on
  many threads only if its result is independent of scheduling. This is where the performance win over the
  single-threaded original comes from.
- **State hash:** a hash of the world is computed periodically and compared in multiplayer and tests to detect desyncs.

## 7. Commands, saves, multiplayer

Every player action is a **command** stamped with a tick. The simulation consumes only commands.
This single decision gives us:

- **Saves** = a state snapshot (+ optional command log).
- **Replays** = initial state + command log.
- **Multiplayer** = deterministic lockstep: peers exchange commands and compare state hashes.
- **Tests** = scripted command sequences.

## 8. Oracle testing against the original game

Formulas and behaviours that live in the original executable are discovered by **observing** the real game, not by
disassembling it (see CONTRIBUTING.md, clean-room rules). The workflow:

1. Pin the game build (`hoi4_rev.txt` of the installation) and the set of enabled DLC.
2. Define a scenario (start date, setup, N days) as a small test mod and/or console script.
3. Run it in the original with logging enabled and capture the output (logs, text saves, screenshots).
4. Run the same scenario headless in ClauseFree and compare, with documented tolerances.
5. Record the result in [CANON.md](CANON.md): the source of truth, the measured values, and any accepted deviation.

## 9. Scripting

`script` compiles trigger / effect / modifier blocks to an intermediate representation and executes it against the
simulation with proper scope handling. The game ships machine-readable-ish references in `documentation/`
(`triggers`, `effects`, `modifiers`, `script_math_functions`, `console_commands`, ...). They act as the checklist for
script coverage.

## 10. Rendering and UI

Rendering and UI are replaceable views. The UI of the original is described by `interface/*.gui` and `*.gfx` files;
the long-term plan is to interpret those files rather than hand-write windows. Hand-written (egui) windows are
acceptable only as temporary developer tools and must not be presented as the game UI.

## 11. Localisation

All player-visible text comes from the game's localisation files. Engine code uses localisation keys, never inline
human-language strings (log messages for developers are English).

## 12. Configuration

The game directory is taken from the `HOI4_DIR` environment variable (later: a settings file / auto-detection).
Absolute paths must never be hardcoded.
