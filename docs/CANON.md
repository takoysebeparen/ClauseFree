# Canon ledger

ClauseFree aims to be *indistinguishable to the eye* from the original, not bit-identical. This file is the ledger of
where our behaviour comes from and where it knowingly differs.

## Sources of truth, in priority order

1. **Game data files** (`common/`, `history/`, `map/`, `interface/`, `localisation/`, ...).
2. **Documentation shipped with the game** (`documentation/*.md`).
3. **Oracle observation**: measured behaviour of the real game in a controlled scenario (see ARCHITECTURE.md, section 8).
4. **The community wiki** (<https://hoi4.paradoxwikis.com>). Useful, but sometimes outdated or imprecise: a claim from
   the wiki is a hypothesis until an oracle measurement confirms it.
5. **Guesses.** Not allowed without an entry below that marks them as such.

Disassembling or decompiling the game executable is **not** a permitted source. See CONTRIBUTING.md.

## Entry template

```
### <system / mechanic>
- Status: verified | wiki-only | guess | knowingly-simplified
- Source: <file path / doc / scenario id / wiki page>
- Behaviour: <what the original does>
- Ours: <what we do, and why it differs if it does>
- Test: <test name or scenario id>
```

## Known problems in the frozen prototype (`legacy/`, tag `skeleton-v0`)

The prototype was built feature-by-feature from prompts and was not verified against the game. Treat **everything**
in it as unverified. The items below are known to be wrong or invented and must be re-derived from the sources above
when the corresponding system is rebuilt:

- Battalion/division statistics use invented scales (for example strength 100 per battalion, where the data files say
  25) and invented attack/defence numbers; real values come from equipment, not from `common/units`.
- Supply uses a fixed "radius 3 from a hub" model; the real system is not this.
- Combat tactics, combat phases, tactic probabilities, counter-pick bonuses and phase durations are hardcoded numbers;
  the original defines tactics in `common/combat_tactics/` and the exact resolution is unmeasured.
- Production, trade, resource consumption and efficiency formulas are simplified inventions.
- Experience, commander skill effects and the "air superiority" model are invented.
- Country and region identifiers (`"GER"`, `"FRA"`, state ids 17 and 51, a fixed player country) are hardcoded in
  several places.
- The UI windows are invented, not derived from `interface/*.gui`.
- User-visible text and comments are in Russian, and content names are hardcoded in English/German/French.

What is reusable (after review and translation): file loaders (tokenizer, states, provinces, rivers, heightmap,
flags, `.dds` decoding, BMFont, localisation), map rendering, camera.

## Verified entries

### Script syntax and malformed original files
- Status: verified (by parsing a full installation)
- Source: all `.txt/.gfx/.gui/.asset/.mod/.map` files of game revision `7c6c1c59` (base game, `integrated_dlc/`, `dlc/`).
- Behaviour: the original ships files with defects and still runs, so it evidently tolerates them: an unclosed `{` at end of file (60 occurrences), a stray `}`,
  stray quotes, a key with no value. 40 files are affected; they are listed in
  `crates/clausewitz/baseline/hoi4-7c6c1c59.txt`. `interface/credits.txt` is plain text, not script.
- Ours: the parser reports each defect as a diagnostic and recovers (the tree keeps everything parsed so far), so
  engine behaviour for these files matches "tolerate". **Unmeasured:** how the original recovers (for example whether a
  quoted string may span lines). To be checked with an oracle scenario before relying on it.
- Test: `crates/clausewitz/tests/parser.rs`, `cargo run --release -p clausewitz --example parse_all -- --baseline ...`

### Layering of base game, DLC and mods
- Status: knowingly-simplified (order of DLC is a guess)
- Source: descriptor files (`*.dlc`, `*.mod`) in the installation; `replace_path` semantics from the mod format.
- Behaviour: a later layer overrides a same-named file of an earlier one; `replace_path` drops earlier content of a
  directory.
- Ours: base, then `integrated_dlc/*`, then `dlc/*`, each sorted by folder name; mods on top in the order given by
  the caller. **The relative order of DLC layers is an assumption.** Case-insensitive paths. Whether the original merges
  some directories by *file name* and others by *definition id* is a concern of the data layer, not the VFS.
- Test: `crates/vfs/tests/layers.rs`
