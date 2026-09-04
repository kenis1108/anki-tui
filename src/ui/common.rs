use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use tui_input::{Input, InputRequest};
use unicode_width::UnicodeWidthChar;

pub fn input_from_key(input: &mut Input, key: KeyEvent) -> bool {
    let req = match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(InputRequest::InsertChar(c))
        }
        KeyCode::Backspace => Some(InputRequest::DeletePrevChar),
        KeyCode::Delete => Some(InputRequest::DeleteNextChar),
        KeyCode::Left => Some(InputRequest::GoToPrevChar),
        KeyCode::Right => Some(InputRequest::GoToNextChar),
        KeyCode::Home => Some(InputRequest::GoToStart),
        KeyCode::End => Some(InputRequest::GoToEnd),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(InputRequest::DeleteFromStart)
        }
        _ => None,
    };
    if let Some(req) = req {
        let _ = input.handle(req);
        true
    } else {
        false
    }
}

/// Draw a bordered text field and place the terminal cursor at the real
/// `tui_input` caret (not glued to the end of the string).
pub fn draw_input_field(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
) {
    draw_input_field_ex(frame, area, title, input, focused, false);
}

pub fn draw_input_field_secret(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
) {
    draw_input_field_ex(frame, area, title, input, focused, true);
}

fn draw_input_field_ex(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
    secret: bool,
) {
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let inner_width = area.width.saturating_sub(2) as usize;
    let scroll = input.visual_scroll(inner_width.max(1));
    let shown = if secret {
        "*".repeat(input.value().chars().count())
    } else {
        input.value().to_string()
    };
    frame.render_widget(
        Paragraph::new(shown)
            .scroll((0, scroll as u16))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(style),
            ),
        area,
    );
    if focused && area.width > 2 && area.height > 2 {
        let x_off = input.visual_cursor().saturating_sub(scroll) as u16;
        let x = area.x + 1 + x_off.min(area.width.saturating_sub(3));
        let y = area.y + 1;
        frame.set_cursor_position((x, y));
    }
}

/// Multiline editor: `\n` becomes a real new row (not a zero-width gap).
pub fn draw_multiline_input_field(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    input: &Input,
    focused: bool,
) {
    let style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;
    let value = input.value();
    let (cur_row, cur_col) = cursor_row_col(value, input.cursor());

    // Keep cursor row visible
    let vscroll = cur_row.saturating_sub(inner_h.saturating_sub(1));
    // Horizontal scroll within the cursor's line
    let hscroll = cur_col.saturating_sub(inner_w.saturating_sub(1));

    // `split('\n')` keeps a trailing empty line after Enter; `lines()` would drop it.
    let all_lines: Vec<&str> = if value.is_empty() {
        vec![""]
    } else {
        value.split('\n').collect()
    };

    let view: Vec<Line> = all_lines
        .iter()
        .skip(vscroll)
        .take(inner_h.max(1))
        .map(|line| {
            let shown = slice_by_display_cols(line, hscroll, inner_w);
            Line::from(shown)
        })
        .collect();

    frame.render_widget(
        Paragraph::new(view).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(style),
        ),
        area,
    );

    if focused && area.width > 2 && area.height > 2 {
        let x_off = cur_col.saturating_sub(hscroll) as u16;
        let y_off = cur_row.saturating_sub(vscroll) as u16;
        let x = area.x + 1 + x_off.min(area.width.saturating_sub(3));
        let y = area.y + 1 + y_off.min(area.height.saturating_sub(3));
        frame.set_cursor_position((x, y));
    }
}

fn cursor_row_col(value: &str, cursor: usize) -> (usize, usize) {
    cursor_row_col_for_edit(value, cursor)
}

pub(crate) fn cursor_row_col_for_edit(value: &str, cursor: usize) -> (usize, usize) {
    let mut row = 0usize;
    let mut col = 0usize;
    for (i, ch) in value.chars().enumerate() {
        if i == cursor {
            return (row, col);
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    (row, col)
}

fn slice_by_display_cols(s: &str, start_col: usize, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut col = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w <= start_col {
            col += w;
            continue;
        }
        let visible_at = col.saturating_sub(start_col);
        if visible_at >= max_cols || visible_at + w > max_cols {
            break;
        }
        out.push(ch);
        col += w;
    }
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn help_text() -> String {
    [
        "Global",
        "  ?             help",
        "  Ctrl-C        quit",
        "",
        "Decks",
        "  ↑/↓ j/k       select deck",
        "  Enter / Space  study selected deck",
        "  a             add note",
        "  b             browse cards",
        "  s             statistics",
        "  y             AnkiWeb sync",
        "  o             deck options",
        "  n             new deck",
        "  r             rename deck",
        "  d             delete deck",
        "  f             reset/forget deck (→ New)",
        "  q             quit",
        "",
        "Study",
        "  Space         show answer",
        "  1/2/3/4       Again / Hard / Good / Easy",
        "  e             edit current card",
        "  f             reset/forget current card (→ New)",
        "  m             replay audio (mpv) · 󰐊 when card has sound",
        "  Esc           back to decks",
        "",
        "Add / Edit",
        "  Tab           next field",
        "  Enter         newline (edit)",
        "  Ctrl+i/Alt+i  insert image via yazi (Front/Back)",
        "  Ctrl+s        save note (edit)",
        "  Enter         save (add)",
        "  Esc           cancel",
        "",
        "Browse",
        "  /             focus search",
        "  Enter         edit selected",
        "  s             suspend / unsuspend",
        "  f             reset/forget card (→ New)",
        "  Esc           back",
        "",
        "Edit",
        "  Shift+D       delete card",
        "  Ctrl+s        save",
        "",
        "AnkiWeb Sync",
        "  F1            login",
        "  F5            incremental sync + media",
        "  F6            media only",
        "  F2            full download (replace local)",
        "  F3            full upload (replace AnkiWeb)",
        "  F4            logout",
    ]
    .join("\n")
}
