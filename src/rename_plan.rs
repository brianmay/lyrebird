//! The resolved rename plan: intermediate between `resolve` and `validate`/`apply`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};

use crate::manifest::ManifestRow;
use crate::tmdb::Tmdb;

/// One planned rename, as serialized to/from `renames.txt`.
/// `new` is a path relative to the library root, e.g.
/// `The Owl House (2020)/Season 01/The Owl House - s01e01 - A Lying Witch and a Warden.mkv`.
#[derive(Debug, Clone)]
pub struct RenamePlan {
    pub old: String,
    pub new: String,
    pub expected_duration_secs: Option<u64>,
}

/// Library roots prepended to resolved targets, read from the
/// LYREBIRD_TV_ROOT and LYREBIRD_MOVIE_ROOT environment variables. Unset
/// means targets stay relative and apply is run from inside the library
/// root. Manual rows are never prefixed — their target is taken as given.
#[derive(Debug, Default)]
pub struct Roots {
    pub tv: Option<String>,
    pub movie: Option<String>,
}

impl Roots {
    pub fn from_env() -> Self {
        Roots {
            tv: std::env::var("LYREBIRD_TV_ROOT").ok().and_then(clean_root),
            movie: std::env::var("LYREBIRD_MOVIE_ROOT")
                .ok()
                .and_then(clean_root),
        }
    }
}

fn clean_root(value: String) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    (!value.is_empty()).then(|| value.to_string())
}

fn under(root: &Option<String>, path: String) -> String {
    match root {
        Some(root) => format!("{root}/{path}"),
        None => path,
    }
}

fn lookup_series(
    cache: &mut HashMap<u32, crate::tmdb::Series>,
    tmdb: &Tmdb,
    series_id: u32,
    source: &str,
) -> Result<crate::tmdb::Series> {
    if let Some(series) = cache.get(&series_id) {
        return Ok(series.clone());
    }
    let series = tmdb
        .series(series_id)
        .with_context(|| format!("{source}: looking up TMDB series {series_id}"))?;
    cache.insert(series_id, series.clone());
    Ok(series)
}

fn lookup_movie(
    cache: &mut HashMap<u32, crate::tmdb::Movie>,
    tmdb: &Tmdb,
    movie_id: u32,
    source: &str,
) -> Result<crate::tmdb::Movie> {
    if let Some(movie) = cache.get(&movie_id) {
        return Ok(movie.clone());
    }
    let movie = tmdb
        .movie(movie_id)
        .with_context(|| format!("{source}: looking up TMDB movie {movie_id}"))?;
    cache.insert(movie_id, movie.clone());
    Ok(movie)
}

