mod db;
mod media_view;
mod media_import;
mod models;
mod scheduler;
mod sync;
mod ui;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::Duration;

use crate::db::Store;
use crate::scheduler::Scheduler;
use crate::ui::{App, PendingExternal, Screen};

fn main() -> Result<()> {
    let store = Store::open_default().context("failed to open collection database")?;
    let scheduler = Scheduler::new();
    let mut app = App::new(store, scheduler)?;

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app);
    app.media.reset();
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.should_quit {
        if app.pending_external != PendingExternal::None {
            let action = std::mem::replace(&mut app.pending_external, PendingExternal::None);
            run_external(terminal, app, action)?;
            continue;
        }

        let image_area = terminal.draw(|frame| ui::draw(frame, app))?.area;
        // After ratatui paints, place Kitty images / autoplay audio for study
        if app.screen == Screen::Study {
            if let Some(card) = app.study_queue.get(app.study_index) {
                // Content pane is roughly below the 3-line meta header inside main area.
                // Use the same vertical split as study::draw.
                let main = {
                    let area = image_area;
                    let chunks = ratatui::layout::Layout::vertical([
                        ratatui::layout::Constraint::Length(1),
                        ratatui::layout::Constraint::Min(0),
                        ratatui::layout::Constraint::Length(1),
                    ])
                    .areas::<3>(area);
                    chunks[1]
                };
                let (_meta, face, _actions) = ui::study::layout_areas(main);
                let content = ui::study::card_face_inner(face);
                let _ = app.media.on_card_side(
                    card.card.id,
                    app.answer_shown,
                    &card.front,
                    &card.back,
                    content,
                );
            }
        } else {
            app.media.reset();
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c'))
                    {
                        app.should_quit = true;
                    } else {
                        app.handle_key(key)?;
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

fn run_external(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    action: PendingExternal,
) -> Result<()> {
    match action {
        PendingExternal::None => {}
        PendingExternal::YaziPickImage => {
            ratatui::restore();
            let result = match app.screen {
                Screen::AddNote => ui::add::insert_image_via_yazi(app),
                _ => ui::edit::insert_image_via_yazi(app),
            };
            *terminal = ratatui::init();
            let _ = terminal.clear();
            match result {
                Ok(Some(fname)) => {
                    app.status = format!("Inserted <img src=\"{fname}\">");
                }
                Ok(None) => {
                    app.status = "Image pick cancelled".into();
                }
                Err(e) => {
                    app.status = format!("{e:#}");
                }
            }
        }
    }
    Ok(())
}
