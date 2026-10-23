//! Apple's catalogue, one of the two places an album is looked for. No key, no signup, roughly 20 calls a
//! minute, which is why the search only runs when you press Enter.
//!
//! https://performance-partners.apple.com/search-api

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::net;
use crate::search::{Kind, SearchResult};

pub fn search(term: &str, limit: usize) -> Result<Vec<SearchResult>> {
    // Apple's catalogue differs slightly by country, and some albums only
    // exist in one of them
    let country = std::env::var("FERRIC_COUNTRY").unwrap_or_else(|_| "US".to_string());

    let url = format!(
        "https://itunes.apple.com/search?term={}&entity=album&limit={}&country={}",
        net::encode(term),
        limit.clamp(1, 200),
        net::encode(&country)
    );

    let body = net::agent()
        .get(&url)
        .call()
        .context("could not reach iTunes")?
        .into_string()
        .context("iTunes sent something unreadable")?;

    parse(&body)
}

/// Kept apart from the request so it can be tested without the network.
pub fn parse(body: &str) -> Result<Vec<SearchResult>> {
    let response: Response = serde_json::from_str(body).context("iTunes sent unexpected JSON")?;

    let results = response
        .results
        .into_iter()
        .filter_map(|entry| {
            // An entry without an artist or a title is no use here
            let artist = entry.artist_name?;
            let title = entry.collection_name?;

            Some(SearchResult {
                artist,
                title,
                year: entry.release_date.as_deref().and_then(year_of),
                cover_url: entry.artwork_url_100.as_deref().map(bigger_cover),
                itunes_id: entry.collection_id,
                deezer_id: None,
                genre: entry.primary_genre_name,
                // Four tracks is where a single or an EP stops and a short
                // album starts. A record Apple gives no count for is taken
                // at its word as an album rather than guessed at
                kind: match entry.track_count {
                    Some(tracks) if tracks < 4 => Kind::Short,
                    _ => Kind::Album,
                },
            })
        })
        .collect();

    Ok(results)
}

/// Release dates arrive as 1997-05-21T07:00:00Z, and only the year matters.
fn year_of(release_date: &str) -> Option<u16> {
    release_date.get(0..4)?.parse().ok()
}

/// Apple hands out a 100 pixel thumbnail, and the same URL with a bigger
/// size in it gives a cover worth looking at.
fn bigger_cover(url: &str) -> String {
    url.replace("100x100bb", "600x600bb")
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    results: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "collectionName")]
    collection_name: Option<String>,
    #[serde(rename = "collectionId")]
    collection_id: Option<u64>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(rename = "artworkUrl100")]
    artwork_url_100: Option<String>,
    #[serde(rename = "primaryGenreName")]
    primary_genre_name: Option<String>,
    // Apple files singles and EPs under `entity=album` with everything
    // else, and the track count is the only thing telling them apart
    #[serde(rename = "trackCount")]
    track_count: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "resultCount": 2,
      "results": [
        {
          "artistName": "Radiohead",
          "collectionName": "OK Computer",
          "collectionId": 1097861387,
          "releaseDate": "1997-05-21T07:00:00Z",
          "artworkUrl100": "https://is1.mzstatic.com/image/thumb/Music/x/100x100bb.jpg",
          "primaryGenreName": "Alternative"
        },
        {
          "collectionName": "An album with no artist",
          "collectionId": 1
        }
      ]
    }"#;

    #[test]
    fn reads_what_it_needs_and_skips_what_it_cannot_use() {
        let results = parse(SAMPLE).unwrap();

        assert_eq!(results.len(), 1);

        let first = &results[0];

        assert_eq!(first.artist, "Radiohead");
        assert_eq!(first.title, "OK Computer");
        assert_eq!(first.year, Some(1997));
        assert_eq!(first.itunes_id, Some(1097861387));
        assert_eq!(first.genre.as_deref(), Some("Alternative"));
        assert!(first.cover_url.as_deref().unwrap().contains("600x600bb"));
    }

    #[test]
    fn an_empty_result_list_is_not_an_error() {
        let results = parse(r#"{"resultCount":0,"results":[]}"#).unwrap();

        assert!(results.is_empty());
    }
}