pub fn resolve(rows: &[ManifestRow], tmdb: &Tmdb, roots: &Roots) -> Result<Vec<RenamePlan>> {
    let mut series_cache: HashMap<u32, crate::tmdb::Series> = HashMap::new();
    let mut movie_cache: HashMap<u32, crate::tmdb::Movie> = HashMap::new();
    let mut plans = Vec::with_capacity(rows.len());

    for row in rows {
        let plan = match row {
            ManifestRow::Tv {
                source,
                series_id,
                season,
                episode,
                episode_end,
                expected_title,
            } => {
                let series = lookup_series(&mut series_cache, tmdb, *series_id, source)?;

                let episodes = (*episode..=episode_end.unwrap_or(*episode))
                    .map(|ep| {
                        tmdb.episode(*series_id, *season, ep).with_context(|| {
                            format!(
                                "{source}: looking up TMDB series {series_id} s{season:02}e{ep:02}"
                            )
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let title = episodes
                    .iter()
                    .map(|ep| ep.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" & ");
                // Expected duration for a multi-episode rip is the sum of the
                // episode runtimes; unknown if any episode's runtime is.
                let runtime_secs = episodes
                    .iter()
                    .map(|ep| ep.runtime.map(|mins| mins * 60))
                    .sum::<Option<u64>>();

                if let Some(expected) = expected_title {
                    // For a range, matching any single episode's title is
                    // enough — the user likely typed just the first one.
                    let mut candidates: Vec<&str> =
                        episodes.iter().map(|ep| ep.name.as_str()).collect();
                    candidates.push(&title);
                    if let Some(score) = title_mismatch(expected, &candidates) {
                        eprintln!(
                            "WARNING {source}: expected title '{expected}' but TMDB returned \
                             '{title}' (similarity {score:.2}) — check the series/season/episode \
                             numbers"
                        );
                    }
                }

                RenamePlan {
                    old: source.clone(),
                    new: under(
                        &roots.tv,
                        tv_path(
                            &series.name,
                            series.first_air_year(),
                            *season,
                            *episode,
                            *episode_end,
                            &title,
                            extension_of(source),
                        ),
                    ),
                    expected_duration_secs: runtime_secs,
                }
            }
            ManifestRow::Movie {
                source,
                movie_id,
                expected_title,
            } => {
                let movie = lookup_movie(&mut movie_cache, tmdb, *movie_id, source)?;
                if let Some(expected) = expected_title {
                    if let Some(score) = title_mismatch(expected, &[&movie.title]) {
                        eprintln!(
                            "WARNING {source}: expected title '{expected}' but TMDB returned \
                             '{}' (similarity {score:.2}) — check the movie id",
                            movie.title
                        );
                    }
                }
                RenamePlan {
                    old: source.clone(),
                    new: under(
                        &roots.movie,
                        movie_path(&movie.title, movie.release_year(), extension_of(source)),
                    ),
                    expected_duration_secs: movie.runtime.map(|mins| mins * 60),
                }
            }
            ManifestRow::MovieExtra {
                source,
                movie_id,
                extra_type,
                name,
                expected_duration,
            } => {
                let movie = lookup_movie(&mut movie_cache, tmdb, *movie_id, source)?;
                RenamePlan {
                    old: source.clone(),
                    new: under(
                        &roots.movie,
                        extra_path(
                            &with_year(&sanitize(&movie.title), movie.release_year()),
                            None,
                            extra_type,
                            name,
                            extension_of(source),
                        ),
                    ),
                    expected_duration_secs: *expected_duration,
                }
            }
            ManifestRow::TvExtra {
                source,
                series_id,
                season,
                extra_type,
                name,
                expected_duration,
            } => {
                let series = lookup_series(&mut series_cache, tmdb, *series_id, source)?;
                RenamePlan {
                    old: source.clone(),
                    new: under(
                        &roots.tv,
                        extra_path(
                            &with_year(&sanitize(&series.name), series.first_air_year()),
                            *season,
                            extra_type,
                            name,
                            extension_of(source),
                        ),
                    ),
                    expected_duration_secs: *expected_duration,
                }
            }
            ManifestRow::Manual {
                source,
                new_name,
                expected_duration,
            } => RenamePlan {
                old: source.clone(),
                new: new_name.clone(),
                expected_duration_secs: *expected_duration,
            },
        };
        plans.push(plan);
    }
    Ok(plans)
}

pub fn write(plans: &[RenamePlan], path: &Path) -> Result<()> {
    let mut out = String::new();
    out.push_str(crate::manifest::RENAMES_MARKER);
    out.push('\n');
    for plan in plans {
        out.push_str(&match plan.expected_duration_secs {
            Some(duration) => format!("{} | {} | {duration}\n", plan.old, plan.new),
            None => format!("{} | {}\n", plan.old, plan.new),
        });
    }
    std::fs::write(path, out).with_context(|| format!("could not write {}", path.display()))
}

/// A plan row together with the line it came from in `renames.txt`,
/// so validation messages can point back at the file.
#[derive(Debug, Clone)]
pub struct PlanEntry {
    pub line: u64,
    pub plan: RenamePlan,
}

pub fn read(path: &Path) -> Result<Vec<PlanEntry>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("could not open plan {}", path.display()))?;
    crate::manifest::expect_marker(&content, crate::manifest::RENAMES_MARKER)
        .with_context(|| format!("in plan {}", path.display()))?;
    read_reader(content.as_bytes()).with_context(|| format!("in plan {}", path.display()))
}

fn read_reader<R: std::io::Read>(reader: R) -> Result<Vec<PlanEntry>> {
    let mut entries = Vec::new();
    for (line, fields) in crate::manifest::split_rows(reader)? {
        let old = match fields.first().filter(|s| !s.trim().is_empty()) {
            Some(s) => s.to_string(),
            None => anyhow::bail!("plan line {line}: missing old path"),
        };
        let new = match fields.get(1).filter(|s| !s.trim().is_empty()) {
            Some(s) => s.to_string(),
            None => anyhow::bail!("plan line {line}: missing new path"),
        };
        let expected_duration_secs = match fields.get(2).map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            Some(s) => Some(s.parse().with_context(|| {
                format!("plan line {line}: invalid expected_duration_secs '{s}'")
            })?),
            None => None,
        };

        entries.push(PlanEntry {
            line,
            plan: RenamePlan {
                old,
                new,
                expected_duration_secs,
            },
        });
    }
    Ok(entries)
}

/// Executes the renames. Callers must have validated first; this only keeps
/// the last-moment safety net (a target appearing between validation and the
/// rename would otherwise be silently overwritten by `fs::rename`).
pub fn apply(entries: &[PlanEntry], root: &Path) -> Result<()> {
    for entry in entries {
        let plan = &entry.plan;
        let source = root.join(&plan.old);
        let target = root.join(&plan.new);

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("could not create directory {}", parent.display()))?;
        }
        if target.exists() {
            anyhow::bail!(
                "target '{}' appeared since validation — aborting before overwriting it",
                plan.new
            );
        }
        match std::fs::rename(&source, &target) {
            Ok(()) => println!("renamed: {} -> {}", plan.old, plan.new),
            Err(err) if err.kind() == std::io::ErrorKind::CrossesDevices => {
                copy_then_remove(&source, &target).with_context(|| {
                    format!(
                        "could not copy '{}' to '{}' (cross-filesystem)",
                        plan.old, plan.new
                    )
                })?;
                println!("copied: {} -> {} (cross-filesystem)", plan.old, plan.new);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("could not rename '{}' to '{}'", plan.old, plan.new))
            }
        }
    }
    Ok(())
}

