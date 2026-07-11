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
    },
    Movie {
        source: String,
        movie_id: u32,
    },
    /// Content not on TMDB (specials, extras) — target path supplied directly.
    Manual {
        source: String,
        new_name: String,
        expected_duration: Option<u64>,
    },
}

pub fn parse(path: &Path) -> Result<Vec<ManifestRow>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("could not open manifest {}", path.display()))?;
    parse_reader(file).with_context(|| format!("in manifest {}", path.display()))
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
        "tv" => Ok(ManifestRow::Tv {
            source,
            series_id: parse_field(fields, 2, "tmdb_series_id")?,
            season: parse_field(fields, 3, "season")?,
            episode: parse_field(fields, 4, "episode")?,
        }),
        "movie" => Ok(ManifestRow::Movie {
            source,
            movie_id: parse_field(fields, 2, "tmdb_movie_id")?,
        }),
        "manual" => {
            let new_name = field(fields, 2, "new_name")?.to_string();
            let expected_duration = match fields.get(3).map(|s| s.trim()).filter(|s| !s.is_empty())
            {
                Some(s) => Some(
                    s.parse()
                        .with_context(|| format!("invalid expected_duration_secs '{s}'"))?,
                ),
                None => None,
            };
            Ok(ManifestRow::Manual {
                source,
                new_name,
                expected_duration,
            })
        }
        other => bail!("unknown row kind '{other}' (expected tv, movie, or manual)"),
    }
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
            } => {
                assert_eq!(source, "title_01.mkv");
                assert_eq!((*series_id, *season, *episode), (84958, 1, 1));
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
