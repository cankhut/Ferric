//! The window: a wall of covers you can rate, in the spirit of a certain
//! film site. Dark, quiet, the artwork doing the talking.
//!
//! egui redraws every frame, so there are no callbacks to wire up, and it
//! only redraws when something happens: moving the mouse, or a lookup
//! coming back. Nothing here runs on a timer, so the window sitting open
//! behind something else costs nothing.
//!
//! Anything that talks to a website runs on a thread and sends its answer
//! back through a channel, because a frozen window is worse than a slow one.
//!
//! The wall is laid out by hand on a fixed grid instead of by egui's
//! wrapping layout. A wrapping layout sizes each card by what is in it, so a
//! title that runs to two lines, or a card with no tags yet, puts one card
//! out of step with its neighbours. Here every card gets the same rectangle
//! and its text is cut to fit it, so a row is always a straight row.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke};

use ferric::album::{clean_title, Album};
use ferric::deezer;
use ferric::discogs;
use ferric::library::{Library, Sort};
use ferric::search::{self, SearchResult};
use ferric::settings::{Settings, Window};
use ferric::storage;
use ferric::tags;

// The palette: near black page, slightly lighter cards, muted grey text,
// and green for anything that means a rating
const BG: Color32 = Color32::from_rgb(20, 24, 28);
const PANEL: Color32 = Color32::from_rgb(28, 34, 40);
const PANEL_HOVER: Color32 = Color32::from_rgb(38, 46, 54);
const BORDER: Color32 = Color32::from_rgb(44, 52, 64);
const TEXT: Color32 = Color32::from_rgb(222, 230, 238);
const MUTED: Color32 = Color32::from_rgb(137, 154, 171);
const GREEN: Color32 = Color32::from_rgb(0, 224, 84);
const GREEN_BRIGHT: Color32 = Color32::from_rgb(130, 255, 178);
const BLUE: Color32 = Color32::from_rgb(64, 188, 244);
const STAR_EMPTY: Color32 = Color32::from_rgb(56, 66, 78);
const HINT: Color32 = Color32::from_rgb(98, 112, 128);

/// Width of the outlines: the selected card, and the line between panels
const LINE: f32 = 1.0;

// The wall
const COVER: f32 = 180.0;
const GAP_X: f32 = 24.0;
const GAP_Y: f32 = 22.0;
/// Space between the wall and the edges around it
const WALL_MARGIN_X: f32 = 16.0;
const WALL_MARGIN_Y: f32 = 14.0;
/// How far a card's highlight reaches past its cover and text
const LIFT: f32 = 8.0;

const TITLE_SIZE: f32 = 15.0;
const SMALL_SIZE: f32 = 13.0;

const STARS: u8 = 5;
/// How far apart the stars sit, and how big each one is drawn. The
/// difference between them is the gap down the middle of the gutter
const STAR_STEP: f32 = 19.0;
const STAR_SIZE: f32 = 17.0;
/// How many drawings there are in `assets/stars`
const STAR_DESIGNS: usize = 10;

/// The note in the top left
const LOGO: f32 = 30.0;

/// How long background changes are collected for before the library is
/// written. See `save_soon`: lookups come back about one a second, and
/// writing the whole file for each one would put the same few kilobytes on
/// the drive hundreds of times over for no reason.
const COLLECT_WRITES_FOR: Duration = Duration::from_secs(10);

// The album panel on the right. Its cover is always as wide as the panel,
// so wherever the edge is dragged there is no gap beside it
const DETAIL_MIN: f32 = 260.0;
const DETAIL_DEFAULT: f32 = 330.0;
const DETAIL_MAX: f32 = 540.0;
const DETAIL_MARGIN: f32 = 16.0;

// The results that drop down under the search box. They are the same
// cards as the wall, so an album looks the same before and after you add
// it, and they are laid out on the same fixed grid
const DROP_COLUMNS: usize = 4;
const DROP_MARGIN: f32 = 14.0;
const DROP_MAX_HEIGHT: f32 = 540.0;

/// How many covers stay decoded. Past this the ones not seen for longest
/// are dropped, and load again by themselves if you scroll back to them.
///
/// egui keeps every image it has ever loaded: the file, the pixels and the
/// texture on the card, roughly three quarters of a megabyte an album. A
/// few searches and a long scroll used to be hundreds of megabytes that
/// were never going to be looked at again.
const COVERS_KEPT: usize = 120;

/// The cog in the bottom corner
const COG: f32 = 9.0;

/// How tall everything in the top bar is, so the search box, the sort menu
/// and the filter all line up instead of each sizing itself
const TOP_ROW: f32 = 24.0;

/// Which album an answer is for. Its position in the library can change
/// while the answer is on its way, if an album before it is removed. Its
/// artist and title cannot.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    artist: String,
    title: String,
}

impl Key {
    fn of(album: &Album) -> Self {
        Key {
            artist: album.artist.clone(),
            title: album.title.clone(),
        }
    }
}

/// Why a lookup is happening, which decides two things: whether it goes
/// ahead of the background ones, and whether what comes back is allowed to
/// replace tags the album already has.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Why {
    /// You clicked Fetch tags, or added the album
    Asked,
    /// An older album had none, filled in after the window opened
    Fill,
    /// Every album, asked again, because the sources changed
    Refresh,
}

impl Why {
    /// Background lookups wait behind anything you asked for yourself, and
    /// leave the status line alone while they do it.
    fn quiet(self) -> bool {
        self != Why::Asked
    }

    /// Filling in only touches albums that have nothing. The other two are
    /// asking for a better answer than the one already there.
    fn replaces(self) -> bool {
        self != Why::Fill
    }
}

/// A question about one album, for the lookup thread.
enum Lookup {
    Tags {
        key: Key,
        deezer_id: Option<u64>,
        /// Taken when the lookup is queued rather than read on the thread,
        /// so a token pasted halfway through a backfill does not change
        /// the source of some albums and not others
        discogs_token: Option<String>,
        why: Why,
    },
    Year {
        key: Key,
        deezer_id: u64,
        why: Why,
    },
}

impl Lookup {
    fn why(&self) -> Why {
        match self {
            Lookup::Tags { why, .. } | Lookup::Year { why, .. } => *why,
        }
    }

    /// The asking itself. Runs on the lookup thread, never the window's.
    fn run(self) -> Message {
        match self {
            Lookup::Tags {
                key,
                deezer_id,
                discogs_token,
                why,
            } => {
                let result =
                    tags::fetch(&key.artist, &key.title, deezer_id, discogs_token.as_deref());

                Message::Tags { key, result, why }
            }
            Lookup::Year {
                key,
                deezer_id,
                why,
            } => {
                // Deezer's search leaves the release date out. The album's
                // own page has it
                let year = deezer::details(deezer_id)
                    .ok()
                    .and_then(|details| details.year);

                Message::Year { key, year, why }
            }
        }
    }
}

/// What the threads send back.
enum Message {
    Results(Vec<SearchResult>),
    SearchFailed(String),
    Tags {
        key: Key,
        result: anyhow::Result<tags::Tags>,
        why: Why,
    },
    Year {
        key: Key,
        year: Option<u16>,
        why: Why,
    },
}

/// Albums with no tags or no year, being filled in after the window opens.
#[derive(Default)]
struct Backfill {
    /// Lookups still to come back
    left: usize,
    /// Albums that got tags out of it
    tagged: usize,
}

pub struct Gui {
    library: Library,
    settings: Settings,
    selected: Option<usize>,
    pending_remove: Option<usize>,
    /// The album removed last and where it was, for Undo
    removed: Option<(usize, Album)>,
    /// The library in memory has changed since it was last written
    unsaved: bool,
    /// When it was last written, so background changes can be collected
    /// up instead of rewriting the whole file once per album
    last_write: Instant,

    search_open: bool,
    focus_search: bool,
    search_query: String,
    searching: bool,
    results: Vec<SearchResult>,
    /// Where the search box was drawn, so the results hang under it
    search_anchor: Option<Rect>,
    /// What the results covered last frame, so a click anywhere else shuts
    /// them without swallowing the click that lands on one
    dropdown_rect: Option<Rect>,

    /// The card being dragged on a wall sorted by hand, by its place in
    /// the view, and where the pointer was when it was let go. The wall
    /// works out which slot that lands in, since it is the only thing that
    /// knows where the grid starts
    drag_from: Option<usize>,
    drag_dropped_at: Option<Pos2>,

    /// Cover art drawn this frame, and the covers kept decoded, oldest
    /// first. egui holds on to every image it has ever loaded, so without
    /// this the app quietly grows for as long as it is left open: see
    /// `trim_covers`
    covers_drawn: Vec<String>,
    covers_kept: VecDeque<String>,

    settings_open: bool,
    /// What is in the token box, which is only copied into the settings
    /// when you press Save
    token_box: String,

    /// How wide the album panel was drawn last frame, to be saved on exit
    panel_width: Option<f32>,

    status: String,

    /// Searches and lookups have a thread each, so a search never waits
    /// behind the tags of twenty older albums
    searches: Sender<String>,
    lookups: Sender<Lookup>,
    messages: Receiver<Message>,

    /// Albums whose tags you asked for that are still on their way
    asked_for_tags: HashSet<Key>,
    backfill: Backfill,
}

