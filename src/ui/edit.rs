use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::{Input, InputRequest};

use super::common::{draw_input_field, draw_multiline_input_field, input_from_key};
use super::study;
use super::{AddField, App, Modal, PendingExternal, Screen};
use crate::media_import;

pub fn open_from_browse(app: &mut App, note_id: i64, card_id: i64, front: String, back: String, tags: String) {
    app.edit_return = Screen::Browse;
    app.edit_note_id = Some(note_id);
    app.edit_card_id = Some(card_id);
    app.edit_field = AddField::Front;
    app.edit_front = Input::new(front);
    app.edit_back = Input::new(back);
    app.edit_tags = Input::new(tags);
    app.screen = Screen::EditNote;
}

pub fn open_from_study(app: &mut App) -> Result<()> {
    let Some(card) = app.study_queue.get(app.study_index).cloned() else {
        return Ok(());
    };
    app.media.stop_audio();
    app.edit_return = Screen::Study;
    app.edit_note_id = Some(card.card.note_id);
    app.edit_card_id = Some(card.card.id);
    app.edit_field = AddField::Front;
    app.edit_front = Input::new(card.front);
    app.edit_back = Input::new(card.back);
    app.edit_tags = Input::new(card.tags);
    app.screen = Screen::EditNote;
    app.status = "Editing current card".into();
    Ok(())
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            return_from_edit(app, false)?;
        }
        KeyCode::Tab => {
            app.edit_field = match app.edit_field {
                AddField::Front => AddField::Back,
                AddField::Back => AddField::Tags,
                AddField::Tags => AddField::Front,
            };
        }
        KeyCode::BackTab => {
            app.edit_field = match app.edit_field {
                AddField::Front => AddField::Tags,
                AddField::Back => AddField::Front,
                AddField::Tags => AddField::Back,
            };
        }
        KeyCode::Char('D') => {
            app.modal = Modal::ConfirmDeleteCard;
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => save(app)?,
        // Ctrl+o / Alt+o — open image via yazi (Ctrl+i == Tab in Kitty)
        KeyCode::Char('o')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if matches!(app.edit_field, AddField::Front | AddField::Back) {
                app.pending_external = PendingExternal::YaziPickImage;
            } else {
                app.status = "Switch to Front/Back to insert an image".into();
            }
        }
        KeyCode::Enter => {
            if matches!(app.edit_field, AddField::Front | AddField::Back) {
                let input = match app.edit_field {
                    AddField::Front => &mut app.edit_front,
                    _ => &mut app.edit_back,
                };
                let _ = input.handle(InputRequest::InsertChar('\n'));
            }
        }
        KeyCode::Up if matches!(app.edit_field, AddField::Front | AddField::Back) => {
            let input = match app.edit_field {
                AddField::Front => &mut app.edit_front,
                _ => &mut app.edit_back,
            };
            move_cursor_vertically(input, -1);
        }
        KeyCode::Down if matches!(app.edit_field, AddField::Front | AddField::Back) => {
            let input = match app.edit_field {
                AddField::Front => &mut app.edit_front,
                _ => &mut app.edit_back,
            };
            move_cursor_vertically(input, 1);
        }
        _ => {
            let input = match app.edit_field {
                AddField::Front => &mut app.edit_front,
                AddField::Back => &mut app.edit_back,
                AddField::Tags => &mut app.edit_tags,
            };
            input_from_key(input, key);
        }
    }
    Ok(())
}

/// Called from main after suspending the TUI. Inserts `<img src="…">` at the caret.
pub fn insert_image_via_yazi(app: &mut App) -> Result<Option<String>> {
    let Some(path) = media_import::pick_with_yazi()? else {
        return Ok(None);
    };
    let fname = media_import::import_media_file(&path)?;
    let tag = media_import::img_tag(&fname);
    let input = match app.edit_field {
        AddField::Front => &mut app.edit_front,
        AddField::Back => &mut app.edit_back,
        AddField::Tags => {
            app.status = "Images go in Front/Back, not Tags".into();
            return Ok(None);
        }
    };
    for ch in tag.chars() {
        let _ = input.handle(InputRequest::InsertChar(ch));
    }
    Ok(Some(fname))
}

/// Move caret up/down by one visual line, preserving column when possible.
fn move_cursor_vertically(input: &mut Input, dir: i32) {
    let value = input.value();
    let cursor = input.cursor();
    let (row, col) = super::common::cursor_row_col_for_edit(value, cursor);
    let lines: Vec<&str> = if value.is_empty() {
        vec![""]
    } else {
        value.split('\n').collect()
    };
    let target_row = if dir < 0 {
        row.saturating_sub(1)
    } else {
        (row + 1).min(lines.len().saturating_sub(1))
    };
    if target_row == row {
        return;
    }
    // char index of start of target row
    let mut idx = 0usize;
    for (r, line) in lines.iter().enumerate() {
        if r == target_row {
            let mut c = 0usize;
            let mut placed = idx;
            for ch in line.chars() {
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if c + w > col {
                    break;
                }
                c += w;
                placed += 1;
            }
            let _ = input.handle(InputRequest::SetCursor(placed));
            return;
        }
        idx += line.chars().count() + 1; // +1 for '\n'
    }
}

