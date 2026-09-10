use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use tui_input::Input;

use super::common::{draw_input_field, draw_input_field_secret, input_from_key};
use super::{App, Screen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncJobKind {
    Login,
    FullDownload,
    FullUpload,
    Incremental,
    Media,
}

enum SyncJob {
    Login { user: String, pass: String },
    FullDownload,
    FullUpload,
    Incremental,
    Media,
}

impl SyncJob {
    fn kind(&self) -> SyncJobKind {
        match self {
            Self::Login { .. } => SyncJobKind::Login,
            Self::FullDownload => SyncJobKind::FullDownload,
            Self::FullUpload => SyncJobKind::FullUpload,
            Self::Incremental => SyncJobKind::Incremental,
            Self::Media => SyncJobKind::Media,
        }
    }
}

struct SyncOutcome {
    message: String,
    refresh_decks: bool,
    requires_user_action: bool,
}

enum SyncTaskMessage {
    Progress(crate::anki_backend::BackendProgress),
    Finished(Result<SyncOutcome, String>),
}

pub(super) struct SyncTask {
    kind: SyncJobKind,
    receiver: Receiver<SyncTaskMessage>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncField {
    User,
    Pass,
}

pub fn open(app: &mut App) {
    app.screen = Screen::Sync;
    app.sync_field = SyncField::User;
    app.sync_log = crate::sync::auth_status();
}

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.sync_busy {
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            if crate::sync::paths::migration_required() {
                app.sync_log = "Legacy shadow is blocked. Confirm Desktop uploaded the correct data to AnkiWeb, then press F2.".into();
                app.status = "Fresh F2 download required before opening decks".into();
            } else {
                app.screen = Screen::DeckBrowser;
                app.refresh_decks()?;
            }
        }
        KeyCode::Tab | KeyCode::BackTab => {
            app.sync_field = match app.sync_field {
                SyncField::User => SyncField::Pass,
                SyncField::Pass => SyncField::User,
            };
        }
        KeyCode::F(1) => do_login(app),
        KeyCode::F(2) => start_job(app, SyncJob::FullDownload),
        KeyCode::F(3) => start_job(app, SyncJob::FullUpload),
        KeyCode::F(4) => {
            crate::sync::logout()?;
            app.sync_log = "Signed out".into();
            app.status = "AnkiWeb credentials cleared".into();
        }
        KeyCode::F(5) => start_job(app, SyncJob::Incremental),
        KeyCode::F(6) => start_job(app, SyncJob::Media),
        KeyCode::Enter if app.sync_field == SyncField::Pass => do_login(app),
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

fn do_login(app: &mut App) {
    let user = app.sync_user.value().trim().to_string();
    let pass = app.sync_pass.value().to_string();
    if user.is_empty() || pass.is_empty() {
        app.sync_log = "Enter AnkiWeb email and password".into();
        return;
    }
    start_job(app, SyncJob::Login { user, pass });
}

fn start_job(app: &mut App, job: SyncJob) {
    if app.sync_task.is_some() {
        return;
    }

    let kind = job.kind();
    app.sync_busy = true;
    let progress = progress_message(kind);
    app.sync_progress = Some(crate::anki_backend::BackendProgress {
        stage: progress.into(),
        current: 0,
        total: None,
        detail: String::new(),
    });
    app.sync_log = progress.into();
    app.status = progress.into();

    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let result = run_job(job, &sender).map_err(|error| format!("{error:#}"));
        let _ = sender.send(SyncTaskMessage::Finished(result));
    });
    app.sync_task = Some(SyncTask {
        kind,
        receiver,
        handle: Some(handle),
    });
}

