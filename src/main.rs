//! Ferric: a window for rating the albums you listen to.
//!
//! Ferric oxide is rust, and it is what magnetic recording tape is coated
//! with, which is the whole joke.

// No console window behind the app on Windows, the way a desktop app
// should start. Debug builds keep it, because that is where panics print
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gui;

use anyhow::{anyhow, Result};

use ferric::library::{Library, Sort};
use ferric::settings::Settings;
use ferric::storage;

fn main() -> Result<()> {
    // --where prints the folder your library and settings live in, which
    // saves hunting for it
    if std::env::args().any(|argument| argument == "--where") {
        println!("{}", storage::library_path().display());
        println!("{}", storage::settings_path().display());
        return Ok(());
    }

    // This app used to be called crate-digger. If that is where your
    // albums still are, they are brought over before anything reads them,
    // so the new name does not look like a lost library
    if let Err(error) = storage::adopt_older_files() {
        eprintln!("could not bring the older files over: {error}");
    }

    let settings = Settings::load();

    // A library that cannot be read must not stop the app opening. In a
    // release build there is no console, so returning the error here meant
    // double-clicking the app did nothing at all, with no clue why
    let (mut library, trouble) = match Library::load() {
        Ok(library) => (library, None),
        Err(error) => {
            match storage::set_broken_library_aside() {
                Ok(path) => (
                    Library::empty(false),
                    Some(format!(
                        "Your library could not be read ({error}). It has been kept as {} \
                         and the app opened empty. Nothing was deleted.",
                        path.display()
                    )),
                ),
                // It could not even be moved, so nothing may be written over
                // it: the file is still there and might be worth rescuing
                Err(_) => (
                    Library::empty(true),
                    Some(format!(
                        "Your library could not be read ({error}). The app opened empty and \
                         will not save anything, so the file is left as it is, in {}",
                        storage::data_dir().display()
                    )),
                ),
            }
        }
    };

    library.sort = settings.sort.unwrap_or(Sort::Added);

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([720.0, 480.0])
        .with_title("Ferric")
        .with_icon(icon());

    // Where you left it last time, unless that was somewhere a window can
    // no longer be, such as a monitor that is not plugged in any more
    if let Some(window) = settings.window.filter(|window| window.sensible()) {
        viewport = viewport
            .with_inner_size([window.width, window.height])
            .with_position([window.x, window.y]);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "ferric",
        options,
        Box::new(move |cc| Ok(Box::new(gui::Gui::new(cc, library, settings, trouble)))),
    )
    .map_err(|error| anyhow!("could not open the window: {error}"))?;

    Ok(())
}

/// The title bar, Alt+Tab and the taskbar. The icon on the .exe itself is
/// a separate thing, set by `build.rs` when the app is built on Windows.
///
/// Decoded here rather than with `eframe::icon_data`, so this depends only
/// on the `image` crate the app already carries. A broken icon is not
/// worth refusing to open the window over, so a failure gives the plain
/// default instead.
fn icon() -> eframe::egui::IconData {
    let Ok(decoded) = image::load_from_memory(include_bytes!("../assets/icon.png")) else {
        return eframe::egui::IconData::default();
    };

    let decoded = decoded.into_rgba8();
    let (width, height) = decoded.dimensions();

    eframe::egui::IconData {
        rgba: decoded.into_raw(),
        width,
        height,
    }
}