impl Gui {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        library: Library,
        settings: Settings,
        trouble: Option<String>,
    ) -> Self {
        // Without this, covers never load: it is what teaches egui to read
        // an image out of a URL
        egui_extras::install_image_loaders(&cc.egui_ctx);

        let mut visuals = egui::Visuals::dark();

        visuals.panel_fill = BG;
        visuals.window_fill = PANEL;
        visuals.extreme_bg_color = Color32::from_rgb(16, 19, 23);
        visuals.override_text_color = Some(TEXT);
        visuals.hyperlink_color = BLUE;
        visuals.selection.bg_fill = Color32::from_rgb(0, 96, 40);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(LINE, BORDER);
        visuals.widgets.inactive.weak_bg_fill = PANEL;
        visuals.widgets.hovered.weak_bg_fill = PANEL_HOVER;
        visuals.widgets.active.weak_bg_fill = PANEL_HOVER;

        cc.egui_ctx.set_visuals(visuals);

        cc.egui_ctx.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 6.0);

            // egui's defaults are made for dense tool windows. This is a
            // thing you sit and look at, so everything goes up a few points
            style.text_styles = [
                (
                    egui::TextStyle::Heading,
                    FontId::new(23.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Body,
                    FontId::new(TITLE_SIZE, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Button,
                    FontId::new(15.0, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Small,
                    FontId::new(SMALL_SIZE, egui::FontFamily::Proportional),
                ),
                (
                    egui::TextStyle::Monospace,
                    FontId::new(14.0, egui::FontFamily::Monospace),
                ),
            ]
            .into();
        });

        let (answers, messages) = std::sync::mpsc::channel();

        let searches = spawn_searcher(cc.egui_ctx.clone(), answers.clone());
        let lookups = spawn_looker(cc.egui_ctx.clone(), answers);

        // Anything that went wrong before the window existed is the first
        // thing it says, rather than an empty shelf with no explanation
        let status = match (trouble, library.len()) {
            (Some(trouble), _) => trouble,
            (None, 0) => "Nothing here yet. Search for an album up at the top.".to_string(),
            (None, count) => albums(count),
        };

        let token_box = settings.discogs_token.clone();

        // An empty library has nothing to look at, so the cursor starts in
        // the search box rather than making you find it first
        let focus_search = library.is_empty();

        let mut gui = Gui {
            library,
            settings,
            selected: None,
            pending_remove: None,
            removed: None,
            unsaved: false,
            last_write: Instant::now(),
            search_open: false,
            focus_search,
            search_query: String::new(),
            searching: false,
            results: Vec::new(),
            search_anchor: None,
            dropdown_rect: None,
            drag_from: None,
            drag_dropped_at: None,
            covers_drawn: Vec::new(),
            covers_kept: VecDeque::new(),
            settings_open: false,
            token_box,
            panel_width: None,
            status,
            searches,
            lookups,
            messages,
            asked_for_tags: HashSet::new(),
            backfill: Backfill::default(),
        };

        gui.fill_in_older_albums();

        gui
    }

    /// The Discogs token, from the settings file or the environment.
    /// `None` means Discogs is skipped and MusicBrainz answers instead.
    fn discogs_token(&self) -> Option<String> {
        discogs::token(&self.settings.discogs_token)
    }

    /// Everything that changes the status line goes through here, apart
    /// from a removal, so the Undo button next to "Removed ..." disappears
    /// as soon as something else has happened.
    fn say(&mut self, text: String) {
        self.status = text;
        self.removed = None;
    }

    /// Writes the library now. Called after the status line has been set,
    /// so a failure is the last thing said and is not hidden by "Added…".
    ///
    /// This is for anything you did yourself: a rating, an album added or
    /// removed, a note you clicked away from. They happen seconds apart at
    /// the very most, and losing one to a crash would be worse than the
    /// write.
    fn save_now(&mut self) {
        match self.library.save() {
            Ok(()) => {
                self.unsaved = false;
                self.last_write = Instant::now();
            }
            // Not through `say`: an Undo still on offer stays on offer
            Err(error) => self.status = format!("Could not save: {error}"),
        }
    }

    /// Notes that the library has changed, and writes it only if it has
    /// been a while since the last time.
    ///
    /// This is for the background lookups, which come back about one a
    /// second and each change one album. Writing the whole file for each
    /// would rewrite it from end to end hundreds of times in a row — the
    /// same few kilobytes of album going to the drive over and over for no
    /// reason. Whatever is still pending is written when the run finishes,
    /// when you touch anything yourself, and when the window closes, so
    /// nothing is lost by waiting.
    fn save_soon(&mut self) {
        self.unsaved = true;

        if write_is_due(self.last_write.elapsed()) {
            self.save_now();
        }
    }

    /// Writes anything still pending. Doing nothing when there is nothing
    /// to write is the point: it is called on every way out of the app.
    fn save_pending(&mut self) {
        if self.unsaved {
            self.save_now();
        }
    }

    fn album_mut(&mut self, key: &Key) -> Option<&mut Album> {
        let index = self.library.position_of(&key.artist, &key.title)?;

        self.library.get_mut(index)
    }

    /// Opens an album in the panel, or closes the panel with None. A note
    /// typed into the panel is written before it changes album.
    fn select(&mut self, index: Option<usize>) {
        self.save_pending();

        self.selected = index;
    }

    /// Albums are taken out between frames. Doing it while the wall is being
    /// drawn would shift every position after this one, and the rest of the
    /// frame would show the wrong covers.
    fn apply_removal(&mut self) {
        let Some(index) = self.pending_remove.take() else {
            return;
        };

        let Some(album) = self.library.remove(index) else {
            return;
        };

        // Whatever is open in the panel stays open, unless it was this one
        self.selected = match self.selected {
            Some(selected) if selected == index => None,
            Some(selected) if selected > index => Some(selected - 1),
            other => other,
        };

        self.status = format!("Removed {}", album.title);
        self.removed = Some((index, album));
        self.save_now();
    }

    fn undo_removal(&mut self) {
        let Some((index, album)) = self.removed.take() else {
            return;
        };

        self.say(format!("Put {} back", album.title));
        self.library.insert(index, album);

        if let Some(selected) = self.selected {
            if selected >= index {
                self.selected = Some(selected + 1);
            }
        }

        self.save_now();
    }

    /// Albums added before tags were looked up by themselves have none,
    /// and some that came from Deezer have no year. They are filled in
    /// quietly, one at a time, behind anything you ask for.
    fn fill_in_older_albums(&mut self) {
        self.queue_tags(Why::Fill);
    }

    /// Looks up every album `why` calls for, in the background. On startup
    /// that is the ones with no tags; from Settings it is all of them.
    fn queue_tags(&mut self, why: Why) {
        let discogs_token = self.discogs_token();
        let mut lookups = Vec::new();

        for index in 0..self.library.len() {
            let Some(album) = self.library.get(index) else {
                continue;
            };

            if let (None, Some(deezer_id)) = (album.year, album.deezer_id) {
                lookups.push(Lookup::Year {
                    key: Key::of(album),
                    deezer_id,
                    why,
                });
            }

            if why.replaces() || (album.tags.is_empty() && !album.looked_up) {
                lookups.push(Lookup::Tags {
                    key: Key::of(album),
                    deezer_id: album.deezer_id,
                    discogs_token: discogs_token.clone(),
                    why,
                });
            }
        }

        self.backfill.left += lookups.len();

        for lookup in lookups {
            let _ = self.lookups.send(lookup);
        }
    }

    /// Picks up whatever the threads have finished.
    fn collect_answers(&mut self) {
        while let Ok(message) = self.messages.try_recv() {
            match message {
                Message::Results(results) => {
                    self.searching = false;

                    let text = match results.len() {
                        0 => "Nothing found. Try just the album name.".to_string(),
                        count => format!("{} found", albums(count)),
                    };

                    self.say(text);
                    self.results = results;
                }
                Message::SearchFailed(error) => {
                    self.searching = false;
                    self.say(format!("Search failed: {error}"));
                }
                Message::Tags { key, result, why } => self.tags_arrived(key, result, why),
                Message::Year { key, year, why } => self.year_arrived(key, year, why),
            }
        }
    }

    fn tags_arrived(&mut self, key: Key, result: anyhow::Result<tags::Tags>, why: Why) {
        if !why.quiet() {
            self.asked_for_tags.remove(&key);
        }

        // An answer of "nobody has tagged this" is still an answer, and is
        // remembered so the album is not asked about again on every launch.
        // A lookup that failed is not: that one deserves another go
        if result.is_ok() {
            if let Some(album) = self.album_mut(&key) {
                if !album.looked_up {
                    album.looked_up = true;
                    self.save_soon();
                }
            }
        }

        let found = match result {
            Ok(found) if !found.names.is_empty() => Some(found),
            Ok(_) => {
                if !why.quiet() {
                    self.say(format!("Nobody has tagged {} yet", key.title));
                }

                None
            }
            Err(error) => {
                if !why.quiet() {
                    self.say(format!("Could not fetch tags for {}: {error}", key.title));
                }

                None
            }
        };

        if let Some(found) = found {
            let stored = match self.album_mut(&key) {
                // Filling in never replaces tags that arrived some other
                // way while this lookup was out
                Some(album) if why.replaces() || album.tags.is_empty() => {
                    album.tags = found.names.clone();
                    true
                }
                _ => false,
            };

            if stored {
                if why.quiet() {
                    self.backfill.tagged += 1;
                    self.save_soon();
                } else {
                    self.say(format!(
                        "{} · from {}",
                        found.names.join(", "),
                        found.source
                    ));

                    self.save_now();
                }
            }
        }

        if why.quiet() {
            self.backfill_step(why);
        }
    }

    fn year_arrived(&mut self, key: Key, year: Option<u16>, why: Why) {
        let changed = match (year, self.album_mut(&key)) {
            (Some(year), Some(album)) if album.year.is_none() => {
                album.year = Some(year);
                true
            }
            _ => false,
        };

        if changed {
            if why.quiet() {
                self.save_soon();
            } else {
                self.save_now();
            }
        }

        if why.quiet() {
            self.backfill_step(why);
        }
    }

    /// One background lookup has come back. Says what they found once the
    /// last one has.
    fn backfill_step(&mut self, why: Why) {
        self.backfill.left = self.backfill.left.saturating_sub(1);

        if self.backfill.left > 0 {
            return;
        }

        // The last one is back, so whatever the run collected goes to the
        // file now rather than waiting for you to do something
        self.save_pending();

        if self.backfill.tagged == 0 {
            return;
        }

        let text = match why {
            Why::Refresh => format!("Looked up {} again", albums(self.backfill.tagged)),
            _ => format!(
                "Found tags for {} that had none",
                albums(self.backfill.tagged)
            ),
        };

        self.backfill.tagged = 0;
        self.say(text);
    }

    fn start_search(&mut self) {
        let query = self.search_query.trim().to_string();

        if query.is_empty() || self.searching {
            return;
        }

        self.searching = true;
        self.results.clear();
        self.say(format!("Searching for \"{query}\"..."));

        if self.searches.send(query).is_err() {
            self.searching = false;
            self.say("Searching has stopped working. Restart the app.".to_string());
        }
    }

    /// Tags for one album, asked for now rather than in the background.
    fn ask_for_tags(&mut self, key: Key, deezer_id: Option<u64>) {
        // Already on its way: a second click would only ask twice
        if !self.asked_for_tags.insert(key.clone()) {
            return;
        }

        self.say(format!("Looking for tags for {}...", key.title));

        let _ = self.lookups.send(Lookup::Tags {
            key,
            deezer_id,
            discogs_token: self.discogs_token(),
            why: Why::Asked,
        });
    }

    fn fetch_tags(&mut self, index: usize) {
        let Some(album) = self.library.get(index) else {
            return;
        };

        let key = Key::of(album);
        let deezer_id = album.deezer_id;

        self.ask_for_tags(key, deezer_id);
    }

    fn set_rating(&mut self, index: usize, rating: Option<u8>) {
        if let Some(album) = self.library.get_mut(index) {
            album.rating = rating;
        }

        self.save_now();
    }

    fn add(&mut self, result: &SearchResult) {
        let album = result.to_album();

        if self.library.contains(&album) {
            self.say(format!("{} is already in your library", album.title));
            return;
        }

        let key = Key::of(&album);
        let title = album.title.clone();
        let deezer_id = album.deezer_id;
        let needs_year = album.year.is_none();

        self.library.add(album);

        if let (true, Some(deezer_id)) = (needs_year, deezer_id) {
            let _ = self.lookups.send(Lookup::Year {
                key: key.clone(),
                deezer_id,
                why: Why::Asked,
            });
        }

        // Nobody wants to ask for tags album by album, so adding one looks
        // them up straight away
        self.ask_for_tags(key, deezer_id);
        self.say(format!("Added {title}. Looking for its tags..."));
        self.save_now();
    }

    // The screen, top to bottom

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        // One height for everything interactive up here. Without it the
        // text box and the sort menu size themselves from their own
        // contents, and a shorter box sits low against a taller menu
        ui.spacing_mut().interact_size.y = TOP_ROW;

        ui.horizontal(|ui| {
            // The name is in the title bar and on the taskbar, so the
            // note stands for it here. Hovering says it out loud, for
            // anyone meeting the app for the first time
            ui.add(
                // The note is drawn white and stays white. Tinting it green
                // tied the one hand-drawn thing in the window to the accent
                // colour, and it reads as part of the furniture rather than
                // as the app's own mark
                egui::Image::new(egui::include_image!("../assets/logo.png"))
                    .fit_to_exact_size(egui::vec2(LOGO, LOGO)),
            )
            .on_hover_text("Ferric");

            ui.add_space(12.0);

            // Looking an album up happens here rather than in a window of
            // its own: you type, press Enter, and the results fall open
            // underneath, the way a search box anywhere else behaves
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.search_query)
                    .desired_width(260.0)
                    .hint_text(RichText::new("add an album: artist, title, or both").color(HINT)),
            );

            // Kept so the results know where to hang
            self.search_anchor = Some(response.rect);

            if self.focus_search {
                response.request_focus();
                self.focus_search = false;
            }

            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.start_search();
                self.search_open = true;

                // Enter leaves the box, and typing the next search should
                // not need a click to get back into it
                response.request_focus();
            }

            // Coming back to a search you already ran shows it again
            if response.gained_focus() && !self.results.is_empty() {
                self.search_open = true;
            }

            if self.searching {
                ui.add_space(6.0);
                ui.spinner();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let average = match self.library.average_rating() {
                    Some(average) => format!("{:.1} average", average / 2.0),
                    None => "nothing rated yet".to_string(),
                };

                ui.label(RichText::new(average).color(MUTED).small());
                ui.label(
                    RichText::new(albums(self.library.len()))
                        .color(MUTED)
                        .small(),
                );

                ui.add_space(12.0);

                // The filter belongs next to the count it changes: both are
                // about what the wall is showing you right now
                ui.add(
                    egui::TextEdit::singleline(&mut self.library.filter)
                        .desired_width(180.0)
                        .hint_text(RichText::new("filter").color(HINT)),
                );

                // Kept from before the menu is drawn: switching to the
                // custom order should hand it the wall as it looks right
                // now, and a moment later the view is already the new one
                let was = self.library.sort;
                let order_before = self.library.visible();

                egui::ComboBox::from_label("")
                    .selected_text(self.library.sort.label())
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for sort in Sort::ALL {
                            ui.selectable_value(&mut self.library.sort, sort, sort.label());
                        }
                    });

                if self.library.sort != was && self.library.sort == Sort::Custom {
                    self.library.seed_custom_order(&order_before);
                    self.save_now();
                }
            });
        });
    }

    fn wall(&mut self, ui: &mut egui::Ui) {
        let visible = self.library.visible();

        if visible.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.label(
                    RichText::new(if self.library.is_empty() {
                        "No albums yet. Search for one up at the top."
                    } else {
                        "Nothing matches the filter."
                    })
                    .color(MUTED),
                );
            });

            return;
        }

        let title_font = FontId::proportional(TITLE_SIZE);
        let small_font = FontId::proportional(SMALL_SIZE);

        let layout = ui.fonts(|fonts| {
            CardLayout::new(fonts.row_height(&title_font), fonts.row_height(&small_font))
        });

        // Not shrunk to fit the cards: the scroll area keeps the full width,
        // which puts its scroll bar on the edge of the window
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let width = ui.available_width();
                let usable = width - 2.0 * WALL_MARGIN_X;

                let columns = columns_for(usable);
                let rows = visible.len().div_ceil(columns);

                let (area, _) = ui.allocate_exact_size(
                    egui::vec2(
                        width,
                        span(rows, layout.height, GAP_Y) + 2.0 * WALL_MARGIN_Y,
                    ),
                    egui::Sense::hover(),
                );

                // The grid sits in the middle, the way a gallery hangs
                // pictures, and fills from the left
                let spare = (usable - span(columns, COVER, GAP_X)).max(0.0);

                let origin = egui::pos2(
                    (area.left() + WALL_MARGIN_X + spare / 2.0).round(),
                    area.top() + WALL_MARGIN_Y,
                );

                for (position, &index) in visible.iter().enumerate() {
                    let (x, y) = card_offset(position, columns, layout.height);

                    let rect = Rect::from_min_size(
                        origin + egui::vec2(x, y),
                        egui::vec2(COVER, layout.height),
                    );

                    // Cards scrolled out of sight are not drawn at all
                    if ui.is_rect_visible(rect.expand(LIFT)) {
                        self.card(ui, position, index, rect, layout);
                    }
                }

                // Only the wall knows where the grid starts, so working out
                // which slot the pointer is over belongs here rather than in
                // the card being dragged
                if self.drag_from.is_some() {
                    let slot =
                        |at: Pos2| slot_at(at, origin, columns, layout.height, visible.len());

                    // A line where the card would land, so a drag says what
                    // it is about to do before you let go
                    if let Some(pointer) = ui.ctx().input(|input| input.pointer.interact_pos()) {
                        let (x, y) = card_offset(slot(pointer), columns, layout.height);
                        let edge = origin + egui::vec2(x - GAP_X / 2.0, y);

                        ui.painter().line_segment(
                            [edge, edge + egui::vec2(0.0, layout.height)],
                            Stroke::new(2.0_f32, GREEN),
                        );
                    }

                    if let (Some(from), Some(at)) = (self.drag_from, self.drag_dropped_at) {
                        if self.library.move_in_custom_order(from, slot(at)) {
                            self.save_now();
                        }

                        self.drag_from = None;
                        self.drag_dropped_at = None;
                    }
                }
            });
    }

    fn card(
        &mut self,
        ui: &egui::Ui,
        position: usize,
        index: usize,
        rect: Rect,
        layout: CardLayout,
    ) {
        // Copied out before drawing, so the album is not borrowed while the
        // stars underneath want to change it
        let Some(album) = self.library.get(index) else {
            return;
        };

        let title = shown_title(&album.title);
        let full_title = album.title.clone();
        let artist = album.artist.clone();
        let meta = match album.year {
            Some(year) => format!("{} · {year}", album.artist),
            None => album.artist.clone(),
        };
        let cover = album.cover_url.clone();
        let rating = album.rating;
        let tag_line = album.tags.join(" · ");
        let designs = designs_for(seed_of(&album.artist, &album.title));
        let selected = self.selected == Some(index);

        // Dragging a card only means something when the wall is in the one
        // order the app does not decide. Under any other sort the card
        // would spring straight back, so it does not pick up at all
        let by_hand = self.library.sort == Sort::Custom;

        let id = ui.id().with(("card", index));
        let lit_rect = rect.expand(LIFT);

        // Made before the stars, so the stars sit on top of it and keep
        // their clicks to themselves
        let response = ui
            .interact(
                lit_rect,
                id,
                if by_hand {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::click()
                },
            )
            .on_hover_cursor(if by_hand {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::PointingHand
            });

        if by_hand {
            if response.drag_started() {
                self.drag_from = Some(position);
            }

            // Where it was let go. The wall turns that into a slot
            if response.drag_stopped() {
                self.drag_dropped_at = response.interact_pointer_pos();
            }
        }

        let being_dragged = by_hand && self.drag_from == Some(position);

        // Lit while the pointer is anywhere on the card, stars included,
        // and fading rather than snapping, which is the whole difference
        // between a highlight and a flicker
        let lit = ui
            .ctx()
            .animate_bool_with_time(id, ui.rect_contains_pointer(lit_rect), 0.12);

        if selected || lit > 0.0 {
            let fill = if selected {
                mix(PANEL, PANEL_HOVER, lit)
            } else {
                mix(BG, PANEL, lit)
            };

            ui.painter().rect_filled(lit_rect, 8.0, fill);
        }

        if selected {
            ui.painter()
                .rect_stroke(lit_rect, 8.0, Stroke::new(LINE, GREEN));
        }

        // The card you have hold of, marked so it is obvious which one is
        // about to move when the line shows where it will land
        if being_dragged {
            ui.painter()
                .rect_filled(lit_rect, 8.0, mix(PANEL, PANEL_HOVER, 1.0));
            ui.painter()
                .rect_stroke(lit_rect, 8.0, Stroke::new(LINE, GREEN_BRIGHT));
        }

        let cover_rect = Rect::from_min_size(rect.min, egui::vec2(COVER, COVER));

        match &cover {
            Some(url) => {
                egui::Image::new(url.as_str())
                    .rounding(4.0)
                    .paint_at(ui, cover_rect);

                self.covers_drawn.push(url.clone());
            }
            None => no_cover(ui.painter(), cover_rect),
        }

        let at = |offset: f32| egui::pos2(rect.left(), rect.top() + offset);

        let cut_title = text_line(ui, at(layout.title_y), &title, TITLE_SIZE, TEXT, COVER);
        let cut_meta = text_line(ui, at(layout.meta_y), &meta, SMALL_SIZE, MUTED, COVER);

        // An album with no tags yet leaves this line empty, and the card
        // stays the same height as the rest
        text_line(ui, at(layout.tags_y), &tag_line, SMALL_SIZE, MUTED, COVER);

        let stars_rect = Rect::from_min_size(
            at(layout.stars_y),
            egui::vec2(STAR_STEP * STARS as f32, STAR_SIZE),
        );

        let stars = ui
            .interact(stars_rect, id.with("stars"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);

        self.stars(ui, &stars, stars_rect, index, rating, designs);

        // The whole name, when the card had to shorten it
        let response = if cut_title || cut_meta || title != full_title {
            response.on_hover_text(format!("{full_title}\n{artist}"))
        } else {
            response
        };

        if response.clicked() {
            self.select(if selected { None } else { Some(index) });
        }

        response.context_menu(|ui| {
            if ui.button("Fetch tags").clicked() {
                self.fetch_tags(index);
                ui.close_menu();
            }

            ui.separator();

            if ui.button("Remove from library").clicked() {
                self.pending_remove = Some(index);
                ui.close_menu();
            }
        });
    }

    /// Five stars, each one two clicks wide: the left half of the third is
    /// a two and a half, the right half a three. The library still stores
    /// 1 to 10, so nothing you rated before has changed.
    ///
    /// The stars are drawings rather than shapes, and `designs` says which
    /// drawing each place in the row uses.
    fn stars(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: Rect,
        index: usize,
        rating: Option<u8>,
        designs: [usize; STARS as usize],
    ) {
        // Hovering shows what the click would give you
        let hovered = response
            .hover_pos()
            .filter(|position| rect.contains(*position))
            .map(|position| rating_at(position.x, rect.left(), STAR_STEP));

        let shown = hovered.or(rating).unwrap_or(0);
        let colour = if hovered.is_some() {
            GREEN_BRIGHT
        } else {
            GREEN
        };

        for (star, &design) in designs.iter().enumerate() {
            let whole = Rect::from_min_size(
                egui::pos2(rect.left() + STAR_STEP * star as f32, rect.top()),
                egui::vec2(STAR_SIZE, STAR_SIZE),
            );

            // The drawing is white, so a tint is the whole of its colour
            egui::Image::new(star_design(design))
                .tint(STAR_EMPTY)
                .paint_at(ui, whole);

            // 0, 1 or 2 halves of this star are filled
            let halves = shown.saturating_sub(star as u8 * 2).min(2);

            if halves == 0 {
                continue;
            }

            let part = halves as f32 / 2.0;

            // Half a star is the left half of the same drawing: the left
            // half of the picture, painted into the left half of the
            // square. Clipping the shape rather than drawing a different
            // one is what keeps a half star the same star
            egui::Image::new(star_design(design))
                .uv(Rect::from_min_max(
                    egui::pos2(0.0, 0.0),
                    egui::pos2(part, 1.0),
                ))
                .tint(colour)
                .paint_at(
                    ui,
                    Rect::from_min_size(whole.min, egui::vec2(STAR_SIZE * part, STAR_SIZE)),
                );
        }

        if response.clicked() {
            if let Some(position) = response.interact_pointer_pos() {
                let value = rating_at(position.x, rect.left(), STAR_STEP);

                // Clicking the rating it already has takes it off again
                let next = if rating == Some(value) {
                    None
                } else {
                    Some(value)
                };

                self.set_rating(index, next);
            }
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(album) = self.library.get(index) else {
            self.selected = None;
            return;
        };

        let artist = album.artist.clone();
        let title = album.title.clone();
        let year = album.year;
        let cover = album.cover_url.clone();
        let tags = album.tags.clone();
        let rating = album.rating;
        let added = album.added_on.clone();
        let mut note = album.note.clone().unwrap_or_default();
        let designs = designs_for(seed_of(&album.artist, &album.title));
        let looking = self.asked_for_tags.contains(&Key::of(album));

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Everything in here takes its width from the panel, never
                // the other way round. Anything wider than the panel would
                // hold its edge in place, which is what the tags used to do
                let width = ui.available_width();

                match &cover {
                    Some(url) => {
                        ui.add(
                            egui::Image::new(url.as_str())
                                .fit_to_exact_size(egui::vec2(width, width))
                                .rounding(6.0),
                        );
                    }
                    None => {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(width, width), egui::Sense::hover());

                        no_cover(ui.painter(), rect);
                    }
                }

                ui.add_space(12.0);
                ui.label(RichText::new(&title).size(20.0).color(TEXT));
                ui.label(RichText::new(&artist).color(MUTED));

                if let Some(year) = year {
                    ui.label(RichText::new(year.to_string()).color(MUTED).small());
                }

                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(STAR_STEP * STARS as f32, STAR_SIZE),
                        egui::Sense::click(),
                    );

                    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

                    self.stars(ui, &response, rect, index, rating, designs);

                    let (text, colour) = match rating {
                        Some(rating) => (format!("{:.1}", rating as f32 / 2.0), GREEN),
                        None => ("not rated".to_string(), MUTED),
                    };

                    ui.label(RichText::new(text).color(colour).small());
                });

                ui.add_space(12.0);

                if tags.is_empty() {
                    let text = if looking {
                        "looking for tags..."
                    } else {
                        "no tags yet"
                    };

                    ui.label(RichText::new(text).color(MUTED).small());
                } else {
                    // Each tag measures itself before it is placed, so a
                    // row that is full sends the next one down a line
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);

                        for tag in &tags {
                            pill(ui, tag, width);
                        }
                    });
                }

                ui.add_space(14.0);
                ui.label(RichText::new("note").color(MUTED).small());

                let response = ui.add(
                    egui::TextEdit::multiline(&mut note)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY)
                        .hint_text(RichText::new("Add your comments.").color(HINT)),
                );

                if response.changed() {
                    if let Some(album) = self.library.get_mut(index) {
                        album.note = if note.trim().is_empty() {
                            None
                        } else {
                            Some(note.clone())
                        };
                    }

                    self.unsaved = true;
                }

                // Written when you click away rather than on every letter.
                // Opening another album or closing the window writes it too
                if response.lost_focus() && self.unsaved {
                    self.save_now();
                }

                ui.add_space(14.0);

                ui.horizontal_wrapped(|ui| {
                    if ui.button("Fetch tags").clicked() {
                        self.fetch_tags(index);
                    }

                    if ui.button("Remove from library").clicked() {
                        self.pending_remove = Some(index);
                    }
                });

                ui.add_space(10.0);
                ui.label(RichText::new(format!("added {added}")).color(MUTED).small());
            });
    }

    /// The results, hanging under the search box.
    ///
    /// Not a window: there is no title bar to drag, nothing to close, and
    /// it goes away the moment you look elsewhere. The albums in it are the
    /// same cards as the wall, so one you are about to add looks exactly
    /// like one you already have.
    fn search_dropdown(&mut self, ctx: &egui::Context) {
        let Some(anchor) = self.search_anchor else {
            return;
        };

        let screen = ctx.screen_rect();

        let title_font = FontId::proportional(TITLE_SIZE);
        let small_font = FontId::proportional(SMALL_SIZE);

        let layout = ctx.fonts(|fonts| {
            CardLayout::adding(fonts.row_height(&title_font), fonts.row_height(&small_font))
        });

        // As wide as four cards, or as much of that as the window has
        let widest = (screen.right() - anchor.left() - DROP_MARGIN).max(COVER + 2.0 * DROP_MARGIN);
        let width = (span(DROP_COLUMNS, COVER, GAP_X) + 2.0 * DROP_MARGIN).min(widest);

        let columns = columns_for(width - 2.0 * DROP_MARGIN);
        let rows = self.results.len().div_ceil(columns.max(1)).max(1);

        // Tall enough for what is in it, never taller than the window
        let wanted = span(rows, layout.height, GAP_Y) + 2.0 * DROP_MARGIN;
        let room = screen.bottom() - anchor.bottom() - 2.0 * DROP_MARGIN;
        let height = wanted.min(DROP_MAX_HEIGHT).min(room.max(120.0));

        let frame = egui::Frame::none()
            .fill(PANEL)
            .rounding(10.0)
            .stroke(Stroke::new(LINE, BORDER))
            .inner_margin(egui::Margin::same(DROP_MARGIN));

        let mut chosen = None;

        let area = egui::Window::new("results")
            .title_bar(false)
            .resizable(false)
            .movable(false)
            .collapsible(false)
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(anchor.left(), anchor.bottom() + 6.0))
            .fixed_size(egui::vec2(width, height))
            .frame(frame)
            .show(ctx, |ui| {
                if self.results.is_empty() {
                    ui.label(
                        RichText::new(if self.searching {
                            "looking..."
                        } else {
                            "Nothing found. Try the artist and the album together."
                        })
                        .color(MUTED),
                    );

                    return;
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let usable = ui.available_width();
                        let columns = columns_for(usable);
                        let rows = self.results.len().div_ceil(columns);

                        let (area, _) = ui.allocate_exact_size(
                            egui::vec2(usable, span(rows, layout.height, GAP_Y)),
                            egui::Sense::hover(),
                        );

                        for position in 0..self.results.len() {
                            let (x, y) = card_offset(position, columns, layout.height);

                            let rect = Rect::from_min_size(
                                area.min + egui::vec2(x, y),
                                egui::vec2(COVER, layout.height),
                            );

                            if ui.is_rect_visible(rect.expand(LIFT))
                                && self.result_card(ui, position, rect, layout)
                            {
                                chosen = Some(position);
                            }
                        }
                    });
            });

        // Added after the cards are drawn: adding needs all of self, and
        // the cards were borrowing the results
        if let Some(position) = chosen {
            let result = self.results[position].clone();

            self.add(&result);
        }

        self.dropdown_rect = area.map(|area| area.response.rect);
    }

    /// One search result, drawn as a card. Says whether it was clicked.
    ///
    /// The whole card is the button. Before this there was an Add button at
    /// the end of each row, which made the one place a click did anything
    /// the smallest part of the result.
    fn result_card(
        &mut self,
        ui: &egui::Ui,
        position: usize,
        rect: Rect,
        layout: CardLayout,
    ) -> bool {
        // Copied out before anything is drawn, so the results are not
        // borrowed while the covers are being noted down
        let (title, full_title, artist, meta, cover, owned) = {
            let Some(result) = self.results.get(position) else {
                return false;
            };

            (
                shown_title(&result.title),
                result.title.clone(),
                result.artist.clone(),
                match result.year {
                    Some(year) => format!("{} · {year}", result.artist),
                    None => result.artist.clone(),
                },
                result.cover_url.clone(),
                self.library.contains(&result.to_album()),
            )
        };

        let id = ui.id().with(("result", position));
        let lit_rect = rect.expand(LIFT);

        let response = ui.interact(
            lit_rect,
            id,
            if owned {
                egui::Sense::hover()
            } else {
                egui::Sense::click()
            },
        );

        let response = if owned {
            response
        } else {
            response.on_hover_cursor(egui::CursorIcon::PointingHand)
        };

        let lit = ui
            .ctx()
            .animate_bool_with_time(id, ui.rect_contains_pointer(lit_rect), 0.12);

        if lit > 0.0 {
            ui.painter()
                .rect_filled(lit_rect, 8.0, mix(PANEL, PANEL_HOVER, lit));
        }

        let cover_rect = Rect::from_min_size(rect.min, egui::vec2(COVER, COVER));

        match &cover {
            Some(url) => {
                egui::Image::new(url.as_str())
                    .rounding(4.0)
                    .paint_at(ui, cover_rect);

                self.covers_drawn.push(url.clone());
            }
            None => no_cover(ui.painter(), cover_rect),
        }

        let at = |offset: f32| egui::pos2(rect.left(), rect.top() + offset);

        let cut_title = text_line(ui, at(layout.title_y), &title, TITLE_SIZE, TEXT, COVER);
        let cut_meta = text_line(ui, at(layout.meta_y), &meta, SMALL_SIZE, MUTED, COVER);

        if owned {
            text_line(
                ui,
                at(layout.tags_y),
                "in your library",
                SMALL_SIZE,
                GREEN,
                COVER,
            );
        } else if lit > 0.0 {
            text_line(
                ui,
                at(layout.tags_y),
                "click to add",
                SMALL_SIZE,
                HINT,
                COVER,
            );
        }

        let response = if cut_title || cut_meta || title != full_title {
            response.on_hover_text(format!("{full_title}\n{artist}"))
        } else {
            response
        };

        response.clicked()
    }

    /// Drops the covers nobody has looked at for longest.
    ///
    /// egui never forgets an image by itself: the file it downloaded, the
    /// pixels it decoded and the texture on the card all stay for as long
    /// as the app is open, about three quarters of a megabyte an album. A
    /// few searches and a scroll through a large library used to be
    /// hundreds of megabytes that would never be looked at again.
    ///
    /// Covers on screen right now are never dropped, so nothing flickers.
    /// One that is dropped and then scrolled back to simply loads again.
    fn trim_covers(&mut self, ctx: &egui::Context) {
        let drawn = std::mem::take(&mut self.covers_drawn);

        // Anything drawn this frame goes to the back of the queue, so the
        // ones at the front are those not seen for longest
        for url in &drawn {
            if let Some(position) = self.covers_kept.iter().position(|kept| kept == url) {
                self.covers_kept.remove(position);
            }

            self.covers_kept.push_back(url.clone());
        }

        while self.covers_kept.len() > COVERS_KEPT {
            // Everything drawn this frame was just moved to the back, so
            // reaching one means the rest are on screen too. Dropping those
            // would only fetch them again, and flicker
            match self.covers_kept.front() {
                Some(url) if drawn.contains(url) => break,
                None => break,
                _ => {}
            }

            if let Some(url) = self.covers_kept.pop_front() {
                ctx.forget_image(&url);
            }
        }
    }

    /// Shuts the results when you are plainly done with them: Escape, or a
    /// click that landed neither on a result nor in the search box.
    fn close_dropdown_if_done(&mut self, ctx: &egui::Context) {
        if !self.search_open {
            return;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.search_open = false;
            return;
        }

        let clicked = ctx.input(|input| input.pointer.any_click());

        if !clicked {
            return;
        }

        let Some(pointer) = ctx.input(|input| input.pointer.interact_pos()) else {
            return;
        };

        let on_results = self
            .dropdown_rect
            .is_some_and(|rect| rect.contains(pointer));

        let on_box = self
            .search_anchor
            .is_some_and(|rect| rect.contains(pointer));

        if !on_results && !on_box {
            self.search_open = false;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let from_environment =
            self.settings.discogs_token.trim().is_empty() && self.discogs_token().is_some();

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Where tags come from").color(TEXT).strong());
                ui.add_space(4.0);

                ui.label(
                    RichText::new(
                        "Discogs has the best genres by a distance: styles like \
                         \"metalcore, post-hardcore\" rather than just \"rock\". \
                         Searching it needs a token, which is free and takes one click.",
                    )
                    .color(MUTED)
                    .small(),
                );

                ui.add_space(8.0);

                if ui.button("Get a token from Discogs").clicked() {
                    self.open_url_or_say("https://www.discogs.com/settings/developers");
                }

                ui.label(
                    RichText::new("Sign in, press Generate token, paste it below.")
                        .color(MUTED)
                        .small(),
                );

                ui.add_space(10.0);

                if from_environment {
                    ui.label(
                        RichText::new(
                            "Using the token in DISCOGS_TOKEN. Anything typed here wins.",
                        )
                        .color(GREEN)
                        .small(),
                    );
                    ui.add_space(4.0);
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.token_box)
                            .desired_width(260.0)
                            .password(true)
                            .hint_text(RichText::new("Discogs token").color(HINT)),
                    );

                    if ui.button("Save").clicked() {
                        self.settings.discogs_token = self.token_box.trim().to_string();

                        match self.settings.save() {
                            Ok(()) => {
                                let text = if self.discogs_token().is_some() {
                                    "Token saved. New albums will use Discogs."
                                } else {
                                    "Token cleared. MusicBrainz will answer instead."
                                };

                                self.say(text.to_string());
                            }
                            Err(error) => self.say(format!("Could not save settings: {error}")),
                        }
                    }
                });

                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Kept as plain text in settings.json, the way config files do. \
                         It is a read-only search token and you can revoke it at Discogs \
                         at any time. To keep it out of a file, leave this empty and set \
                         DISCOGS_TOKEN in your environment instead.",
                    )
                    .color(HINT)
                    .small(),
                );

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // No button while one is already running: asking twice
                // would put every album in the queue twice
                if self.backfill.left > 0 {
                    ui.label(
                        RichText::new(format!(
                            "Looking up {} now, one a second.",
                            albums(self.backfill.left)
                        ))
                        .color(GREEN)
                        .small(),
                    );
                } else {
                    if ui.button("Look up every album's tags again").clicked() {
                        self.queue_tags(Why::Refresh);
                        self.say("Looking up every album again...".to_string());
                    }

                    ui.label(
                        RichText::new(format!(
                            "Replaces what is there now, one album a second: about {} for {}.",
                            roughly(self.library.len()),
                            albums(self.library.len())
                        ))
                        .color(MUTED)
                        .small(),
                    );
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.label(RichText::new("Your files").color(TEXT).strong());
                ui.add_space(4.0);
                ui.label(
                    RichText::new(storage::data_dir().display().to_string())
                        .color(MUTED)
                        .small(),
                );
                ui.label(
                    RichText::new(
                        "library.json is your albums and ratings, settings.json is this \
                         window. Nothing else is stored, and nothing is uploaded anywhere.",
                    )
                    .color(HINT)
                    .small(),
                );
            });

        self.settings_open = open;
    }

    /// Everything worth keeping between runs, written in one go when the
    /// window closes. The token is not here: that is saved the moment you
    /// press Save, because losing it to a crash would be annoying.
    fn remember(&mut self, ctx: &egui::Context) {
        self.settings.sort = Some(self.library.sort);
        self.settings.panel_width = self.panel_width;

        // `outer_rect` is the whole window, frame included, which is what
        // a position means when it is handed back to the system
        let placed = ctx.input(|input| {
            let viewport = input.viewport();

            viewport.outer_rect.zip(viewport.inner_rect)
        });

        if let Some((outer, inner)) = placed {
            let window = Window {
                x: outer.left(),
                y: outer.top(),
                width: inner.width(),
                height: inner.height(),
            };

            if window.sensible() {
                self.settings.window = Some(window);
            }
        }

        let _ = self.settings.save();
    }

    fn open_url_or_say(&mut self, url: &str) {
        if let Err(error) = open_url(url) {
            self.say(format!("Could not open the browser: {error}"));
        }
    }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_removal();
        self.collect_answers();

        // The window is closing: this is the last frame there will be, so
        // a note still being typed and where the window ended up both get
        // written now
        if ctx.input(|input| input.viewport().close_requested()) {
            self.save_pending();
            self.remember(ctx);
        }

        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::none()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14.0, 8.0)),
            )
            .show(ctx, |ui| {
                self.top_bar(ui);
            });

        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::none()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14.0, 4.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&self.status).color(MUTED).small());

                    if self.removed.is_some() && ui.small_button("Undo").clicked() {
                        self.undo_removal();
                    }

                    // Out of the way in the corner: it is opened once to put
                    // a Discogs token in and then forgotten about
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if cog(ui, PANEL) {
                            self.token_box = self.settings.discogs_token.clone();
                            self.settings_open = true;
                        }
                    });
                });
            });

        if let Some(index) = self.selected {
            // Never so wide that not even one column of covers is left
            let widest = (ctx.screen_rect().width() - COVER - 2.0 * WALL_MARGIN_X - 2.0 * LIFT)
                .clamp(DETAIL_MIN, DETAIL_MAX);

            let opening = self
                .settings
                .panel_width
                .unwrap_or(DETAIL_DEFAULT)
                .clamp(DETAIL_MIN, widest);

            let panel = egui::SidePanel::right("detail")
                .resizable(true)
                .default_width(opening)
                .min_width(DETAIL_MIN)
                .max_width(widest)
                .frame(
                    egui::Frame::none()
                        .fill(BG)
                        .inner_margin(egui::Margin::same(DETAIL_MARGIN)),
                )
                .show(ctx, |ui| self.detail(ui, index));

            // Where you dragged it to, kept for `remember` on the way out
            self.panel_width = Some(panel.response.rect.width());
        }

        // No margin of its own: the wall keeps its own distance from the
        // edges, and its scroll bar sits right on the edge of the window
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG))
            .show(ctx, |ui| {
                self.wall(ui);
            });

        if self.search_open {
            self.search_dropdown(ctx);
        }

        if self.settings_open {
            self.settings_window(ctx);
        }

        // After everything has been drawn, so it knows what is on screen
        self.close_dropdown_if_done(ctx);
        self.trim_covers(ctx);
    }
}