fn run_job(job: SyncJob, sender: &mpsc::Sender<SyncTaskMessage>) -> Result<SyncOutcome> {
    match job {
        SyncJob::Login { user, pass } => {
            crate::sync::login(&user, &pass, None)?;
            if crate::sync::paths::official_ready() {
                match crate::sync::normal_sync() {
                    Ok(result) => Ok(SyncOutcome {
                        message: format!("Signed in · {}", result.message),
                        refresh_decks: true,
                        requires_user_action: result.requires_user_action,
                    }),
                    Err(error) => Ok(SyncOutcome {
                        message: format!("Signed in; initial sync failed: {error:#}"),
                        refresh_decks: false,
                        requires_user_action: true,
                    }),
                }
            } else {
                Ok(SyncOutcome {
                    message: "Signed in · upload the known-good Desktop collection to AnkiWeb, then press F2 for a fresh official download".into(),
                    refresh_decks: false,
                    requires_user_action: false,
                })
            }
        }
        SyncJob::FullDownload => {
            let result = crate::sync::full_download(|progress| {
                let _ = sender.send(SyncTaskMessage::Progress(progress));
            })?;
            Ok(SyncOutcome {
                message: result.message,
                refresh_decks: true,
                requires_user_action: result.requires_user_action,
            })
        }
        SyncJob::FullUpload => {
            let result = crate::sync::full_upload(|progress| {
                let _ = sender.send(SyncTaskMessage::Progress(progress));
            })?;
            Ok(SyncOutcome {
                message: result.message,
                refresh_decks: false,
                requires_user_action: result.requires_user_action,
            })
        }
        SyncJob::Incremental => {
            let result = crate::sync::normal_sync()?;
            Ok(SyncOutcome {
                message: result.message,
                refresh_decks: true,
                requires_user_action: result.requires_user_action,
            })
        }
        SyncJob::Media => {
            let result = crate::sync::media_only_sync()?;
            Ok(SyncOutcome {
                message: result.message,
                refresh_decks: false,
                requires_user_action: result.requires_user_action,
            })
        }
    }
}

pub(super) fn start_automatic(app: &mut App) {
    if !crate::sync::paths::official_ready() {
        if crate::sync::paths::anki2_exists() {
            app.status = "Legacy shadow blocked · upload the correct Desktop collection to AnkiWeb, then use Sync F2".into();
        }
        return;
    }
    match crate::sync::load_auth() {
        Ok(Some(_)) => start_job(app, SyncJob::Incremental),
        Ok(None) => {}
        Err(error) => {
            app.status = format!("Could not read AnkiWeb credentials: {error:#}");
        }
    }
}

pub(super) fn request_quit(app: &mut App) {
    if app.quit_after_sync {
        return;
    }

    app.quit_after_sync = true;
    if app.sync_task.is_some() {
        app.status = "Finishing AnkiWeb sync before exit…".into();
        return;
    }

    match crate::sync::load_auth() {
        Ok(Some(_)) if crate::sync::paths::official_ready() => {
            start_job(app, SyncJob::Incremental);
            app.status = "Syncing with AnkiWeb before exit…".into();
        }
        Ok(_) => app.should_quit = true,
        Err(error) => {
            app.exit_sync_error = Some(format!(
                "Could not read AnkiWeb credentials before exit: {error:#}"
            ));
            app.should_quit = true;
        }
    }
}

pub(super) fn poll(app: &mut App) -> Result<()> {
    let mut received = None;
    let mut latest_progress = None;
    if let Some(task) = app.sync_task.as_ref() {
        loop {
            match task.receiver.try_recv() {
                Ok(SyncTaskMessage::Progress(progress)) => latest_progress = Some(progress),
                Ok(SyncTaskMessage::Finished(result)) => {
                    received = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    received = Some(Err("sync worker stopped unexpectedly".into()));
                    break;
                }
            }
        }
    }
    if let Some(progress) = latest_progress {
        app.status = progress_summary(&progress);
        app.sync_progress = Some(progress);
    }
    let Some(result) = received else {
        return Ok(());
    };

    let mut task = app.sync_task.take().expect("sync task disappeared");
    if let Some(handle) = task.handle.take() {
        let _ = handle.join();
    }
    app.sync_busy = false;
    app.sync_progress = None;

    match result {
        Ok(outcome) => {
            if task.kind == SyncJobKind::Login {
                app.sync_pass.reset();
            }
            app.sync_log = outcome.message.clone();
            app.status = outcome.message;
            if outcome.refresh_decks {
                app.refresh_decks()?;
            }
            if app.quit_after_sync && outcome.requires_user_action {
                app.exit_sync_error =
                    Some(format!("Automatic exit sync incomplete: {}", app.sync_log));
            }
        }
        Err(error) => {
            let message = format!("{} failed: {error}", job_name(task.kind));
            app.sync_log = message.clone();
            app.status = message.clone();
            if app.quit_after_sync {
                app.exit_sync_error = Some(message);
            }
        }
    }

    if app.quit_after_sync {
        if task.kind == SyncJobKind::Media && matches!(crate::sync::load_auth(), Ok(Some(_))) {
            start_job(app, SyncJob::Incremental);
            app.status = "Syncing collection before exit…".into();
        } else {
            app.should_quit = true;
        }
    }
    Ok(())
}

