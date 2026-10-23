//! What you have chosen, kept between runs: `settings.json`, in the same
//! folder as your library.
//!
//! Everything in here is optional and everything has a sensible answer when
//! it is missing, so deleting the file, or never having one, costs you
//! nothing but the window size.
//!
//! The one thing worth knowing: a Discogs token saved here is stored as
//! plain text, the way every app's config file stores its keys. It is a
//! read-only search token tied to no payment and nothing else, and you can
//! revoke it at <https://www.discogs.com/settings/developers> whenever you
//! like. If you would rather it were not in a file at all, leave the box
//! empty and set `DISCOGS_TOKEN` in your environment instead.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::library::Sort;
use crate::storage;

/// Where the window was and how big, so it opens where you left it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Window {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Window {
    /// A window that is off the screen, or the size of a postage stamp,
    /// is not worth restoring. Monitors get unplugged.
    pub fn sensible(&self) -> bool {
        let finite = [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite());

        finite && self.width >= 600.0 && self.height >= 400.0
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub window: Option<Window>,

    /// How wide you dragged the album panel
    #[serde(default)]
    pub panel_width: Option<f32>,

    #[serde(default)]
    pub sort: Option<Sort>,

    /// Free, one click at discogs.com/settings/developers. Empty means
    /// Discogs sits out and MusicBrainz answers instead
    #[serde(default)]
    pub discogs_token: String,
}

impl Settings {
    /// Reads the file. Anything wrong with it — missing, empty, edited into
    /// nonsense — gives you the defaults rather than an error on startup,
    /// because nobody wants an app that will not open over a settings file.
    pub fn load() -> Self {
        storage::read_json(&storage::settings_path())
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        storage::write_json(&storage::settings_path(), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_file_from_an_older_version_still_reads() {
        // Only the token in it, which is what the first version to write
        // one of these would have left behind
        let older: Settings = serde_json::from_str(r#"{"discogs_token":"abc"}"#).unwrap();

        assert_eq!(older.discogs_token, "abc");
        assert!(older.window.is_none());
        assert!(older.panel_width.is_none());
        assert!(older.sort.is_none());

        // And an empty one is just the defaults
        let empty: Settings = serde_json::from_str("{}").unwrap();

        assert!(empty.discogs_token.is_empty());
    }

    #[test]
    fn the_sort_you_chose_survives_a_round_trip() {
        let settings = Settings {
            sort: Some(Sort::Rating),
            panel_width: Some(412.0),
            ..Default::default()
        };

        let written = serde_json::to_string(&settings).unwrap();
        let read: Settings = serde_json::from_str(&written).unwrap();

        assert_eq!(read.sort, Some(Sort::Rating));
        assert_eq!(read.panel_width, Some(412.0));
    }

    #[test]
    fn a_window_off_the_screen_is_not_restored() {
        let good = Window {
            x: 100.0,
            y: 80.0,
            width: 1180.0,
            height: 760.0,
        };

        assert!(good.sensible());

        // Minimised to nothing, or saved from a monitor that has gone
        assert!(!Window { width: 0.0, ..good }.sensible());
        assert!(!Window {
            height: 12.0,
            ..good
        }
        .sensible());
        assert!(!Window {
            x: f32::NAN,
            ..good
        }
        .sensible());
    }
}
