use super::common::{draw_input_field, input_from_key, truncate};
use super::{App, Screen};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Row, Table, TableState},
    Frame,
};

pub fn reload(app: &mut App) -> Result<()> {
    app.browse_rows = app.store.browse(app.browse_query.value(), 500)?;
    if app.browse_selected >= app.browse_rows.len() && !app.browse_rows.is_empty() {
        app.browse_selected = app.browse_rows.len() - 1;
    }
    if app.browse_rows.is_empty() {
        app.browse_selected = 0;
    }
    app.status = format!("{} cards", app.browse_rows.len());
    Ok(())
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.browse_focus_query {
        match key.code {
            KeyCode::Esc => {
                app.browse_focus_query = false;
            }
            KeyCode::Enter | KeyCode::Down => {
                app.browse_focus_query = false;
                reload(app)?;
            }
            _ => {
                if input_from_key(&mut app.browse_query, key) {
                    reload(app)?;
                }
            }
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::DeckBrowser;
            app.refresh_decks()?;
        }
        KeyCode::Char('/') => {
            app.browse_focus_query = true;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.browse_selected > 0 {
                app.browse_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.browse_selected + 1 < app.browse_rows.len() {
                app.browse_selected += 1;
            }
        }
        KeyCode::Enter => {
            if let Some(row) = app.browse_rows.get(app.browse_selected).cloned() {
                super::edit::open_from_browse(app, row);
            }
        }
        KeyCode::Char('s') => {
            if let Some(row) = app.browse_rows.get(app.browse_selected) {
                let new_val = !row.suspended;
                app.store.set_card_suspended(row.card_id, new_val)?;
                app.status = if new_val {
                    "Card suspended".into()
                } else {
                    "Card unsuspended".into()
                };
                reload(app)?;
            }
        }
        KeyCode::Char('f') => {
            if let Some(row) = app.browse_rows.get(app.browse_selected) {
                app.store.forget_card(row.card_id)?;
                app.status = "Card reset to New (Anki Forget)".into();
                reload(app)?;
                let _ = app.refresh_decks();
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas::<2>(area);

    draw_input_field(
        frame,
        chunks[0],
        " Search (front/back/tags/deck) · / to focus ",
        &app.browse_query,
        app.browse_focus_query,
    );

    let header = Row::new([
        Cell::from("Deck"),
        Cell::from("Front"),
        Cell::from("Back"),
        Cell::from("State"),
        Cell::from("Due"),
        Cell::from("Flags"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .bottom_margin(1);

    let rows = app.browse_rows.iter().enumerate().map(|(i, r)| {
        let style = if i == app.browse_selected {
            Style::default()
                .bg(Color::Rgb(30, 40, 55))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let flags = if r.suspended {
            Span::styled("suspended", Style::default().fg(Color::Red))
        } else {
            Span::raw("")
        };
        Row::new([
            Cell::from(truncate(&r.deck_name, 16)),
            Cell::from(truncate(&r.front, 28)),
            Cell::from(truncate(&r.back, 28)),
            Cell::from(r.state.as_str()),
            Cell::from(if r.due_label.is_empty() {
                r.due.format("%Y-%m-%d").to_string()
            } else {
                r.due_label.clone()
            }),
            Cell::from(flags),
        ])
        .style(style)
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(14),
            Constraint::Percentage(28),
            Constraint::Percentage(28),
            Constraint::Percentage(10),
            Constraint::Percentage(12),
            Constraint::Percentage(8),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Cards "));

    let mut state = TableState::default().with_selected(Some(app.browse_selected));
    frame.render_stateful_widget(table, chunks[1], &mut state);
}
