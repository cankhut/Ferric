//! Looking an album up.
//!
//! Two catalogues are asked, because neither has everything: Deezer knows
//! records Apple has never carried, and the other way round. What comes
//! back is merged, so an album in both ends up as one entry with the year
//! from whichever knew it and the better cover.

use anyhow::{anyhow, Result};

use crate::album::{clean_title, Album};
use crate::deezer;
use crate::itunes;

/// How big a release is. Singles and EPs are still worth finding, they
/// just should not sit above the album somebody was obviously looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// A full record.
    #[default]
    Album,
    /// A single or an EP: a handful of tracks, often one.
    Short,
}

/// One album a catalogue offered. Its own type, not anybody's JSON, so the
/// rest of the app never has to know whose field names these were.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub artist: String,
    pub title: String,
    pub year: Option<u16>,
    pub cover_url: Option<String>,
    pub itunes_id: Option<u64>,
    pub deezer_id: Option<u64>,
    pub genre: Option<String>,
    pub kind: Kind,
}

impl SearchResult {
    pub fn to_album(&self) -> Album {
        let mut album = Album::new(self.artist.clone(), self.title.clone());

        album.year = self.year;
        album.cover_url = self.cover_url.clone();
        album.itunes_id = self.itunes_id;
        album.deezer_id = self.deezer_id;

        // One broad word from the catalogue, to show something straight
        // away; the real genres arrive a second later from Discogs
        if let Some(genre) = &self.genre {
            album.tags.push(genre.to_lowercase());
        }

        album
    }

    fn key(&self) -> (String, String) {
        (
            self.artist.to_lowercase(),
            clean_title(&self.title).to_lowercase(),
        )
    }

    /// Fills in what this result is missing from another copy of the same
    /// album, and remembers both catalogue ids.
    fn absorb(&mut self, other: SearchResult) {
        // The plainest name wins: an album is "Sempiternal", not
        // "Sempiternal (Expanded Edition)"
        if other.title.chars().count() < self.title.chars().count() {
            self.title = other.title.clone();
        }

        if self.year.is_none() {
            self.year = other.year;
        }

        if self.cover_url.is_none() {
            self.cover_url = other.cover_url;
        }

        if self.genre.is_none() {
            self.genre = other.genre;
        }

        if self.itunes_id.is_none() {
            self.itunes_id = other.itunes_id;
        }

        if self.deezer_id.is_none() {
            self.deezer_id = other.deezer_id;
        }

        // One catalogue calling it an album is enough. Apple lists plenty
        // of short records with no track count at all, and guessing "short"
        // from a missing number would bury real albums
        if other.kind == Kind::Album {
            self.kind = Kind::Album;
        }
    }
}

/// Searches both catalogues, best match first.
///
/// Typing "Bring Me The Horizon - amo" works, and so does "amo": the dash
/// is dropped, and if the whole query finds nothing the halves are tried
/// on their own.
pub fn search(query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    let mut failures: Vec<String> = Vec::new();

    for attempt in attempts(query) {
        let mut found: Vec<SearchResult> = Vec::new();

        for outcome in [
            deezer::search(&attempt, limit),
            itunes::search(&attempt, limit),
        ] {
            match outcome {
                Ok(mut results) => found.append(&mut results),
                // One catalogue being down is not worth failing over
                Err(error) => failures.push(error.to_string()),
            }
        }

        if !found.is_empty() {
            return Ok(rank(found, query));
        }
    }

    // Nothing found and nothing worked is a failure, nothing found on its
    // own just means nobody has the record
    if failures.len() >= 2 {
        return Err(anyhow!("{}", failures.join("; ")));
    }

    Ok(Vec::new())
}