impl Drop for Gui {
    fn drop(&mut self) {
        // The last chance for anything the closing frame did not catch
        if self.unsaved {
            let _ = self.library.save();
        }
    }
}

/// Searches get a thread of their own.
fn spawn_searcher(ctx: egui::Context, answers: Sender<Message>) -> Sender<String> {
    let (queries, incoming) = std::sync::mpsc::channel::<String>();

    std::thread::spawn(move || {
        // Ends by itself when the window closes and the sender goes away
        while let Ok(query) = incoming.recv() {
            let message = match search::search(&query, 40) {
                Ok(results) => Message::Results(results),
                Err(error) => Message::SearchFailed(error.to_string()),
            };

            if answers.send(message).is_err() {
                break;
            }

            // Wakes the window up, otherwise the answer sits in the channel
            // until the next time the mouse moves
            ctx.request_repaint();
        }
    });

    queries
}

/// Tags and years, one album at a time, so MusicBrainz is never asked
/// twice at once. Whatever you clicked for goes ahead of the albums being
/// filled in in the background. Nothing runs, and nothing wakes up, while
/// there is nothing to look up.
fn spawn_looker(ctx: egui::Context, answers: Sender<Message>) -> Sender<Lookup> {
    let (lookups, incoming) = std::sync::mpsc::channel::<Lookup>();

    std::thread::spawn(move || {
        let mut asked = VecDeque::new();
        let mut background = VecDeque::new();

        loop {
            // Sorts whatever arrived while the last lookup was running
            while let Ok(lookup) = incoming.try_recv() {
                if lookup.why().quiet() {
                    background.push_back(lookup);
                } else {
                    asked.push_back(lookup);
                }
            }

            let lookup = match asked.pop_front().or_else(|| background.pop_front()) {
                Some(lookup) => lookup,
                // Waits for the next one, or stops once the window has closed
                None => match incoming.recv() {
                    Ok(lookup) => lookup,
                    Err(_) => break,
                },
            };

            if answers.send(lookup.run()).is_err() {
                break;
            }

            ctx.request_repaint();
        }
    });

    lookups
}

