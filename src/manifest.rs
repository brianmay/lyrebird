//! Parsing of the TMDB-based manifest: one tab-separated row per ripped file.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{bail, Context, Result};

/// One row of the manifest. The `kind` column determines the variant.
#[derive(Debug, Clone)]
pub enum ManifestRow {
    Tv {
        source: String,
        series_id: u32,
        season: u32,
        episode: u32,
        /// Set when the rip contains a run of episodes (`3-4` in the episode
        /// column): the inclusive end of the range.
        episode_end: Option<u32>,
        /// Title the user expects (typed from the disc/box); resolve warns if
        /// TMDB's title is wildly different — catches wrong IDs and
        /// disc-vs-broadcast ordering that the duration check can't.
        expected_title: Option<String>,
    },
    Movie {
        source: String,
        movie_id: u32,
        expected_title: Option<String>,
    },
    /// A movie's special feature: lands in a Jellyfin extras subfolder
    /// (`Title (Year)/<extra_type>/<name>.ext`) so it attaches to the movie.
    MovieExtra {
        source: String,
        movie_id: u32,
        extra_type: String,
        name: String,
        expected_duration: Option<u64>,
    },
    /// A show's special feature: series-wide without a season
    /// (`Series (Year)/<extra_type>/...`), season-specific with one
    /// (`Series (Year)/Season SS/<extra_type>/...`).
    TvExtra {
        source: String,
        series_id: u32,
        season: Option<u32>,
        extra_type: String,
        name: String,
        expected_duration: Option<u64>,
    },
    /// Content not on TMDB (specials, extras) — target path supplied directly.
    Manual {
        source: String,
        new_name: String,
        expected_duration: Option<u64>,
    },
}

/// Subfolder names Jellyfin recognizes as extras of the containing
/// movie/series/season. A typo here would silently detach the extra, so the
/// type is validated at parse time.
pub const EXTRA_TYPES: &[&str] = &[
    "extras",
    "featurettes",
    "trailers",
    "behind the scenes",
    "deleted scenes",
    "interviews",
    "scenes",
    "shorts",
    "clips",
    "other",
];

/// First-line markers distinguishing the two tab-separated file kinds, so a
/// manifest can never be fed to validate/apply or a rename plan to resolve.
/// Comment-prefixed, so the row parsers skip them like any other comment.
pub const MANIFEST_MARKER: &str = "#lyrebird:manifest";
pub const RENAMES_MARKER: &str = "#lyrebird:renames";

pub fn expect_marker(content: &str, expected: &str) -> Result<()> {
    let first = content.lines().next().map(str::trim).unwrap_or("");
    if first == expected {
        return Ok(());
    }
    match first {
        RENAMES_MARKER => bail!(
            "this file is a rename plan (first line {RENAMES_MARKER}) — \
             resolve takes a manifest; run validate or apply on this file instead"
        ),
        MANIFEST_MARKER => bail!(
            "this file is a manifest (first line {MANIFEST_MARKER}) — \
             run lyrebird resolve on it first to produce a rename plan"
        ),
        _ => bail!(
            "first line must be {expected} — regenerate the file with lyrebird, \
             or add that line if it was written by hand"
        ),
    }
}

pub fn parse(path: &Path) -> Result<Vec<ManifestRow>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("could not open manifest {}", path.display()))?;
    expect_marker(&content, MANIFEST_MARKER)
        .with_context(|| format!("in manifest {}", path.display()))?;
    parse_reader(content.as_bytes()).with_context(|| format!("in manifest {}", path.display()))
}

pub fn parse_reader<R: Read>(reader: R) -> Result<Vec<ManifestRow>> {
    let mut rows = Vec::new();
    for (line, fields) in tsv_lines(reader)? {
        rows.push(parse_record(&fields).with_context(|| format!("manifest line {line}"))?);
    }
    Ok(rows)
}

/// Splits input into (line number, tab-separated fields), skipping blank
/// lines and `#` comments. Shared by the manifest and plan readers; line
/// numbers are 1-based positions in the file, comments included.
pub fn tsv_lines<R: Read>(reader: R) -> Result<Vec<(u64, Vec<String>)>> {
    let mut lines = Vec::new();
    for (idx, line) in BufReader::new(reader).lines().enumerate() {
        let line = line.context("could not read line")?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').map(str::to_string).collect();
        lines.push((idx as u64 + 1, fields));
    }
    Ok(lines)
}

