//! The albums you have, and the ways of looking at them.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::album::Album;
use crate::storage;

/// The orders the list can be shown in. Saved in the settings file, so the
/// one you chose is the one you get next time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Added,
    Rating,
    Artist,
    Year,
    /// However you dragged them. The only order the app does not decide.
    Custom,
}

impl Sort {
    /// Every one of them, in the order the menu lists them.
    pub const ALL: [Sort; 5] = [
        Sort::Added,
        Sort::Rating,
        Sort::Artist,
        Sort::Year,
        Sort::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Added => "recently added",
            Sort::Rating => "rating",
            Sort::Artist => "artist",
            Sort::Year => "year",
            Sort::Custom => "custom",
        }
    }
}

pub struct Library {
    albums: Vec<Album>,
    pub sort: Sort,
    pub filter: String,
    /// Set when the library on disk could not be read or moved aside.
    /// Nothing is written while it is on, so a file that might still be
    /// rescued is never written over.
    read_only: bool,
}

/// Are these two the same record?
///
/// A matching id from either catalogue settles it. Ids that *disagree* settle
/// nothing, which is the part that used to be wrong: the same album can carry
/// two different iTunes ids — the standard pressing and the deluxe one, or the
/// same record re-issued — and whichever the search picked depended on what
/// you typed. Treating that as "a different album" let you add the same record
/// twice, and then every tag fetched for the second copy landed on the first,
/// leaving a card that could never fill itself in.
fn same_album(left: &Album, right: &Album) -> bool {
    if let (Some(left), Some(right)) = (left.itunes_id, right.itunes_id) {
        if left == right {
            return true;
        }
    }

    if let (Some(left), Some(right)) = (left.deezer_id, right.deezer_id) {
        if left == right {
            return true;
        }
    }

    left.artist.eq_ignore_ascii_case(&right.artist) && left.title.eq_ignore_ascii_case(&right.title)
}

impl Library {
    pub fn load() -> Result<Self> {
        Ok(Library {
            albums: storage::load()?,
            sort: Sort::Added,
            filter: String::new(),
            read_only: false,
        })
    }

    /// An empty library, for when the real one could not be read.
    ///
    /// `read_only` is for the case where the unreadable file could not even
    /// be moved out of the way. The app still opens, so you can see what
    /// happened, but it must never write an empty library over whatever is
    /// in that file.
    pub fn empty(read_only: bool) -> Self {
        Library {
            albums: Vec::new(),
            sort: Sort::Added,
            filter: String::new(),
            read_only,
        }
    }

    pub fn save(&self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }

