use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::common::{draw_input_field, input_from_key};
use super::{App, OptionsField, Screen};
use crate::models::DeckOptions;

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::DeckBrowser;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            app.options_field = match app.options_field {
                OptionsField::NewPerDay => OptionsField::RevPerDay,
                OptionsField::RevPerDay => OptionsField::NewPerDay,
            };
        }
        KeyCode::Enter => save(app)?,
        _ => {
            let input = match app.options_field {
                OptionsField::NewPerDay => &mut app.options_new,
                OptionsField::RevPerDay => &mut app.options_rev,
            };
            // digits and editing only
            match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    input_from_key(input, key);
                }
                KeyCode::Backspace
                | KeyCode::Delete
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End => {
                    input_from_key(input, key);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn save(app: &mut App) -> Result<()> {
    let Some(id) = app.current_deck_id() else {
        return Ok(());
    };
    let new_per_day = app.options_new.value().parse::<i64>().unwrap_or(20).max(0);
    let rev_per_day = app.options_rev.value().parse::<i64>().unwrap_or(200).max(0);
    app.store.update_deck_options(
        id,
        &DeckOptions {
            new_per_day,
            rev_per_day,
        },
    )?;
    app.status = format!("Options saved (new {new_per_day}/day, review {rev_per_day}/day)");
    app.refresh_decks()?;
    app.screen = Screen::DeckBrowser;
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let deck_name = app
        .decks
        .get(app.selected_deck)
        .map(|d| d.name.as_str())
        .unwrap_or("?");
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas::<4>(area);

    frame.render_widget(
        Paragraph::new(format!("Deck: {deck_name}")).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Deck Options "),
        ),
        chunks[0],
    );

    draw_input_field(
        frame,
        chunks[1],
        " New cards/day ",
        &app.options_new,
        app.options_field == OptionsField::NewPerDay,
    );
    draw_input_field(
        frame,
        chunks[2],
        " Review cards/day ",
        &app.options_rev,
        app.options_field == OptionsField::RevPerDay,
    );

    frame.render_widget(
        Paragraph::new("Limits apply when building the study queue (Anki-style daily caps).")
            .block(Block::default().borders(Borders::ALL).title(" Note ")),
        chunks[3],
    );
}
