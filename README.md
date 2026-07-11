# lyrebird

Identify and rename HandBrake video rips (DVD/Blu-ray) using [TheMovieDB](https://www.themoviedb.org) metadata, through a validated, two-stage rename pipeline.

Ripping multi-title discs produces generically-named files (`title_01.mkv`, `title_02.mkv`, …) that need to be identified and renamed into a Jellyfin/Plex-friendly library layout. lyrebird splits that into identification aids, a hand-edited manifest, and a resolve → validate → apply pipeline that never touches the filesystem until the plan has passed every check.

Named after the Australian lyrebird, known for its uncanny mimicry — identification is the whole game.

## Pipeline

```console
$ lyrebird sheet *.mkv                 # contact-sheet PNG per file: identify without watching
$ lyrebird template *.mkv              # write manifest.txt, pre-filled one row per file
$ $EDITOR manifest.txt                 # fill in TMDB ids / kinds
$ lyrebird resolve manifest.txt        # TMDB lookups -> renames.txt (the rename plan)
$ lyrebird validate renames.txt        # all safety checks, exit non-zero on any error
$ lyrebird apply renames.txt           # re-validates, then executes the renames
```

`resolve` needs a TMDB credential in the `TMDB_API_KEY` environment variable — either the legacy v3 API key or the v4 "API Read Access Token" works (detected by format; both are free from themoviedb.org).

Optionally, set `LYREBIRD_TV_ROOT` and/or `LYREBIRD_MOVIE_ROOT` (e.g. `/media/tv`, `/media/movies`) and resolve will produce absolute targets under the matching root — TV rows and extras under the TV root, movie rows and extras under the movie root. Left unset, targets are relative and you run `apply` from inside the library root. `manual` targets are always used exactly as written.

## The manifest

One row per ripped file, columns separated by `|` (surrounding spaces are ignored, so you can align columns however you like; rows containing a tab are read as tab-separated for compatibility with older files). The first line must be the `#lyrebird:manifest` marker (`lyrebird template` writes it). The row kind determines the remaining columns:

```
#lyrebird:manifest
title_01.mkv  | tv    | 84958  | 1 | 1
title_02.mkv  | tv    | 84958  | 1 | 2   | Witches Before Wizards
title_03.mkv  | tv    | 84958  | 1 | 3-4
movie_rip.mkv | movie | 161795
making_of.mkv | movie-extra | 161795 | featurettes | Reflections from the Deep
gag_reel.mkv  | tv-extra    | 84958  | 2 | extras  | Gag Reel
oddball.mkv   | manual | Show (2020)/Season 00/Show - s00e01 - Pilot Workprint.mkv | 1290
```

| Kind | Columns after the kind | Result |
|---|---|---|
| `tv` | series id, season, episode, *[expected title]* | `Series (Year)/Season 01/Series - s01e01 - Episode Title.mkv`. Episode accepts a range (`3-4`) for multi-episode rips: `s01e03-e04`, titles joined with " & ", runtimes summed. |
| `movie` | movie id, *[expected title]* | `Title (Year)/Title (Year).mkv` |
| `movie-extra` | movie id, extra type, name, *[duration secs]* | `Title (Year)/<extra type>/Name.mkv` — a [Jellyfin extras folder](https://jellyfin.org/docs/general/server/media/movies/), so the extra attaches to the movie. |
| `tv-extra` | series id, *[season]*, extra type, name, *[duration secs]* | `Series (Year)/<extra type>/Name.mkv`, or inside `Season SS/` when a season is given. |
| `manual` | new path, *[duration secs]* | Anything TMDB doesn't know about; target path used as-is. |

Extra types are validated against Jellyfin's recognized folder names (`extras`, `featurettes`, `trailers`, `behind the scenes`, `deleted scenes`, `interviews`, `scenes`, `shorts`, `clips`, `other`) because a typo would silently detach the extra.

The optional *expected title* on `tv`/`movie` rows is a cross-check: type what the disc's box says, and resolve warns when TMDB's title is wildly different (fuzzy comparison, so spelling and punctuation differences don't cry wolf). This catches wrong ids and disc-vs-broadcast episode ordering — mistakes the duration check can't see when every episode is the same length.

## What validate checks

- duplicate sources, duplicate/colliding targets, no-op renames
- source files that don't exist, targets that already exist
- malformed target paths (absolute, empty/`..` components); warnings for suspect characters and unexpected extensions
- **duration cross-check**: each file's actual duration (ffprobe) against the expected duration from TMDB runtimes (or supplied by hand), with a tolerance of ±10% or ±30 s, whichever is looser — the highest-value check for catching swapped episodes or an extra mislabeled as an episode

`apply` re-runs all of this itself and refuses on any error — there's no `--force` and no trusting a stale validate run. Renames create parent directories as needed, never overwrite, and fall back to a verified copy-then-remove when the target is on a different filesystem.

## Installation

With Nix flakes:

```console
$ nix run github:brianmay/lyrebird -- --help
$ nix profile install github:brianmay/lyrebird
```

or from a local checkout: `nix build && ./result/bin/lyrebird --help`, or add `github:brianmay/lyrebird` as an input to your NixOS configuration.

The packaged binary is wrapped with ffmpeg/ffprobe on its PATH, so nothing else needs installing.

## Development

```console
$ nix develop        # rust toolchain, rust-analyzer, ffmpeg
$ cargo test
$ nix flake check    # builds the package and runs the test suite
```

## License

[AGPL-3.0-or-later](LICENSE).
