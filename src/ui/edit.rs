use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::time::Instant;
use tui_input::{Input, InputRequest};

use super::common::{draw_input_field, draw_multiline_input_field, input_from_key};
use super::study;
use super::{App, Modal, PendingExternal, Screen};
use crate::media_import;
use crate::models::{BrowseRow, NoteField};

pub fn open_from_browse(app: &mut App, row: BrowseRow) {
    app.edit_return = Screen::Browse;
    app.edit_note_id = Some(row.note_id);
    app.edit_card_id = Some(row.card_id);
    set_edit_fields(app, row.fields, row.tags);
    clear_edit_preview(app);
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
    set_edit_fields(app, card.fields, card.tags);
    clear_edit_preview(app);
    app.screen = Screen::EditNote;
    app.status = "Editing current card".into();
    Ok(())
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => return_from_edit(app, false)?,
        KeyCode::Tab => {
            app.edit_focus = (app.edit_focus + 1) % (app.edit_fields.len() + 1);
        }
        KeyCode::BackTab => {
            app.edit_focus = if app.edit_focus == 0 {
                app.edit_fields.len()
            } else {
                app.edit_focus - 1
            };
        }
        KeyCode::Char('D') => app.modal = Modal::ConfirmDeleteCard,
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => save(app)?,
        KeyCode::Char('o')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            if app.edit_focus < app.edit_fields.len() {
                app.pending_external = PendingExternal::YaziPickImage;
            } else {
                app.status = "Switch to a note field to insert an image".into();
            }
        }
        KeyCode::Enter if app.edit_focus < app.edit_fields.len() => {
            let _ = app.edit_fields[app.edit_focus]
                .1
                .handle(InputRequest::InsertChar('\n'));
        }
        KeyCode::Up if app.edit_focus < app.edit_fields.len() => {
            move_cursor_vertically(&mut app.edit_fields[app.edit_focus].1, -1);
        }
        KeyCode::Down if app.edit_focus < app.edit_fields.len() => {
            move_cursor_vertically(&mut app.edit_fields[app.edit_focus].1, 1);
        }
        _ => {
            input_from_key(current_input(app), key);
        }
    }
    Ok(())
}

/// Called from main after suspending the TUI. Inserts an image tag at the caret.
pub fn insert_image_via_yazi(app: &mut App) -> Result<Option<String>> {
    let Some(path) = media_import::pick_with_yazi()? else {
        return Ok(None);
    };
    let fname = media_import::import_media_file(&path)?;
    let tag = media_import::img_tag(&fname);
    if app.edit_focus >= app.edit_fields.len() {
        app.status = "Images go in note fields, not Tags".into();
        return Ok(None);
    }
    let input = &mut app.edit_fields[app.edit_focus].1;
    for ch in tag.chars() {
        let _ = input.handle(InputRequest::InsertChar(ch));
    }
    Ok(Some(fname))
}

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
    let mut idx = 0usize;
    for (r, line) in lines.iter().enumerate() {
        if r == target_row {
            let mut c = 0usize;
            let mut placed = idx;
            for ch in line.chars() {
                let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if c + width > col {
                    break;
                }
                c += width;
                placed += 1;
            }
            let _ = input.handle(InputRequest::SetCursor(placed));
            return;
        }
        idx += line.chars().count() + 1;
    }
}

fn save(app: &mut App) -> Result<()> {
    let Some(note_id) = app.edit_note_id else {
        return Ok(());
    };
    let fields: Vec<NoteField> = app
        .edit_fields
        .iter()
        .map(|(name, input)| NoteField {
            name: name.clone(),
            value: input.value().to_string(),
        })
        .collect();
    if fields.is_empty() {
        app.status = "Note has no editable fields".into();
        return Ok(());
    }
    let tags = app.edit_tags.value().to_string();
    app.store.update_note_fields(note_id, &fields, &tags)?;
    app.status = "Note saved".into();

    if let Some(card) = app.study_queue.get_mut(app.study_index) {
        if card.card.note_id == note_id {
            card.fields = fields;
            card.tags = tags;
        }
    }

    return_from_edit(app, true)
}

