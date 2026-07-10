//! TMDB v3 API client (blocking). Reads the API key from the TMDB_API_KEY env var.

use anyhow::{Context, Result};
use serde::Deserialize;

const BASE: &str = "https://api.themoviedb.org/3";

pub struct Tmdb {
    client: reqwest::blocking::Client,
    auth: Auth,
}

/// TMDB accepts either credential on the v3 endpoints; which one we hold
/// is detected from its format (the v4 token is a JWT).
enum Auth {
    /// Legacy v3 API key, sent as an `api_key` query parameter.
    V3Key(String),
    /// v4 API Read Access Token, sent as a Bearer header.
    V4Token(String),
}

impl Auth {
    fn detect(credential: String) -> Self {
        if credential.starts_with("eyJ") {
            Auth::V4Token(credential)
        } else {
            Auth::V3Key(credential)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Series {
    pub name: String,
    #[serde(default)]
    first_air_date: Option<String>,
}

impl Series {
    pub fn first_air_year(&self) -> Option<&str> {
        year_of(self.first_air_date.as_deref())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Episode {
    pub name: String,
    /// Minutes, as published by TMDB.
    #[serde(default)]
    pub runtime: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Movie {
    pub title: String,
    #[serde(default)]
    release_date: Option<String>,
    /// Minutes, as published by TMDB.
    #[serde(default)]
    pub runtime: Option<u64>,
}

impl Movie {
    pub fn release_year(&self) -> Option<&str> {
        year_of(self.release_date.as_deref())
    }
}

fn year_of(date: Option<&str>) -> Option<&str> {
    date.and_then(|d| d.get(..4)).filter(|y| !y.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_credential_kind() {
        assert!(matches!(
            Auth::detect("eyJhbGciOiJIUzI1NiJ9.payload.sig".to_string()),
            Auth::V4Token(_)
        ));
        assert!(matches!(
            Auth::detect("0123456789abcdef0123456789abcdef".to_string()),
            Auth::V3Key(_)
        ));
    }
}

impl Tmdb {
    pub fn from_env() -> Result<Self> {
        let credential = std::env::var("TMDB_API_KEY").context(
            "TMDB_API_KEY environment variable not set (either the v3 API key or the \
             v4 API Read Access Token from themoviedb.org works)",
        )?;
        Ok(Self {
            client: reqwest::blocking::Client::new(),
            auth: Auth::detect(credential),
        })
    }

    pub fn series(&self, series_id: u32) -> Result<Series> {
        self.get(&format!("tv/{series_id}"))
    }

    pub fn episode(&self, series_id: u32, season: u32, episode: u32) -> Result<Episode> {
        self.get(&format!("tv/{series_id}/season/{season}/episode/{episode}"))
    }

    pub fn movie(&self, movie_id: u32) -> Result<Movie> {
        self.get(&format!("movie/{movie_id}"))
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        // The credential is attached separately so error messages below can
        // include the URL without leaking it.
        let url = format!("{BASE}/{path}");
        let request = self.client.get(&url);
        let request = match &self.auth {
            Auth::V3Key(key) => request.query(&[("api_key", key)]),
            Auth::V4Token(token) => request.bearer_auth(token),
        };
        request
            .send()
            .with_context(|| format!("request to {url} failed"))?
            .error_for_status()
            .with_context(|| format!("TMDB returned an error for {url}"))?
            .json()
            .with_context(|| format!("could not parse TMDB response from {url}"))
    }
}
