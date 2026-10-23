//! Tags from Last.fm. This part is optional on purpose: no key, no
//! internet or an album nobody has tagged all end the same way, with the
//! album saved and no tags, never with an error you have to deal with.
//!
//! A free key comes from https://www.last.fm/api/account/create and is read
//! from the LASTFM_API_KEY environment variable.

use anyhow::{anyhow, Context, Result};

use crate::album::clean_title;
use crate::net;

pub const KEY_VARIABLE: &str = "LASTFM_API_KEY";

pub fn api_key() -> Option<String> {
    match std::env::var(KEY_VARIABLE) {
        Ok(key) if !key.trim().is_empty() => Some(key),
        _ => None,
    }
}

/// The tags people have put on this album, most used first.
///
/// The title is cleaned first, because Last.fm has never heard of
/// "Loveless (Remastered)".
pub fn tags(artist: &str, title: &str) -> Result<Vec<String>> {
    let key = api_key()
        .ok_or_else(|| anyhow!("no Last.fm key: set {KEY_VARIABLE} and start the app again"))?;

    let cleaned = clean_title(title);

    let mut found = fetch_tags(&key, artist, &cleaned)?;

    // Some albums are only found under their full name, so that is the
    // second thing to try
    if found.is_empty() && cleaned != title {
        found = fetch_tags(&key, artist, title)?;
    }

    Ok(found)
}

fn fetch_tags(key: &str, artist: &str, album: &str) -> Result<Vec<String>> {
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=album.getinfo&api_key={}&artist={}&album={}&autocorrect=1&format=json",
        net::encode(key),
        net::encode(artist),
        net::encode(album)
    );

    let body = net::agent()
        .get(&url)
        .call()
        .context("could not reach Last.fm")?
        .into_string()
        .context("Last.fm sent something unreadable")?;

    parse(&body)
}

/// Picks the tag names out of the answer.
///
/// Parsed as loose JSON rather than into a struct, because Last.fm sends
/// `"tags": ""` when an album has none and `"tags": {"tag": [...]}` when it
/// does. One shape would have to be fought, two would have to be fought
/// twice, and this way neither of them can fail the call.
pub fn parse(body: &str) -> Result<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_str(body).context("Last.fm sent unexpected JSON")?;

    // {"error":6,"message":"Album not found"} is an answer, not a failure
    if value.get("error").is_some() {
        return Ok(Vec::new());
    }

    let tags = value
        .get("album")
        .and_then(|album| album.get("tags"))
        .and_then(|tags| tags.get("tag"));

    let Some(tags) = tags else {
        return Ok(Vec::new());
    };

    // One tag comes back as an object, several as a list
    let names: Vec<String> = match tags {
        serde_json::Value::Array(list) => list.iter().filter_map(tag_name).collect(),
        single => tag_name(single).into_iter().collect(),
    };

    let mut cleaned: Vec<String> = Vec::new();

    for name in names {
        let name = name.trim().to_lowercase();

        if !name.is_empty() && !cleaned.contains(&name) {
            cleaned.push(name);
        }

        // Five is plenty for one line of a table
        if cleaned.len() == 5 {
            break;
        }
    }

    Ok(cleaned)
}

fn tag_name(tag: &serde_json::Value) -> Option<String> {
    Some(tag.get("name")?.as_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_list_of_tags() {
        let body = r#"{"album":{"name":"Loveless","tags":{"tag":[
            {"name":"shoegaze"},{"name":"Dream Pop"},{"name":"shoegaze"},
            {"name":"90s"},{"name":"alternative"},{"name":"noise pop"},{"name":"british"}
        ]}}}"#;

        let tags = parse(body).unwrap();

        // Lowercased, no repeats, and no more than five
        assert_eq!(
            tags,
            vec!["shoegaze", "dream pop", "90s", "alternative", "noise pop"]
        );
    }

    #[test]
    fn an_album_with_one_tag_is_not_a_special_case() {
        let body = r#"{"album":{"tags":{"tag":{"name":"Jazz"}}}}"#;

        assert_eq!(parse(body).unwrap(), vec!["jazz"]);
    }

    #[test]
    fn an_album_with_no_tags_gives_nothing_back() {
        // The empty string is what Last.fm really sends here
        assert!(parse(r#"{"album":{"tags":""}}"#).unwrap().is_empty());
        assert!(parse(r#"{"album":{}}"#).unwrap().is_empty());
    }

    #[test]
    fn an_error_answer_is_not_a_failure() {
        let body = r#"{"error":6,"message":"Album not found"}"#;

        assert!(parse(body).unwrap().is_empty());
    }
}
