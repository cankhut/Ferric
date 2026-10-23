//! Everything the app knows and everything a key press does to it.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::{ListState, TableState};

use crate_digger::library::Library;
use crate_digger::search::{self, SearchResult};
use crate_digger::tags;

/// What the screen is doing right now. Every key press is read in the
/// light of this, which is what keeps the key handling from turning into
/// one giant if.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Filter,
    Search,
    Results,
    Confirm,
    Help,
}

/// Work that needs the network. It is put here rather than done on the
/// spot so the screen can be drawn once more first, otherwise the app
/// would freeze with no hint of why.
pub enum Job {
    Search(String),
    Tags(usize),
}

pub struct App {
    pub library: Library,
    pub mode: Mode,
    pub table: TableState,

    /// What is being typed, in Filter and Search modes
    pub input: String,

    pub results: Vec<SearchResult>,
    pub results_state: ListState,

    pub status: String,
    pub job: Option<Job>,
    pub should_quit: bool,
}

impl App {
    pub fn new(library: Library) -> Self {
        let mut app = App {
            library,
            mode: Mode::Browse,
            table: TableState::default(),
            input: String::new(),
            results: Vec::new(),
            results_state: ListState::default(),
            status: String::new(),
            job: None,
            should_quit: false,
        };

        if !app.library.is_empty() {
            app.table.select(Some(0));
        }

        app.status = match app.library.len() {
            0 => "No albums yet. Press a to look one up.".to_string(),
            1 => "1 album. Press ? for the keys.".to_string(),
            count => format!("{count} albums. Press ? for the keys."),
        };

        app
    }

    /// Where the highlighted row sits in the library itself.
    pub fn selected(&self) -> Option<usize> {
        let visible = self.library.visible();
        let row = self.table.selected()?;

        visible.get(row).copied()
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.library.visible().len();

        if count == 0 {
            self.table.select(None);
            return;
        }

        let current = self.table.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, count as isize - 1);