fn progress_message(kind: SyncJobKind) -> &'static str {
    match kind {
        SyncJobKind::Login => "Signing in and syncing…",
        SyncJobKind::FullDownload => "Full download + media…",
        SyncJobKind::FullUpload => "Full upload + media…",
        SyncJobKind::Incremental => "Incremental sync + media…",
        SyncJobKind::Media => "Media sync only…",
    }
}

fn job_name(kind: SyncJobKind) -> &'static str {
    match kind {
        SyncJobKind::Login => "Login",
        SyncJobKind::FullDownload => "Download",
        SyncJobKind::FullUpload => "Upload",
        SyncJobKind::Incremental => "Sync",
        SyncJobKind::Media => "Media sync",
    }
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
            Span::raw(" login + sync  "),
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
            "An initialized official collection syncs automatically at startup and exit.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "Media: ~/.local/share/anki-tui/anki/collection.media/",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "F2 keeps AnkiWeb; F3 keeps this official collection; F5 is bidirectional.",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Actions "));
    frame.render_widget(actions, chunks[3]);

    draw_status(frame, chunks[4], app);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    if app.sync_busy && area.height >= 3 {
        let progress_height = if area.height >= 6 { 3 } else { area.height };
        let areas = Layout::vertical([Constraint::Length(progress_height), Constraint::Min(0)])
            .areas::<2>(area);
        draw_progress(frame, areas[0], app.sync_progress.as_ref());
        if !areas[1].is_empty() {
            frame.render_widget(
                Paragraph::new(app.sync_log.as_str())
                    .wrap(Wrap { trim: false })
                    .block(Block::default().borders(Borders::ALL).title(" Status ")),
                areas[1],
            );
        }
    } else {
        frame.render_widget(
            Paragraph::new(app.sync_log.as_str())
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" Status ")),
            area,
        );
    }
}

fn draw_progress(
    frame: &mut Frame,
    area: Rect,
    progress: Option<&crate::anki_backend::BackendProgress>,
) {
    let Some(progress) = progress else {
        return;
    };
    let block = Block::default().borders(Borders::ALL).title(" Progress ");
    if let Some(total) = progress.total.filter(|total| *total > 0) {
        let current = progress.current.min(total);
        let ratio = current as f64 / total as f64;
        let label = format!(
            "{} · {:.0}% · {} / {}",
            progress.stage,
            ratio * 100.0,
            format_bytes(current),
            format_bytes(total)
        );
        frame.render_widget(
            Gauge::default()
                .block(block)
                .gauge_style(
                    Style::default()
                        .fg(Color::Cyan)
                        .bg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                )
                .use_unicode(true)
                .ratio(ratio)
                .label(label),
            area,
        );
    } else {
        frame.render_widget(
            Paragraph::new(progress_summary(progress))
                .style(Style::default().fg(Color::Cyan))
                .block(block),
            area,
        );
    }
}

fn progress_summary(progress: &crate::anki_backend::BackendProgress) -> String {
    if let Some(total) = progress.total.filter(|total| *total > 0) {
        let current = progress.current.min(total);
        let percent = current.saturating_mul(100) / total;
        format!(
            "{} · {percent}% · {} / {}",
            progress.stage,
            format_bytes(current),
            format_bytes(total)
        )
    } else if progress.detail.is_empty() {
        format!("{}…", progress.stage)
    } else {
        format!("{} · {}", progress.stage, progress.detail)
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn key(label: &str) -> Span<'_> {
    Span::styled(
        label,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn renders_download_percentage_and_bytes() {
        let backend = TestBackend::new(72, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let progress = crate::anki_backend::BackendProgress {
            stage: "Downloading collection".into(),
            current: 512,
            total: Some(1024),
            detail: String::new(),
        };

        terminal
            .draw(|frame| draw_progress(frame, frame.area(), Some(&progress)))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("Downloading collection · 50% · 512 B / 1.0 KiB"));
    }

    #[test]
    fn formats_progress_without_a_fake_percentage() {
        let progress = crate::anki_backend::BackendProgress {
            stage: "Syncing media".into(),
            current: 0,
            total: None,
            detail: "Added 3 · Checked 12".into(),
        };

        assert_eq!(
            progress_summary(&progress),
            "Syncing media · Added 3 · Checked 12"
        );
    }
}