fn parse_record(fields: &[String]) -> Result<ManifestRow> {
    let source = field(fields, 0, "source")?.to_string();
    let kind = field(fields, 1, "kind")?;

    match kind {
        "tv" => {
            let (episode, episode_end) = parse_episode_range(field(fields, 4, "episode")?)?;
            Ok(ManifestRow::Tv {
                source,
                series_id: parse_field(fields, 2, "tmdb_series_id")?,
                season: parse_field(fields, 3, "season")?,
                episode,
                episode_end,
                expected_title: optional_field(fields, 5),
            })
        }
        "movie" => Ok(ManifestRow::Movie {
            source,
            movie_id: parse_field(fields, 2, "tmdb_movie_id")?,
            expected_title: optional_field(fields, 3),
        }),
        "movie-extra" => Ok(ManifestRow::MovieExtra {
            source,
            movie_id: parse_field(fields, 2, "tmdb_movie_id")?,
            extra_type: parse_extra_type(field(fields, 3, "extra_type")?)?,
            name: field(fields, 4, "name")?.to_string(),
            expected_duration: optional_duration(fields, 5)?,
        }),
        "tv-extra" => {
            let series_id = parse_field(fields, 2, "tmdb_series_id")?;
            // The season column is optional; a number there is a season,
            // anything else is the extra type (which is never numeric).
            let after_id = field(fields, 3, "season or extra_type")?;
            let (season, type_idx) = if after_id.chars().all(|c| c.is_ascii_digit()) {
                (Some(parse_field(fields, 3, "season")?), 4)
            } else {
                (None, 3)
            };
            Ok(ManifestRow::TvExtra {
                source,
                series_id,
                season,
                extra_type: parse_extra_type(field(fields, type_idx, "extra_type")?)?,
                name: field(fields, type_idx + 1, "name")?.to_string(),
                expected_duration: optional_duration(fields, type_idx + 2)?,
            })
        }
        "manual" => Ok(ManifestRow::Manual {
            source,
            new_name: field(fields, 2, "new_name")?.to_string(),
            expected_duration: optional_duration(fields, 3)?,
        }),
        other => bail!(
            "unknown row kind '{other}' (expected tv, movie, movie-extra, tv-extra, or manual)"
        ),
    }
}

fn parse_extra_type(s: &str) -> Result<String> {
    let extra_type = s.trim().to_lowercase();
    if EXTRA_TYPES.contains(&extra_type.as_str()) {
        Ok(extra_type)
    } else {
        bail!(
            "unknown extra type '{s}' — Jellyfin folder names are: {}",
            EXTRA_TYPES.join(", ")
        )
    }
}

fn optional_duration(fields: &[String], idx: usize) -> Result<Option<u64>> {
    match fields.get(idx).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(s) => {
            Ok(Some(s.parse().with_context(|| {
                format!("invalid expected_duration_secs '{s}'")
            })?))
        }
        None => Ok(None),
    }
}

/// `3` -> (3, None); `3-4` -> (3, Some(4)) for a rip containing several
/// episodes in one file.
fn parse_episode_range(s: &str) -> Result<(u32, Option<u32>)> {
    let Some((start, end)) = s.split_once('-') else {
        let episode = s
            .parse()
            .with_context(|| format!("invalid episode '{s}'"))?;
        return Ok((episode, None));
    };
    let parse = |part: &str| {
        part.trim()
            .parse::<u32>()
            .with_context(|| format!("invalid episode range '{s}'"))
    };
    let (start, end) = (parse(start)?, parse(end)?);
    if end <= start {
        bail!("invalid episode range '{s}' (end must be greater than start)");
    }
    Ok((start, Some(end)))
}

/// Writes a manifest template for `files`, ready to hand-edit and pass to
/// `resolve`. Refuses to overwrite so it can never eat an edited manifest.
pub fn template(files: &[std::path::PathBuf], output: &Path) -> Result<()> {
    if output.exists() {
        bail!(
            "{} already exists — refusing to overwrite (delete it first if you really want a fresh template)",
            output.display()
        );
    }

    let files: Vec<(String, Option<f64>)> = files
        .iter()
        .map(|f| {
            (
                f.display().to_string(),
                crate::ffprobe::duration_secs(f).ok(),
            )
        })
        .collect();
    std::fs::write(output, template_body(&files))
        .with_context(|| format!("could not write {}", output.display()))
}