        self.table.select(Some(next as usize));
    }

    /// Keeps the highlight on a real row after albums come and go.
    fn clamp_selection(&mut self) {
        let count = self.library.visible().len();

        if count == 0 {
            self.table.select(None);
        } else {
            let row = self.table.selected().unwrap_or(0).min(count - 1);

            self.table.select(Some(row));
        }
    }

    fn save(&mut self) {
        if let Err(error) = self.library.save() {
            self.status = format!("Could not save: {error}");
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Browse => self.browse_key(key),
            Mode::Filter => self.filter_key(key),
            Mode::Search => self.search_key(key),
            Mode::Results => self.results_key(key),
            Mode::Confirm => self.confirm_key(key),
            Mode::Help => {
                self.mode = Mode::Browse;
            }
        }
    }

    fn browse_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,

            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::Home => self.table.select(Some(0)),
            KeyCode::End => self.move_selection(isize::MAX / 2),

            // 1 to 9 rate straight away, 0 is a ten, x takes the rating off
            KeyCode::Char(digit @ '0'..='9') => {
                let rating = if digit == '0' { 10 } else { digit as u8 - b'0' };

                self.rate(Some(rating));
            }
            KeyCode::Char('x') => self.rate(None),

            KeyCode::Char('a') => {
                self.mode = Mode::Search;
                self.input.clear();
                self.status = "Type an album or artist, Enter to search, Esc to go back".into();
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.input = self.library.filter.clone();
            }
            KeyCode::Char('s') => {
                self.library.sort = self.library.sort.next();
                self.clamp_selection();
                self.status = format!("Sorted by {}", self.library.sort.label());
            }
            KeyCode::Char('t') => match self.selected() {
                Some(index) => {
                    self.status = "Looking for tags...".into();
                    self.job = Some(Job::Tags(index));
                }
                None => self.status = "Nothing selected".into(),
            },
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('d') => {
                if self.selected().is_some() {
                    self.mode = Mode::Confirm;
                } else {
                    self.status = "Nothing selected".into();
                }
            }
            KeyCode::Char('?') => self.mode = Mode::Help,

            _ => {}
        }
    }

    fn rate(&mut self, rating: Option<u8>) {
        let Some(index) = self.selected() else {
            self.status = "Nothing selected".into();
            return;
        };

        let Some(album) = self.library.get_mut(index) else {
            return;
        };

        album.rating = rating;

        let title = album.title.clone();
        let text = album.rating_text();

        self.save();

        // Rating changes the order when sorted by rating, so the highlight
        // follows the album rather than the row
        let visible = self.library.visible();

        if let Some(row) = visible.iter().position(|&candidate| candidate == index) {
            self.table.select(Some(row));
        }

        self.status = format!("{title}: {text}");
    }

    fn filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.library.filter.clear();
                self.input.clear();
                self.mode = Mode::Browse;
                self.clamp_selection();
            }
            KeyCode::Enter => {
                self.mode = Mode::Browse;
                self.status = if self.library.filter.is_empty() {
                    String::new()
                } else {
                    format!("Showing albums matching \"{}\"", self.library.filter)
                };
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.library.filter = self.input.clone();
                self.clamp_selection();
            }
            KeyCode::Char(character) => {
                self.input.push(character);
                self.library.filter = self.input.clone();
                self.clamp_selection();
            }
            _ => {}
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                self.input.clear();
            }
            KeyCode::Enter => {
                let term = self.input.trim().to_string();

                if term.is_empty() {
                    self.mode = Mode::Browse;
                    return;
                }

                self.status = format!("Searching iTunes for \"{term}\"...");
                self.job = Some(Job::Search(term));
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character) => self.input.push(character),
            _ => {}
        }
    }

    fn results_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browse;
                self.results.clear();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let next = match self.results_state.selected() {
                    Some(row) if row + 1 < self.results.len() => row + 1,
                    Some(row) => row,
                    None => 0,
                };

                self.results_state.select(Some(next));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let next = self.results_state.selected().unwrap_or(0).saturating_sub(1);

                self.results_state.select(Some(next));
            }
            KeyCode::Enter => self.add_selected_result(),
            _ => {}
        }
    }

    fn add_selected_result(&mut self) {
        let Some(row) = self.results_state.selected() else {
            return;
        };

        let Some(result) = self.results.get(row) else {
            return;
        };

        let album = result.to_album();

        if self.library.contains(&album) {
            self.status = format!("{} is already in the library", album.title);
            self.mode = Mode::Browse;
            self.results.clear();
            return;
        }

        let title = album.title.clone();

        self.library.add(album);
        self.save();

        self.mode = Mode::Browse;
        self.results.clear();
        self.input.clear();

        // Show it even if the filter would have hidden it
        self.library.filter.clear();
        self.clamp_selection();

        let visible = self.library.visible();
        let added = self.library.len() - 1;

        if let Some(row) = visible.iter().position(|&candidate| candidate == added) {
            self.table.select(Some(row));
        }

        self.status = format!("Added {title}. Press 1 to 9 to rate it, t for tags.");
    }

    fn confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => {
                if let Some(index) = self.selected() {
                    let removed = self.library.remove(index);

                    self.save();
                    self.clamp_selection();

                    if let Some(album) = removed {
                        self.status = format!("Removed {}", album.title);
                    }
                }

                self.mode = Mode::Browse;
            }
            _ => self.mode = Mode::Browse,
        }
    }

    /// Does the work that needs the network, after the screen has been
    /// drawn once so the waiting is visible.
    pub fn run_job(&mut self, job: Job) {
        match job {
            // Asking for plenty and sorting them here beats asking for a
            // handful and hoping the album you wanted is among them
            Job::Search(term) => match search::search(&term, 50) {
                Ok(results) if results.is_empty() => {
                    self.mode = Mode::Browse;
                    self.status =
                        format!("iTunes found nothing for \"{term}\". Try just the album name.");
                }
                Ok(results) => {
                    self.status = format!(
                        "{} albums · Enter to add, j and k to move, Esc to go back",
                        results.len()
                    );
                    self.results = results;
                    self.results_state.select(Some(0));
                    self.mode = Mode::Results;
                }
                Err(error) => {
                    self.mode = Mode::Browse;
                    self.status = format!("Search failed: {error}");
                }
            },
            Job::Tags(index) => {
                let Some(album) = self.library.get(index) else {
                    return;
                };

                let artist = album.artist.clone();
                let title = album.title.clone();
                let deezer_id = album.deezer_id;

                match tags::fetch(&artist, &title, deezer_id) {
                    Ok(found) if found.names.is_empty() => {
                        self.status = format!("Nobody has tagged {title}");
                    }
                    Ok(found) => {
                        if let Some(album) = self.library.get_mut(index) {
                            album.tags = found.names.clone();
                        }

                        self.save();

                        self.status = format!(
                            "{title}: {} · from {}",
                            found.names.join(", "),
                            found.source
                        );
                    }
                    Err(error) => self.status = format!("{error}"),
                }
            }
        }
    }

    /// Opens the album's page on Album of the Year, which has the things no
    /// API will give you: scores, reviews, the tracklist people argue about.
    fn open_in_browser(&mut self) {
        let Some(index) = self.selected() else {
            self.status = "Nothing selected".into();
            return;
        };

        let Some(album) = self.library.get(index) else {
            return;
        };

        let url = format!(
            "https://www.albumoftheyear.org/search/?q={}",
            crate_digger::net::encode(&format!("{} {}", album.artist, album.title))
        );

        match open_url(&url) {
            Ok(()) => self.status = format!("Opened {} in your browser", album.title),
            Err(error) => self.status = format!("Could not open the browser: {error}"),
        }
    }
}

/// Hands a URL to whatever the system uses for links. Its output goes
/// nowhere on purpose: anything it printed would land in the middle of the
/// drawn screen.
fn open_url(url: &str) -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    Ok(())
}
