use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};
use std::time::Instant;
use tui_input::Input;

use super::{App, Modal, Screen};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if crate::sync::paths::migration_required() {
        if key.code == KeyCode::Char('y') {
            super::sync::open(app);
        } else {
            app.status =
                "Legacy shadow is blocked · press y, sign in, then use F2 to download from AnkiWeb"
                    .into();
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.selected_deck > 0 {
                app.selected_deck -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.selected_deck + 1 < app.decks.len() {
                app.selected_deck += 1;
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            start_study(app)?;
        }
        KeyCode::Char('a') => {
            if app.current_deck_id().is_some() {
                app.add_field = super::AddField::Front;
                app.add_front = Input::default();
                app.add_back = Input::default();
                app.add_tags = Input::default();
                app.screen = Screen::AddNote;
                app.status = "Add a new note".into();
            }
        }
        KeyCode::Char('b') => {
            app.browse_query = Input::default();
            app.browse_focus_query = true;
            app.browse_selected = 0;
            app.screen = Screen::Browse;
            super::browse::reload(app)?;
        }
        KeyCode::Char('s') => {
            app.stats = app.store.stats()?;
            app.screen = Screen::Stats;
        }
        KeyCode::Char('y') => {
            super::sync::open(app);
        }
        KeyCode::Char('o') => {
            if let Some(id) = app.current_deck_id() {
                if let Some(deck) = app.store.get_deck(id)? {
                    app.options_field = super::OptionsField::NewPerDay;
                    app.options_new = Input::new(deck.new_per_day.to_string());
                    app.options_rev = Input::new(deck.rev_per_day.to_string());
                    app.screen = Screen::DeckOptions;
                }
            }
        }
        KeyCode::Char('n') => {
            app.modal = Modal::CreateDeck;
            app.modal_input = Input::default();
        }
        KeyCode::Char('r') => {
            if let Some(deck) = app.decks.get(app.selected_deck) {
                app.modal = Modal::RenameDeck;
                app.modal_input = Input::new(deck.name.clone());
            }
        }
        KeyCode::Char('d') => {
            app.modal = Modal::ConfirmDeleteDeck;
        }
        KeyCode::Char('f') if app.current_deck_id().is_some() => {
            app.modal = Modal::ConfirmResetDeck;
        }
        _ => {}
    }
    Ok(())
}

pub fn start_study(app: &mut App) -> Result<()> {
    let Some(deck_id) = app.current_deck_id() else {
        return Ok(());
    };
    app.study_queue = app.store.next_study_queue(deck_id, 50)?;
    app.study_index = 0;
    app.answer_shown = false;
    app.study_done = 0;
    app.study_started_at = Instant::now();
    if app.study_queue.is_empty() {
        app.status = "No cards due · press f to Reset/Forget deck (Anki), then study again".into();
    } else {
        app.screen = Screen::Study;
        app.status = format!("{} cards in queue", app.study_queue.len());
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(4)]).areas::<2>(area);

    let header = Row::new([
        Cell::from("Deck"),
        Cell::from("New"),
        Cell::from("Learn"),
        Cell::from("Review"),
        Cell::from("Due"),
        Cell::from("Total"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    let rows = app.decks.iter().enumerate().map(|(i, d)| {
        let style = if i == app.selected_deck {
            Style::default()
                .bg(Color::Rgb(30, 40, 55))
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Row::new([
            Cell::from(format!("{}{}", "  ".repeat(d.level), d.name)),
            Cell::from(d.new.to_string()).style(Style::default().fg(Color::Blue)),
            Cell::from(d.learning.to_string()).style(Style::default().fg(Color::Yellow)),
            Cell::from(d.review.to_string()).style(Style::default().fg(Color::Green)),
            Cell::from(d.due_total().to_string()),
            Cell::from(d.total.to_string()),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(40),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Decks "));

    let mut state = TableState::default().with_selected(Some(app.selected_deck));
    frame.render_stateful_widget(table, chunks[0], &mut state);

    let migration_required = crate::sync::paths::migration_required();
    let selected = app.decks.get(app.selected_deck);
    let summary = if let Some(d) = selected {
        Line::from(vec![
            Span::raw("Selected: "),
            Span::styled(&d.name, Style::default().fg(Color::Cyan)),
            Span::raw(format!(
                "  ·  {} due ({} new / {} learn / {} review)  ·  {} cards",
                d.due_total(),
                d.new,
                d.learning,
                d.review,
                d.total
            )),
        ])
    } else if migration_required {
        Line::from("Legacy collection blocked · press y to open AnkiWeb Sync")
    } else {
        Line::from("No decks")
    };
    frame.render_widget(
        Paragraph::new(vec![
            summary,
            Line::from(""),
            Line::from(if migration_required {
                "Upload the correct Desktop collection to AnkiWeb, then download it with F2."
            } else if app.store.uses_official_backend() {
                "Scheduler and daily limits: official Anki backend."
            } else {
                "Offline fallback: New → Learning → Review, scheduled with rs-fsrs."
            }),
            Line::from(if migration_required {
                "The blocked collection and fallback database cannot be changed from this screen."
            } else {
                "No cards due?  f  = Reset/Forget deck to New (Anki Cards → Reset)."
            }),
        ])
        .block(Block::default().borders(Borders::ALL).title(" Summary ")),
        chunks[1],
    );
}
