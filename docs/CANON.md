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

## Reference game build

Entries below were checked against the installation's `hoi4_rev.txt`. Current reference: **game 1.20.0.0, revision
`8873ec77`** (with DLC up to `dlc054_to_the_bitter_end`). Earlier entries were first made on revision `7c6c1c59`
and re-checked on `8873ec77`: the parser baseline is identical, the data loaders report no unknown keys, and the
counts (provinces, states, equipment, sub-units, OOB files, buildings) did not change. When the game updates, re-run
`parse_all --baseline` and the `coverage` example, then update this section.

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
- Source: all `.txt/.gfx/.gui/.asset/.mod/.map` files of game revisions `7c6c1c59` and `8873ec77` (base game, `integrated_dlc/`, `dlc/`).
- Behaviour: the original ships files with defects and still runs, so it evidently tolerates them: an unclosed `{` at end of file (60 occurrences), a stray `}`,
  stray quotes, a key with no value. 39 files are affected; they are listed in
  `crates/clausewitz/baseline/hoi4-8873ec77.txt`. `interface/credits.txt` is plain text, not script.
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
  quotes in the original files into local problems (39 malformed files remain: 60 unclosed braces, 3 stray `}` and one key without a value).
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
  Inheritance is described in "Equipment inheritance"; modular equipment in "Equipment modules".
- Original defects found: `max_strength = 250§` in `ship_hull_carrier.txt`; several duplicated stat keys in equipment
  and sub-units (first one kept, reported).

### Terrain-like combat situations
- Status: guess (list observed in files)
- Source: sub-unit blocks `river`, `amphibious`, `fort` and `snow` take the same shape as terrain blocks but are not
  categories in `common/terrain`.
- Ours: `data::units::COMBAT_SITUATIONS`; any other block name must be a terrain category or it is reported.

### Balance constants live in Lua
- Status: verified (files load completely)
- Source: `common/defines/00_defines.lua`, `00_graphics.lua`, `01_career_profile.lua` (revision `7c6c1c59`; re-checked on `8873ec77`, where
  patch 1.20 changed `00_defines.lua` and `NDefines` has 5927 values).
- Behaviour: three global tables (`NDefines`, `NDefines_Graphics`, `NDefines_CareerProfile`); the last two are copied
  into `NDefines` by a `for k,v in pairs(...) do NDefines[k] = v end` loop at the end of their files. Five constants
  are written as products (`7 * 24`). Mods override single values with `NDefines.<section>.<key> = <value>`.
- Ours: `data::lua` reads this subset (tables, numbers, strings, booleans, `nil`, `+ - * /` with parentheses, the merge
  loop); anything else is reported as unsupported, never guessed. Files run in evaluation order, so later
  assignments win. The whole installation loads with no problems.
- **To measure:** Lua computes in floating point and the engine then stores the result; we compute in `Fixed`
  directly. For the products in the base game this is exact; a division that does not terminate (`1 / 3`) may differ
  in the last digit.
- Test: `crates/data/tests/defines.rs`

### Equipment inheritance
- Status: wiki-only, partly guessed
- Source: community wiki, "Equipment modding": "Archetype equipment is used to assign more general attributes that
  regular equipment then inherits via the archetype attribute"; `parent` is "the previous version of equipment".
- Ours: `Equipment::effective` follows the `archetype` chain; own values win; `parent` is not followed for stats.
  **Guesses:** `is_archetype` and `is_buildable` are never inherited (base-game archetypes are all unbuildable while
  their variants are buildable); list attributes (`type`, `upgrades`, `resources`) are replaced as a whole, not
  merged. All 342 equipment entries of the real installation resolve.