/// The settings cog, drawn rather than written.
///
/// A gear character would depend on whichever font happened to carry it,
/// and egui's bundled fonts differ by platform. This is the solid shape
/// the Deadlock picker uses from Font Awesome: a filled body with eight
/// stubby teeth and a hole punched through the middle, rather than the
/// thin spokes that were here before and read as a sun.
fn cog(ui: &mut egui::Ui, behind: Color32) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(COG * 2.4, COG * 2.4), egui::Sense::click());

    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Settings");

    let colour = if response.hovered() { TEXT } else { MUTED };
    let middle = rect.center();
    let painter = ui.painter();

    // Short and thick, so they read as teeth on a solid wheel. They start
    // inside the body so no gap shows where they meet it
    for tooth in 0..8 {
        let angle = std::f32::consts::TAU * tooth as f32 / 8.0;
        let (sin, cos) = angle.sin_cos();
        let step = egui::vec2(cos, sin);

        painter.line_segment(
            [middle + step * (COG * 0.45), middle + step * (COG * 0.98)],
            Stroke::new(COG * 0.40, colour),
        );
    }

    painter.circle_filled(middle, COG * 0.66, colour);

    // The hole, in whatever colour sits behind the cog
    painter.circle_filled(middle, COG * 0.26, behind);

    response.clicked()
}

