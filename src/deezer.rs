//! Deezer's catalogue. No key, no signup, square covers at a size worth
//! looking at, and it carries plenty of records Apple's store does not.
//!
//! https://developers.deezer.com/api/search

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::net;
use crate::search::{Kind, SearchResult};

pub fn search(term: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let term = term.trim();

    if term.is_empty() {
        return Ok(Vec::new());
    }

    let url = format!(
        "https://api.deezer.com/search/album?q={}&limit={}",
        net::encode(term),
        limit.clamp(1, 100)
    );

    let body = net::agent()
        .get(&url)
        .call()
        .context("could not reach Deezer")?
        .into_string()
        .context("Deezer sent something unreadable")?;

    parse(&body)
}

/// Kept apart from the request so it can be tested without the network.
pub fn parse(body: &str) -> Result<Vec<SearchResult>> {
    let response: Response = serde_json::from_str(body).context("Deezer sent unexpected JSON")?;

    let results = response
        .data
        .into_iter()
        .filter_map(|entry| {
            // Singles and EPs used to be thrown away here. They are worth
            // finding, so now they are kept and ranked below the albums
            // instead: see `search::score`
            let kind = match entry.record_type.as_deref() {
                Some("single") | Some("ep") => Kind::Short,
                _ => Kind::Album,
            };

            let title = entry.title?;
            let artist = entry.artist?.name?;

            Some(SearchResult {
                artist,
                title,
                // The search does not carry the release date. It is fetched
                // for the one album you add, in `year_of`
                year: None,
                cover_url: entry.cover_big.or(entry.cover_xl).or(entry.cover_medium),
                itunes_id: None,
                deezer_id: entry.id,
                genre: None,
                kind,
            })
        })
        .collect();

    Ok(results)
}

/// What an album's own page says, which the search results leave out.
#[derive(Debug, Default, Clone)]
pub struct Details {
    pub year: Option<u16>,
    pub genres: Vec<String>,
    /// The album's songs in order. Deezer puts them on the album's own
    /// page, so the request that fetches the year brings these too and
    /// nothing extra is asked for
    pub tracks: Vec<String>,
}

pub fn details(deezer_id: u64) -> Result<Details> {
    let url = format!("https://api.deezer.com/album/{deezer_id}");

    let body = net::agent()
        .get(&url)
        .call()
        .context("could not reach Deezer")?
        .into_string()
        .context("Deezer sent something unreadable")?;

    Ok(parse_details(&body))
}

/// Release dates arrive as 2019-01-25, and only the year matters.
pub fn parse_details(body: &str) -> Details {
    let Ok(album) = serde_json::from_str::<AlbumDetails>(body) else {
        return Details::default();
    };

    let year = album
        .release_date
        .as_deref()
        .and_then(|date| date.get(0..4))
        .and_then(|year| year.parse().ok());

    let genres = album
        .genres
        .map(|genres| {
            genres
                .data
                .into_iter()
                .filter_map(|genre| genre.name)
                .map(|name| name.trim().to_lowercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let tracks = album
        .tracks
        .map(|tracks| {
            tracks
                .data
                .into_iter()
                .filter_map(|track| track.title)
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
                .collect()
        })
        .unwrap_or_default();

    Details {
        year,
        genres,
        tracks,
    }
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    data: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    id: Option<u64>,
    title: Option<String>,
    artist: Option<Artist>,
    cover_medium: Option<String>,
    cover_big: Option<String>,
    cover_xl: Option<String>,
    record_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Artist {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AlbumDetails {
    release_date: Option<String>,
    genres: Option<Genres>,
    tracks: Option<Tracks>,
}

#[derive(Debug, Deserialize)]
struct Tracks {
    #[serde(default)]
    data: Vec<TrackEntry>,
}

#[derive(Debug, Deserialize)]
struct TrackEntry {
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Genres {
    #[serde(default)]
    data: Vec<Genre>,
}

#[derive(Debug, Deserialize)]
struct Genre {
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "data": [
        {
          "id": 87404392,
          "title": "amo",
          "cover_medium": "https://e-cdns-images.dzcdn.net/images/cover/x/250x250.jpg",
          "cover_big": "https://e-cdns-images.dzcdn.net/images/cover/x/500x500.jpg",
          "cover_xl": "https://e-cdns-images.dzcdn.net/images/cover/x/1000x1000.jpg",
          "record_type": "album",
          "artist": { "id": 1282, "name": "Bring Me The Horizon" },
          "type": "album"
        },
        {
          "id": 2,
          "title": "medicine",
          "record_type": "single",
          "artist": { "name": "Bring Me The Horizon" }
        },
        {
          "id": 3,
          "record_type": "album",
          "artist": { "name": "An album with no title" }
        }
      ],
      "total": 3
    }"#;

    #[test]
    fn reads_albums_and_keeps_singles_below_them() {
        let results = parse(SAMPLE).unwrap();

        // The album and the single both come back. Singles used to be
        // thrown away here; they are kept now and ranked under the albums
        // instead, so a single you search for by name can still be found.
        // The third entry has no title at all and is no use to anyone
        assert_eq!(results.len(), 2);

        let found = &results[0];

        assert_eq!(found.artist, "Bring Me The Horizon");
        assert_eq!(found.title, "amo");
        assert_eq!(found.deezer_id, Some(87404392));
        assert!(found.cover_url.as_deref().unwrap().contains("500x500"));
        assert_eq!(found.kind, Kind::Album);

        // The search answer has no release date in it
        assert_eq!(found.year, None);

        // Marked as the short record it is, which is what pushes it down
        // the list in `search::score`
        assert_eq!(results[1].title, "medicine");
        assert_eq!(results[1].kind, Kind::Short);
    }

    #[test]
    fn an_empty_answer_is_not_an_error() {
        assert!(parse(r#"{"data":[],"total":0}"#).unwrap().is_empty());
    }

    #[test]
    fn the_year_and_the_genres_come_from_the_album_page() {
        let body = r#"{
          "id": 87404392,
          "title": "amo",
          "release_date": "2019-01-25",
          "genres": { "data": [ { "name": "Rock" }, { "name": "Alternative" } ] }
        }"#;

        let details = parse_details(body);

        assert_eq!(details.year, Some(2019));
        assert_eq!(details.genres, vec!["rock", "alternative"]);
    }

    #[test]
    fn an_album_page_missing_the_lot_is_not_an_error() {
        let details = parse_details(r#"{"id":1,"title":"no date"}"#);

        assert_eq!(details.year, None);
        assert!(details.genres.is_empty());
        assert!(details.tracks.is_empty());

        assert_eq!(parse_details("not json").year, None);
    }

    #[test]
    fn the_track_list_comes_back_in_order() {
        let body = r#"{
          "id": 87404392,
          "title": "amo",
          "tracks": { "data": [
            { "title": "i apologise if you feel something" },
            { "title": "MANTRA" },
            { "title": "nihilist blues" }
          ] }
        }"#;

        let details = parse_details(body);

        assert_eq!(
            details.tracks,
            vec![
                "i apologise if you feel something",
                "MANTRA",
                "nihilist blues"
            ]
        );
    }

    #[test]
    fn a_track_with_no_name_is_left_out_rather_than_listed_blank() {
        let body = r#"{
          "id": 1,
          "tracks": { "data": [
            { "title": "one" },
            { "id": 99 },
            { "title": "   " },
            { "title": " two " }
          ] }
        }"#;

        // Trimmed, and the nameless and empty ones never reach the panel
        assert_eq!(parse_details(body).tracks, vec!["one", "two"]);
    }
}