fn template_body(files: &[(String, Option<f64>)]) -> String {
    let mut out = String::from(MANIFEST_MARKER);
    out.push_str(
        "\n# lyrebird manifest — edit each line, then run: lyrebird resolve <this file>\n\
         #\n\
         # Row kinds (TAB-separated columns):\n\
         #   <file>  tv           <tmdb_series_id>  <season>  <episode | ep1-ep2>  [expected title]\n\
         #   <file>  movie        <tmdb_movie_id>  [expected title]\n\
         #   <file>  movie-extra  <tmdb_movie_id>  <extra type>  <name>  [expected duration secs]\n\
         #   <file>  tv-extra     <tmdb_series_id>  [season]  <extra type>  <name>  [expected duration secs]\n\
         #   <file>  manual       <new name>  [expected duration secs]\n\
         #\n\
         # Extra types (Jellyfin folder names): extras, featurettes, trailers,\n\
         # behind the scenes, deleted scenes, interviews, scenes, shorts, clips, other.\n\
         #\n\
         # Find TMDB ids at https://www.themoviedb.org — the number in the title's URL.\n\
         # Rows are pre-filled as tv season 1 with episodes in file order; SERIES_ID\n\
         # will not resolve until replaced.\n",
    );

    for (episode, (file, duration)) in files.iter().enumerate() {
        let duration = match duration {
            Some(secs) => format!(
                "{}m{:02}s ({secs:.0}s)",
                (secs / 60.0) as u64,
                (secs % 60.0) as u64
            ),
            None => "unavailable".to_string(),
        };
        out.push_str(&format!(
            "\n# duration: {duration}\n{file}\ttv\tSERIES_ID\t1\t{}\n",
            episode + 1
        ));
    }
    out
}

fn optional_field(fields: &[String], idx: usize) -> Option<String> {
    fields
        .get(idx)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn field<'r>(fields: &'r [String], idx: usize, name: &str) -> Result<&'r str> {
    match fields.get(idx).map(|s| s.trim()) {
        Some(s) if !s.is_empty() => Ok(s),
        _ => bail!("missing {name} column"),
    }
}

