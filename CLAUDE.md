# lyrebird

A Rust CLI tool for identifying and renaming HandBrake video rips (DVD/Blu-ray) using TheMovieDB (TMDB) metadata, with a validated, two-stage manifest-driven rename pipeline.

## Why this exists

Ripping multi-title discs (TV box sets, movie collections) with HandBrake produces generically-named files (`title_01.mkv`, `title_02.mkv`, ...) that need to be identified and renamed to something sane (e.g. Jellyfin/Plex-friendly `Show - S01E02 - Episode Title.mkv`). Doing this by watching each file and renaming by hand is slow and error-prone.

`lyrebird` splits the problem into two clean stages:

1. **Identification** — figure out what's actually in each ripped file (visual contact sheets + a manifest describing what each title *should* be).
2. **Renaming** — turn that identification into an actual `mv` operation, but only after validating the plan for mistakes.

Named after the Australian lyrebird, known for mimicry/identification — plus a nice nod to being Melbourne/Dandenong Ranges based.

## Design principles

- **Never touch the filesystem until a plan is validated.** All renames go through a `RenamePlan` (old path, new path, expected duration) that gets checked for errors *before* anything is renamed.
- **Prefer automated metadata lookup over hand-typed filenames.** TMDB IDs + season/episode numbers are less error-prone to type than full episode titles, and give a free cross-check (if the ID/season/episode is wrong, the fetched title will look wrong too).
- **Not everything is on TMDB.** DVD/Blu-ray specials, behind-the-scenes featurettes, deleted scenes etc. often aren't in TMDB — the manifest format must support a manual/passthrough row type for these.
- **Duration is a free correctness signal.** Every video file's actual duration (via `ffprobe`) can be cross-checked against the expected duration (from TMDB, or manually supplied) to catch season/episode mix-ups, wrong titles mapped to wrong files, or accidentally-included extras mistaken for real episodes.
- **This will likely grow beyond a one-off script.** Chose Rust over Python for this reason — real enums/structs for manifest row types and rename plans, rather than loosely-typed dicts, will pay off as the tool grows.

## Pipeline overview

```
HandBrake rips (title_01.mkv, title_02.mkv, ...)
        │
        ▼
[Stage 0] Generate contact sheets (ffmpeg)  ──────► visually identify each title without watching full files
        │
        ▼
[Stage 1] Write a TMDB-based manifest (tab-separated text file, hand-edited by Brian)
        │
        ▼
[Stage 2] `lyrebird resolve` — looks up TMDB, builds RenamePlan list (old, new, expected_duration)
        │
        ▼
[Stage 3] `lyrebird validate` — checks the RenamePlan list for errors (see Validation checks below)
        │
        ▼
[Stage 4] `lyrebird apply` — executes the renames (only if validation passed)
```

## Stage 0 — Contact sheets (identification aid)

Not part of the Rust binary necessarily (could be, could stay a shell/ffmpeg helper) — generates a grid-of-thumbnails image per input file so Brian can visually identify content without playing each file start to finish.

```bash
ffmpeg -i "$f" -vf "select='not(mod(n\,1000))',scale=320:-1,tile=4x4" \
  -frames:v 1 -update 1 "${f%.mkv}_sheet.png"
```

- Frame sampling interval should scale with file duration (e.g. `mod(n\,3000)` for long files, smaller for short ones) so the 16 tiles are spread evenly across the runtime.
- Output is a single PNG per input file — a literal image grid, not a data file.
- Could optionally be a `lyrebird sheet` subcommand later, shelling out to ffmpeg.

## Manifest format (input to `lyrebird resolve`)

Tab-separated text file, one row per ripped file. Row "kind" determines the remaining columns:

```
title_01.mkv	tv	84958	1	1
title_02.mkv	tv	84958	1	2
title_03.mkv	tv	84958	1	3
special_01.mkv	manual	Show.S00E01.Behind.The.Scenes.mkv	600
movie_rip.mkv	movie	603
```

| Kind | Columns after `source` | Behavior |
|---|---|---|
| `tv` | `tmdb_series_id`, `season`, `episode` | Look up series name + episode title + runtime from TMDB. Build filename `Series - SxxEyy - Episode Title.mkv`. |
| `movie` | `tmdb_movie_id` | Look up movie title + year + runtime from TMDB. Build filename `Title (Year).mkv`. |
| `manual` | `new_name`, `expected_duration_secs` (optional) | Not on TMDB (specials/extras). Filename supplied directly, duration optional. |

Comment lines start with `#` and should be skipped.

## Intermediate format: RenamePlan / `renames.txt`

Output of the resolve stage, input to validate/apply stages. Three tab-separated columns:

```
title_01.mkv	Show - S01E01 - Pilot.mkv	1320
title_02.mkv	Show - S01E02 - The Reveal.mkv	1350
```

Columns: `old_path`, `new_path`, `expected_duration_secs` (may be blank if unknown).

Keeping this as an explicit intermediate (rather than going straight from TMDB manifest to `mv`) means:
- The plan can be hand-edited/reviewed before applying.
- Validation logic doesn't need to know anything about TMDB — it only deals with old/new/duration triples.
- It's a natural stage boundary for `jj`/git tracking — each ripping batch's manifest + resolved plan can be committed for an audit trail.

## Validation checks (`lyrebird validate`)

Run before any renames happen. Should catch:

- **Duplicate source**: same `old_path` appears on multiple lines.
- **Duplicate target**: two lines resolve to the same `new_path` (collision).
- **No-op rename**: `old_path == new_path`.
- **Source doesn't exist**: `old_path` not found on disk.
- **Target already exists**: `new_path` already exists (would silently overwrite without `-n`/no-clobber logic).
- **Invalid target filename**: contains path separators, empty string, or unexpected extension (warn, not necessarily error).
- **Duration mismatch** (see below): actual file duration vs. expected duration from the plan, outside tolerance.

### Duration cross-check details

- Get actual duration via `ffprobe -v error -show_entries format=duration -of csv=p=0 <file>`.
- Compare against `expected_duration_secs` from the RenamePlan.
- Use a tolerance, not exact match — published runtimes are often rounded, and rips trim intros/credits differently. Suggested: **±10% or ±30–60 seconds, whichever is looser**, to avoid an unreasonably tight tolerance on short episodes.
- Flag as **ERROR** (or WARNING, tune to preference) when a mismatch exceeds tolerance — this is the single highest-value check for catching season/episode swaps or an extras/deleted-scenes title mistakenly mapped as a real episode.
- Possible future addition: fuzzy title cross-check too (supply an expected title in the manifest, compare via string similarity e.g. `strsim`, warn on low similarity) — not yet implemented, discussed as a nice-to-have.

## Tech stack decision

**Rust**, chosen deliberately over Python after discussion. Rationale:

- Project is expected to grow beyond a one-off script (Brian's stated intent).
- Real enums (`ManifestRow::Tv | Movie | Manual`) and structs (`RenamePlan`) give compile-time guarantees that all row kinds are handled everywhere, rather than stringly-typed branching in a dict-based Python script.
- Brian is already comfortable in Rust (existing Modbus/`tokio-modbus` work for his Fox ESS solar inverter integration).
- Blocking HTTP is fine here — this is a linear CLI tool, no need for `tokio`/async. Use `reqwest`'s `blocking` feature to avoid async ceremony entirely.

### Suggested `Cargo.toml` dependencies

```toml
[dependencies]
reqwest = { version = "0.12", features = ["blocking", "json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
csv = "1"
anyhow = "1"
clap = { version = "4", features = ["derive"] }
strsim = "0.11"       # for fuzzy title matching later (not yet implemented)
```

### Suggested module layout

```
lyrebird/
├── Cargo.toml
└── src/
    ├── main.rs         # CLI entry (clap subcommands: resolve / validate / apply / sheet?)
    ├── manifest.rs      # ManifestRow enum + parsing of the TMDB-based manifest
    ├── tmdb.rs          # TMDB client: get_series_name, get_episode, get_movie
    ├── rename_plan.rs   # RenamePlan struct + manifest -> plan resolution logic
    ├── validate.rs       # all validation checks listed above
    └── ffprobe.rs        # shells out to ffprobe, parses duration
```

### Core types to start from

```rust
// manifest.rs
pub enum ManifestRow {
    Tv { source: String, series_id: u32, season: u32, episode: u32 },
    Movie { source: String, movie_id: u32 },
    Manual { source: String, new_name: String, expected_duration: Option<u64> },
}

// rename_plan.rs
pub struct RenamePlan {
    pub old: String,
    pub new: String,
    pub expected_duration_secs: Option<u64>,
}
```

## CLI shape (proposed, not yet implemented)

```bash
lyrebird resolve  tmdb_manifest.txt  -o renames.txt   # Stage 2: TMDB lookups -> RenamePlan list
lyrebird validate renames.txt                          # Stage 3: run all checks, exit non-zero on error
lyrebird apply    renames.txt                          # Stage 4: execute mv, only if validation passes
lyrebird sheet    *.mkv                                # Stage 0 (maybe): generate contact sheet PNGs
```

`resolve` and `validate` could also be combined into a single `plan` command that resolves then immediately validates, with `apply` requiring an explicit `--force` or a prior successful validate run — undecided, worth deciding early.

## Open questions / not yet decided

- Exact output filename convention — current draft is `Series - SxxEyy - Episode Title.mkv` (Jellyfin/Plex-friendly), but Brian's actual Jellyfin library convention should be confirmed (he self-hosts Jellyfin at `jellyfin.linuxpenguins.xyz`).
- Whether `lyrebird sheet` (contact sheet generation) should live inside the Rust binary (shelling out to `ffmpeg`) or stay a separate shell script — leaning toward folding it in for a single cohesive tool, but not decided.
- Whether to add the fuzzy title-similarity cross-check (manifest supplies expected title, compare against TMDB's actual title via `strsim`) — discussed as a nice-to-have, not required for v1.
- Tolerance values for duration mismatch (±10%/±30s suggested, not finalized).
- TMDB API key handling — planned as an environment variable (`TMDB_API_KEY`), free v3 auth key from themoviedb.org.
- Whether resolve/validate/apply should be separate subcommands (current plan) or a single pipeline with flags to stop at each stage.

## Context on Brian (for continuity)

- Runs a NixOS-based homelab (multiple machines: `miacis`, `canidae`, `minidell`, `heimdal`, `minion`, `iot2`), self-hosts Jellyfin, uses Jujutsu (`jj`) for version control on his repos (e.g. `nix-deploy`, `time-tracking`).
- Comfortable in Rust already via a Modbus/`tokio-modbus` integration for a Fox ESS solar inverter.
- Prefers `fish` shell, Helix editor, Alacritty terminal, Niri (Wayland compositor).
- This project's likely home: alongside his other `jj`-tracked repos, possibly on `minion.pri` or wherever he currently hosts personal git-style repos.
