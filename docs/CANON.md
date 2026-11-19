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
- Status: wiki-only
- Source: community wiki, "Modding", section "Loading files" (https://hoi4.paradoxwikis.com/Modding).
- Behaviour (wiki): load order is base game, then DLC "in the order of their internal ID (follows release date)", then
  the user directory, then mods ordered by the file names of their descriptors (overridable with `dependencies`). A
  same-named file in a later source replaces the earlier one; `replace_path` unloads everything previously loaded from
  a folder.
- Ours: base, then `integrated_dlc/*`, then `dlc/*`, each sorted by folder name (`dlcNNN_...`, i.e. internal id);
  mods on top in the order given by the caller. **Not yet implemented:** the user-directory layer and ordering mods by
  descriptor file name / `dependencies`. **Unverified:** where `integrated_dlc` sits (the wiki does not mention it).
- Test: `crates/vfs/tests/layers.rs`

### Evaluation order of files and duplicate definitions
- Status: wiki-only
- Source: https://hoi4.paradoxwikis.com/Modding, section "Loading files".
- Behaviour (wiki): for most folders files are evaluated in ASCII order of their file names (upper case before lower
  case). How duplicate definitions are resolved depends on the kind of object ("some prefer the first-created entry,
  some prefer the last-created entry"). For states, the wiki's example shows the **first** evaluated definition kept.
  The order of tags in `common/country_tags` matters.
- Ours: `data::source::evaluation_order` sorts by file name bytes. States: first definition kept (wiki). Terrain
  categories, country tags, province ids/colours: first kept, **guess**, reported as `Redefinition`.
- Test: `crates/data/tests/loaders.rs` (`first_state_definition_wins_in_evaluation_order`)

### Quoted strings end at a newline
- Status: wiki-only
- Source: https://hoi4.paradoxwikis.com/Modding, section "Code structure": "A space will not interrupt the string, but a direct newline will." The
  same section says strings are limited to 255 characters (not enforced by us yet).
- Ours: the lexer ends an unterminated string at the end of the line. On the real installation this turned all stray
  quotes in the original files into local problems (39 malformed files remain, all unclosed braces).
- Test: `crates/clausewitz/tests/parser.rs` (`unterminated_string_and_math`)

### Number representation
- Status: wiki-only, contradictory
- Source: community wiki, "Data structures" (variables): a signed 32-bit whole part plus a fraction from 0 to 99 999;
  elsewhere: modifiers support at most 3 decimals.
- Ours: `common::Fixed`, i64 scaled by 100 000 (5 decimals); parsing drops digits beyond the fifth (toward zero) and
  rejects whole parts beyond the 32-bit range. **To measure:** precision, rounding, overflow behaviour (oracle).
- Test: `crates/common/src/fixed.rs`

### Duplicate keys inside one block
- Status: guess
- Source: real data, e.g. some states define `state_category` twice and one defines `resources` twice.
- Ours: the first is used and the rest are reported as `Duplicate`. Which one the original uses is unmeasured.

### Country colours
- Status: open question
- Source: a colour exists both in the country file (`color`) and in `common/countries/colors.txt` (`color`,
  `color_ui`).
- Ours: all three are stored; which one the map uses is decided (and verified) by the renderer.

### Sub-unit stats are modifiers on top of equipment
- Status: wiki-only
- Source: community wiki, "Unit modding" (https://hoi4.paradoxwikis.com/Unit_modding), section "Modifiers": e.g.
  `soft_attack = -0.1` on a unit means "10% less Soft Attack than they would usually get from its equipment";
  `maximum_speed` acts as a bonus multiplier: `<base speed> * (1 + maximum_speed)`. Other keys (`max_organisation`,
  `combat_width`, `max_strength`, `manpower`, `training_time`) are absolute values of the unit.
- Ours: `data::units` stores every number as written; the combat and division systems decide which rule applies and
  must verify it against the original (oracle).

### Equipment syntax, archetypes and duplicate archetypes
- Status: verified (official syntax reference shipped with the game)
- Source: `common/units/equipment/_documentation.md`; header of `common/units/equipment/x_plane_airframes.txt`
  ("This file must be evaluated after the file(s) defining the archetypes being duplicated. The equipment
  attributes alias, type, variant_name, and derived_variant_name are not duplicated.").
- Ours: `data::equipment` loads `equipments` and `duplicate_archetypes`; entries of the latter count as archetypes.
  Inheritance (variant -> archetype -> parent) and modular equipment (`module_slots`, `default_modules`, ...) are
  not resolved yet (reported as deferred).
- Original defects found: `max_strength = 250§` in `ship_hull_carrier.txt`; several duplicated stat keys in equipment
  and sub-units (first one kept, reported).

### Terrain-like combat situations
- Status: guess (list observed in files)
- Source: sub-unit blocks `river`, `amphibious`, `fort` and `snow` take the same shape as terrain blocks but are not
  categories in `common/terrain`.
- Ours: `data::units::COMBAT_SITUATIONS`; any other block name must be a terrain category or it is reported.

### Balance constants live in Lua
- Status: verified (files exist)
- Source: `common/resources/_documentation.md` refers to the define `NAI::MIN_FACTORIES_TO_WANT_TO_IMPORT`; defines are
  in `common/defines/*.lua`.
- Ours: not loaded yet. Many combat, supply and production constants are there; a small Lua-table reader is needed
  before those systems are built (no guessing their values).

### Localisation precedence
- Status: guess
- Source: files under `localisation/**/replace/` exist to override keys, which suggests that ordinary duplicates do
  not override.
- Ours: `replace` files first, then the rest in evaluation order; the first definition of a key wins.
- Test: `crates/data/tests/loaders.rs` (`localisation_lines_and_precedence`)
