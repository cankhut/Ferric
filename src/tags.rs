//! Where an album's tags come from.
//!
//! Three places are tried in turn, best first, and the app works without
//! any of them being set up:
//!
//! 1. **Discogs**, if you have put a token in Settings. Styles as well as
//!    genres, so "metalcore, post-hardcore" rather than "rock".
//! 2. **MusicBrainz**, which needs no key. Its curated genre list only,
//!    never its free text tags, which are full of review sites and
//!    languages.
//! 3. **Deezer's genres** for the album, which is always something, though
//!    it is only ever one of their two dozen shop categories.
//!
//! A source that answers with nothing hands over to the next one, so an
//! album Discogs has never heard of still ends up with genres.

use anyhow::Result;

use crate::deezer;
use crate::discogs;
use crate::musicbrainz;

/// What a lookup found, and where it came from, so the app can say so.
pub struct Tags {
    pub names: Vec<String>,
    pub source: &'static str,
}

impl Tags {
    fn from(names: Vec<String>, source: &'static str) -> Option<Self> {
        (!names.is_empty()).then_some(Tags { names, source })
    }

    fn nowhere() -> Self {
        Tags {
            names: Vec::new(),
            source: "nowhere",
        }
    }
}

pub fn fetch(
    artist: &str,
    title: &str,
    deezer_id: Option<u64>,
    discogs_token: Option<&str>,
) -> Result<Tags> {
    if let Some(token) = discogs_token {
        if let Some(found) = discogs::tags(artist, title, token)
            .ok()
            .and_then(|names| Tags::from(names, "Discogs"))
        {
            return Ok(found);
        }
    }

    if let Some(found) = musicbrainz::genres(artist, title)
        .ok()
        .and_then(|names| Tags::from(names, "MusicBrainz"))
    {
        return Ok(found);
    }

    if let Some(deezer_id) = deezer_id {
        if let Some(found) = deezer::details(deezer_id)
            .ok()
            .and_then(|details| Tags::from(details.genres, "Deezer"))
        {
            return Ok(found);
        }
    }

    Ok(Tags::nowhere())
}
