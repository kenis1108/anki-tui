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
use std::time::Instant;

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
                match app.media.replay_sounds(
                    &card.front,
                    &card.back,
                    app.answer_shown,
                    card.answer_includes_question,
                ) {
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
                if app.store.uses_official_backend() {
                    refill_official_queue(app)?;
                } else {
                    app.study_queue.remove(app.study_index);
                }
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
    if app.store.uses_official_backend() {
        let Some(deck_id) = app.current_deck_id() else {
            return Ok(());
        };
        app.study_queue = crate::anki_backend::answer_card(
            current.card.id,
            rating as i32,
            app.study_started_at.elapsed().as_millis(),
            &current.scheduling_states_hex,
            deck_id,
            50,
        )?;
        app.study_index = 0;
    } else {
        let (updated, _) = app.scheduler.review(&current.card, rating);
        app.store.update_card_schedule(&updated, rating as i32)?;
        app.study_index += 1;
    }
    app.study_done += 1;
    app.answer_shown = false;
    app.media.stop_audio();
    let _ = app.refresh_decks();

    app.study_started_at = Instant::now();

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

fn refill_official_queue(app: &mut App) -> Result<()> {
    if let Some(deck_id) = app.current_deck_id() {
        app.study_queue = app.store.next_study_queue(deck_id, 50)?;
    } else {
        app.study_queue.clear();
    }
    app.study_index = 0;
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

    let _ = app.media.prepare_card(
        card.card.id,
        app.answer_shown,
        card.answer_includes_question,
        &card.front,
        &card.back,
    );
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

fn draw_card_face(frame: &mut Frame, area: Rect, app: &mut App, card: &crate::models::StudyCard) {
    draw_card_preview(
        frame,
        area,
        CardPreview {
            front: &card.front,
            back: &card.back,
            front_document: card.front_document.as_ref(),
            back_document: card.back_document.as_ref(),
            answer_shown: app.answer_shown,
            answer_includes_question: card.answer_includes_question,
            title_prefix: None,
        },
        Some(&mut app.media),
    );
}

pub struct CardPreview<'a> {
    pub front: &'a str,
    pub back: &'a str,
    pub front_document: Option<&'a crate::models::CardDocument>,
    pub back_document: Option<&'a crate::models::CardDocument>,
    pub answer_shown: bool,
    pub answer_includes_question: bool,
    pub title_prefix: Option<&'a str>,
}

/// Study-style card face (question centered / answer with FrontSide + hr + Back).
/// Used by Study and by Edit's live preview pane.
///
/// Images are placed in document order (where `<img>` appears in the field).
pub fn draw_card_preview(
    frame: &mut Frame,
    area: Rect,
    preview: CardPreview<'_>,
    mut media: Option<&mut crate::media_view::MediaSession>,
) {
    use crate::media_view::{parse_flow, FlowItem};

    let CardPreview {
        front: front_raw,
        back: back_raw,
        front_document,
        back_document,
        answer_shown,
        answer_includes_question,
        title_prefix,
    } = preview;

    let side = if answer_shown && !answer_includes_question {
        parse_card_text(&format!("{front_raw}\n{back_raw}"))
    } else if answer_shown {
        parse_card_text(back_raw)
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

    #[derive(Clone)]
    enum Row {
        Text {
            lines: Vec<Line<'static>>,
            alignment: Alignment,
            background: Option<Color>,
        },
        Separator,
        Blank,
        Image {
            source: String,
        },
        Audio {
            count: usize,
        },
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
                lines: text_lines(rest, style),
                alignment: Alignment::Center,
                background: None,
            });
        } else if text.is_empty() {
            rows.push(Row::Blank);
        }
    };

    let official_document = if answer_shown {
        back_document
    } else {
        front_document
    };

    if let Some(document) = official_document {
        use crate::models::CardBlock;
        for block in &document.blocks {
            match block {
                CardBlock::Text {
                    runs,
                    alignment,
                    background,
                } => rows.push(Row::Text {
                    lines: styled_document_lines(runs),
                    alignment: parse_alignment(alignment),
                    background: parse_card_color(background),
                }),
                CardBlock::Separator => rows.push(Row::Separator),
                CardBlock::Image { source } => rows.push(Row::Image {
                    source: source.clone(),
                }),
                CardBlock::Audio { sources } => rows.push(Row::Audio {
                    count: sources.len(),
                }),
                CardBlock::Spacer => rows.push(Row::Blank),
            }
        }
        if let Some(background) = parse_card_color(&document.background) {
            frame.render_widget(
                Block::default().style(Style::default().bg(background)),
                inner,
            );
        }
    } else {
        // Standalone cards do not have official rendered template metadata.
        if !answer_shown || !answer_includes_question {
            for item in parse_flow(front_raw) {
                match item {
                    FlowItem::Text(t) => push_text_rows(&mut rows, t, front_style),
                    FlowItem::Image(source) => rows.push(Row::Image { source }),
                }
            }
        }
        if answer_shown && !answer_includes_question {
            rows.push(Row::Separator);
        }
        if answer_shown {
            for item in parse_flow(back_raw) {
                match item {
                    FlowItem::Text(t) => push_text_rows(&mut rows, t, back_style),
                    FlowItem::Image(source) => rows.push(Row::Image { source }),
                }
            }
        }
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

    let horizontal_margin: u16 = if inner.width >= 8 { 2 } else { 0 };
    let text_width = inner
        .width
        .saturating_sub(horizontal_margin.saturating_mul(2))
        .max(1);

    // Fixed block heights (image capped), then center the card's document flow.
    let row_heights: Vec<u16> = rows
        .iter()
        .map(|r| match r {
            Row::Text { lines, .. } => lines
                .iter()
                .map(|line| line.width().max(1).div_ceil(text_width as usize))
                .sum::<usize>()
                .clamp(1, 16) as u16,
            Row::Separator => 3,
            Row::Blank => 1,
            Row::Image { source } => media
                .as_ref()
                .and_then(|session| {
                    session.image_cell_size_for(
                        source,
                        ratatui::layout::Size::new(inner.width, inner.height.saturating_sub(6)),
                    )
                })
                .map(|size| size.height.max(4))
                .unwrap_or(1),
            Row::Audio { .. } => 1,
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

    for (row, rect) in rows.iter().zip(row_chunks.iter()) {
        let content_rect = Rect {
            x: rect.x.saturating_add(horizontal_margin),
            y: rect.y,
            width: rect
                .width
                .saturating_sub(horizontal_margin.saturating_mul(2)),
            height: rect.height,
        };
        match row {
            Row::Text {
                lines,
                alignment,
                background,
            } => {
                let mut style = Style::default();
                if let Some(background) = background {
                    style = style.bg(*background);
                }
                frame.render_widget(
                    Paragraph::new(lines.clone())
                        .alignment(*alignment)
                        .style(style)
                        .wrap(Wrap { trim: false }),
                    content_rect,
                );
            }
            Row::Separator => {
                let line = Line::from(Span::styled(
                    "─".repeat(content_rect.width as usize),
                    Style::default().fg(Color::DarkGray),
                ));
                frame.render_widget(
                    Paragraph::new(vec![Line::from(""), line, Line::from("")]),
                    content_rect,
                );
            }
            Row::Blank => {
                frame.render_widget(Paragraph::new(""), *rect);
            }
            Row::Image { source } => {
                if let Some(ref mut m) = media {
                    if m.has_ready_image_for(source) {
                        m.render_image_for(frame, *rect, source);
                    } else {
                        draw_image_placeholder(frame, content_rect, source);
                    }
                } else {
                    draw_image_placeholder(frame, content_rect, source);
                }
            }
            Row::Audio { count } => draw_audio_buttons(frame, content_rect, *count),
        }
    }
}

fn styled_document_lines(runs: &[crate::models::CardTextRun]) -> Vec<Line<'static>> {
    let mut lines = vec![Vec::new()];
    for run in runs {
        let mut style = Style::default();
        if let Some(color) = parse_card_color(&run.color) {
            style = style.fg(color);
        }
        if run.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if run.italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if run.underline {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if run.crossed_out {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        for (index, part) in run.text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty() {
                lines
                    .last_mut()
                    .expect("document line")
                    .push(Span::styled(part.to_string(), style));
            }
        }
    }
    lines.into_iter().map(Line::from).collect()
}

fn parse_alignment(value: &str) -> Alignment {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Alignment::Left,
        "right" | "end" => Alignment::Right,
        _ => Alignment::Center,
    }
}

fn parse_card_color(value: &str) -> Option<Color> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || matches!(value.as_str(), "transparent" | "none" | "inherit") {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        let expanded;
        let hex = if hex.len() == 3 {
            expanded = hex.chars().flat_map(|ch| [ch, ch]).collect::<String>();
            expanded.as_str()
        } else {
            hex
        };
        if hex.len() >= 6 {
            let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(red, green, blue));
        }
    }
    if let Some(rgb) = value.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let channels: Vec<_> = rgb
            .split(',')
            .filter_map(|part| part.trim().parse::<u8>().ok())
            .collect();
        if channels.len() == 3 {
            return Some(Color::Rgb(channels[0], channels[1], channels[2]));
        }
    }
    match value.as_str() {
        "black" => Some(Color::Black),
        "white" => Some(Color::White),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "fuchsia" => Some(Color::Magenta),
        "cyan" | "aqua" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        _ => None,
    }
}

