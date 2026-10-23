//! One album and the things you can ask about it.

use serde::{Deserialize, Serialize};

/// An album you have in your library, with your own rating on it.
///
/// Everything that comes from a website is optional, because a website can
/// always be missing a field, be unreachable, or know nothing about the album.
/// Your rating and note are the only parts that are really yours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub artist: String,
    pub title: String,

    #[serde(default)]
    pub year: Option<u16>,

    /// 1 to 10, or None while you have not made up your mind
    #[serde(default)]
    pub rating: Option<u8>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub note: Option<String>,

    #[serde(default)]
    pub cover_url: Option<String>,

    #[serde(default)]
    pub itunes_id: Option<u64>,

    #[serde(default)]
    pub deezer_id: Option<u64>,

    /// yyyy-mm-dd, the day you added it
    #[serde(default)]
    pub added_on: String,

    /// Where this album sits when the wall is sorted by hand. Albums from
    /// before the custom order existed, and ones just added, have none and
    /// go to the end until they are moved.
    #[serde(default)]
    pub custom_position: Option<u32>,

    /// Whether the genre sources have been asked about this album and
    /// answered, even if the answer was nothing.
    ///
    /// Without this, an album nobody has tagged is looked up again on
    /// every single launch, forever: a library with 300 obscure records
    /// means 300 paced lookups, several minutes of requests, every time
    /// you open the app, with the same empty answer each time. Older
    /// libraries default to false and so get one more run, which is right
    /// -- they have never been asked under the current sources.
    #[serde(default)]
    pub looked_up: bool,
}

impl Album {
    pub fn new(artist: impl Into<String>, title: impl Into<String>) -> Self {
        Album {
            artist: artist.into(),
            title: title.into(),
            year: None,
            rating: None,
            tags: Vec::new(),
            note: None,
            cover_url: None,
            itunes_id: None,
            deezer_id: None,
            added_on: today(),
            custom_position: None,
            looked_up: false,
        }
    }

    /// Does this album match what was typed in the filter box? Artist, title
    /// and tags all count, and case never matters.
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();

        if needle.is_empty() {
            return true;
        }

        // Every word has to appear somewhere, so "ok pink" finds
        // "OK Computer" only if a tag or the artist mentions pink
        let haystack = format!(
            "{} {} {} {}",
            self.artist,
            self.title,
            self.tags.join(" "),
            self.year.map(|y| y.to_string()).unwrap_or_default()
        )
        .to_lowercase();

        needle
            .split_whitespace()
            .all(|word| haystack.contains(word))
    }
}

/// Today as yyyy-mm-dd, in local time.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Strips the decorations iTunes puts in album titles, so the genre sources
/// can find it: "Blonde on Blonde (Remastered)" becomes "Blonde on Blonde".
///
/// A bracket is only dropped when it contains one of the words below, so
/// "(What's the Story) Morning Glory?" survives intact.
pub fn clean_title(title: &str) -> String {
    const NOISE: [&str; 14] = [
        "remaster",
        "remastered",
        "deluxe",
        "expanded",
        "edition",
        "anniversary",
        "bonus",
        "explicit",
        "clean",
        "reissue",
        "version",
        "mono",
        "stereo",
        "soundtrack from",
    ];

    let mut out = String::with_capacity(title.len());
    let mut bracket = String::new();
    let mut depth = 0usize;

    for character in title.chars() {
        match character {
            '(' | '[' => {
                depth += 1;

                if depth == 1 {
                    bracket.clear();
                }

                bracket.push(character);
            }
            ')' | ']' => {
                bracket.push(character);

                depth = depth.saturating_sub(1);

                if depth == 0 {
                    let inside = bracket.to_lowercase();

                    // Kept unless it is one of the decorations
                    if !NOISE.iter().any(|word| inside.contains(word)) {
                        out.push_str(&bracket);
                    }

                    bracket.clear();
                }
            }
            _ if depth > 0 => bracket.push(character),
            _ => out.push(character),
        }
    }

    // An unclosed bracket is text like any other
    out.push_str(&bracket);

    // iTunes also uses trailing dashes: "Pablo Honey - Remastered"
    let mut cleaned = out.trim().to_string();

    if let Some(dash) = cleaned.rfind(" - ") {
        let tail = cleaned[dash + 3..].to_lowercase();

        if NOISE.iter().any(|word| tail.contains(word)) {
            cleaned.truncate(dash);
        }
    }

    // Collapse the double spaces a removed bracket leaves behind
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_decorations_itunes_adds() {
        assert_eq!(
            clean_title("Blonde on Blonde (Remastered)"),
            "Blonde on Blonde"
        );
        assert_eq!(clean_title("Nevermind [Deluxe Edition]"), "Nevermind");
        assert_eq!(clean_title("Pablo Honey - Remastered"), "Pablo Honey");
        assert_eq!(
            clean_title("Abbey Road (Super Deluxe Edition) [Remastered 2019]"),
            "Abbey Road"
        );
    }

    #[test]
    fn keeps_brackets_that_are_part_of_the_title() {
        assert_eq!(
            clean_title("(What's the Story) Morning Glory?"),
            "(What's the Story) Morning Glory?"
        );
        assert_eq!(clean_title("Space Oddity"), "Space Oddity");
        assert_eq!(clean_title("Follow the Leader"), "Follow the Leader");
    }

    #[test]
    fn keeps_a_title_that_is_only_a_bracket() {
        assert_eq!(clean_title("( )"), "( )");
    }

    #[test]
    fn filtering_looks_at_artist_title_and_tags() {
        let mut album = Album::new("Radiohead", "OK Computer");
        album.tags = vec!["alternative rock".into(), "90s".into()];
        album.year = Some(1997);

        assert!(album.matches(""));
        assert!(album.matches("radio"));
        assert!(album.matches("OK"));
        assert!(album.matches("alternative"));
        assert!(album.matches("1997"));
        assert!(album.matches("radiohead computer"));
        assert!(!album.matches("jazz"));
    }
}
