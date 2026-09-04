use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use rs_fsrs::Rating;

use super::{App, Screen};
use crate::media_view::{parse_card_text, NF_PLAY};
use crate::scheduler::{rating_key, rating_label};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.media.reset();
            app.screen = Screen::DeckBrowser;
            app.refresh_decks()?;
            app.status = "Back to decks".into();
        }
        KeyCode::Char(' ') | KeyCode::Enter if !app.answer_shown => {
            app.answer_shown = true;
        }
        KeyCode::Char('m') => {
            if let Some(card) = app.study_queue.get(app.study_index) {
                match app
                    .media
                    .replay_sounds(&card.front, &card.back, app.answer_shown)
                {
                    Ok(msg) => app.status = msg,
                    Err(e) => app.status = format!("{e:#}"),
                }
            }
        }
        KeyCode::Char('e') => {
            super::edit::open_from_study(app)?;
        }
        KeyCode::Char('f') => {
            // Anki study: Reset card → New, then skip to next
            if let Some(card) = app.study_queue.get(app.study_index).cloned() {
                app.store.forget_card(card.card.id)?;
                app.study_queue.remove(app.study_index);
                app.answer_shown = false;
                app.media.stop_audio();
                let _ = app.refresh_decks();
                if app.study_index >= app.study_queue.len() {
                    app.study_index = app.study_queue.len().saturating_sub(1);
                }
                if app.study_queue.is_empty() {
                    app.media.reset();
                    app.screen = Screen::DeckBrowser;
                    app.refresh_decks()?;
                    app.status = "Card reset to New · queue empty".into();
                } else {
                    app.status = "Card reset to New (Anki Forget)".into();
                }
            }
        }
        KeyCode::Char('1') if app.answer_shown => rate(app, Rating::Again)?,
        KeyCode::Char('2') if app.answer_shown => rate(app, Rating::Hard)?,
        KeyCode::Char('3') if app.answer_shown => rate(app, Rating::Good)?,
        KeyCode::Char('4') if app.answer_shown => rate(app, Rating::Easy)?,
        _ => {}
    }
    Ok(())
}

fn rate(app: &mut App, rating: Rating) -> Result<()> {
    let Some(current) = app.study_queue.get(app.study_index).cloned() else {
        return Ok(());
    };
    let (updated, rating) = app.scheduler.review(&current.card, rating);
    app.store
        .update_card_schedule(&updated, rating as i32)?;
    app.study_done += 1;
    app.study_index += 1;
    app.answer_shown = false;
    app.media.stop_audio();
    let _ = app.refresh_decks();

    if app.study_index >= app.study_queue.len() {
        if let Some(deck_id) = app.current_deck_id() {
            app.study_queue = app.store.next_study_queue(deck_id, 50)?;
            app.study_index = 0;
        }
        if app.study_queue.is_empty() {
            app.media.reset();
            app.screen = Screen::DeckBrowser;
            app.refresh_decks()?;
            app.status = format!("Session done — reviewed {} cards", app.study_done);
        } else {
            app.status = format!("Continuing… {} remaining", app.study_queue.len());
        }
    } else {
        app.status = format!(
            "Rated {} · {} left in queue",
            rating_label(rating),
            app.study_queue.len().saturating_sub(app.study_index)
        );
    }
    Ok(())
}

/// Main study pane splits — keep in sync with study layout.
pub fn layout_areas(main: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(5),
    ])
    .areas::<3>(main);
    (chunks[0], chunks[1], chunks[2])
}