fn draw_image_placeholder(frame: &mut Frame, area: Rect, source: &str) {
    frame.render_widget(
        Paragraph::new(format!("[image: {source}]")).alignment(Alignment::Center),
        area,
    );
}

fn draw_audio_buttons(frame: &mut Frame, area: Rect, count: usize) {
    let mut spans = Vec::new();
    for index in 0..count {
        if index > 0 {
            spans.push(Span::raw("    "));
        }
        spans.push(Span::styled(
            NF_PLAY.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
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
    let text = if answer_shown && !card.answer_includes_question {
        format!("{}\n{}", card.front, card.back)
    } else if answer_shown {
        card.back.clone()
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

fn draw_actions(frame: &mut Frame, area: Rect, app: &App, card: &crate::models::StudyCard) {
    let has_audio = side_has_audio(card, app.answer_shown);
    if app.answer_shown {
        let ratings = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy];
        let intervals: Vec<(Rating, String)> = if card.answer_intervals.len() == 4 {
            ratings
                .into_iter()
                .zip(card.answer_intervals.iter().cloned())
                .collect()
        } else {
            app.scheduler
                .preview_intervals(&card.card)
                .into_iter()
                .collect()
        };
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
                    Rating::Hard => Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    Rating::Good => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    Rating::Easy => Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
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
        let mut lines = vec![Line::from(Span::styled(
            "Space  Show Answer",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center)];
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CardBlock, CardDocument, CardTextRun};
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn parses_template_colors() {
        assert_eq!(parse_card_color("#00aaaa"), Some(Color::Rgb(0, 170, 170)));
        assert_eq!(parse_card_color("#9CF"), Some(Color::Rgb(153, 204, 255)));
        assert_eq!(
            parse_card_color("rgb(255, 0, 255)"),
            Some(Color::Rgb(255, 0, 255))
        );
        assert_eq!(parse_card_color("transparent"), None);
    }

    #[test]
    fn renders_official_document_styles_and_audio_on_one_line() {
        let document = CardDocument {
            background: "black".into(),
            blocks: vec![
                CardBlock::Text {
                    runs: vec![CardTextRun {
                        text: "Meaning: To avoid something.".into(),
                        color: "#00aaaa".into(),
                        italic: true,
                        ..CardTextRun::default()
                    }],
                    alignment: "left".into(),
                    background: "black".into(),
                },
                CardBlock::Separator,
                CardBlock::Audio {
                    sources: vec!["one.mp3".into(), "two.mp3".into(), "three.mp3".into()],
                },
            ],
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 14)).unwrap();
        terminal
            .draw(|frame| {
                draw_card_preview(
                    frame,
                    frame.area(),
                    CardPreview {
                        front: "",
                        back: "",
                        front_document: Some(&document),
                        back_document: Some(&document),
                        answer_shown: true,
                        answer_includes_question: true,
                        title_prefix: None,
                    },
                    None,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Meaning: To avoid something."));
        assert_eq!(rendered.matches(NF_PLAY).count(), 3);

        let meaning_cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "M")
            .unwrap();
        assert_eq!(meaning_cell.fg, Color::Rgb(0, 170, 170));
        assert!(meaning_cell.modifier.contains(Modifier::ITALIC));
    }
}
