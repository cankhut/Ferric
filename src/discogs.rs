//! Discogs, the record collectors' database, and the best genres of the
//! three sources here.
//!
//! Discogs keeps two fields on every record: a **genre**, which is broad
//! ("Rock", "Electronic"), and a **style**, which is not ("Metalcore",
//! "Post-Hardcore", "Goa Trance"). Both come back from the search itself,
//! so one request is the whole lookup. Styles are listed first, because a
//! record shelved under "Metalcore, Post-Hardcore, Rock" tells you more
//! than one shelved under "Rock".
//!
//! Searching needs a token, which is free and takes one click:
//! <https://www.discogs.com/settings/developers> → Generate token. Paste it
//! into Settings in the app, or set `DISCOGS_TOKEN`. Without one this
//! source sits out and MusicBrainz answers instead.
//!
//! The token is sent in an `Authorization` header, never in the URL, so it
//! stays out of logs and proxies along the way.
//!
//! <https://www.discogs.com/developers>

use std::time::Duration;

use anyhow::{Context, Result};

use crate::album::clean_title;
use crate::net::{self, Pace};

/// Discogs counts sixty requests a minute for a token, which is one a
/// second.
static PACE: Pace = Pace::new(Duration::from_millis(1100));

/// The token, from the settings file if there is one there, otherwise from
/// the environment. Blank means Discogs is skipped.
pub fn token(from_settings: &str) -> Option<String> {
    let settings = from_settings.trim();

    if !settings.is_empty() {
        return Some(settings.to_string());
    }

    let variable = std::env::var("DISCOGS_TOKEN").ok()?;
    let variable = variable.trim();

    (!variable.is_empty()).then(|| variable.to_string())
}

/// The styles and genres Discogs has for an album.
pub fn tags(artist: &str, title: &str, token: &str) -> Result<Vec<String>> {
    // `master` is the record itself rather than one of its forty pressings,
    // which is what carries the genres people agree on
    let url = format!(
        "https://api.discogs.com/database/search?type=master&artist={}&release_title={}&per_page=5",
        net::encode(artist),
        net::encode(&clean_title(title))
    );

    let body = get(&url, token)?;
    let found = parse(&body);

    if !found.is_empty() {
        return Ok(found);
    }

    // Nothing under that exact name: the record may only be here as a
    // pressing, or the artist may be spelled differently
    let loose = format!(
        "https://api.discogs.com/database/search?type=master&q={}&per_page=5",
        net::encode(&format!("{artist} {}", clean_title(title)))
    );

    Ok(parse(&get(&loose, token)?))
}

/// One request, no sooner than a second after the one before it, with the
/// token in a header rather than the address.
fn get(url: &str, token: &str) -> Result<String> {
    PACE.wait();

    net::agent()
        .get(url)
        .set("Authorization", &format!("Discogs token={token}"))
        .call()
        .context("could not reach Discogs")?
        .into_string()
        .context("Discogs sent something unreadable")
}

/// Kept apart from the request so it can be tested without the network.
///
/// The first result that has anything on it wins: the search was given an
/// artist and a title, so anything it returns is that record, and the
/// first is the most collected pressing of it.
pub fn parse(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };

    let Some(results) = value.get("results").and_then(|results| results.as_array()) else {
        return Vec::new();
    };

    for result in results {
        // Styles first: they are the ones worth reading
        let names: Vec<String> = ["style", "genre"]
            .into_iter()
            .filter_map(|field| result.get(field)?.as_array())
            .flatten()
            .filter_map(|name| {
                let name = name.as_str()?.trim().to_lowercase();

                (!name.is_empty()).then_some(name)
            })
            .collect();

        let mut unique: Vec<String> = Vec::new();

        for name in names {
            if !unique.contains(&name) {
                unique.push(name);
            }

            if unique.len() == 5 {
                break;
            }
        }

        if !unique.is_empty() {
            return unique;
        }
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "pagination": { "items": 2 },
      "results": [
        {
          "title": "Bring Me The Horizon - Sempiternal",
          "type": "master",
          "year": "2013",
          "genre": ["Rock"],
          "style": ["Metalcore", "Post-Hardcore", "Electronicore"],
          "id": 561234
        },
        {
          "title": "Bring Me The Horizon - Sempiternal",
          "type": "release",
          "genre": ["Rock"],
          "style": ["Metalcore"],
          "id": 561235
        }
      ]
    }"#;

    #[test]
    fn styles_come_before_the_broad_genre() {
        assert_eq!(
            parse(SAMPLE),
            vec!["metalcore", "post-hardcore", "electronicore", "rock"]
        );
    }

    #[test]
    fn a_result_with_nothing_on_it_is_skipped() {
        let body = r#"{"results":[
            {"title":"a pressing nobody filled in","id":1},
            {"title":"the one somebody did","genre":["Electronic"],"style":["Dubstep"],"id":2}
        ]}"#;

        assert_eq!(parse(body), vec!["dubstep", "electronic"]);
    }

    #[test]
    fn the_same_name_twice_is_listed_once() {
        let body = r#"{"results":[{"genre":["Rock","rock"],"style":["Rock"]}]}"#;

        assert_eq!(parse(body), vec!["rock"]);
    }

    #[test]
    fn an_empty_or_broken_answer_is_not_an_error() {
        assert!(parse(r#"{"results":[]}"#).is_empty());
        assert!(
            parse(r#"{"message":"You must authenticate to access this resource."}"#).is_empty()
        );
        assert!(parse("not json").is_empty());
    }

    #[test]
    fn a_token_in_the_settings_file_is_used_before_the_environment() {
        assert_eq!(token("abc123").as_deref(), Some("abc123"));

        // Blank and spaces both mean "nothing set here"
        assert!(token("   ").is_none() || std::env::var("DISCOGS_TOKEN").is_ok());
    }
}