pub fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(card) = app.study_queue.get(app.study_index).cloned() else {
        frame.render_widget(
            Paragraph::new("No cards to study.")
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let _ = app
        .media
        .prepare_card(card.card.id, app.answer_shown, &card.front, &card.back);
    if let Some(msg) = app.media.last_image_status.clone() {
        if app.status != msg {
            app.status = msg;
        }
    }

    let (meta_area, face_area, action_area) = layout_areas(area);

    draw_meta(frame, meta_area, app, &card);
    draw_card_face(frame, face_area, app, &card);
    draw_actions(frame, action_area, app, &card);
}

fn draw_meta(frame: &mut Frame, area: Rect, app: &App, card: &crate::models::StudyCard) {
    let remaining = app.study_queue.len().saturating_sub(app.study_index);
    let due = app
        .decks
        .get(app.selected_deck)
        .map(|d| {
            format!(
                "{} due ({}n/{}l/{}r)",
                d.due_total(),
                d.new,
                d.learning,
                d.review
            )
        })
        .unwrap_or_default();

    let line = Line::from(vec![
        Span::styled(
            &card.deck_name,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ·  {}  ·  ", card.card.state.as_str()),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(due),
        Span::styled(
            format!("  ·  {} done / {} left", app.study_done, remaining),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_card_face(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    card: &crate::models::StudyCard,
) {
    draw_card_preview(
        frame,
        area,
        &card.front,
        &card.back,
        app.answer_shown,
        None,
        Some(&mut app.media),
    );
}

/// Study-style card face (question centered / answer with FrontSide + hr + Back).
/// Used by Study and by Edit's live preview pane.
///
/// Images are placed in document order (where `<img>` appears in the field).
pub fn draw_card_preview(
    frame: &mut Frame,
    area: Rect,
    front_raw: &str,
    back_raw: &str,
    answer_shown: bool,
    title_prefix: Option<&str>,
    mut media: Option<&mut crate::media_view::MediaSession>,
) {
    use crate::media_view::{parse_flow, FlowItem};

    let side = if answer_shown {
        parse_card_text(&format!("{front_raw}\n{back_raw}"))
    } else {
        parse_card_text(front_raw)
    };

    let kind = if side.has_sound() {
        if answer_shown {
            format!("Answer  {NF_PLAY}")
        } else {
            format!("Question  {NF_PLAY}")
        }
    } else if answer_shown {
        "Answer".into()
    } else {
        "Question".into()
    };
    let title = match title_prefix {
        Some(p) => format!(" {p} · {kind} "),
        None => format!(" {kind} "),
    };
    let border_style = Style::default().fg(if answer_shown {
        Color::Green
    } else {
        Color::Cyan
    });

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build flow: front items, optional separator, back items — keep img order.
    #[derive(Clone)]
    enum Row {
        Text { text: String, style: Style },
        Separator,
        Blank,
        Image,
    }

    let mut rows: Vec<Row> = Vec::new();
    let front_style = if answer_shown {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    };
    let back_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let push_text_rows = |rows: &mut Vec<Row>, text: String, style: Style| {
        // Leading blank lines (after <img>, etc.) become explicit Blank rows.
        let mut rest = text.as_str();
        while let Some(r) = rest.strip_prefix('\n') {
            rows.push(Row::Blank);
            rest = r;
        }
        if !rest.is_empty() {
            rows.push(Row::Text {
                text: rest.to_string(),
                style,
            });
        } else if text.is_empty() {
            rows.push(Row::Blank);
        }
    };

    for item in parse_flow(front_raw) {
        match item {
            FlowItem::Text(t) => push_text_rows(&mut rows, t, front_style),
            FlowItem::Image(_) => rows.push(Row::Image),
        }
    }
    if answer_shown {
        rows.push(Row::Separator);
        for item in parse_flow(back_raw) {
            match item {
                FlowItem::Text(t) => push_text_rows(&mut rows, t, back_style),
                FlowItem::Image(_) => rows.push(Row::Image),
            }
        }
    }

    let show_image = media.as_ref().is_some_and(|m| m.has_ready_image());
    if !show_image {
        rows.retain(|r| !matches!(r, Row::Image));
    }

    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new("(empty)")
                .alignment(Alignment::Center)
                .style(front_style),
            inner,
        );
        return;
    }

    // Fixed row heights (image capped), then pad so the whole stack is centered.
    let img_h = media
        .as_ref()
        .and_then(|m| {
            m.image_cell_size(ratatui::layout::Size::new(
                inner.width,
                inner.height.saturating_sub(6),
            ))
        })
        .map(|s| s.height.max(4))
        .unwrap_or(10);

    let row_heights: Vec<u16> = rows
        .iter()
        .map(|r| match r {
            Row::Text { text, .. } => {
                let lines = if text.is_empty() {
                    1
                } else {
                    text.lines().count().max(1) as u16
                };
                lines.min(16).max(1)
            }
            Row::Separator => 3,
            Row::Blank => 1,
            Row::Image => img_h,
        })
        .collect();

    let content_h: u16 = row_heights.iter().sum();
    let spare = inner.height.saturating_sub(content_h);
    let pad_top = spare / 2;
    let pad_bottom = spare.saturating_sub(pad_top);

    let mut constraints: Vec<Constraint> = Vec::with_capacity(rows.len() + 2);
    if pad_top > 0 {
        constraints.push(Constraint::Length(pad_top));
    }
    for h in &row_heights {
        constraints.push(Constraint::Length(*h));
    }
    if pad_bottom > 0 {
        constraints.push(Constraint::Length(pad_bottom));
    }

    let chunks = Layout::vertical(constraints).split(inner);
    let row_chunks = if pad_top > 0 {
        &chunks[1..1 + rows.len()]
    } else {
        &chunks[..rows.len()]
    };

    let mut image_drawn = false;
    for (row, rect) in rows.iter().zip(row_chunks.iter()) {
        match row {
            Row::Text { text, style } => {
                let body = text_lines(text, *style);
                frame.render_widget(
                    Paragraph::new(body)
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: false }),
                    *rect,
                );
            }
            Row::Separator => {
                let line = Line::from(Span::styled(
                    "─".repeat(24),
                    Style::default().fg(Color::DarkGray),
                ));
                frame.render_widget(
                    Paragraph::new(vec![Line::from(""), line, Line::from("")])
                        .alignment(Alignment::Center),
                    *rect,
                );
            }
            Row::Blank => {
                frame.render_widget(Paragraph::new(""), *rect);
            }
            Row::Image => {
                if let Some(ref mut m) = media {
                    if !image_drawn {
                        m.render_image(frame, *rect);
                        image_drawn = true;
                    }
                }
            }
        }
    }
}

fn text_lines(text: &str, style: Style) -> Vec<Line<'static>> {
    // Preserve blank lines from the field (e.g. empty line under <img>).
    if text.is_empty() {
        return vec![Line::from("")];
    }
    if text.trim().is_empty() {
        return text.lines().map(|_| Line::from("")).collect();
    }
    text.lines()
        .map(|l| {
            if l.trim().is_empty() {
                Line::from("")
            } else {
                Line::from(spans_with_play_icon(l, style))
            }
        })
        .collect()
}

/// Highlight Nerd Font play glyphs in cyan so they read as buttons.
fn spans_with_play_icon(text: &str, style: Style) -> Vec<Span<'static>> {
    let play_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find(NF_PLAY) {
        if idx > 0 {
            spans.push(Span::styled(rest[..idx].to_string(), style));
        }
        spans.push(Span::styled(NF_PLAY.to_string(), play_style));
        rest = &rest[idx + NF_PLAY.len()..];
    }
    if !rest.is_empty() || spans.is_empty() {
        spans.push(Span::styled(rest.to_string(), style));
    }
    spans
}

