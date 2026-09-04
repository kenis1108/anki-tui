use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use tui_input::Input;

use super::common::{draw_input_field, draw_input_field_secret, input_from_key};
use super::{App, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncField {
    User,
    Pass,
}

pub fn open(app: &mut App) {
    app.screen = Screen::Sync;
    app.sync_field = SyncField::User;
    app.sync_busy = false;
    app.sync_log = crate::sync::auth_status();
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.sync_busy {
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            app.screen = Screen::DeckBrowser;
            app.refresh_decks()?;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            app.sync_field = match app.sync_field {
                SyncField::User => SyncField::Pass,
                SyncField::Pass => SyncField::User,
            };
        }
        KeyCode::F(1) => do_login(app)?,
        KeyCode::F(2) => do_download(app)?,
        KeyCode::F(3) => do_upload(app)?,
        KeyCode::F(4) => {
            crate::sync::logout()?;
            app.sync_log = "Signed out".into();
            app.status = "AnkiWeb credentials cleared".into();
        }
        KeyCode::F(5) => do_normal(app)?,
        KeyCode::F(6) => do_media(app)?,
        KeyCode::Enter if app.sync_field == SyncField::Pass => do_login(app)?,
        _ => {
            input_from_key(current_input(app), key);
        }
    }
    Ok(())
}

fn current_input(app: &mut App) -> &mut Input {
    match app.sync_field {
        SyncField::User => &mut app.sync_user,
        SyncField::Pass => &mut app.sync_pass,
    }
}

fn do_login(app: &mut App) -> Result<()> {
    let user = app.sync_user.value().trim().to_string();
    let pass = app.sync_pass.value().to_string();
    if user.is_empty() || pass.is_empty() {
        app.sync_log = "Enter AnkiWeb email and password".into();
        return Ok(());
    }
    app.sync_busy = true;
    app.sync_log = "Signing in…".into();
    let result = crate::sync::login(&user, &pass, None);
    app.sync_busy = false;
    match result {
        Ok(_) => {
            app.sync_pass.reset();
            app.sync_log = crate::sync::auth_status();
            app.status = "Logged in to AnkiWeb".into();
        }
        Err(e) => {
            app.sync_log = format!("Login failed: {e:#}");
            app.status = "Login failed".into();
        }
    }
    Ok(())
}

fn do_download(app: &mut App) -> Result<()> {
    app.sync_busy = true;
    app.sync_log = "Full download + media…".into();
    let result = crate::sync::full_download(&app.store);
    app.sync_busy = false;
    match result {
        Ok(r) => {
            app.sync_log = r.message.clone();
            app.status = r.message;
            app.refresh_decks()?;
        }
        Err(e) => {
            app.sync_log = format!("Download failed: {e:#}");
            app.status = "Download failed".into();
        }
    }
    Ok(())
}

fn do_upload(app: &mut App) -> Result<()> {
    app.sync_busy = true;
    app.sync_log = "Full upload + media…".into();
    let result = crate::sync::full_upload(&app.store);
    app.sync_busy = false;
    match result {
        Ok(r) => {
            app.sync_log = r.message.clone();
            app.status = r.message;
        }
        Err(e) => {
            app.sync_log = format!("Upload failed: {e:#}");
            app.status = "Upload failed".into();
        }
    }
    Ok(())
}

fn do_normal(app: &mut App) -> Result<()> {
    app.sync_busy = true;
    app.sync_log = "Incremental sync + media…".into();
    let result = crate::sync::normal_sync(&app.store);
    app.sync_busy = false;
    match result {
        Ok(r) => {
            app.sync_log = r.message.clone();
            app.status = r.message;
            app.refresh_decks()?;
        }
        Err(e) => {
            app.sync_log = format!("Sync failed: {e:#}");
            app.status = "Sync failed".into();
        }
    }
    Ok(())
}

fn do_media(app: &mut App) -> Result<()> {
    app.sync_busy = true;
    app.sync_log = "Media sync only…".into();
    let result = crate::sync::media_only_sync();
    app.sync_busy = false;
    match result {
        Ok(r) => {
            app.sync_log = r.message.clone();
            app.status = r.message;
        }
        Err(e) => {
            app.sync_log = format!("Media sync failed: {e:#}");
            app.status = "Media sync failed".into();
        }
    }
    Ok(())
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(10),
        Constraint::Min(0),
    ])
    .areas::<5>(area);

    frame.render_widget(
        Paragraph::new(crate::sync::auth_status()).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" AnkiWeb Sync "),
        ),
        chunks[0],
    );

    draw_input_field(
        frame,
        chunks[1],
        " Email / Username ",
        &app.sync_user,
        app.sync_field == SyncField::User,
    );
    draw_input_field_secret(
        frame,
        chunks[2],
        " Password ",
        &app.sync_pass,
        app.sync_field == SyncField::Pass,
    );

    let actions = Paragraph::new(vec![
        Line::from(vec![
            key("F1"),
            Span::raw(" login  "),
            key("F5"),
            Span::raw(" incremental  "),
            key("F6"),
            Span::raw(" media only"),
        ]),
        Line::from(vec![
            key("F2"),
            Span::raw(" full download  "),
            key("F3"),
            Span::raw(" full upload  "),
            key("F4"),
            Span::raw(" logout"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Incremental uses a local shadow collection.anki2 (created by F2).",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "Media: ~/.local/share/anki-tui/anki/collection.media/",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "F2/F3 overwrite one side completely — prefer F5 for daily use.",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Actions "));
    frame.render_widget(actions, chunks[3]);

    let log = if app.sync_busy {
        "Working… (network I/O may freeze the UI briefly)"
    } else {
        app.sync_log.as_str()
    };
    frame.render_widget(
        Paragraph::new(log)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Status ")),
        chunks[4],
    );
}

fn key(label: &str) -> Span<'_> {
    Span::styled(
        label,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}