- Copies made by duplicate archetypes: a duplicate archetype (`light_tank_aa_chassis`, archetype
  `light_tank_chassis`) also copies every equipment of its archetype. The copy of `light_tank_chassis_1` is named
  `light_tank_aa_chassis_1`: the comment in `x_tank_chassis.txt` says "For each duplicated equipment type (excluding
  the archetype)" and gives that name as its example. These names are defined nowhere in the files, yet
  technologies enable 99 of them and AI files and `variant_name` lists use them. `Equipment::duplicate_source`
  resolves such a name (duplicate name + what follows the archetype name in the source's name; the source must have
  that archetype and not be an archetype); the copies themselves, with `for_each` changes, are not built yet.
  **Status of the naming rule:** read from one example and confirmed by every reference in the base game.
- Test: `crates/data/tests/oob.rs` (`equipment_inherits_from_its_archetype`), `crates/data/tests/technologies.rs`
  (`duplicate_archetypes_imply_copies_of_their_variants`)

### Equipment modules
- Status: verified (files load completely); module stat arithmetic unmeasured
- Source: `common/units/equipment/modules/*.txt` (revision `8873ec77`, a key inventory of the files) for the module
  blocks, which the shipped documentation does not cover; `common/units/equipment/_documentation.md` for
  `module_slots`, `module_count_limit` and `default_modules` on equipment.
- Ours: `data::modules` stores every module as written: `category`, `add_stats` / `multiply_stats` /
  `add_average_stats`, per-mission stats, costs, conversions, restrictions and the slots a module opens
  (`allowed_module_categories`). An `equipment_modules` block with a `limit` (a `has_dlc` trigger) marks its modules
  as conditional; the trigger is deferred to the script VM. Module categories are not declared anywhere: a category
  exists when a module names it. Stat names are not checked (no file lists them; modules use names such as `thrust`
  that no equipment sets). 312 modules in 95 categories load with no unknown keys; every module, category and resource
  named by modules and equipment exists.
- Slot inheritance: `module_slots = inherit`, `<slot> = inherit` and `<default module> = inherit` (the last is
  undocumented, used by `ship_hull_midget_submarine`) take "the parent equipment type" (documentation): the `parent`
  equipment if set, else the archetype. **Verified by data consistency:** with the archetype, `heavy_cruiser_3` and
  `heavy_cruiser_4` would put default modules into `front_1_custom_slot` / `rear_2_custom_slot`, which only
  `heavy_cruiser_1` (reached through `parent`) defines. Inherited count limits follow the documentation (overridden by
  a limit with the same match conditions; "same" is taken as the same set, in any order — a guess).
- **To measure:** how `add_stats`, `multiply_stats` and `add_average_stats` combine with the frame's stats and with
  `module_add_stats_multiplier`; what `need_equipment_modules = { any = { ... } }` in sub-units means (still deferred).
- Original defects found: `ship_hull_escort_carrier` sets a default module for `mid_2_custom_slot`, a slot it does
  not have; `module_slots = inherit` written twice in three light cruisers; duplicated `xp_cost` and
  `dismantle_cost_ic` in two tank modules (first kept); six empty `can_convert_from = { }` blocks (ignored).
- Test: `crates/data/tests/modules.rs`

### Orders of battle (history/units)
- Status: verified (files load completely)
- Source: 609 files in `history/units` (revisions `7c6c1c59` and `8873ec77`).
- Ours: `data::oob` reads division templates (regiments, support, regimental support grid) and divisions (name or
  ordered name, location, template, start experience and equipment factors, forced variants). Fleets, air wings,
  officers and `instant_effect` (starting production, a script) are deferred. 804 templates and 5074 divisions load.
- Open: 22 files set `start_equipment_factor` at the top level; how it combines with a division's own factor is
  unknown. 32 divisions use templates that no OOB defines (`SWI_1939_BBA`, `AST_TFV_*`): those templates are created
  by script (decisions, focuses) before the OOB is loaded, so the reference is resolved by the script VM.
- Test: `crates/data/tests/oob.rs`