/// Fallback for renames across filesystems, where `fs::rename` fails with
/// EXDEV. Copies to a `.lyrebird-partial` file in the target directory,
/// verifies the byte count, renames into place (same-filesystem, so atomic),
/// and only then removes the source — an interrupted copy can never leave a
/// plausible-looking target or lose the source.
fn copy_then_remove(source: &Path, target: &Path) -> Result<()> {
    let mut partial_name = target.file_name().unwrap_or_default().to_os_string();
    partial_name.push(".lyrebird-partial");
    let partial = target.with_file_name(partial_name);

    let result = (|| {
        let copied = std::fs::copy(source, &partial).context("copy failed")?;
        let expected = std::fs::metadata(source)?.len();
        if copied != expected {
            anyhow::bail!("copied {copied} bytes but source is {expected} bytes");
        }
        std::fs::rename(&partial, target).context("could not move copy into place")?;
        std::fs::remove_file(source).context("copy succeeded but could not remove source")
    })();

    if result.is_err() && partial.exists() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

/// `Series (Year)/Season SS/Series - sSSeEE - Episode Title.ext`
/// (year in the series folder only, not the filename). Multi-episode files
/// get `sSSeE1-eE2` (Jellyfin's documented multi-episode form).
fn tv_path(
    series_name: &str,
    year: Option<&str>,
    season: u32,
    episode: u32,
    episode_end: Option<u32>,
    episode_title: &str,
    ext: &str,
) -> String {
    let series = sanitize(series_name);
    let episode_title = sanitize(episode_title);
    let series_dir = with_year(&series, year);
    let code = match episode_end {
        Some(end) => format!("s{season:02}e{episode:02}-e{end:02}"),
        None => format!("s{season:02}e{episode:02}"),
    };
    format!("{series_dir}/Season {season:02}/{series} - {code} - {episode_title}.{ext}")
}

/// `Title (Year)/Title (Year).ext` — the movie sits in its own folder.
fn movie_path(title: &str, year: Option<&str>, ext: &str) -> String {
    let name = with_year(&sanitize(title), year);
    format!("{name}/{name}.{ext}")
}

/// Jellyfin extras subfolder inside the movie/series (or season) folder:
/// `<parent_dir>[/Season SS]/<extra_type>/<Name>.ext`.
fn extra_path(
    parent_dir: &str,
    season: Option<u32>,
    extra_type: &str,
    name: &str,
    ext: &str,
) -> String {
    let name = sanitize(name);
    match season {
        Some(season) => format!("{parent_dir}/Season {season:02}/{extra_type}/{name}.{ext}"),
        None => format!("{parent_dir}/{extra_type}/{name}.{ext}"),
    }
}

fn with_year(name: &str, year: Option<&str>) -> String {
    match year {
        Some(year) => format!("{name} ({year})"),
        None => name.to_string(),
    }
}

/// Hand-typed titles never exactly match TMDB's (punctuation, "&" vs "and",
/// typos), so exact comparison would warn on every row. Below this
/// Jaro-Winkler similarity, though, the titles are probably different
/// episodes/films rather than different spellings.
const TITLE_SIMILARITY_THRESHOLD: f64 = 0.7;

/// Some(best score) when even the closest candidate falls below the
/// threshold — i.e. a probable misidentification.
fn title_mismatch(expected: &str, candidates: &[&str]) -> Option<f64> {
    let expected = expected.to_lowercase();
    let best = candidates
        .iter()
        .map(|candidate| strsim::jaro_winkler(&expected, &candidate.to_lowercase()))
        .fold(0.0_f64, f64::max);
    (best < TITLE_SIMILARITY_THRESHOLD).then_some(best)
}

/// Strip characters that are illegal or troublesome in filenames.
fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn extension_of(source: &str) -> &str {
    Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mkv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tv_path_matches_library_convention() {
        assert_eq!(
            tv_path(
                "The Owl House",
                Some("2020"),
                1,
                1,
                None,
                "A Lying Witch and a Warden",
                "mkv"
            ),
            "The Owl House (2020)/Season 01/The Owl House - s01e01 - A Lying Witch and a Warden.mkv"
        );
    }

    #[test]
    fn tv_path_without_year_omits_parens() {
        assert_eq!(
            tv_path("Some Show", None, 2, 10, None, "Title", "mkv"),
            "Some Show/Season 02/Some Show - s02e10 - Title.mkv"
        );
    }

    #[test]
    fn tv_path_episode_range() {
        assert_eq!(
            tv_path(
                "Some Show",
                Some("1999"),
                1,
                1,
                Some(2),
                "Part One & Part Two",
                "mkv"
            ),
            "Some Show (1999)/Season 01/Some Show - s01e01-e02 - Part One & Part Two.mkv"
        );
    }

    #[test]
    fn movie_path_matches_library_convention() {
        assert_eq!(
            movie_path("Airplane!", Some("1980"), "mkv"),
            "Airplane! (1980)/Airplane! (1980).mkv"
        );
    }

    #[test]
    fn roots_prefix_targets() {
        assert_eq!(
            under(&Some("/media/tv".to_string()), "A (2020)/a.mkv".to_string()),
            "/media/tv/A (2020)/a.mkv"
        );
        assert_eq!(under(&None, "A (2020)/a.mkv".to_string()), "A (2020)/a.mkv");

        assert_eq!(
            clean_root("/media/tv/".to_string()),
            Some("/media/tv".to_string())
        );
        assert_eq!(clean_root("  ".to_string()), None);
    }

    #[test]
    fn extra_paths_land_in_jellyfin_folders() {
        assert_eq!(
            extra_path(
                "Ghosts of the Abyss (2003)",
                None,
                "featurettes",
                "Echos in Time",
                "mkv"
            ),
            "Ghosts of the Abyss (2003)/featurettes/Echos in Time.mkv"
        );
        assert_eq!(
            extra_path("Some Show (1999)", Some(2), "extras", "Gag Reel", "mkv"),
            "Some Show (1999)/Season 02/extras/Gag Reel.mkv"
        );
    }

    #[test]
    fn title_mismatch_tolerates_spelling_but_catches_wrong_episodes() {
        // Case and small punctuation differences pass.
        assert_eq!(
            title_mismatch("witches before wizards", &["Witches Before Wizards"]),
            None
        );
        assert_eq!(title_mismatch("Part 1", &["Part One"]), None);
        // A range matches if any single episode's title matches.
        assert_eq!(
            title_mismatch("Part One", &["Part One", "Part Two", "Part One & Part Two"]),
            None
        );
        // A genuinely different title is flagged.
        let score = title_mismatch("Witches Before Wizards", &["I Was a Teenage Abomination"]);
        assert!(score.is_some_and(|s| s < TITLE_SIMILARITY_THRESHOLD));
    }

    #[test]
    fn sanitize_strips_illegal_chars() {
        assert_eq!(sanitize("What / Why: A \"Story\"?"), "What  Why A Story");
        assert_eq!(sanitize("Airplane!"), "Airplane!");
    }

    #[test]
    fn read_parses_pipe_rows() {
        let plan = "a.mkv | A (2020)/A.mkv | 600\n\
                    b with space.mkv | B (1999)/B.mkv\n";
        let entries = read_reader(plan.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].plan.new, "A (2020)/A.mkv");
        assert_eq!(entries[0].plan.expected_duration_secs, Some(600));
        assert_eq!(entries[1].plan.old, "b with space.mkv");
        assert_eq!(entries[1].plan.expected_duration_secs, None);
    }

    #[test]
    fn read_parses_plan_rows() {
        let plan = "# comment\n\
                    a.mkv\tA (2020)/A.mkv\t600\n\
                    b.mkv\tB (1999)/B.mkv\t\n\
                    c.mkv\tC (2001)/C.mkv\n";
        let entries = read_reader(plan.as_bytes()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].line, 2);
        assert_eq!(entries[0].plan.old, "a.mkv");
        assert_eq!(entries[0].plan.expected_duration_secs, Some(600));
        assert_eq!(entries[1].plan.expected_duration_secs, None);
        assert_eq!(entries[2].plan.expected_duration_secs, None);
    }

    #[test]
    fn apply_renames_and_creates_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("title 01.mkv"), b"video").unwrap();

        let entries = [PlanEntry {
            line: 1,
            plan: RenamePlan {
                old: "title 01.mkv".to_string(),
                new: "Show (2020)/Season 01/Show - s01e01 - Pilot.mkv".to_string(),
                expected_duration_secs: None,
            },
        }];
        apply(&entries, dir.path()).unwrap();

        assert!(!dir.path().join("title 01.mkv").exists());
        let target = dir
            .path()
            .join("Show (2020)/Season 01/Show - s01e01 - Pilot.mkv");
        assert_eq!(std::fs::read(target).unwrap(), b"video");
    }

    #[test]
    fn copy_then_remove_moves_content_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src.mkv");
        let target = dir.path().join("dst.mkv");
        std::fs::write(&source, b"payload").unwrap();

        copy_then_remove(&source, &target).unwrap();

        assert!(!source.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        assert!(!dir.path().join("dst.mkv.lyrebird-partial").exists());
    }

    #[test]
    fn apply_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mkv"), b"source").unwrap();
        std::fs::write(dir.path().join("taken.mkv"), b"precious").unwrap();

        let entries = [PlanEntry {
            line: 1,
            plan: RenamePlan {
                old: "a.mkv".to_string(),
                new: "taken.mkv".to_string(),
                expected_duration_secs: None,
            },
        }];
        let err = apply(&entries, dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("appeared since validation"));
        assert_eq!(std::fs::read(dir.path().join("a.mkv")).unwrap(), b"source");
        assert_eq!(
            std::fs::read(dir.path().join("taken.mkv")).unwrap(),
            b"precious"
        );
    }

    #[test]
    fn write_then_read_roundtrips_with_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("renames.txt");
        let plans = [RenamePlan {
            old: "a.mkv".to_string(),
            new: "A (2020)/A.mkv".to_string(),
            expected_duration_secs: Some(600),
        }];
        write(&plans, &path).unwrap();

        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .starts_with("#lyrebird:renames\n"));
        let entries = read(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].plan.new, "A (2020)/A.mkv");
    }

    #[test]
    fn read_rejects_wrong_file_kind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.txt");
        std::fs::write(&path, "#lyrebird:manifest\na.mkv\ttv\t1\t1\t1\n").unwrap();

        let err = read(&path).unwrap_err();
        assert!(format!("{err:#}").contains("run lyrebird resolve on it first"));
    }

    #[test]
    fn read_rejects_missing_target() {
        let err = read_reader("only_one_column.mkv\n".as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("missing new path"));
    }

    #[test]
    fn extension_follows_source() {
        assert_eq!(extension_of("title_01.mp4"), "mp4");
        assert_eq!(extension_of("title_01"), "mkv");
    }
}