/// One line of text, cut short with an ellipsis if it is wider than
/// `width`, so a long title can never make a card taller. Says whether it
/// had to cut.
fn text_line(ui: &egui::Ui, at: Pos2, text: &str, size: f32, colour: Color32, width: f32) -> bool {
    if text.is_empty() {
        return false;
    }

    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::text::TextFormat::simple(FontId::proportional(size), colour),
    );

    job.wrap = egui::text::TextWrapping::truncate_at_width(width);

    let galley = ui.fonts(|fonts| fonts.layout_job(job));
    let cut = galley.elided;

    ui.painter().galley(at, galley, colour);

    cut
}

/// A tag in a rounded box. It is measured before it is placed, which is
/// what lets a wrapping row move it down a line when the panel narrows,
/// instead of the tag holding the panel open.
fn pill(ui: &mut egui::Ui, text: &str, max_width: f32) {
    let padding = egui::vec2(9.0, 4.0);

    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::text::TextFormat::simple(FontId::proportional(SMALL_SIZE), MUTED),
    );

    // A tag longer than the whole panel is shortened rather than allowed
    // to widen it
    job.wrap = egui::text::TextWrapping::truncate_at_width((max_width - padding.x * 2.0).max(24.0));

    let galley = ui.fonts(|fonts| fonts.layout_job(job));

    let (rect, _) = ui.allocate_exact_size(galley.size() + padding * 2.0, egui::Sense::hover());

    ui.painter().rect_filled(rect, 10.0, PANEL);
    ui.painter().galley(rect.min + padding, galley, MUTED);
}

