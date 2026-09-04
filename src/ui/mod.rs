pub(crate) mod add;
pub(crate) mod browse;
mod common;
mod decks;
pub(crate) mod edit;
mod options;
mod stats;
pub(crate) mod study;
mod sync;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use tui_input::Input;

use crate::db::Store;
use crate::media_view::MediaSession;
use crate::models::{BrowseRow, DeckCounts, StatsSummary, StudyCard};
use crate::scheduler::Scheduler;

use self::sync::SyncField;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    DeckBrowser,
    Study,
    AddNote,
    Browse,
    Stats,
    EditNote,
    DeckOptions,
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingExternal {
    None,
    /// Suspend TUI and open yazi to pick an image for the current Front/Back field.
    YaziPickImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    CreateDeck,
    RenameDeck,
    ConfirmDeleteDeck,
    ConfirmDeleteCard,
    /// Anki Cards → Reset (Forget) for whole deck
    ConfirmResetDeck,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddField {
    Front,
    Back,
    Tags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsField {
    NewPerDay,
    RevPerDay,
}

pub struct App {
    pub store: Store,
    pub scheduler: Scheduler,
    pub should_quit: bool,
    pub screen: Screen,
    pub modal: Modal,
    pub status: String,
    pub decks: Vec<DeckCounts>,
    pub selected_deck: usize,
    pub study_queue: Vec<StudyCard>,
    pub study_index: usize,
    pub answer_shown: bool,
    pub study_done: usize,
    pub add_field: AddField,
    pub add_front: Input,
    pub add_back: Input,
    pub add_tags: Input,
    pub browse_query: Input,
    pub browse_focus_query: bool,
    pub browse_rows: Vec<BrowseRow>,
    pub browse_selected: usize,
    pub edit_note_id: Option<i64>,
    pub edit_card_id: Option<i64>,
    pub edit_field: AddField,
    pub edit_front: Input,
    pub edit_back: Input,
    pub edit_tags: Input,
    pub options_field: OptionsField,
    pub options_new: Input,
    pub options_rev: Input,
    pub modal_input: Input,
    pub stats: StatsSummary,
    pub sync_field: SyncField,
    pub sync_user: Input,
    pub sync_pass: Input,
    pub sync_log: String,
    pub sync_busy: bool,
    pub media: MediaSession,
    /// Where EditNote should return (Browse or Study).
    pub edit_return: Screen,
    pub pending_external: PendingExternal,
}

impl App {
    pub fn new(store: Store, scheduler: Scheduler) -> Result<Self> {
        let mut app = Self {
            store,
            scheduler,
            should_quit: false,
            screen: Screen::DeckBrowser,
            modal: Modal::None,
            status: "Welcome to anki-tui — press ? for help".into(),
            decks: Vec::new(),
            selected_deck: 0,
            study_queue: Vec::new(),
            study_index: 0,
            answer_shown: false,
            study_done: 0,
            add_field: AddField::Front,
            add_front: Input::default(),
            add_back: Input::default(),
            add_tags: Input::default(),
            browse_query: Input::default(),
            browse_focus_query: true,
            browse_rows: Vec::new(),
            browse_selected: 0,
            edit_note_id: None,
            edit_card_id: None,
            edit_field: AddField::Front,
            edit_front: Input::default(),
            edit_back: Input::default(),
            edit_tags: Input::default(),
            options_field: OptionsField::NewPerDay,
            options_new: Input::default(),
            options_rev: Input::default(),
            modal_input: Input::default(),
            stats: StatsSummary::default(),
            sync_field: SyncField::User,
            sync_user: Input::default(),
            sync_pass: Input::default(),
            sync_log: String::new(),
            sync_busy: false,
            media: MediaSession::new(),
            edit_return: Screen::Browse,
            pending_external: PendingExternal::None,
        };
        app.refresh_decks()?;
        Ok(app)
    }

    pub fn refresh_decks(&mut self) -> Result<()> {
        self.decks = self.store.list_deck_counts()?;
        if self.selected_deck >= self.decks.len() && !self.decks.is_empty() {
            self.selected_deck = self.decks.len() - 1;
        }
        Ok(())
    }

    pub fn current_deck_id(&self) -> Option<i64> {
        self.decks.get(self.selected_deck).map(|d| d.deck_id)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.modal != Modal::None {
            return self.handle_modal_key(key);
        }

        // Global shortcuts when not typing heavily
        match (self.screen, key.code) {
            (_, KeyCode::Char('?')) => {
                self.modal = Modal::Help;
                return Ok(());
            }
            (Screen::DeckBrowser, KeyCode::Char('q')) => {
                self.should_quit = true;
                return Ok(());
            }
            _ => {}
        }

        match self.screen {
            Screen::DeckBrowser => decks::handle_key(self, key)?,
            Screen::Study => study::handle_key(self, key)?,
            Screen::AddNote => add::handle_key(self, key)?,
            Screen::Browse => browse::handle_key(self, key)?,
            Screen::Stats => stats::handle_key(self, key)?,
            Screen::EditNote => edit::handle_key(self, key)?,
            Screen::DeckOptions => options::handle_key(self, key)?,
            Screen::Sync => sync::handle_key(self, key)?,
        }
        Ok(())
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> Result<()> {
        match self.modal {
            Modal::Help => {
                self.modal = Modal::None;
            }
            Modal::CreateDeck | Modal::RenameDeck => match key.code {
                KeyCode::Esc => {
                    self.modal = Modal::None;
                    self.modal_input.reset();
                }
                KeyCode::Enter => {
                    let name = self.modal_input.value().trim().to_string();
                    if name.is_empty() {
                        self.status = "Deck name cannot be empty".into();
                    } else if self.modal == Modal::CreateDeck {
                        match self.store.create_deck(&name) {
                            Ok(_) => {
                                self.status = format!("Created deck '{name}'");
                                self.refresh_decks()?;
                            }
                            Err(e) => self.status = format!("Create failed: {e}"),
                        }
                    } else if let Some(id) = self.current_deck_id() {
                        match self.store.rename_deck(id, &name) {
                            Ok(()) => {
                                self.status = format!("Renamed to '{name}'");
                                self.refresh_decks()?;
                            }
                            Err(e) => self.status = format!("Rename failed: {e}"),
                        }
                    }
                    self.modal = Modal::None;
                    self.modal_input.reset();
                }
                _ => {
                    let _ = common::input_from_key(&mut self.modal_input, key);
                }
            },
            Modal::ConfirmDeleteDeck => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(id) = self.current_deck_id() {
                        match self.store.delete_deck(id) {
                            Ok(()) => {
                                self.status = "Deck deleted".into();
                                self.refresh_decks()?;
                            }
                            Err(e) => self.status = format!("Delete failed: {e}"),
                        }
                    }
                    self.modal = Modal::None;
                }
                _ => self.modal = Modal::None,
            },
            Modal::ConfirmDeleteCard => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(card_id) = self.edit_card_id {
                        match self.store.delete_card(card_id) {
                            Ok(()) => {
                                self.status = "Card deleted".into();
                                if self.edit_return == Screen::Study {
                                    edit::after_delete_from_study(self)?;
                                } else {
                                    self.screen = Screen::Browse;
                                    browse::reload(self)?;
                                }
                            }
                            Err(e) => self.status = format!("Delete failed: {e}"),
                        }
                    }
                    self.modal = Modal::None;
                }
                _ => self.modal = Modal::None,
            },
            Modal::ConfirmResetDeck => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Some(id) = self.current_deck_id() {
                        match self.store.forget_deck(id) {
                            Ok(n) => {
                                self.status = format!(
                                    "Reset {n} cards to New (Anki Forget) — ready to study again"
                                );
                                self.refresh_decks()?;
                            }
                            Err(e) => self.status = format!("Reset failed: {e}"),
                        }
                    }
                    self.modal = Modal::None;
                }
                _ => self.modal = Modal::None,
            },
            Modal::None => {}
        }
        Ok(())
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas::<3>(area);

    draw_title(frame, chunks[0], app);
    match app.screen {
        Screen::DeckBrowser => decks::draw(frame, chunks[1], app),
        Screen::Study => study::draw(frame, chunks[1], app),
        Screen::AddNote => add::draw(frame, chunks[1], app),
        Screen::Browse => browse::draw(frame, chunks[1], app),
        Screen::Stats => stats::draw(frame, chunks[1], app),
        Screen::EditNote => edit::draw(frame, chunks[1], app),
        Screen::DeckOptions => options::draw(frame, chunks[1], app),
        Screen::Sync => sync::draw(frame, chunks[1], app),
    }
    draw_status(frame, chunks[2], app);

    if app.modal != Modal::None {
        draw_modal(frame, area, app);
    }
}

