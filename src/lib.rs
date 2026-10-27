//! OMDb (Open Movie Database, <https://www.omdbapi.com/>) metadata provider
//! extension for LisekMovie.
//!
//! OMDb serves IMDb's data (plot, genres, runtime, IMDb rating, US
//! certification, poster) keyed by IMDb ids. A free key (1,000 requests a
//! day) is requested at <https://www.omdbapi.com/apikey.aspx> and entered in
//! Admin → Plugins.
//!
//! - search: `GET /?apikey=…&s={title}&type=movie|series[&y=year]`
//! - details: `GET /?apikey=…&i={imdb id}&plot=full`
//!
//! OMDb reports errors in the body (`"Response": "False", "Error": "…"`),
//! sometimes with a 200 status: "Movie not found!" is no match, "Invalid
//! API key!" is [`ProviderError::Unauthorized`].

use std::time::Duration;

use async_trait::async_trait;
use lisek_plugin_api::{
    DirectId, MediaKind, MetadataProvider, MovieQuery, ProviderCandidate, ProviderError,
    ProviderMatch, SeriesQuery,
};
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://www.omdbapi.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawSearchItem {
    title: String,
    year: String,
    #[serde(rename = "imdbID")]
    imdb_id: String,
    poster: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSearch {
    #[serde(rename = "Search", default)]
    search: Vec<RawSearchItem>,
}

/// One title's details (`?i=`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase", default)]
pub struct OmdbTitle {
    pub title: String,
    pub year: String,
    pub rated: String,
    pub runtime: String,
    pub genre: String,
    pub plot: String,
    pub poster: String,
    pub language: String,
    pub production: String,
    #[serde(rename = "imdbRating")]
    pub imdb_rating: String,
    #[serde(rename = "imdbID")]
    pub imdb_id: String,
    #[serde(rename = "Type")]
    pub kind: String,
}

/// OMDb writes "N/A" for every unknown value.
fn known(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value != "N/A").then_some(value)
}

/// `"1999"`, `"2008–2013"`, `"2019–"` -> the first year.
pub fn first_year(value: &str) -> Option<i32> {
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

/// `"136 min"` -> 136.
fn minutes(value: &str) -> Option<u32> {
    known(value)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|m| *m > 0)
}

/// The minimum age of a US certification (MPA film ratings and TV
/// Parental Guidelines).
pub fn age_from_rating(rating: &str) -> Option<i32> {
    Some(match rating.trim() {
        "G" | "TV-Y" | "TV-G" | "Approved" | "Passed" => 0,
        "TV-Y7" | "TV-Y7-FV" => 7,
        "PG" | "TV-PG" => 10,
        "PG-13" => 13,
        "TV-14" => 14,
        "R" | "TV-MA" => 17,
        "NC-17" | "X" => 18,
        _ => return None,
    })
}

fn list(value: &str) -> Vec<String> {
    known(value)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl OmdbTitle {
    fn fields(&self, with_title: bool) -> serde_json::Value {
        let mut fields = serde_json::json!({
            "imdb_id": self.imdb_id,
            "omdb_id": self.imdb_id,
            "overview": known(&self.plot).unwrap_or_default(),
            "genres": list(&self.genre),
            "studios": list(&self.production),
        });
        if let Some(minutes) = minutes(&self.runtime) {
            fields["runtime_minutes"] = serde_json::json!(minutes);
        }
        if let Some(rating) = known(&self.imdb_rating).and_then(|r| r.parse::<f64>().ok()) {
            fields["rating"] = serde_json::json!(rating);
        }
        if let Some(rated) = known(&self.rated)
            && let Some(age) = age_from_rating(rated)
        {
            fields["age_rating"] = serde_json::json!(age);
            fields["certification"] = serde_json::json!(rated);
            fields["certification_country"] = serde_json::json!("US");
        }
        if with_title {
            fields["title"] = serde_json::json!(self.title.trim());
            fields["year"] = serde_json::json!(first_year(&self.year));
        }
        fields
    }

    fn to_match(&self, with_title: bool) -> ProviderMatch {
        ProviderMatch {
            fields: self.fields(with_title),
            poster_url: known(&self.poster).map(str::to_string),
            ..ProviderMatch::default()
        }
    }
}

fn omdb_type(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Series => "series",
    }
}