### Buildings
- Status: verified (files load completely); semantics of most keys unmeasured
- Source: `common/buildings/00_buildings.txt` and `01_landmark_buildings.txt` (revision `8873ec77`), including the
  comment at the top of the files: "if province_max is set, then building is provincial"; `country_modifiers` accept
  country modifiers and apply only to the controllers listed in `enable_for_controllers` "or if the list is empty";
  `local_resources_` may be followed by any resource name.
- Ours: `data::buildings` stores every value as written (costs, level caps, flags, modifiers, prerequisites, spawn
  points); what the values do is left to the economy, map and combat systems. `dlc_allowed` (a trigger) and
  `missing_tech_loc` (bindable localisation) are deferred. 56 buildings and 7 spawn points load with no unknown keys.
  Every building named by state history, terrain, prerequisites, `exclusive_with` and `spawn_point` exists, and every
  state history puts provincial buildings on provinces and the others on the state.
- Original defects found: `damage_factor` written twice (`0`, then `-1`) in the dams and canals, `only_costal` twice
  in `dockyard` (first kept, see "Duplicate keys inside one block"). Which `damage_factor` the original uses matters for
  strategic bombing and must be measured.
- Test: `crates/data/tests/buildings.rs`

### Technologies
- Status: verified (files load completely); research mechanics unmeasured
- Source: `common/technologies/*.txt` and `common/technology_tags/00_technology.txt` (revision `8873ec77`, a key
  inventory of the files). No shipped documentation covers technologies.