fn no_cover(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 4.0, PANEL_HOVER);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "no cover",
        FontId::proportional(SMALL_SIZE),
        MUTED,
    );
}

/// Where each line of a card goes, measured down from its top. Worked out
/// from the font sizes, so a card is exactly as tall as what is in it and
/// the gap between rows is only ever `GAP_Y`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CardLayout {
    title_y: f32,
    meta_y: f32,
    stars_y: f32,
    tags_y: f32,
    height: f32,
}

impl CardLayout {
    fn new(title_row: f32, small_row: f32) -> Self {
        let title_y = COVER + 10.0;
        let meta_y = (title_y + title_row + 2.0).round();
        let stars_y = (meta_y + small_row + 7.0).round();
        let tags_y = stars_y + STAR_SIZE + 6.0;
        let height = (tags_y + small_row).ceil();

        CardLayout {
            title_y,
            meta_y,
            stars_y,
            tags_y,
            height,
        }
    }

    /// The same card without the stars, for an album you have not added.
    /// There is nothing to rate yet, and a row of empty stars only invites
    /// a click that would do nothing. The last line says whether the album
    /// is already on your wall.
    fn adding(title_row: f32, small_row: f32) -> Self {
        let title_y = COVER + 10.0;
        let meta_y = (title_y + title_row + 2.0).round();
        let tags_y = (meta_y + small_row + 4.0).round();
        let height = (tags_y + small_row).ceil();

        CardLayout {
            title_y,
            meta_y,
            // Nothing is drawn there, but the field has to say something
            stars_y: tags_y,
            tags_y,
            height,
        }
    }
}