/// HTTP client of the OMDb API. `base_url` is injectable for tests.
pub struct OmdbClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OmdbClient {
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }

    /// One request; `Ok(None)` for OMDb's "not found" answers.
    async fn get(
        &self,
        params: &[(&str, &str)],
    ) -> Result<Option<serde_json::Value>, ProviderError> {
        let mut query = vec![("apikey", self.api_key.as_str())];
        query.extend_from_slice(params);
        let response = self
            .http
            .get(format!("{}/", self.base_url.trim_end_matches('/')))
            .query(&query)
            .send()
            .await
            .map_err(|err| ProviderError::Request(err.to_string()))?;
        let status = response.status();
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| ProviderError::UnexpectedResponse(err.to_string()))?;
        if body.get("Response").and_then(|r| r.as_str()) == Some("False") {
            let error = body
                .get("Error")
                .and_then(|e| e.as_str())
                .unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED || error.contains("API key") {
                return Err(ProviderError::Unauthorized);
            }
            if error.contains("not found") || error.contains("Incorrect IMDb ID") {
                return Ok(None);
            }
            return Err(ProviderError::UnexpectedResponse(error.to_string()));
        }
        if !status.is_success() {
            return Err(ProviderError::Request(format!("HTTP {status}")));
        }
        Ok(Some(body))
    }

    /// Search results (`imdb id`, title, year, poster), best first.
    pub async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Vec<ProviderCandidate>, ProviderError> {
        let year = year.map(|y| y.to_string());
        let mut params = vec![("s", title), ("type", omdb_type(kind))];
        if let Some(year) = year.as_deref() {
            params.push(("y", year));
        }
        let Some(body) = self.get(&params).await? else {
            return Ok(Vec::new());
        };
        let found: RawSearch = serde_json::from_value(body)
            .map_err(|err| ProviderError::UnexpectedResponse(err.to_string()))?;
        Ok(found
            .search
            .into_iter()
            .map(|item| ProviderCandidate {
                provider_id: item.imdb_id,
                title: item.title,
                year: first_year(&item.year),
                overview: String::new(),
                poster_url: item.poster.as_deref().and_then(known).map(str::to_string),
            })
            .collect())
    }

    /// One title by its IMDb id.
    pub async fn title(&self, imdb_id: &str) -> Result<Option<OmdbTitle>, ProviderError> {
        let Some(body) = self.get(&[("i", imdb_id), ("plot", "full")]).await? else {
            return Ok(None);
        };
        serde_json::from_value(body)
            .map(Some)
            .map_err(|err| ProviderError::UnexpectedResponse(err.to_string()))
    }
}

fn is_imdb_id(id: &str) -> bool {
    id.len() > 2 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
}

/// The [`MetadataProvider`] for `"omdb"`; ids are IMDb ids.
pub struct OmdbProvider {
    api_key: String,
    base_url: String,
}

impl OmdbProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }

    fn client(&self) -> OmdbClient {
        OmdbClient::with_base_url(self.api_key.clone(), self.base_url.clone())
    }

    /// The first search result's details (the scanner's automatic match).
    async fn find(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        let client = self.client();
        let mut found = client.search(kind, title, year).await?;
        if found.is_empty() && year.is_some() {
            found = client.search(kind, title, None).await?;
        }
        let Some(first) = found.into_iter().next() else {
            return Ok(None);
        };
        Ok(client
            .title(&first.provider_id)
            .await?
            .map(|t| t.to_match(false)))
    }
}

