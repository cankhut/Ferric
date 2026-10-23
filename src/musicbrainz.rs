//! MusicBrainz, for genres that nobody needs a key to read.
//!
//! MusicBrainz holds two different things, and the difference is the whole
//! reason this file asks for one and not the other:
//!
//! - **tags** are free text. Anybody can put anything on an album, so
//!   alongside "metalcore" you get "laut.de" (a review site), "english"
//!   (the language) and "1-4 wochen" (how long somebody's shop took to
//!   deliver it). Useless as genres.
//! - **genres** are the same votes, but only the ones whose name is on
//!   MusicBrainz's own curated list of genres. Nothing else can get in.
//!
//! So this asks for `inc=genres`, and what comes back is always a genre.
//!
//! The service asks for an honest User-Agent, which `net::agent` sends, and
//! no more than one request a second, which `PACE` makes sure of. One album
//! takes two requests, and the window fills in older albums in the
//! background, so this matters.

use std::time::Duration;

use anyhow::{Context, Result};

use crate::album::clean_title;
use crate::net::{self, Pace};

/// A little over the second they ask for, to be safe.
static PACE: Pace = Pace::new(Duration::from_millis(1100));

/// The genres on an album, most voted for first.
pub fn genres(artist: &str, title: &str) -> Result<Vec<String>> {
    let Some(id) = find_release_group(artist, title)? else {
        return Ok(Vec::new());
    };

    let body = get(&format!(
        "https://musicbrainz.org/ws/2/release-group/{id}?inc=genres&fmt=json"
    ))?;

    Ok(parse_genres(&body))
}

fn find_release_group(artist: &str, title: &str) -> Result<Option<String>> {
    // Their query language, so the album name is matched as an album name
    // rather than as words that might appear anywhere
    let query = format!(
        "releasegroup:\"{}\" AND artist:\"{}\"",
        escape(&clean_title(title)),
        escape(artist)
    );

    let body = get(&format!(
        "https://musicbrainz.org/ws/2/release-group/?query={}&fmt=json&limit=1",
        net::encode(&query)
    ))?;

    Ok(parse_id(&body))
}

/// One request, no sooner than a second after the one before it.
fn get(url: &str) -> Result<String> {
    PACE.wait();

    net::agent()
        .get(url)
        .call()
        .context("could not reach MusicBrainz")?
        .into_string()
        .context("MusicBrainz sent something unreadable")
}

/// The id of the first album the search matched.
pub fn parse_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;

    let id = value
        .get("release-groups")?
        .as_array()?
        .first()?
        .get("id")?
        .as_str()?;

    Some(id.to_string())
}

/// Genre names, the most voted for first, five at most.
pub fn parse_genres(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let Some(genres) = value.get("genres").and_then(|genres| genres.as_array()) else {
        return Vec::new();
    };

    let mut counted: Vec<(i64, String)> = genres
        .iter()
        .filter_map(|genre| {
            let name = genre.get("name")?.as_str()?.trim().to_lowercase();

            if name.is_empty() {
                return None;
            }

            // A genre nobody has voted for, or voted against, is not one
            let count = genre.get("count").and_then(|count| count.as_i64())?;

            (count > 0).then_some((count, name))
        })
        .collect();

    counted.sort_by_key(|(count, _)| std::cmp::Reverse(*count));

    let mut names: Vec<String> = Vec::new();

    for (_, name) in counted {
        if !names.contains(&name) {
            names.push(name);
        }

        if names.len() == 5 {
            break;
        }
    }

    names
}

/// Makes a title or artist safe to drop inside a quoted phrase of
/// MusicBrainz's search language.
///
/// Inside quotes only the quote itself and the backslash carry meaning, and
/// a title holding one used to end the phrase early and leave the rest as
/// broken syntax. Bowie's `"Heroes"` is written with the quotes, so it could
/// never fetch its genres: the search came back empty every time, the album
/// was marked as looked up, and nothing ever tried again.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for character in text.chars() {
        if character == '"' || character == '\\' {
            out.push('\\');
        }

        out.push(character);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_written_with_quotes_does_not_break_the_search() {
        assert_eq!(escape(r#""Heroes""#), r#"\"Heroes\""#);
        assert_eq!(escape(r"AC\DC"), r"AC\\DC");
    }

    #[test]
    fn an_ordinary_title_is_left_alone() {
        assert_eq!(escape("Blonde on Blonde"), "Blonde on Blonde");
        assert_eq!(
            escape("(What's the Story) Morning Glory?"),
            "(What's the Story) Morning Glory?"
        );
    }

    #[test]
    fn reads_the_first_match_of_a_search() {
        let body = r#"{"count":2,"release-groups":[
            {"id":"9b2c2bd1-0f8b-4a64-9f1f-1bdf8c2b1234","title":"amo"},
            {"id":"other","title":"amo (deluxe)"}
        ]}"#;

        assert_eq!(
            parse_id(body).as_deref(),
            Some("9b2c2bd1-0f8b-4a64-9f1f-1bdf8c2b1234")
        );
    }

    #[test]
    fn a_search_that_matched_nothing_is_not_an_error() {
        assert_eq!(parse_id(r#"{"count":0,"release-groups":[]}"#), None);
        assert_eq!(parse_id("not json"), None);
    }

    #[test]
    fn the_most_voted_for_genres_come_first() {
        let body = r#"{"id":"x","genres":[
            {"count":1,"name":"Rock","id":"a"},
            {"count":9,"name":"metalcore","id":"b"},
            {"count":4,"name":"Post-Hardcore","id":"c"},
            {"count":2,"name":"electronicore","id":"d"},
            {"count":1,"name":"emo","id":"e"},
            {"count":1,"name":"alternative metal","id":"f"},
            {"count":1,"name":"nu metal","id":"g"}
        ]}"#;

        assert_eq!(
            parse_genres(body),
            vec!["metalcore", "post-hardcore", "electronicore", "rock", "emo"]
        );
    }

    #[test]
    fn a_genre_voted_down_to_nothing_is_left_out() {
        let body = r#"{"id":"x","genres":[
            {"count":0,"name":"jazz","id":"a"},
            {"count":-2,"name":"country","id":"b"},
            {"count":3,"name":"metalcore","id":"c"}
        ]}"#;

        assert_eq!(parse_genres(body), vec!["metalcore"]);
    }

    #[test]
    fn an_album_with_no_genres_gives_nothing_back() {
        assert!(parse_genres(r#"{"id":"x","genres":[]}"#).is_empty());
        assert!(parse_genres(r#"{"id":"x"}"#).is_empty());
        assert!(parse_genres("not json").is_empty());

        // The free text tags are a different field, and are ignored even
        // when the answer carries them
        let with_tags = r#"{"id":"x","tags":[{"count":5,"name":"laut.de"}]}"#;

        assert!(parse_genres(with_tags).is_empty());
    }
}