- Ours: `data::technologies` stores, as written: research cost, start year, research screen positions (`folder`),
  categories, `path` arrows (`leads_to_tech`, `research_cost_coeff`, `ignore_for_layout`), `XOR`, `dependencies`,
  sub-technologies, unlocks (`enable_equipments`, `enable_equipment_modules`, `enable_subunits`, `enable_building`),
  special project specialisations, `ai_research_weights`, experience-research keys and display switches. Every other
  `name = number` entry is kept as a country modifier, every other block as stats for a sub-unit or sub-unit category
  (stats, terrain / combat situation blocks, `battalion_mult`). Triggers, effects and AI weights (`allow`,
  `allow_branch`, `ai_will_do`, `on_research_complete`, `on_research_complete_limit`) and the `limit` of an unlock
  block are deferred to the script VM; an unlock with a `limit` is marked conditional. Categories and folders come
  from `technology_categories` / `technology_folders` (a folder's `available` trigger is deferred). 553
  technologies, 133 categories and 16 folders load with no unknown keys. Every category, folder, technology,
  equipment (counting duplicate-archetype copies, see "Equipment inheritance"), sub-unit, building, terrain and
  sub-unit category named by technologies exists, with one exception below.
- `XOR` is written in upper case in `industry.txt` and `MTG_naval.txt` (one empty block) and as `xor` in two
  special-forces technologies of `infantry.txt`; both are read. **Guess:** the original accepts both spellings;
  whether `xor` actually excludes `improved_special_forces` / `advanced_special_forces` in game is unverified.
- Doctrines are no longer technologies (since 1.17 they live in `common/doctrines`), though the doctrine folders
  are still declared in `technology_folders`. Technology sharing groups (`common/technology_sharing`) are not loaded
  yet.
- **To measure:** how `research_cost`, `start_year` (ahead-of-time penalty) and `research_cost_coeff` combine into
  research time; what a `dependencies` value (`= 1`) means; what `XOR` does when both sides are researched by effect.
- Original defects found: `excavation1`-`excavation5` have `path = { research_cost_coeff = 1 }` without a target
  (reported); three empty `path = { }` blocks (ignored); `air_equipment` declared twice in `technology_categories`;
  `advanced_modern_tank_turret_tech` enables the module `advanced_modern_tank_turret`, which no file defines.
- Test: `crates/data/tests/technologies.rs`

### Ideas
- Status: verified (files load completely); effects of ideas unmeasured
- Source: `common/ideas/*.txt` and `common/idea_tags/00_idea.txt` (revision `8873ec77`, a key inventory of the files),
  including the comments in `00_idea.txt`: the ledger "can be define for idea > idea slot > idea category", "first one
  overrides the others"; `slot_ledgers` entries marked `invalid # must be overridden`.
- Ours: `data::ideas` reads `idea_categories` (slots, character slots with repeats, type, hidden, `politics_tab`,
  ledger, costs) and `slot_ledgers`, then every idea under its group key (an idea category or a slot) with the
  group's `law`, `designer` and `use_list_view` switches. An idea keeps, as written: name key, picture, ledger, costs,
  `level`, `default`, `cancel_if_invalid`, traits, `modifier` (country modifiers, `custom_modifier_tooltip`,
  `hidden_modifier`), `targeted_modifier` (target and modifiers), `research_bonus`, `equipment_bonus` (stats and
  `instant`) and `rule`. Triggers, effects and AI weights (`allowed`, `allowed_civil_war`, `allowed_to_remove`,
  `available`, `visible`, `cancel`, `do_effect`, `on_add`, `on_remove`, `ai_will_do`) are deferred; which of them an
  idea has is recorded. 6692 ideas and 14 categories load with no unknown keys. Every group is a category, slot or
  character slot; every research bonus names a technology category; every equipment bonus names an equipment, an
  equipment type, an equipment `alias` (`destroyer`) or an equipment group (see "Equipment groups").
- **Guesses:** the group switches hold for the whole group wherever they are written (ethiopia.txt writes
  `designer = yes` after the ideas, twice); repeated `equipment_bonus` / `research_bonus` blocks in one idea all apply
  (16 ideas write one block per equipment); `instant = yes` holds for its whole equipment block (it is often repeated
  after each stat). A `targeted_modifier` target may be a tag or a scope (`ROOT`, `event_target:...`,
  `JAP.<variable>`), resolved by the script VM.
- Original defects found: two `targeted_modifier` blocks without `tag` (denmark.txt, italy.txt; reported, dropped);
  ten ideas with `picture` twice, two with `removal_cost` twice, one with `modifier` twice (first kept, see "Duplicate
  keys inside one block"); unclosed braces in `SOV.txt`, `persia.txt` and `switzerland.txt` (see "Script syntax").
- **To measure:** what `level`, `cost` / `removal_cost = -1` and `instant` do; whether a `law` group allows exactly
  one idea; how a repeated group switch or bonus block behaves in game.
- Test: `crates/data/tests/ideas_characters_history.rs`

### Equipment groups
- Status: verified (documented, files load completely)
- Source: `common/equipment_groups/_documentation.md` (shipped): a group lists types, archetypes, categories or other
  groups under `equipment_type`, has an optional `description`, and can be an `equipment_bonus` target.
- Ours: `data::equipment_groups` stores the members as written; 24 groups load, every member resolves.
- Test: `crates/data/tests/ideas_characters_history.rs` (`equipment_groups_list_members`)

### Characters
- Status: verified (files load completely); role mechanics unmeasured
- Source: `common/characters/_documentation.md` (shipped: the `characters` root, `can_be_captured` with default yes,
  the scientist role with skills defaulting to level 1) and a key inventory of `common/characters/*.txt` (revision
  `8873ec77`) for the other roles.
- Ours: `data::characters` keeps name, gender, `can_be_captured`, portraits (`army` / `civilian` / `navy`, `large` /
  `small`), and the roles as written: country leader (one per ideology; ideology, `expire`, `id`, traits), advisor
  (slot, `idea_token`, name, ledger, costs, `can_be_fired`, traits, modifier, `research_bonus`), corps commander, field
  marshal and navy leader (skills, `legacy_id`, traits), scientist (desc, traits, skills). `instance = { ... }` blocks
  are kept as alternative versions of the character; their `allowed` trigger, which picks one, is deferred, as are the
  triggers and effects of roles (`visible`, `allowed`, `available`, `do_effect`, `on_add`, `on_remove`, `ai_will_do`)
  and `allowed_civil_war`. 5470 characters load with no unknown keys; every advisor slot is a character slot of an
  idea category, every advisor research bonus names a technology category.
- **Guess:** portraits written in two blocks (PER.txt) are both used. **To check:** which instance the original picks
  when several match, and what an absent `allowed` means for an instance.
- Original defects found: `name` twice in six characters / instances, `cost` twice in five advisors, `ledger` twice in
  four (first kept).
- Traits (`common/country_leader`, `common/unit_leaders`, `common/scientist_traits`) and ideologies are not loaded
  yet, so trait and ideology names are not checked.
- Test: `crates/data/tests/ideas_characters_history.rs`

### Country history
- Status: files load completely; the file name rule is a guess
- Source: `history/countries/*.txt` (revision `8873ec77`, 364 files) and `documentation/effects_documentation.md`
  (`set_politics`, `set_popularities`: "If an ideology is not specified its popularity will be set to zero",
  `set_oob`: "replacing any previously registered OOB", `set_research_slots`, `recruit_character`,
  `set_grand_doctrine`, `set_sub_doctrine`, `set_fuel_ratio`, `add_ideas`, `load_oob`).
- Behaviour: a history file is an effect block run for its country at game start; dated blocks run at their date.
- Ours: `data::country_history` reads the plain top-level effects as start data: capital, OOB (`oob` / `set_oob`),
  research slots, convoys, stability, war support, fuel ratio, `starting_train_buffer`, `set_technology`,
  `set_politics`, `set_popularities`, `add_ideas`, `recruit_character`, grand doctrines and sub-doctrines. Everything
  else (`if`, variables, scope blocks, other effects, dated blocks) is deferred; only the dates are kept. A plain value
  written twice is taken from the last one, because effects run in order (`set_convoys`, `set_stability` repeat in six
  files). `set_popularities` with variable values (PER) is deferred, leaving the plain value empty. 364 histories load
  with no unknown keys; every tag, capital, technology, idea (or advisor `idea_token`: `add_ideas =
  SOV_genrikh_yagoda`) and character they name exists.
- **Guesses:** the tag is the first three characters of the file name (all base game files start with the tag; two
  use an irregular separator, `HBC  - Hebei-Chahar.txt`, `KLT -Kalat.txt`); a second file for the same tag is
  ignored (none in the base game); `OOB` in upper case (AFA, ALB) means `oob`. Note that an `if` elsewhere in the file
  may still change a plain value when the script VM runs (GER picks its OOB inside `if` blocks).
- Original defects found: ten histories name OOB files that no file defines (ARM names `AZR_1936`; AZR, BLR, CAM,
  GEO, KAZ, LAO, MNT, UKR, VIN name `<TAG>_1936`).
- Test: `crates/data/tests/ideas_characters_history.rs`

### Script constants (`@name`)
- Status: guess
- Source: `01_landmark_buildings.txt` defines `@landmark_repair_speed_factor = 0.15` at the top and uses it in the
  same file. Seven files of `common/technologies` define constants inside the `technologies` block (`@1936 = 2`, used
  for research screen positions); `@1936` is defined in three of them, and no file uses a constant it does not define,
  consistent with file scope. The wiki could not be consulted (its pages served a bot challenge).
- Ours: `data::source::Constants` treats constants as local to the file that defines them, first definition kept;
  an unknown constant is reported, inline math `@[ ... ]` is reported as unsupported. **To check:** scope (file vs.
  folder vs. global) and redefinition behaviour.
- Test: `crates/data/tests/buildings.rs` (`constants_are_local_to_their_file`)

### Localisation precedence
- Status: guess
- Source: files under `localisation/**/replace/` exist to override keys, which suggests that ordinary duplicates do
  not override.
- Ours: `replace` files first, then the rest in evaluation order; the first definition of a key wins.
- Test: `crates/data/tests/loaders.rs` (`localisation_lines_and_precedence`)