        storage::save(&self.albums)
    }

    pub fn len(&self) -> usize {
        self.albums.len()
    }

    pub fn is_empty(&self) -> bool {
        self.albums.is_empty()
    }

    pub fn add(&mut self, album: Album) {
        self.albums.push(album);
    }

    /// Is this album already in the library?
    pub fn contains(&self, album: &Album) -> bool {
        self.albums
            .iter()
            .any(|existing| same_album(existing, album))
    }

    /// The albums the list should show right now: filtered, then sorted.
    ///
    /// Returns positions into the real list, not copies of the albums, so
    /// what you select can still be changed or deleted afterwards.
    pub fn visible(&self) -> Vec<usize> {
        let mut indexes: Vec<usize> = self
            .albums
            .iter()
            .enumerate()
            .filter(|(_, album)| album.matches(&self.filter))
            .map(|(index, _)| index)
            .collect();

        match self.sort {
            // Newest first, and the last one added wins a tie
            Sort::Added => indexes.sort_by(|&left, &right| {
                self.albums[right]
                    .added_on
                    .cmp(&self.albums[left].added_on)
                    .then(right.cmp(&left))
            }),
            // Best first, unrated albums last
            Sort::Rating => indexes.sort_by(|&left, &right| {
                self.albums[right]
                    .rating
                    .unwrap_or(0)
                    .cmp(&self.albums[left].rating.unwrap_or(0))
                    .then_with(|| self.sort_key(left).cmp(&self.sort_key(right)))
            }),
            Sort::Artist => indexes.sort_by_key(|&index| self.sort_key(index)),
            // Oldest first, albums without a year last
            Sort::Year => indexes.sort_by(|&left, &right| {
                self.albums[left]
                    .year
                    .unwrap_or(u16::MAX)
                    .cmp(&self.albums[right].year.unwrap_or(u16::MAX))
                    .then_with(|| self.sort_key(left).cmp(&self.sort_key(right)))
            }),
            // However you left them. An album with no place yet, because it
            // was added before this order existed or added just now, waits
            // at the end rather than landing somewhere arbitrary
            Sort::Custom => indexes.sort_by_key(|&index| {
                (
                    self.albums[index].custom_position.unwrap_or(u32::MAX),
                    index,
                )
            }),
        }

        indexes
    }

    /// Gives every album a place, in the order they are listed in `order`,
    /// but only if the custom order has never been used.
    ///
    /// Switching to it should leave the wall looking exactly as it did a
    /// moment ago under whatever sort you were on, rather than throwing
    /// everything into the order it happens to sit in the file. Once you
    /// have dragged anything, this leaves well alone.
    pub fn seed_custom_order(&mut self, order: &[usize]) {
        if self
            .albums
            .iter()
            .any(|album| album.custom_position.is_some())
        {
            return;
        }

        for (place, &index) in order.iter().enumerate() {
            if let Some(album) = self.albums.get_mut(index) {
                album.custom_position = Some(place as u32);
            }
        }
    }

    /// Moves the album sitting at `from` in the current view so that it
    /// lands at `to`, and writes the whole order back.
    ///
    /// Only the albums you can see are renumbered, and only into the places
    /// they already occupied between them. An album hidden by the filter
    /// keeps the position it had, so dragging inside a filtered wall cannot
    /// quietly shuffle everything you are not looking at.
    ///
    /// Says whether anything actually moved.
    pub fn move_in_custom_order(&mut self, from: usize, to: usize) -> bool {
        let mut shown = self.visible();

        if from >= shown.len() || to >= shown.len() || from == to {
            return false;
        }

        // The places these albums hold between them, which the new order
        // will be poured back into
        let mut places: Vec<u32> = shown
            .iter()
            .map(|&index| self.albums[index].custom_position.unwrap_or(u32::MAX))
            .collect();

        // A wall that has never been dragged has no places yet, so it gets
        // them from the order it is already in
        if places.contains(&u32::MAX) {
            places = (0..shown.len() as u32).collect();
        }

        places.sort_unstable();

        let moved = shown.remove(from);
        shown.insert(to, moved);

        for (album_index, place) in shown.into_iter().zip(places) {
            if let Some(album) = self.albums.get_mut(album_index) {
                album.custom_position = Some(place);
            }
        }

        true
    }

    fn sort_key(&self, index: usize) -> (String, String) {
        let album = &self.albums[index];

        (album.artist.to_lowercase(), album.title.to_lowercase())
    }

    pub fn get(&self, index: usize) -> Option<&Album> {
        self.albums.get(index)
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut Album> {
        self.albums.get_mut(index)
    }

    /// Where an album is now. An answer from a website can take a few
    /// seconds, and if an album was removed in the meantime every position
    /// after it has moved, so answers find their album by name instead.
    pub fn position_of(&self, artist: &str, title: &str) -> Option<usize> {
        self.albums.iter().position(|album| {
            album.artist.eq_ignore_ascii_case(artist) && album.title.eq_ignore_ascii_case(title)
        })
    }

    pub fn remove(&mut self, index: usize) -> Option<Album> {
        if index < self.albums.len() {
            Some(self.albums.remove(index))
        } else {
            None
        }
    }

    /// Puts an album back where it was, which is what undoing a removal
    /// needs. A position past the end puts it last.
    pub fn insert(&mut self, index: usize, album: Album) {
        let index = index.min(self.albums.len());

        self.albums.insert(index, album);
    }

    /// Everything rated, best first. Nothing calls this yet: it is here for
    /// the stats screen in the README's list of things to add next.
    #[allow(dead_code)]
    pub fn rated(&self) -> Vec<&Album> {
        let mut rated: Vec<&Album> = self.albums.iter().filter(|a| a.rating.is_some()).collect();

        rated.sort_by_key(|album| std::cmp::Reverse(album.rating));

        rated
    }

    pub fn average_rating(&self) -> Option<f32> {
        let ratings: Vec<u8> = self
            .albums
            .iter()
            .filter_map(|album| album.rating)
            .collect();

        if ratings.is_empty() {
            return None;
        }

        let total: u32 = ratings.iter().map(|&rating| rating as u32).sum();

        Some(total as f32 / ratings.len() as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library_with(albums: Vec<Album>) -> Library {
        Library {
            albums,
            ..Library::empty(false)
        }
    }

    #[test]
    fn a_library_that_could_not_be_read_is_never_written_over() {
        let mut library = Library::empty(true);

        library.add(album("A", "one", None, None));

        // Saving does nothing rather than putting an empty library over a
        // file that might still be worth rescuing
        assert!(library.save().is_ok());
        assert_eq!(library.len(), 1);
    }

    fn album(artist: &str, title: &str, rating: Option<u8>, year: Option<u16>) -> Album {
        let mut album = Album::new(artist, title);
        album.rating = rating;
        album.year = year;
        album
    }

    /// The titles the wall is showing, in the order it is showing them.
    fn shown(library: &Library) -> Vec<String> {
        library
            .visible()
            .into_iter()
            .map(|index| library.get(index).unwrap().title.clone())
            .collect()
    }

    #[test]
    fn switching_to_the_custom_order_keeps_the_wall_as_it_looked() {
        let mut library = library_with(vec![
            album("A", "one", Some(4), None),
            album("B", "two", Some(10), None),
            album("C", "three", Some(7), None),
        ]);

        library.sort = Sort::Rating;
        let by_rating = shown(&library);

        let order = library.visible();
        library.sort = Sort::Custom;
        library.seed_custom_order(&order);

        assert_eq!(shown(&library), by_rating);
    }

    #[test]
    fn an_album_dragged_somewhere_else_stays_there() {
        let mut library = library_with(vec![
            album("A", "one", None, None),
            album("B", "two", None, None),
            album("C", "three", None, None),
            album("D", "four", None, None),
        ]);

        library.sort = Sort::Custom;

        // The last one dragged to the front
        assert!(library.move_in_custom_order(3, 0));
        assert_eq!(shown(&library), ["four", "one", "two", "three"]);

        // And back to where it came from
        assert!(library.move_in_custom_order(0, 3));
        assert_eq!(shown(&library), ["one", "two", "three", "four"]);
    }

    #[test]
    fn dragging_a_card_onto_itself_changes_nothing() {
        let mut library = library_with(vec![
            album("A", "one", None, None),
            album("B", "two", None, None),
        ]);

        library.sort = Sort::Custom;

        assert!(!library.move_in_custom_order(1, 1));
        assert!(!library.move_in_custom_order(0, 9));
        assert_eq!(shown(&library), ["one", "two"]);
    }

    #[test]
    fn dragging_inside_a_filter_leaves_the_hidden_albums_where_they_were() {
        let mut library = library_with(vec![
            album("Burial", "Untrue", None, None),
            album("Other", "hidden", None, None),
            album("Burial", "Rival Dealer", None, None),
        ]);

        library.sort = Sort::Custom;
        library.seed_custom_order(&[0, 1, 2]);

        // Only the two Burial records are on the wall, and the second is
        // dragged in front of the first
        library.filter = "burial".to_string();
        assert_eq!(shown(&library), ["Untrue", "Rival Dealer"]);
        assert!(library.move_in_custom_order(1, 0));
        assert_eq!(shown(&library), ["Rival Dealer", "Untrue"]);

        // The album the filter was hiding is still in the middle, exactly
        // where it was left
        library.filter.clear();
        assert_eq!(shown(&library), ["Rival Dealer", "hidden", "Untrue"]);
    }

    #[test]
    fn an_album_added_later_waits_at_the_end_until_it_is_moved() {
        let mut library = library_with(vec![
            album("A", "one", None, None),
            album("B", "two", None, None),
        ]);

        library.sort = Sort::Custom;
        library.seed_custom_order(&[1, 0]);
        assert_eq!(shown(&library), ["two", "one"]);

        // Nothing is re-seeded once the order has been used
        library.add(album("C", "three", None, None));
        library.seed_custom_order(&[0, 1, 2]);

        assert_eq!(shown(&library), ["two", "one", "three"]);
    }

    #[test]
    fn sorting_by_rating_puts_the_best_first_and_unrated_last() {
        let mut library = library_with(vec![
            album("Boards of Canada", "Geogaddi", Some(8), Some(2002)),
            album("Burial", "Untrue", None, Some(2007)),
            album("Aphex Twin", "Drukqs", Some(10), Some(2001)),
        ]);

        library.sort = Sort::Rating;

        let order: Vec<&str> = library
            .visible()
            .iter()
            .map(|&index| library.get(index).unwrap().title.as_str())
            .collect();

        assert_eq!(order, vec!["Drukqs", "Geogaddi", "Untrue"]);
    }

    #[test]
    fn sorting_by_year_puts_albums_without_one_at_the_end() {
        let mut library = library_with(vec![
            album("Slint", "Spiderland", None, Some(1991)),
            album("Unknown", "Demo", None, None),
            album("Talk Talk", "Laughing Stock", None, Some(1991)),
        ]);

        library.sort = Sort::Year;

        let order: Vec<&str> = library
            .visible()
            .iter()
            .map(|&index| library.get(index).unwrap().artist.as_str())
            .collect();

        assert_eq!(order, vec!["Slint", "Talk Talk", "Unknown"]);
    }

    #[test]
    fn the_filter_hides_everything_that_does_not_match() {
        let mut library = library_with(vec![
            album("Miles Davis", "Kind of Blue", Some(10), Some(1959)),
            album("Mogwai", "Young Team", Some(9), Some(1997)),
        ]);

        library.filter = "miles".to_string();

        assert_eq!(library.visible().len(), 1);

        library.filter = "  ".to_string();

        assert_eq!(library.visible().len(), 2);
    }

    #[test]
    fn an_album_already_in_the_library_is_recognised() {
        let mut existing = album("Fiona Apple", "Fetch the Bolt Cutters", Some(9), Some(2020));
        existing.itunes_id = Some(1502318622);

        let library = library_with(vec![existing]);

        let mut same_id = Album::new("different spelling", "different title");
        same_id.itunes_id = Some(1502318622);

        assert!(library.contains(&same_id));
        assert!(library.contains(&Album::new("fiona apple", "fetch the bolt cutters")));
        assert!(!library.contains(&Album::new("Fiona Apple", "Tidal")));
    }

    #[test]
    fn the_same_album_under_a_different_id_is_still_the_same_album() {
        let mut existing = album("Bring Me The Horizon", "amo", None, None);
        existing.itunes_id = Some(1);

        let library = library_with(vec![existing]);

        // The deluxe pressing of a record carries its own id. Before, an id
        // that did not match ended the comparison then and there, so this
        // came back as a different album and you got two of them
        let mut other_pressing = Album::new("Bring Me The Horizon", "amo");
        other_pressing.itunes_id = Some(2);

        assert!(library.contains(&other_pressing));

        // A matching id still settles it even when the names differ
        let mut renamed = Album::new("BMTH", "Amo (Deluxe)");
        renamed.itunes_id = Some(1);

        assert!(library.contains(&renamed));

        // And a different record is still a different record
        let mut different = Album::new("Bring Me The Horizon", "Sempiternal");
        different.itunes_id = Some(3);

        assert!(!library.contains(&different));
    }

    #[test]
    fn an_album_is_found_by_name_after_another_is_removed() {
        let mut library = library_with(vec![
            album("Slint", "Spiderland", None, None),
            album("Bring Me The Horizon", "amo", None, None),
        ]);

        assert_eq!(library.position_of("bring me the horizon", "AMO"), Some(1));

        library.remove(0);

        assert_eq!(library.position_of("Bring Me The Horizon", "amo"), Some(0));
        assert_eq!(library.position_of("Slint", "Spiderland"), None);
    }

    #[test]
    fn a_removed_album_goes_back_where_it_was() {
        let mut library = library_with(vec![
            album("A", "one", None, None),
            album("B", "two", None, None),
            album("C", "three", None, None),
        ]);

        let removed = library.remove(1).unwrap();
        library.insert(1, removed);

        assert_eq!(library.get(1).unwrap().title, "two");
        assert_eq!(library.len(), 3);

        // Past the end is not a panic, it is last
        library.insert(99, album("D", "four", None, None));

        assert_eq!(library.get(3).unwrap().title, "four");
    }

    #[test]
    fn the_average_ignores_unrated_albums() {
        let library = library_with(vec![
            album("A", "one", Some(10), None),
            album("B", "two", Some(6), None),
            album("C", "three", None, None),
        ]);

        assert_eq!(library.average_rating(), Some(8.0));
        assert_eq!(library_with(vec![]).average_rating(), None);
    }
}
