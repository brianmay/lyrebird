//! Parsing of the TMDB-based manifest: one tab-separated row per ripped file.

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
    /// Content not on TMDB (specials, extras) — filename supplied directly.
    Manual {
        source: String,
        new_name: String,
        expected_duration: Option<u64>,
    },
}