/// How many cards fit side by side in this width. Always at least one.
fn columns_for(width: f32) -> usize {
    let fits = ((width + GAP_X) / (COVER + GAP_X)).floor();

    // The max(1) after the cast is for a width that is not a number at
    // all, which casts to 0 and would leave the grid dividing by zero
    (fits.clamp(1.0, 64.0) as usize).max(1)
}

/// The length of `count` things of `size`, with `gap` between each two.
fn span(count: usize, size: f32, gap: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }

    count as f32 * size + (count - 1) as f32 * gap
}

/// Which place in the grid a point falls on.
///
/// The gap to a card's left counts as part of that card, so letting go
/// between two of them drops into the one on the right rather than missing
/// altogether. A point above or left of the grid is the first place, and
/// one past the end is the last.
fn slot_at(at: Pos2, origin: Pos2, columns: usize, card_height: f32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }

    let columns = columns.max(1);

    let column = ((at.x - origin.x + GAP_X / 2.0) / (COVER + GAP_X))
        .floor()
        .clamp(0.0, (columns - 1) as f32) as usize;

    let row = ((at.y - origin.y) / (card_height + GAP_Y)).floor().max(0.0) as usize;

    (row * columns + column).min(count - 1)
}

/// Where the card at this position goes, from the grid's top left corner.
fn card_offset(position: usize, columns: usize, card_height: f32) -> (f32, f32) {
    let columns = columns.max(1);

    (
        (position % columns) as f32 * (COVER + GAP_X),
        (position / columns) as f32 * (card_height + GAP_Y),
    )
}

/// The title a card shows: "Sempiternal (Expanded Edition)" is just
/// "Sempiternal" on the wall. The panel and the tooltip keep the full one.
fn shown_title(title: &str) -> String {
    let cleaned = clean_title(title);

    if cleaned.is_empty() {
        title.to_string()
    } else {
        cleaned
    }
}

/// Has enough time passed since the last write to be worth another?
///
/// Its own function so the rule can be tested without a library or a disk.
fn write_is_due(since_last_write: Duration) -> bool {
    since_last_write >= COLLECT_WRITES_FOR
}

/// "1 album", "2 albums".
fn albums(count: usize) -> String {
    if count == 1 {
        "1 album".to_string()
    } else {
        format!("{count} albums")
    }
}

/// How long a job of one album a second will take, said the way a person
/// would say it rather than to the second.
fn roughly(albums: usize) -> String {
    let seconds = albums.max(1);

    match seconds {
        1 => "a second".to_string(),
        2..=90 => format!("{seconds} seconds"),
        91..=5400 => {
            let minutes = seconds.div_ceil(60);

            format!("{minutes} minutes")
        }
        _ => {
            let hours = seconds.div_ceil(3600);

            format!("{hours} hours")
        }
    }
}

/// Which half star, 1 to 10, a pointer at this position is on.
fn rating_at(x: f32, left: f32, step: f32) -> u8 {
    let halves = ((x - left) / (step / 2.0)).floor();

    halves.clamp(0.0, (STARS * 2 - 1) as f32) as u8 + 1
}

/// One of the ten drawings in `assets/stars`, baked into the binary.
///
/// `include_image!` wants a path it can read at compile time, so this is a
/// list rather than a loop. They are white on nothing, which is what lets
/// the same drawing be an empty star and a filled one.
fn star_design(which: usize) -> egui::ImageSource<'static> {
    match which % STAR_DESIGNS {
        0 => egui::include_image!("../assets/stars/01.png"),
        1 => egui::include_image!("../assets/stars/02.png"),
        2 => egui::include_image!("../assets/stars/03.png"),
        3 => egui::include_image!("../assets/stars/04.png"),
        4 => egui::include_image!("../assets/stars/05.png"),
        5 => egui::include_image!("../assets/stars/06.png"),
        6 => egui::include_image!("../assets/stars/07.png"),
        7 => egui::include_image!("../assets/stars/08.png"),
        8 => egui::include_image!("../assets/stars/09.png"),
        _ => egui::include_image!("../assets/stars/10.png"),
    }
}

/// A number for an album that is always the same for that album and looks
/// like nothing in particular. FNV-1a, which is eight lines and plenty for
/// picking a drawing.
fn seed_of(artist: &str, title: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;

    for byte in artist
        .bytes()
        .chain(b" - ".iter().copied())
        .chain(title.bytes())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }

    hash
}

/// Which drawing each of the five stars uses for this album.
///
/// Random to look at, but worked out from the album's name, so it is the
/// same every frame and the same every time you open the app: an album
/// keeps its own row of stars rather than flickering through ten of them.
/// No two next to each other are the same, which is what a row of five
/// drawn by hand would look like.
fn designs_for(seed: u64) -> [usize; STARS as usize] {
    let mut row = [0usize; STARS as usize];
    let mut previous = usize::MAX;

    for (star, slot) in row.iter_mut().enumerate() {
        // Mixed again with the star's place in the row, so the five are
        // unrelated to each other rather than five of the same
        let mut mixed = seed
            ^ (star as u64)
                .wrapping_add(1)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15);

        mixed ^= mixed >> 29;
        mixed = mixed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed ^= mixed >> 32;

        let mut design = (mixed % STAR_DESIGNS as u64) as usize;

        if design == previous {
            design = (design + 1) % STAR_DESIGNS;
        }

        *slot = design;
        previous = design;
    }

    row
}

/// A step of the way from one colour to another, for the hover fade.
fn mix(from: Color32, to: Color32, amount: f32) -> Color32 {
    let amount = amount.clamp(0.0, 1.0);

    let blend = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * amount).round() as u8;

    Color32::from_rgb(
        blend(from.r(), to.r()),
        blend(from.g(), to.g()),
        blend(from.b(), to.b()),
    )
}

