use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::InputRequest;

use super::common::{draw_input_field, input_from_key};
use super::{AddField, App, PendingExternal, Screen};
use crate::media_import;

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::DeckBrowser;
            app.status = "Cancelled add".into();
        }
        KeyCode::Tab => {
            app.add_field = match app.add_field {
                AddField::Front => AddField::Back,
                AddField::Back => AddField::Tags,
                AddField::Tags => AddField::Front,
            };
        }
        KeyCode::BackTab => {
            app.add_field = match app.add_field {
                AddField::Front => AddField::Tags,
                AddField::Back => AddField::Front,
                AddField::Tags => AddField::Back,
            };
        }
        KeyCode::Char('o')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if matches!(app.add_field, AddField::Front | AddField::Back) {
                app.pending_external = PendingExternal::YaziPickImage;
            } else {
                app.status = "Switch to Front/Back to insert an image".into();
            }
        }
        KeyCode::Enter => {
            save(app)?;
        }
        _ => {
            let input = match app.add_field {
                AddField::Front => &mut app.add_front,
                AddField::Back => &mut app.add_back,
                AddField::Tags => &mut app.add_tags,
            };
            input_from_key(input, key);
        }
    }
    Ok(())
}

/// Same yazi flow as Edit — inserts into the active Add Front/Back field.
pub fn insert_image_via_yazi(app: &mut App) -> Result<Option<String>> {
    let Some(path) = media_import::pick_with_yazi()? else {
        return Ok(None);
    };
    let fname = media_import::import_media_file(&path)?;
    let tag = media_import::img_tag(&fname);
    let input = match app.add_field {
        AddField::Front => &mut app.add_front,
        AddField::Back => &mut app.add_back,
        AddField::Tags => return Ok(None),
    };
    for ch in tag.chars() {
        let _ = input.handle(InputRequest::InsertChar(ch));
    }
    Ok(Some(fname))
}

fn save(app: &mut App) -> Result<()> {
    let Some(deck_id) = app.current_deck_id() else {
        return Ok(());
    };
    let front = app.add_front.value().trim();
    let back = app.add_back.value().trim();
    if front.is_empty() || back.is_empty() {
        app.status = "Front and Back are required".into();
        return Ok(());
    }
    app.store
        .add_note(deck_id, front, back, app.add_tags.value())?;
    app.status = "Note added".into();
    app.add_front.reset();
    app.add_back.reset();
    // keep tags for rapid entry
    app.add_field = AddField::Front;
    app.refresh_decks()?;
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
        Constraint::Percentage(40),
        Constraint::Percentage(40),
        Constraint::Min(3),
    ])
    .areas::<4>(area);

    frame.render_widget(
        Paragraph::new(format!("Deck: {deck_name}"))
            .block(Block::default().borders(Borders::ALL).title(" Target ")),
        chunks[0],
    );

    draw_input_field(
        frame,
        chunks[1],
        " Front ",
        &app.add_front,
        app.add_field == AddField::Front,
    );
    draw_input_field(
        frame,
        chunks[2],
        " Back ",
        &app.add_back,
        app.add_field == AddField::Back,
    );
    draw_input_field(
        frame,
        chunks[3],
        " Tags ",
        &app.add_tags,
        app.add_field == AddField::Tags,
    );
}
