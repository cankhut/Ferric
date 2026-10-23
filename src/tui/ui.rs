//! Drawing. Nothing in here changes anything, it only looks at the app
//! and says what the screen should be.

use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
    Frame,
};

use crate::app::{App, Mode};

const ACCENT: Color = Color::Cyan;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let areas = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());

    draw_header(frame, app, areas[0]);
    draw_table(frame, app, areas[1]);
    draw_footer(frame, app, areas[2]);

    match app.mode {
        Mode::Search => draw_search_box(frame, app),
        Mode::Results => draw_results(frame, app),
        Mode::Confirm => draw_confirm(frame, app),
        Mode::Help => draw_help(frame),
        _ => {}
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let count = app.library.len();

    let average = match app.library.average_rating() {
        Some(average) => format!("{average:.1} average"),
        None => "nothing rated yet".to_string(),
    };

    let header = Line::from(vec![
        Span::styled(" crate-digger ", Style::new().fg(ACCENT).bold()),
        Span::raw(format!(
            " {count} albums · {average} · by {}",
            app.library.sort.label()
        )),
    ]);

    frame.render_widget(Paragraph::new(header), area);
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible = app.library.visible();

    if visible.is_empty() {
        let message = if app.library.is_empty() {
            "Nothing here yet.\n\nPress a to look up an album."
        } else {
            "No album matches the filter.\n\nPress / and then Esc to clear it."
        };

        let empty = Paragraph::new(message)
            .style(Style::new().fg(Color::DarkGray))
            .block(Block::bordered())
            .wrap(Wrap { trim: true });

        frame.render_widget(empty, area);
        return;
    }

    let rows: Vec<Row> = visible
        .iter()
        .filter_map(|&index| app.library.get(index))
        .map(|album| {
            let year = album.year.map(|year| year.to_string()).unwrap_or_default();

            Row::new(vec![
                Span::styled(album.rating_bar(), Style::new().fg(ACCENT)),
                Span::raw(album.artist.clone()),
                Span::styled(
                    album.title.clone(),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled(year, Style::new().fg(Color::DarkGray)),
                Span::styled(album.tags.join(", "), Style::new().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Percentage(25),
        Constraint::Percentage(33),
        Constraint::Length(5),
        Constraint::Min(8),
    ];

    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["rating", "artist", "album", "year", "tags"])
                .style(Style::new().fg(Color::DarkGray))
                .bottom_margin(1),
        )
        .block(Block::bordered().border_style(Style::new().fg(Color::DarkGray)))
        .row_highlight_style(
            Style::new()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");

    frame.render_stateful_widget(table, area, &mut app.table);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = match app.mode {
        Mode::Filter => Line::from(vec![
            Span::styled(" filter ", Style::new().bg(ACCENT).fg(Color::Black)),
            Span::raw(format!(" {}", app.input)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]),
        _ if !app.status.is_empty() => Line::from(format!(" {}", app.status)),
        _ => Line::from(
            " a add · / filter · 1-9 rate · t tags · o open · d delete · s sort · ? keys",
        )
        .style(Style::new().fg(Color::DarkGray)),
    };

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_search_box(frame: &mut Frame, app: &App) {
    let area = centered(60, 3, frame.area());

    let input = Paragraph::new(Line::from(vec![
        Span::raw(app.input.clone()),
        Span::styled("▏", Style::new().fg(ACCENT)),
    ]))
    .block(
        Block::bordered()
            .title(" search iTunes ")
            .border_style(Style::new().fg(ACCENT)),
    );

    frame.render_widget(Clear, area);
    frame.render_widget(input, area);
}

fn draw_results(frame: &mut Frame, app: &mut App) {
    let area = centered(70, 18, frame.area());

    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|result| {
            let year = result
                .year
                .map(|year| format!(" · {year}"))
                .unwrap_or_default();

            ListItem::new(Line::from(vec![
                Span::styled(result.artist.clone(), Style::new().fg(ACCENT)),
                Span::raw(" — "),
                Span::raw(result.title.clone()),
                Span::styled(year, Style::new().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::bordered()
                .title(" iTunes results ")
                .border_style(Style::new().fg(ACCENT)),
        )
        .highlight_style(
            Style::new()
                .bg(Color::Rgb(40, 44, 52))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");

    frame.render_widget(Clear, area);
    frame.render_stateful_widget(list, area, &mut app.results_state);
}

fn draw_confirm(frame: &mut Frame, app: &App) {
    let area = centered(50, 5, frame.area());

    let title = app
        .selected()
        .and_then(|index| app.library.get(index))
        .map(|album| album.title.clone())
        .unwrap_or_default();

    let text = Paragraph::new(vec![
        Line::from(format!("Remove \"{title}\"?")),
        Line::from(""),
        Line::from("y to remove, anything else to keep it").style(Style::new().fg(Color::DarkGray)),
    ])
    .block(
        Block::bordered()
            .title(" are you sure ")
            .border_style(Style::new().fg(Color::Yellow)),
    )
    .wrap(Wrap { trim: true });

    frame.render_widget(Clear, area);
    frame.render_widget(text, area);
}

fn draw_help(frame: &mut Frame) {
    let keys = [
        ("j k", "move up and down"),
        ("1-9, 0", "rate the album, 0 is a ten"),
        ("x", "take the rating off"),
        ("a", "look an album up on iTunes and add it"),
        ("t", "fetch tags from Last.fm"),
        ("o", "open it on Album of the Year"),
        ("/", "filter, Esc clears it"),
        ("s", "change the order"),
        ("d", "remove the album"),
        ("q", "quit"),
    ];

    let lines: Vec<Line> = keys
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!("{key:>8}  "), Style::new().fg(ACCENT).bold()),
                Span::raw(*what),
            ])
        })
        .collect();

    let area = centered(50, keys.len() as u16 + 2, frame.area());

    let help = Paragraph::new(lines).block(
        Block::bordered()
            .title(" keys ")
            .border_style(Style::new().fg(ACCENT)),
    );

    frame.render_widget(Clear, area);
    frame.render_widget(help, area);
}

/// A box of the given width in percent and height in rows, in the middle
/// of the screen.
fn centered(width_percent: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .split(area);

    Layout::horizontal([Constraint::Percentage(width_percent)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