fn side_has_audio(card: &crate::models::StudyCard, answer_shown: bool) -> bool {
    let text = if answer_shown {
        format!("{}\n{}", card.front, card.back)
    } else {
        card.front.clone()
    };
    parse_card_text(&text).has_sound()
}

fn play_hint_line(has_audio: bool) -> Line<'static> {
    if has_audio {
        Line::from(vec![
            Span::styled(
                format!("{NF_PLAY}  "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "[m]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Play audio"),
            Span::styled(
                "  ·  e edit  ·  Esc decks",
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
    } else {
        Line::from(Span::styled(
            "e edit  ·  Esc decks",
            Style::default().fg(Color::DarkGray),
        ))
        .alignment(Alignment::Center)
    }
}

fn draw_actions(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    card: &crate::models::StudyCard,
) {
    let has_audio = side_has_audio(card, app.answer_shown);
    if app.answer_shown {
        let intervals = app.scheduler.preview_intervals(&card.card);
        let mut spans = Vec::new();
        for (i, (rating, interval)) in intervals.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("     "));
            }
            spans.push(Span::styled(
                format!(
                    "[{}] {}  {}",
                    rating_key(*rating),
                    rating_label(*rating),
                    interval
                ),
                match rating {
                    Rating::Again => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    Rating::Hard => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    Rating::Good => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    Rating::Easy => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                },
            ));
        }
        let body = Paragraph::new(vec![
            Line::from(spans).alignment(Alignment::Center),
            Line::from(""),
            play_hint_line(has_audio),
        ])
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Rate recall "),
        );
        frame.render_widget(body, area);
    } else {
        let mut lines = vec![
            Line::from(Span::styled(
                "Space  Show Answer",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Center),
        ];
        if has_audio {
            lines.push(Line::from(""));
            lines.push(
                Line::from(vec![
                    Span::styled(
                        format!(" {NF_PLAY}  "),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "Play",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  [m]", Style::default().fg(Color::DarkGray)),
                ])
                .alignment(Alignment::Center),
            );
        }
        lines.push(Line::from(""));
        lines.push(
            Line::from(Span::styled(
                "e edit  ·  Esc decks",
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(Alignment::Center),
        );
        let body = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" "));
        frame.render_widget(body, area);
    }
}
