//! crate-digger, the terminal version. The window version is in
//! src/main.rs; both use the same library and the same JSON file.
//!
//! The loop is the whole program: draw the screen, do any job that needs
//! the network, wait for a key, repeat.

mod app;
mod ui;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;

use crate::app::App;
use crate_digger::library::Library;

fn main() -> Result<()> {
    // --where prints the library file, which saves hunting for it
    if std::env::args().any(|argument| argument == "--where") {
        println!("{}", crate_digger::storage::library_path().display());
        return Ok(());
    }

    let library = Library::load()?;
    let mut app = App::new(library);

    // Takes over the terminal, and puts it back on the way out even if
    // something panics
    let mut terminal = ratatui::init();

    let result = run(&mut terminal, &mut app);

    ratatui::restore();

    // Saving again at the end costs nothing and has saved many a library
    if let Err(error) = app.library.save() {
        eprintln!("Could not save the library: {error}");
    }

    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Anything that talks to a website happens here, right after the
        // screen was drawn, so "Searching..." is on screen while it waits
        if let Some(job) = app.job.take() {
            app.run_job(job);
            continue;
        }

        if app.should_quit {
            return Ok(());
        }

        // Waking up a few times a second keeps the app responsive without
        // spinning the processor
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                // Windows sends both a press and a release, and acting on
                // both would run every key twice
                if key.kind == KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    }
}