fn parse_field<T>(fields: &[String], idx: usize, name: &str) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let s = field(fields, idx, name)?;
    s.parse().with_context(|| format!("invalid {name} '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_row_kinds() {
        let manifest = "# a comment\n\
                        title_01.mkv\ttv\t84958\t1\t1\n\
                        \n\
                        special_01.mkv\tmanual\tShow.S00E01.Behind.The.Scenes.mkv\t600\n\
                        extra.mkv\tmanual\tExtra.mkv\n\
                        movie_rip.mkv\tmovie\t603\n";
        let rows = parse_reader(manifest.as_bytes()).unwrap();
        assert_eq!(rows.len(), 4);

        match &rows[0] {
            ManifestRow::Tv {
                source,
                series_id,
                season,
                episode,
                episode_end,
                expected_title,
            } => {
                assert_eq!(source, "title_01.mkv");
                assert_eq!((*series_id, *season, *episode), (84958, 1, 1));
                assert_eq!(*episode_end, None);
                assert_eq!(*expected_title, None);
            }
            other => panic!("expected tv row, got {other:?}"),
        }
        match &rows[1] {
            ManifestRow::Manual {
                expected_duration, ..
            } => {
                assert_eq!(*expected_duration, Some(600));
            }
            other => panic!("expected manual row, got {other:?}"),
        }
        match &rows[2] {
            ManifestRow::Manual {
                expected_duration, ..
            } => {
                assert_eq!(*expected_duration, None);
            }
            other => panic!("expected manual row, got {other:?}"),
        }
        match &rows[3] {
            ManifestRow::Movie { movie_id, .. } => assert_eq!(*movie_id, 603),
            other => panic!("expected movie row, got {other:?}"),
        }
    }

    #[test]
    fn parses_expected_titles() {
        let manifest = "a.mkv\ttv\t84958\t1\t2\tWitches Before Wizards\n\
                        b.mkv\tmovie\t603\tThe Matrix\n";
        let rows = parse_reader(manifest.as_bytes()).unwrap();
        match &rows[0] {
            ManifestRow::Tv { expected_title, .. } => {
                assert_eq!(expected_title.as_deref(), Some("Witches Before Wizards"));
            }
            other => panic!("expected tv row, got {other:?}"),
        }
        match &rows[1] {
            ManifestRow::Movie { expected_title, .. } => {
                assert_eq!(expected_title.as_deref(), Some("The Matrix"));
            }
            other => panic!("expected movie row, got {other:?}"),
        }
    }

    #[test]
    fn parses_extra_rows() {
        let manifest = "d2t4.mkv\tmovie-extra\t161795\tfeaturettes\tEchos in Time\t600\n\
                        d2t11.mkv\tmovie-extra\t161795\tTrailers\tTrailer\n\
                        s1.mkv\ttv-extra\t84958\tbehind the scenes\tMaking Of\n\
                        s2.mkv\ttv-extra\t84958\t2\textras\tSeason Two Gag Reel\t300\n";
        let rows = parse_reader(manifest.as_bytes()).unwrap();

        match &rows[0] {
            ManifestRow::MovieExtra {
                movie_id,
                extra_type,
                name,
                expected_duration,
                ..
            } => {
                assert_eq!(*movie_id, 161795);
                assert_eq!(extra_type, "featurettes");
                assert_eq!(name, "Echos in Time");
                assert_eq!(*expected_duration, Some(600));
            }
            other => panic!("expected movie-extra row, got {other:?}"),
        }
        match &rows[1] {
            // Extra types are normalized to Jellyfin's lowercase folder names.
            ManifestRow::MovieExtra { extra_type, .. } => assert_eq!(extra_type, "trailers"),
            other => panic!("expected movie-extra row, got {other:?}"),
        }
        match &rows[2] {
            ManifestRow::TvExtra {
                season, extra_type, ..
            } => {
                assert_eq!(*season, None);
                assert_eq!(extra_type, "behind the scenes");
            }
            other => panic!("expected tv-extra row, got {other:?}"),
        }
        match &rows[3] {
            ManifestRow::TvExtra {
                season,
                extra_type,
                name,
                expected_duration,
                ..
            } => {
                assert_eq!(*season, Some(2));
                assert_eq!(extra_type, "extras");
                assert_eq!(name, "Season Two Gag Reel");
                assert_eq!(*expected_duration, Some(300));
            }
            other => panic!("expected tv-extra row, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_extra_type() {
        let err =
            parse_reader("x.mkv\tmovie-extra\t603\tfeaturete\tOops\n".as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("unknown extra type 'featurete'"));
    }

    #[test]
    fn parses_episode_ranges() {
        let rows = parse_reader("double.mkv\ttv\t84958\t1\t3-4\n".as_bytes()).unwrap();
        match &rows[0] {
            ManifestRow::Tv {
                episode,
                episode_end,
                ..
            } => {
                assert_eq!(*episode, 3);
                assert_eq!(*episode_end, Some(4));
            }
            other => panic!("expected tv row, got {other:?}"),
        }
    }

    #[test]
    fn rejects_backwards_episode_range() {
        let err = parse_reader("x.mkv\ttv\t84958\t1\t4-3\n".as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("end must be greater than start"));

        let err = parse_reader("x.mkv\ttv\t84958\t1\t3-3\n".as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("end must be greater than start"));
    }

    #[test]
    fn markers_distinguish_the_two_file_kinds() {
        assert!(expect_marker("#lyrebird:manifest\na\tb\n", MANIFEST_MARKER).is_ok());
        assert!(expect_marker("#lyrebird:renames\na\tb\n", RENAMES_MARKER).is_ok());

        let err = expect_marker("#lyrebird:renames\n", MANIFEST_MARKER).unwrap_err();
        assert!(format!("{err:#}").contains("run validate or apply on this file instead"));

        let err = expect_marker("#lyrebird:manifest\n", RENAMES_MARKER).unwrap_err();
        assert!(format!("{err:#}").contains("run lyrebird resolve on it first"));

        let err = expect_marker("a.mkv\ttv\t1\t1\t1\n", MANIFEST_MARKER).unwrap_err();
        assert!(format!("{err:#}").contains("first line must be #lyrebird:manifest"));
    }

    #[test]
    fn template_rows_roundtrip_through_the_parser() {
        let body = template_body(&[
            ("title_01.mkv".to_string(), Some(1471.4)),
            ("title 02.mkv".to_string(), None),
        ]);

        assert!(body.starts_with("#lyrebird:manifest\n"));
        assert!(body.contains("# duration: 24m31s (1471s)\ntitle_01.mkv\ttv\tSERIES_ID\t1\t1\n"));
        assert!(body.contains("# duration: unavailable\ntitle 02.mkv\ttv\tSERIES_ID\t1\t2\n"));

        // The pre-filled rows must parse once SERIES_ID is replaced...
        let edited = body.replace("SERIES_ID", "84958");
        assert_eq!(parse_reader(edited.as_bytes()).unwrap().len(), 2);
        // ...and must NOT parse while the placeholder is still there.
        let err = parse_reader(body.as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("invalid tmdb_series_id 'SERIES_ID'"));
    }

    #[test]
    fn template_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("manifest.txt");
        std::fs::write(&output, "precious hand-edited manifest").unwrap();

        let err = template(&["a.mkv".into()], &output).unwrap_err();
        assert!(format!("{err:#}").contains("refusing to overwrite"));
        assert_eq!(
            std::fs::read_to_string(&output).unwrap(),
            "precious hand-edited manifest"
        );
    }

    #[test]
    fn rejects_unknown_kind() {
        let err = parse_reader("x.mkv\tbogus\t1\n".as_bytes()).unwrap_err();
        assert!(format!("{err:#}").contains("unknown row kind 'bogus'"));
    }

    #[test]
    fn reports_line_number_on_error() {
        let err = parse_reader("a.mkv\ttv\t1\t1\t1\nb.mkv\ttv\tnot_a_number\t1\t1\n".as_bytes())
            .unwrap_err();
        assert!(format!("{err:#}").contains("line 2"));
    }
}