fn draw_title(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::DeckBrowser => "anki-tui · Decks",
        Screen::Study => "anki-tui · Study",
        Screen::AddNote => "anki-tui · Add",
        Screen::Browse => "anki-tui · Browse",
        Screen::Stats => "anki-tui · Stats",
        Screen::EditNote => "anki-tui · Edit",
        Screen::DeckOptions => "anki-tui · Deck Options",
        Screen::Sync => "anki-tui · AnkiWeb Sync",
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {title}"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("? help", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.screen {
        Screen::DeckBrowser => "Enter study · a add · b browse · f reset deck · y sync · s stats · q quit",
        Screen::Study => "Space show · 1-4 rate · e edit · f forget · m audio · Esc decks",
        Screen::AddNote => "Tab fields · Ctrl+i image · Enter save · Esc back",
        Screen::Browse => "/ search · Enter edit · s suspend · f forget · Esc decks",
        Screen::Stats => "Esc decks",
        Screen::EditNote => "Tab fields · Enter newline · Ctrl+i image · Ctrl+s save · Esc",
        Screen::DeckOptions => "Tab fields · Enter save · Esc decks",
        Screen::Sync => "F1 login · F5 incremental · F6 media · F2/F3 full · Esc",
    };
    let text = format!(" {}  │  {}", app.status, hints);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_modal(frame: &mut Frame, area: Rect, app: &App) {
    let popup = centered_rect(60, 40, area);
    frame.render_widget(Clear, popup);

    match app.modal {
        Modal::Help => {
            let help = Paragraph::new(common::help_text())
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Help · any key to close "),
                )
                .style(Style::default().fg(Color::White));
            frame.render_widget(help, popup);
        }
        Modal::CreateDeck | Modal::RenameDeck => {
            let title = if app.modal == Modal::CreateDeck {
                " New Deck "
            } else {
                " Rename Deck "
            };
            let body = Paragraph::new(vec![
                Line::from("Enter deck name:"),
                Line::from(app.modal_input.value()),
                Line::from(""),
                Line::from("Enter confirm · Esc cancel"),
            ])
            .block(Block::default().borders(Borders::ALL).title(title));
            frame.render_widget(body, popup);
            // Cursor on the name line (inside border, under the prompt)
            if popup.width > 2 && popup.height > 3 {
                let scroll = app
                    .modal_input
                    .visual_scroll(popup.width.saturating_sub(2) as usize);
                let x_off = app.modal_input.visual_cursor().saturating_sub(scroll) as u16;
                let x = popup.x + 1 + x_off.min(popup.width.saturating_sub(3));
                frame.set_cursor_position((x, popup.y + 2));
            }
        }
        Modal::ConfirmDeleteDeck => {
            let name = app
                .decks
                .get(app.selected_deck)
                .map(|d| d.name.as_str())
                .unwrap_or("?");
            let body = Paragraph::new(vec![
                Line::from(format!("Delete deck '{name}' and all its cards?")),
                Line::from(""),
                Line::from("y confirm · any other key cancel"),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm Delete ")
                    .border_style(Style::default().fg(Color::Red)),
            );
            frame.render_widget(body, popup);
        }
        Modal::ConfirmDeleteCard => {
            let body = Paragraph::new(vec![
                Line::from("Delete this card/note permanently?"),
                Line::from(""),
                Line::from("y confirm · any other key cancel"),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm Delete ")
                    .border_style(Style::default().fg(Color::Red)),
            );
            frame.render_widget(body, popup);
        }
        Modal::ConfirmResetDeck => {
            let name = app
                .decks
                .get(app.selected_deck)
                .map(|d| d.name.as_str())
                .unwrap_or("?");
            let body = Paragraph::new(vec![
                Line::from(format!(
                    "Reset all cards in '{name}' to New?"
                )),
                Line::from("Like Anki: Cards → Reset (Forget). Review history kept but ignored."),
                Line::from(""),
                Line::from("y confirm · any other key cancel"),
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Reset / Forget ")
                    .border_style(Style::default().fg(Color::Yellow)),
            );
            frame.render_widget(body, popup);
        }
        Modal::None => {}
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas::<3>(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas::<3>(vertical[1])[1]
}
