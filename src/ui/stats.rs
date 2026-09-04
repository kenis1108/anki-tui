use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::{App, Screen};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.screen = Screen::DeckBrowser;
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let s = &app.stats;
    let chunks = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
        .areas::<2>(area);

    let collection = Paragraph::new(vec![
        line("Decks", s.total_decks, Color::Cyan),
        line("Notes", s.total_notes, Color::White),
        line("Cards", s.total_cards, Color::White),
        Line::from(""),
        line("New", s.new_cards, Color::Blue),
        line("Learning", s.learning_cards, Color::Yellow),
        line("Review", s.review_cards, Color::Green),
        line("Relearning", s.relearning_cards, Color::Magenta),
        line("Suspended", s.suspended_cards, Color::Red),
        line("Mature (≥21d)", s.mature_cards, Color::Cyan),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Collection "),
    );
    frame.render_widget(collection, chunks[0]);

    let activity = Paragraph::new(vec![
        line("Reviews today", s.reviews_today, Color::Green),
        line("Reviews (7d)", s.reviews_7d, Color::Green),
        line("Reviews (30d)", s.reviews_30d, Color::Green),
        Line::from(""),
        Line::from(Span::styled(
            "Scheduler: FSRS (rs-fsrs)",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "Database: ~/.local/share/anki-tui/collection.db",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Activity "));
    frame.render_widget(activity, chunks[1]);
}

fn line(label: &str, value: i64, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("  {label:<16}")),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}
