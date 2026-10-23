//! The parts of ferric that know nothing about how it is shown.
//!
//! Nothing in here draws anything or knows that egui exists, which is what
//! keeps `src/gui.rs` to drawing and these to music.

pub mod album;
pub mod deezer;
pub mod discogs;
pub mod itunes;
pub mod library;
pub mod musicbrainz;
pub mod net;
pub mod search;
pub mod settings;
pub mod storage;
pub mod tags;