/// The ways a query is worth trying, in order, stopping at the first that
/// finds anything.
///
/// "Bring Me The Horizon - amo" becomes:
///   1. bring me the horizon amo   (the whole thing, dash removed)
///   2. amo                        (what is after the dash)
///   3. bring me the horizon       (what is before it)
pub fn attempts(query: &str) -> Vec<String> {
    let mut attempts = Vec::new();

    let whole = normalize(query);

    if whole.is_empty() {
        return attempts;
    }

    attempts.push(whole);

    // A dash between words is how everyone writes "artist - album", and
    // neither catalogue reads it that way
    if let Some((left, right)) = split_on_dash(query) {
        for part in [right, left] {
            let part = normalize(&part);

            if !part.is_empty() && !attempts.contains(&part) {
                attempts.push(part);
            }
        }
    }

    attempts
}

fn split_on_dash(query: &str) -> Option<(String, String)> {
    for separator in [" - ", " – ", " — "] {
        if let Some((left, right)) = query.split_once(separator) {
            return Some((left.to_string(), right.to_string()));
        }
    }

    None
}

/// Drops the punctuation the catalogues choke on and squeezes the spaces.
fn normalize(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character.is_whitespace() || character == '\'' {
                character
            } else {
                ' '
            }
        })
        .collect();

    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sorts by how well each result matches what was typed, and folds the
/// copies of one album, deluxe editions and both catalogues alike, into a
/// single entry.
pub fn rank(results: Vec<SearchResult>, query: &str) -> Vec<SearchResult> {
    let words: Vec<String> = normalize(query)
        .to_lowercase()
        .split_whitespace()
        .map(|word| word.to_string())
        .collect();

    let mut scored: Vec<(usize, SearchResult)> = results
        .into_iter()
        .map(|result| (score(&result, &words), result))
        .collect();

    // Stable, so the order the catalogues gave decides between equal matches
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));

    let mut ranked: Vec<SearchResult> = Vec::new();

    for (_, result) in scored {
        match ranked.iter_mut().find(|kept| kept.key() == result.key()) {
            Some(kept) => kept.absorb(result),
            None => ranked.push(result),
        }
    }

    ranked
}

