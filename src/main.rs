mod db;
mod media_view;
mod media_import;
mod models;
mod scheduler;
mod sync;
mod ui;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use ratatui::DefaultTerminal;
use std::io::stdout;
use std::time::Duration;

use crate::db::Store;
use crate::scheduler::Scheduler;
use crate::ui::{App, PendingExternal, Screen};

fn main() -> Result<()> {
    let store = Store::open_default().context("failed to open collection database")?;
    let scheduler = Scheduler::new();
    let mut app = App::new(store, scheduler)?;

    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    // Query Kitty/Sixel/etc. once; may briefly touch stdin before the event loop.
    app.media.init_picker();
    let result = run(&mut terminal, &mut app);
    app.media.reset();
    let _ = execute!(stdout(), DisableBracketedPaste);
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

        terminal.draw(|frame| ui::draw(frame, app))?;

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
                Event::Paste(text) => {
                    app.handle_paste(&text)?;
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
            let _ = execute!(stdout(), EnableBracketedPaste);
            let _ = terminal.clear();
            // Re-detect graphics after alternate screen restore.
            app.media.picker_reset();
            app.media.init_picker();
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
