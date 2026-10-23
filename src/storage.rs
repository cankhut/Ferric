//! Where the library lives on disk, and how it gets there.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::album::Album;

/// What the app was called before, and where it kept its files. Kept so
/// that a library built up under the old name is not lost when it changes.
const OLDER_NAME: &str = "crate-digger";

/// The folder everything an app owns goes under, following what each
/// system expects. `None` when there is nothing to go on at all.
///
/// Done with environment variables instead of a crate, because it is a
/// dozen lines and one less dependency to keep up to date.
fn base_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return Some(PathBuf::from(appdata));
        }
    }

    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Some(PathBuf::from(data_home));
    }

    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".local/share"));
    }

    None
}

/// This app's own folder:
///
/// - Windows: `%APPDATA%\ferric`
/// - macOS and Linux: `$XDG_DATA_HOME/ferric`, or `~/.local/share/ferric`
pub fn data_dir() -> PathBuf {
    match base_dir() {
        Some(base) => base.join(env!("CARGO_PKG_NAME")),
        // Nothing to go on, so the current folder it is
        None => PathBuf::from(".ferric"),
    }
}

pub fn library_path() -> PathBuf {
    data_dir().join("library.json")
}

/// What you have chosen rather than what you have collected, so that
/// deleting one never costs you the other.
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Takes over the files the app left behind under its old name, once, the
/// first time this version runs. Without this, renaming the app would look
/// to you exactly like losing your library.
///
/// The old folder is copied rather than moved, so an older build still
/// opens its own copy and nothing is destroyed if this goes wrong. Once
/// you are happy, the old folder can be deleted by hand.
pub fn adopt_older_files() -> Result<bool> {
    let Some(base) = base_dir() else {
        return Ok(false);
    };

    adopt_from(&base.join(OLDER_NAME), &data_dir())
}

/// Kept apart from the folder names so it can be tested on real files.
fn adopt_from(older: &Path, now: &Path) -> Result<bool> {
    // Anything here already is the live copy, and is never written over
    if now.join("library.json").exists() || !older.is_dir() {
        return Ok(false);
    }

    let mut adopted = false;

    for name in ["library.json", "settings.json"] {
        let from = older.join(name);

        if !from.is_file() {
            continue;
        }

        std::fs::create_dir_all(now)
            .with_context(|| format!("could not create {}", now.display()))?;

        std::fs::copy(&from, now.join(name))
            .with_context(|| format!("could not copy {}", from.display()))?;

        adopted = true;
    }

    Ok(adopted)
}

/// Reads the library. A file that is not there yet is not a problem, it
/// just means an empty library.
pub fn load() -> Result<Vec<Album>> {
    Ok(read_json(&library_path())?.unwrap_or_default())
}

/// Moves a library file that cannot be read out of the way, and says where
/// it went.
///
/// A library the app cannot parse used to stop it opening at all, and in a
/// release build there is no console for the error to appear in, so the app
/// simply did not start. Worse, the alternative — carrying on with an empty
/// library — would have written `[]` over the file on the next change and
/// destroyed whatever was left of it. So the broken file is kept, under a
/// name that says what it is, and the app opens on an empty shelf and tells
/// you where to look.
pub fn set_broken_library_aside() -> Result<PathBuf> {
    let path = library_path();

    let mut aside = path.with_extension(format!("broken-{}", crate::album::today()));
    let mut attempt = 1;

    // Not over the top of an earlier broken copy: that one might be the one
    // worth rescuing
    while aside.exists() {
        attempt += 1;
        aside = path.with_extension(format!("broken-{}-{attempt}", crate::album::today()));
    }

    std::fs::rename(&path, &aside)
        .with_context(|| format!("could not move {} aside", path.display()))?;

    Ok(aside)
}

/// Writes the library.
pub fn save(albums: &[Album]) -> Result<()> {
    write_json(&library_path(), albums)
}

/// Reads one of this app's JSON files. `None` means it is not there yet,
/// which is how every one of them starts.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }

    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;

    if text.trim().is_empty() {
        return Ok(None);
    }

    let value = serde_json::from_str(&text)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;

    Ok(Some(value))
}

/// Writes one of this app's JSON files. The new one is written next to the
/// real one and then renamed over it, so a crash halfway through cannot
/// leave you with half a file.
///
/// `?Sized` so that the library can be written straight from a `&[Album]`
/// without copying it into a `Vec` first: a slice has no size known at
/// compile time, and the default bound on a type parameter says it must.
pub fn write_json<T: serde::Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(value)?;

    let temporary = path.with_extension("json.tmp");

    // Written beside the real file and then moved over it, so the file you
    // own is only ever replaced by a complete one.
    //
    // The flush in the middle is what makes that true rather than merely
    // likely. The name and the bytes reach the drive separately, and a
    // rename that lands first leaves a correctly named library full of
    // nothing — every album and rating gone to a power cut. Settling the
    // bytes before the move costs one flush per save, and a save only
    // happens when you change something
    {
        let mut file = std::fs::File::create(&temporary)
            .with_context(|| format!("could not write {}", temporary.display()))?;

        file.write_all(json.as_bytes())
            .with_context(|| format!("could not write {}", temporary.display()))?;

        file.sync_all()
            .with_context(|| format!("could not finish writing {}", temporary.display()))?;
    }

    std::fs::rename(&temporary, path)
        .with_context(|| format!("could not replace {}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own, cleaned up when it is done.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("ferric-test-{name}"));

            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();

            Scratch(path)
        }

        fn dir(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_library_from_the_old_name_is_taken_over() {
        let scratch = Scratch::new("adopts");
        let (older, now) = (scratch.dir("crate-digger"), scratch.dir("ferric"));

        write(
            &older.join("library.json"),
            r#"[{"artist":"a","title":"b"}]"#,
        );
        write(&older.join("settings.json"), r#"{"discogs_token":"abc"}"#);

        assert!(adopt_from(&older, &now).unwrap());

        assert_eq!(
            std::fs::read_to_string(now.join("library.json")).unwrap(),
            r#"[{"artist":"a","title":"b"}]"#
        );
        assert!(now.join("settings.json").exists());

        // The old copy is left alone, so an older build still works and
        // nothing has been destroyed
        assert!(older.join("library.json").exists());
    }

    #[test]
    fn a_library_under_the_new_name_is_never_written_over() {
        let scratch = Scratch::new("keeps");
        let (older, now) = (scratch.dir("crate-digger"), scratch.dir("ferric"));

        write(&older.join("library.json"), "the old one");
        write(&now.join("library.json"), "the one in use");

        assert!(!adopt_from(&older, &now).unwrap());

        assert_eq!(
            std::fs::read_to_string(now.join("library.json")).unwrap(),
            "the one in use"
        );
    }

    #[test]
    fn nothing_to_take_over_is_not_an_error() {
        let scratch = Scratch::new("fresh");

        // Somebody who never ran the old version
        assert!(!adopt_from(&scratch.dir("crate-digger"), &scratch.dir("ferric")).unwrap());

        // Or ran it, but never added an album
        std::fs::create_dir_all(scratch.dir("crate-digger")).unwrap();

        assert!(!adopt_from(&scratch.dir("crate-digger"), &scratch.dir("ferric")).unwrap());
    }
}