fn return_from_edit(app: &mut App, saved: bool) -> Result<()> {
    let destination = app.edit_return;
    app.edit_return = Screen::Browse;
    clear_edit_preview(app);
    match destination {
        Screen::Study => {
            if saved && app.store.uses_official_backend() {
                if let Some(deck_id) = app.current_deck_id() {
                    app.study_queue = app.store.next_study_queue(deck_id, 50)?;
                    app.study_index = 0;
                    app.study_started_at = Instant::now();
                }
            }
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
    app.edit_return = Screen::Browse;
    if app.store.uses_official_backend() {
        if let Some(deck_id) = app.current_deck_id() {
            app.study_queue = app.store.next_study_queue(deck_id, 50)?;
            app.study_index = 0;
        }
    } else if let Some(card_id) = app.edit_card_id {
        app.study_queue.retain(|card| card.card.id != card_id);
        if app.study_index >= app.study_queue.len() {
            app.study_index = app.study_queue.len().saturating_sub(1);
        }
    }
    app.study_started_at = Instant::now();
    app.answer_shown = false;
    app.media.reset();
    let _ = app.refresh_decks();

    if app.study_queue.is_empty() {
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
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .areas::<3>(area);

    let hint = if app.edit_return == Screen::Study {
        "From study · Tab fields · Ctrl+s save · Esc"
    } else {
        "Tab fields · Ctrl+s save · Esc"
    };
    frame.render_widget(
        Paragraph::new(hint).block(Block::default().borders(Borders::ALL).title(" Edit ")),
        chunks[0],
    );

    if let Some((name, input)) = app
        .edit_fields
        .get(app.edit_focus.min(app.edit_fields.len().saturating_sub(1)))
    {
        let position = app.edit_focus.min(app.edit_fields.len().saturating_sub(1)) + 1;
        draw_multiline_input_field(
            frame,
            chunks[1],
            &format!(" Field {position}/{} · {name} ", app.edit_fields.len()),
            input,
            app.edit_focus < app.edit_fields.len(),
        );
    } else {
        frame.render_widget(
            Paragraph::new("No fields").block(Block::default().borders(Borders::ALL)),
            chunks[1],
        );
    }
    draw_input_field(
        frame,
        chunks[2],
        " Tags ",
        &app.edit_tags,
        app.edit_focus == app.edit_fields.len(),
    );
}

fn draw_preview_pane(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::vertical([Constraint::Min(6), Constraint::Length(3)]).areas::<2>(area);
    let answer_shown = app.edit_focus > 0;
    refresh_edit_preview(app);

    let (front, back, answer_includes_question, front_document, back_document) =
        if let Some(preview) = app.edit_preview.clone() {
            (
                preview.front,
                preview.back,
                preview.answer_includes_question,
                preview.front_document,
                preview.back_document,
            )
        } else {
            let front = app
                .edit_fields
                .first()
                .map(|field| field.1.value().to_string())
                .unwrap_or_default();
            let back = app
                .edit_fields
                .iter()
                .skip(1)
                .map(|field| field.1.value())
                .collect::<Vec<_>>()
                .join("\n");
            (front, back, false, None, None)
        };

    let card_id = app.edit_card_id.unwrap_or(-1);
    let _ = app.media.prepare_card(
        card_id,
        answer_shown,
        answer_includes_question,
        &front,
        &back,
    );
    if let Some(message) = app.media.last_image_status.clone() {
        if app.status != message {
            app.status = message;
        }
    }

    study::draw_card_preview(
        frame,
        chunks[0],
        study::CardPreview {
            front: &front,
            back: &back,
            front_document: front_document.as_ref(),
            back_document: back_document.as_ref(),
            answer_shown,
            answer_includes_question,
            title_prefix: Some("Field preview"),
        },
        Some(&mut app.media),
    );

    let field_name = app
        .edit_fields
        .get(app.edit_focus)
        .map(|field| field.0.as_str())
        .unwrap_or("Tags");
    let tip = Line::from(vec![
        Span::styled("Editing", Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "  ·  {field_name}  ·  all note fields are preserved"
        )),
    ]);
    frame.render_widget(
        Paragraph::new(tip).block(Block::default().borders(Borders::ALL).title(" ")),
        chunks[1],
    );
}

fn clear_edit_preview(app: &mut App) {
    app.edit_preview = None;
    app.edit_preview_key.clear();
}

fn refresh_edit_preview(app: &mut App) {
    if !app.store.uses_official_backend() {
        clear_edit_preview(app);
        return;
    }
    let Some(card_id) = app.edit_card_id else {
        clear_edit_preview(app);
        return;
    };
    let fields: Vec<NoteField> = app
        .edit_fields
        .iter()
        .map(|(name, input)| NoteField {
            name: name.clone(),
            value: input.value().to_string(),
        })
        .collect();
    let tags = app.edit_tags.value().to_string();
    let key = preview_cache_key(card_id, &fields, &tags);
    if app.edit_preview_key == key {
        return;
    }
    match app.store.preview_card(card_id, &fields, &tags) {
        Ok(preview) => {
            app.edit_preview = Some(preview);
            app.edit_preview_key = key;
        }
        Err(error) => {
            app.edit_preview = None;
            app.edit_preview_key = key;
            let message = format!("preview: {error:#}");
            if app.status != message {
                app.status = message;
            }
        }
    }
}

fn preview_cache_key(card_id: i64, fields: &[NoteField], tags: &str) -> String {
    let mut key = format!("{card_id}\n{tags}");
    for field in fields {
        key.push('\n');
        key.push_str(&field.name);
        key.push('\0');
        key.push_str(&field.value);
    }
    key
}

fn set_edit_fields(app: &mut App, fields: Vec<NoteField>, tags: String) {
    app.edit_fields = fields
        .into_iter()
        .map(|field| (field.name, Input::new(field.value)))
        .collect();
    app.edit_focus = 0;
    app.edit_tags = Input::new(tags);
}

fn current_input(app: &mut App) -> &mut Input {
    if app.edit_focus < app.edit_fields.len() {
        &mut app.edit_fields[app.edit_focus].1
    } else {
        &mut app.edit_tags
    }
}