/// Hands a URL to whatever the system uses for links.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_left_half_of_a_star_is_half_a_point() {
        // five stars starting at x = 100, each 17 wide
        assert_eq!(rating_at(100.0, 100.0, 17.0), 1); // left half of the first
        assert_eq!(rating_at(109.0, 100.0, 17.0), 2); // right half of the first
        assert_eq!(rating_at(134.0, 100.0, 17.0), 5); // left half of the third
        assert_eq!(rating_at(143.0, 100.0, 17.0), 6); // right half of the third
        assert_eq!(rating_at(184.0, 100.0, 17.0), 10); // the very end
    }

    #[test]
    fn a_pointer_past_either_end_still_gives_a_rating() {
        assert_eq!(rating_at(0.0, 100.0, 17.0), 1);
        assert_eq!(rating_at(9000.0, 100.0, 17.0), 10);
    }

    #[test]
    fn an_album_always_gets_the_same_row_of_stars() {
        let amo = seed_of("Bring Me The Horizon", "amo");

        assert_eq!(designs_for(amo), designs_for(amo));

        // A different album gets a different row. Not guaranteed by the
        // maths, but these two are worth knowing about if it ever changes
        assert_ne!(designs_for(amo), designs_for(seed_of("Burial", "Untrue")));

        // The artist counts, not just the title
        assert_ne!(
            seed_of("Nirvana", "Nevermind"),
            seed_of("Nevermind", "Nirvana")
        );
    }

    #[test]
    fn no_two_stars_next_to_each_other_are_the_same_drawing() {
        // Every album anybody is likely to own, near enough
        for album in 0..5000u64 {
            let row = designs_for(seed_of("artist", &album.to_string()));

            assert!(row.iter().all(|&design| design < STAR_DESIGNS));

            for pair in row.windows(2) {
                assert_ne!(pair[0], pair[1], "two of the same in {row:?}");
            }
        }
    }

    #[test]
    fn every_drawing_gets_used() {
        let mut seen = [0usize; STAR_DESIGNS];

        for album in 0..2000u64 {
            for design in designs_for(seed_of("artist", &album.to_string())) {
                seen[design] += 1;
            }
        }

        // 10 000 stars over ten drawings: a drawing that never turned up,
        // or turned up half the time, would mean the mixing is broken
        assert!(
            seen.iter().all(|&count| (500..1600).contains(&count)),
            "lopsided: {seen:?}"
        );
    }

    #[test]
    fn a_card_dropped_on_a_slot_lands_in_that_slot() {
        let origin = egui::pos2(100.0, 50.0);
        let height = 240.0;
        let at = |x: f32, y: f32| slot_at(egui::pos2(x, y), origin, 3, height, 7);

        // Dead centre of the first card, and of the one beside it
        assert_eq!(at(origin.x + COVER / 2.0, origin.y + 10.0), 0);
        assert_eq!(at(origin.x + COVER + GAP_X + 5.0, origin.y + 10.0), 1);

        // Second row, third column: three across means place five
        assert_eq!(
            at(
                origin.x + 2.0 * (COVER + GAP_X) + 5.0,
                origin.y + height + GAP_Y + 5.0
            ),
            5
        );
    }

    #[test]
    fn a_card_dropped_in_a_gap_goes_to_the_card_on_its_right() {
        let origin = egui::pos2(0.0, 0.0);

        // Right in the gap between the first and second cards
        let gap = egui::pos2(COVER + GAP_X / 2.0, 10.0);

        assert_eq!(slot_at(gap, origin, 4, 240.0, 8), 1);
    }

    #[test]
    fn a_card_dropped_off_the_grid_lands_at_the_near_end() {
        let origin = egui::pos2(100.0, 50.0);

        // Above and to the left of everything
        assert_eq!(slot_at(egui::pos2(0.0, 0.0), origin, 3, 240.0, 7), 0);

        // Far past the last card, which there are only seven of
        assert_eq!(slot_at(egui::pos2(5000.0, 5000.0), origin, 3, 240.0, 7), 6);

        // An empty wall has nowhere to drop
        assert_eq!(slot_at(egui::pos2(10.0, 10.0), origin, 3, 240.0, 0), 0);
    }

    #[test]
    fn as_many_cards_fit_across_as_there_is_room_for() {
        // One card is 180 wide and each one after it needs 24 more of gap
        assert_eq!(columns_for(180.0), 1);
        assert_eq!(columns_for(383.0), 1);
        assert_eq!(columns_for(384.0), 2);
        assert_eq!(columns_for(4.0 * 180.0 + 3.0 * 24.0), 4);

        // Too narrow for even one is still one, never zero
        assert_eq!(columns_for(50.0), 1);
        assert_eq!(columns_for(-10.0), 1);
    }

    #[test]
    fn every_card_in_a_row_starts_at_the_same_height() {
        let height = 270.0;

        // Four across: the first row is 0 to 3, the fifth card starts the next
        for position in 0..4 {
            assert_eq!(card_offset(position, 4, height).1, 0.0);
        }

        assert_eq!(card_offset(4, 4, height), (0.0, height + GAP_Y));
        assert_eq!(card_offset(5, 4, height).0, COVER + GAP_X);
        assert_eq!(card_offset(5, 4, height).1, card_offset(4, 4, height).1);
    }

    #[test]
    fn a_grid_is_as_long_as_its_cards_and_the_gaps_between_them() {
        assert_eq!(span(0, 180.0, 24.0), 0.0);
        assert_eq!(span(1, 180.0, 24.0), 180.0);
        assert_eq!(span(3, 180.0, 24.0), 3.0 * 180.0 + 2.0 * 24.0);
    }

    #[test]
    fn a_card_is_exactly_as_tall_as_its_lines() {
        let layout = CardLayout::new(18.0, 15.0);

        // Top to bottom, nothing overlapping
        assert!(layout.title_y >= COVER);
        assert!(layout.meta_y >= layout.title_y + 18.0);
        assert!(layout.stars_y >= layout.meta_y + 15.0);
        assert!(layout.tags_y >= layout.stars_y + STAR_STEP);
        assert!(layout.height >= layout.tags_y + 15.0);

        // And no spare room left at the bottom to widen the gap between rows
        assert!(layout.height < layout.tags_y + 15.0 + 1.0);
    }

    #[test]
    fn the_wall_shows_titles_without_their_edition() {
        assert_eq!(shown_title("Sempiternal (Expanded Edition)"), "Sempiternal");
        assert_eq!(shown_title("amo"), "amo");

        // A title that is nothing but an edition keeps it, rather than
        // showing an empty line
        assert_eq!(shown_title("(Deluxe Edition)"), "(Deluxe Edition)");
    }

    #[test]
    fn a_backfill_does_not_rewrite_the_library_once_per_album() {
        // Looking up 300 albums, one answer a second, each one changing a
        // single field. Writing the file for every one of them would put
        // the whole library on the drive 300 times over -- which is what
        // this used to do
        let mut writes = 0;
        let mut since_last_write = Duration::ZERO;

        for _album in 0..300 {
            since_last_write += Duration::from_secs(1);

            if write_is_due(since_last_write) {
                writes += 1;
                since_last_write = Duration::ZERO;
            }
        }

        assert!(
            writes <= 31,
            "{writes} whole-file writes for 300 albums is too many"
        );

        // But it does still write as it goes, so killing the app halfway
        // through does not throw the whole run away
        assert!(writes >= 25, "{writes} is so few that work would be lost");
    }

    #[test]
    fn a_change_is_never_written_twice_in_the_same_breath() {
        assert!(!write_is_due(Duration::ZERO));
        assert!(!write_is_due(COLLECT_WRITES_FOR - Duration::from_millis(1)));
        assert!(write_is_due(COLLECT_WRITES_FOR));
        assert!(write_is_due(Duration::from_secs(3600)));
    }

    #[test]
    fn one_album_is_not_one_albums() {
        assert_eq!(albums(1), "1 album");
        assert_eq!(albums(0), "0 albums");
        assert_eq!(albums(12), "12 albums");
    }

    #[test]
    fn how_long_a_refetch_takes_is_said_in_round_numbers() {
        // One album a second, so the count is the seconds
        assert_eq!(roughly(12), "12 seconds");
        assert_eq!(roughly(90), "90 seconds");
        assert_eq!(roughly(120), "2 minutes");

        // Rounded up, because finishing early is the pleasant surprise
        assert_eq!(roughly(121), "3 minutes");
        assert_eq!(roughly(4000), "67 minutes");
        assert_eq!(roughly(9000), "3 hours");

        // And neither one album nor none reads as "1 seconds"
        assert_eq!(roughly(1), "a second");
        assert_eq!(roughly(0), "a second");
    }

    #[test]
    fn the_hover_fade_goes_from_one_colour_to_the_other() {
        assert_eq!(mix(BG, PANEL, 0.0), BG);
        assert_eq!(mix(BG, PANEL, 1.0), PANEL);

        // An animation can overshoot a little, which must not wrap around
        assert_eq!(mix(BG, PANEL, 7.0), PANEL);
    }
}
