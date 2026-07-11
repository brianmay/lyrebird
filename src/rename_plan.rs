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

pub fn resolve(rows: &[ManifestRow], tmdb: &Tmdb) -> Result<Vec<RenamePlan>> {
    let mut series_cache: HashMap<u32, crate::tmdb::Series> = HashMap::new();
    let mut plans = Vec::with_capacity(rows.len());

    for row in rows {
        let plan = match row {
            ManifestRow::Tv {
                source,
                series_id,
                season,
                episode,
            } => {
                let series = match series_cache.get(series_id) {
                    Some(series) => series.clone(),
                    None => {
                        let series = tmdb.series(*series_id).with_context(|| {
                            format!("{source}: looking up TMDB series {series_id}")
                        })?;
                        series_cache.insert(*series_id, series.clone());
                        series
                    }
                };
                let ep = tmdb.episode(*series_id, *season, *episode).with_context(|| {
                    format!("{source}: looking up TMDB series {series_id} s{season:02}e{episode:02}")
                })?;
                RenamePlan {
                    old: source.clone(),
                    new: tv_path(
                        &series.name,
                        series.first_air_year(),
                        *season,
                        *episode,
                        &ep.name,
                        extension_of(source),
                    ),
                    expected_duration_secs: ep.runtime.map(|mins| mins * 60),
                }
            }
            ManifestRow::Movie { source, movie_id } => {
                let movie = tmdb
                    .movie(*movie_id)
                    .with_context(|| format!("{source}: looking up TMDB movie {movie_id}"))?;
                RenamePlan {
                    old: source.clone(),
                    new: movie_path(&movie.title, movie.release_year(), extension_of(source)),
                    expected_duration_secs: movie.runtime.map(|mins| mins * 60),
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
    for plan in plans {
        let duration = plan
            .expected_duration_secs
            .map(|d| d.to_string())
            .unwrap_or_default();
        out.push_str(&format!("{}\t{}\t{}\n", plan.old, plan.new, duration));
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
    let file = std::fs::File::open(path)
        .with_context(|| format!("could not open plan {}", path.display()))?;
    read_reader(file).with_context(|| format!("in plan {}", path.display()))
}

fn read_reader<R: std::io::Read>(reader: R) -> Result<Vec<PlanEntry>> {
    let mut entries = Vec::new();
    for (line, fields) in crate::manifest::tsv_lines(reader)? {
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

/// `Series (Year)/Season SS/Series - sSSeEE - Episode Title.ext`
/// (year in the series folder only, not the filename).
fn tv_path(
    series_name: &str,
    year: Option<&str>,
    season: u32,
    episode: u32,
    episode_title: &str,
    ext: &str,
) -> String {
    let series = sanitize(series_name);
    let episode_title = sanitize(episode_title);
    let series_dir = with_year(&series, year);
    format!(
        "{series_dir}/Season {season:02}/{series} - s{season:02}e{episode:02} - {episode_title}.{ext}"
    )
}

/// `Title (Year)/Title (Year).ext` — the movie sits in its own folder.
fn movie_path(title: &str, year: Option<&str>, ext: &str) -> String {
    let name = with_year(&sanitize(title), year);
    format!("{name}/{name}.{ext}")
}

fn with_year(name: &str, year: Option<&str>) -> String {
    match year {
        Some(year) => format!("{name} ({year})"),
        None => name.to_string(),
    }
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
            tv_path("The Owl House", Some("2020"), 1, 1, "A Lying Witch and a Warden", "mkv"),
            "The Owl House (2020)/Season 01/The Owl House - s01e01 - A Lying Witch and a Warden.mkv"
        );
    }

    #[test]
    fn tv_path_without_year_omits_parens() {
        assert_eq!(
            tv_path("Some Show", None, 2, 10, "Title", "mkv"),
            "Some Show/Season 02/Some Show - s02e10 - Title.mkv"
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
    fn sanitize_strips_illegal_chars() {
        assert_eq!(sanitize("What / Why: A \"Story\"?"), "What  Why A Story");
        assert_eq!(sanitize("Airplane!"), "Airplane!");
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