fn save(app: &mut App) -> Result<()> {
    let Some(note_id) = app.edit_note_id else {
        return Ok(());
    };
    let front = app.edit_front.value().trim().to_string();
    let back = app.edit_back.value().trim().to_string();
    let tags = app.edit_tags.value().to_string();
    if front.is_empty() || back.is_empty() {
        app.status = "Front and Back are required".into();
        return Ok(());
    }
    app.store
        .update_note(note_id, &front, &back, &tags)?;
    app.status = "Note saved".into();

    // Keep in-memory study card in sync
    if let Some(sc) = app.study_queue.get_mut(app.study_index) {
        if sc.card.note_id == note_id {
            sc.front = front;
            sc.back = back;
            sc.tags = tags;
        }
    }

    return_from_edit(app, true)?;
    Ok(())
}

fn return_from_edit(app: &mut App, saved: bool) -> Result<()> {
    let dest = app.edit_return;
    app.edit_return = Screen::Browse;
    match dest {
        Screen::Study => {
            app.screen = Screen::Study;
            if !saved {
                app.status = "Edit cancelled".into();
            }
        }
        _ => {
            app.screen = Screen::Browse;
            super::browse::reload(app)?;
            if saved {
                app.status = "Note saved".into();
            }
        }
    }
    Ok(())
}

pub fn after_delete_from_study(app: &mut App) -> Result<()> {
    let card_id = app.edit_card_id;
    app.edit_return = Screen::Browse;
    if let Some(cid) = card_id {
        app.study_queue.retain(|c| c.card.id != cid);
        if app.study_index >= app.study_queue.len() {
            app.study_index = app.study_queue.len().saturating_sub(1);
        }
    }
    app.answer_shown = false;
    app.media.reset();
    let _ = app.refresh_decks();

    if app.study_queue.is_empty() {
        // try refill
        if let Some(deck_id) = app.current_deck_id() {
            app.study_queue = app.store.next_study_queue(deck_id, 50)?;
            app.study_index = 0;
        }
    }

    if app.study_queue.is_empty() {
        app.screen = Screen::DeckBrowser;
        app.status = "Card deleted · no cards left in queue".into();
    } else {
        app.screen = Screen::Study;
        app.status = "Card deleted · continued study".into();
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let panes = Layout::horizontal([Constraint::Percentage(48), Constraint::Percentage(52)])
        .areas::<2>(area);

    draw_editor_pane(frame, panes[0], app);
    draw_preview_pane(frame, panes[1], app);
}

fn draw_editor_pane(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Percentage(42),
        Constraint::Percentage(42),
        Constraint::Min(3),
    ])
    .areas::<4>(area);

    let hint = if app.edit_return == Screen::Study {
        "From study · Tab · Ctrl+o/Alt+o image · Ctrl+s save · Esc"
    } else {
        "Tab · Ctrl+o/Alt+o image · Ctrl+s save · Esc"
    };
    frame.render_widget(
        Paragraph::new(hint).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Edit "),
        ),
        chunks[0],
    );

    draw_multiline_input_field(
        frame,
        chunks[1],
        " Front ",
        &app.edit_front,
        app.edit_field == AddField::Front,
    );
    draw_multiline_input_field(
        frame,
        chunks[2],
        " Back ",
        &app.edit_back,
        app.edit_field == AddField::Back,
    );
    draw_input_field(
        frame,
        chunks[3],
        " Tags ",
        &app.edit_tags,
        app.edit_field == AddField::Tags,
    );
}

fn draw_preview_pane(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(6), Constraint::Length(3)]).areas::<2>(area);

    // Mirror study: editing Front → Question side; Back/Tags → Answer side
    let answer_shown = app.edit_field != AddField::Front;
    let _ = app.media.prepare_card(
        -1,
        answer_shown,
        app.edit_front.value(),
        app.edit_back.value(),
    );
    if let Some(msg) = app.media.last_image_status.clone() {
        if app.status != msg {
            app.status = msg;
        }
    }

    study::draw_card_preview(
        frame,
        chunks[0],
        app.edit_front.value(),
        app.edit_back.value(),
        answer_shown,
        Some("Preview"),
        Some(&mut app.media),
    );

    let mode = if answer_shown {
        "Answer (after Space)"
    } else {
        "Question"
    };
    let tip = Line::from(vec![
        Span::styled("Study preview", Style::default().fg(Color::Cyan)),
        Span::raw(format!("  ·  {mode}  ·  Tab switches side")),
    ]);
    frame.render_widget(
        Paragraph::new(tip).block(Block::default().borders(Borders::ALL).title(" ")),
        chunks[1],
    );
}