fn score(result: &SearchResult, words: &[String]) -> usize {
    let artist = result.artist.to_lowercase();
    let title = clean_title(&result.title).to_lowercase();

    let mut score = 0;

    for word in words {
        // A word in the title counts more: people search for the album, and
        // every record by that artist matches the artist's name
        if title.split_whitespace().any(|part| part == word) {
            score += 4;
        } else if title.contains(word.as_str()) {
            score += 2;
        }

        if artist.contains(word.as_str()) {
            score += 1;
        }
    }

    // An exact title is what you were after, whatever else matched
    if words.join(" ") == title || words.contains(&title) {
        score += 8;
    }

    // A full record outranks a single or an EP that matched just as well.
    // Searching an artist's name used to bring back their singles mixed in
    // among the albums, in whatever order the catalogues happened to give.
    // Short records are still here, they are just no longer in front of the
    // album you were plainly looking for, and a single whose name you typed
    // exactly still comes first
    if result.kind == Kind::Album {
        score += 5;
    }

    // Deezer is the catalogue this app leans on: big square covers, it says
    // whether a record is an album, and far fewer odd one-track entries. On
    // an equal match it wins
    if result.deezer_id.is_some() {
        score += 2;
    }

    // Something you can actually look at beats something you cannot
    if result.cover_url.is_some() {
        score += 1;
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(artist: &str, title: &str) -> SearchResult {
        SearchResult {
            artist: artist.to_string(),
            title: title.to_string(),
            year: None,
            cover_url: None,
            itunes_id: None,
            deezer_id: None,
            genre: None,
            kind: Kind::Album,
        }
    }

    fn short(artist: &str, title: &str) -> SearchResult {
        SearchResult {
            kind: Kind::Short,
            ..result(artist, title)
        }
    }

    #[test]
    fn an_album_comes_before_a_single_that_matched_as_well() {
        let single = short("Bring Me The Horizon", "Drown");
        let album = result("Bring Me The Horizon", "That's the Spirit");

        let ranked = rank(vec![single, album], "bring me the horizon");

        assert_eq!(ranked[0].title, "That's the Spirit");
        // The single is still there, just not in front
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn a_single_you_named_exactly_still_comes_first() {
        let single = short("Bring Me The Horizon", "Drown");
        let album = result("Bring Me The Horizon", "That's the Spirit");

        let ranked = rank(vec![album, single], "drown");

        assert_eq!(ranked[0].title, "Drown");
    }

    #[test]
    fn deezer_wins_a_tie() {
        let from_itunes = result("Portishead", "Dummy");
        let mut from_deezer = result("Portishead", "Dummy");
        from_deezer.deezer_id = Some(1);
        from_deezer.title = "Dummy ".to_string();

        // Different titles so they are not folded into one entry
        let ranked = rank(vec![from_itunes, from_deezer], "portishead");

        assert!(ranked[0].deezer_id.is_some());
    }

    #[test]
    fn the_dash_people_write_between_artist_and_album_is_dealt_with() {
        assert_eq!(
            attempts("Bring Me The Horizon - amo"),
            vec!["Bring Me The Horizon amo", "amo", "Bring Me The Horizon"]
        );

        assert_eq!(attempts("Portishead – Dummy")[1], "Dummy");
    }

    #[test]
    fn a_plain_query_is_tried_once() {
        assert_eq!(attempts("kind of blue"), vec!["kind of blue"]);
        assert!(attempts("   ").is_empty());
    }

    #[test]
    fn punctuation_does_not_go_to_the_catalogues() {
        assert_eq!(
            attempts("Godspeed You! Black Emperor")[0],
            "Godspeed You Black Emperor"
        );
        assert_eq!(attempts("Mother's Milk")[0], "Mother's Milk");
    }

    #[test]
    fn the_album_you_asked_for_comes_first() {
        let results = vec![
            result("Bring Me The Horizon", "Sempiternal"),
            result("Bring Me The Horizon", "That's the Spirit"),
            result("Bring Me The Horizon", "amo"),
            result("Amo Amo", "Amo Amo"),
        ];

        let ranked = rank(results, "Bring Me The Horizon - amo");

        assert_eq!(ranked[0].artist, "Bring Me The Horizon");
        assert_eq!(ranked[0].title, "amo");
    }

    #[test]
    fn the_same_album_from_both_catalogues_becomes_one_entry() {
        let mut from_deezer = result("Bring Me The Horizon", "amo");
        from_deezer.deezer_id = Some(87404392);
        from_deezer.cover_url = Some("https://deezer/cover.jpg".to_string());

        let mut from_itunes = result("Bring Me The Horizon", "amo (Deluxe Edition)");
        from_itunes.itunes_id = Some(1441164589);
        from_itunes.year = Some(2019);

        let ranked = rank(vec![from_deezer, from_itunes], "amo");

        assert_eq!(ranked.len(), 1);

        // Whichever matched better is kept, and it takes what it was
        // missing from the other
        let found = &ranked[0];

        assert_eq!(found.deezer_id, Some(87404392));
        assert_eq!(found.itunes_id, Some(1441164589));
        assert_eq!(found.year, Some(2019));
        assert_eq!(found.cover_url.as_deref(), Some("https://deezer/cover.jpg"));
    }

    #[test]
    fn the_plainest_name_of_an_album_is_the_one_shown() {
        let deluxe = result("Bring Me The Horizon", "Sempiternal (Expanded Edition)");
        let plain = result("Bring Me The Horizon", "Sempiternal");

        let ranked = rank(vec![deluxe, plain], "sempiternal");

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].title, "Sempiternal");
    }

    #[test]
    fn a_result_with_a_cover_wins_a_tie() {
        let plain = result("Burial", "Untrue");

        let mut with_cover = result("Burial", "Untrue (Remastered)");
        with_cover.cover_url = Some("https://cover".to_string());

        let ranked = rank(vec![plain, with_cover], "untrue");

        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].cover_url.is_some());
    }
}