#[async_trait]
impl MetadataProvider for OmdbProvider {
    fn provider_key(&self) -> &'static str {
        "omdb"
    }

    async fn find_movie(
        &self,
        query: &MovieQuery<'_>,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        self.find(MediaKind::Movie, query.title, query.year).await
    }

    async fn find_series(
        &self,
        query: &SeriesQuery<'_>,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        self.find(MediaKind::Series, query.title, None).await
    }

    async fn search(
        &self,
        kind: MediaKind,
        title: &str,
        year: Option<i32>,
        _lang: &str,
    ) -> Result<Vec<ProviderCandidate>, ProviderError> {
        Ok(self
            .client()
            .search(kind, title, year)
            .await?
            .into_iter()
            .take(10)
            .collect())
    }

    async fn lookup(
        &self,
        _kind: MediaKind,
        provider_id: &str,
        _lang: &str,
    ) -> Result<Option<ProviderMatch>, ProviderError> {
        let id = provider_id.trim().to_ascii_lowercase();
        if !is_imdb_id(&id) {
            return Ok(None);
        }
        Ok(self.client().title(&id).await?.map(|t| t.to_match(true)))
    }

    async fn lookup_by_id(
        &self,
        kind: MediaKind,
        id: &DirectId,
        lang: &str,
    ) -> Result<Option<ProviderCandidate>, ProviderError> {
        let DirectId::Imdb(imdb) = id else {
            return Ok(None);
        };
        Ok(self
            .lookup(kind, imdb, lang)
            .await?
            .and_then(|matched| ProviderCandidate::from_match(imdb, &matched)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn matrix() -> serde_json::Value {
        serde_json::json!({
            "Title": "The Matrix", "Year": "1999", "Rated": "R", "Runtime": "136 min",
            "Genre": "Action, Sci-Fi", "Plot": "Neo learns the truth.", "Production": "N/A",
            "Poster": "https://m.media-amazon.com/images/M/m.jpg", "imdbRating": "8.7",
            "imdbID": "tt0133093", "Type": "movie", "Response": "True"
        })
    }

    #[test]
    fn values_parse_and_na_means_unknown() {
        assert_eq!(first_year("2008–2013"), Some(2008));
        assert_eq!(first_year("N/A"), None);
        assert_eq!(minutes("136 min"), Some(136));
        assert_eq!(minutes("N/A"), None);
        assert_eq!(age_from_rating("PG-13"), Some(13));
        assert_eq!(age_from_rating("TV-MA"), Some(17));
        assert_eq!(age_from_rating("Unrated"), None);
        assert!(list("N/A").is_empty());
    }

    #[tokio::test]
    async fn finds_a_movie_and_maps_the_details() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("s", "The Matrix"))
            .and(query_param("type", "movie"))
            .and(query_param("apikey", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Search": [{ "Title": "The Matrix", "Year": "1999", "imdbID": "tt0133093", "Type": "movie", "Poster": "N/A" }],
                "totalResults": "1", "Response": "True"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("i", "tt0133093"))
            .respond_with(ResponseTemplate::new(200).set_body_json(matrix()))
            .mount(&server)
            .await;
        let provider = OmdbProvider::with_base_url("k", server.uri());

        let matched = provider
            .find_movie(
                &MovieQuery {
                    title: "The Matrix",
                    year: None,
                },
                "pl",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(matched.fields["imdb_id"], "tt0133093");
        assert_eq!(matched.fields["runtime_minutes"], 136);
        assert_eq!(matched.fields["rating"], 8.7);
        assert_eq!(matched.fields["age_rating"], 17);
        assert_eq!(
            matched.fields["genres"],
            serde_json::json!(["Action", "Sci-Fi"])
        );
        assert!(matched.fields.get("title").is_none());
        assert_eq!(
            matched.poster_url.as_deref(),
            Some("https://m.media-amazon.com/images/M/m.jpg")
        );

        let found = provider
            .search(MediaKind::Movie, "The Matrix", None, "pl")
            .await
            .unwrap();
        assert_eq!(
            (
                found[0].provider_id.as_str(),
                found[0].poster_url.as_deref()
            ),
            ("tt0133093", None)
        );

        let candidate = provider
            .lookup_by_id(MediaKind::Movie, &DirectId::Imdb("tt0133093".into()), "pl")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (candidate.title.as_str(), candidate.year),
            ("The Matrix", Some(1999))
        );
        assert_eq!(candidate.overview, "Neo learns the truth.");
    }

    #[tokio::test]
    async fn errors_in_the_body_are_mapped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("apikey", "bad"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "Response": "False", "Error": "Invalid API key!"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(query_param("apikey", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Response": "False", "Error": "Movie not found!"
            })))
            .mount(&server)
            .await;
        let bad = OmdbProvider::with_base_url("bad", server.uri());
        assert!(matches!(
            bad.search(MediaKind::Movie, "x", None, "pl").await,
            Err(ProviderError::Unauthorized)
        ));
        let good = OmdbProvider::with_base_url("k", server.uri());
        assert!(
            good.search(MediaKind::Movie, "x", None, "pl")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            good.lookup(MediaKind::Movie, "tt0000001", "pl")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            good.lookup(MediaKind::Movie, "603", "pl")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn provider_key_is_omdb() {
        assert_eq!(OmdbProvider::new("k").provider_key(), "omdb");
    }
}
