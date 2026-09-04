# anki-tui

Terminal Anki-like spaced repetition app written in Rust (ratatui + FSRS).

## Features

- **Deck browser** with New / Learning / Review counts
- **Study session** — reveal answer, rate Again / Hard / Good / Easy
- **FSRS scheduling** via [`rs-fsrs`](https://crates.io/crates/rs-fsrs) (same family Anki uses)
- **Add / edit / delete** notes
- **Browse & search** cards; suspend / unsuspend
- **Deck options** — new cards/day & review cards/day
- **Stats** — collection overview and review activity
- **AnkiWeb sync** (incremental + media)
- Local **SQLite** collection (no Anki desktop required)
- Study media: **audio via mpv**, **images via Kitty graphics protocol**

## Dependencies

### Build

- Rust toolchain (`cargo`)

### Runtime (media)

| Tool | Used for | Notes |
|------|----------|--------|
| **[mpv](https://mpv.io/)** | Audio playback (`[sound:…]`) | Required to hear card audio. Install e.g. `brew install mpv` |
| **Nerd Font** (e.g. JetBrainsMono NF) | Play icon `󰐊` on cards with audio | Optional; without it the glyph may look like a box |
| **[yazi](https://yazi-rs.github.io/)** | Pick images in Add/Edit (`Ctrl+i` / `Alt+i`) | Optional. Install yazi; without it, insert image shows an error. |
| **[Kitty](https://sw.kovidgoyal.net/kitty/)** (or a Kitty-protocol terminal) | Inline images (`<img src="…">`) | Requires [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/). WezTerm also works. Plain terminals show `[img]` placeholders only. |

Without mpv, study still works but audio is skipped. Without Kitty protocol, images show as `[img]` placeholders only.

## Run

```bash
cargo run --release
```

Prefer running inside **Kitty** (or WezTerm) so images render.

Data (macOS): `~/Library/Application Support/anki-tui/`
- `collection.db` — TUI database
- `anki/collection.media/` — media files
- `anki/collection.anki2` — AnkiWeb shadow collection

## Keys

Press `?` inside the app for the full help screen.

| Screen | Keys |
|--------|------|
| Decks | `Enter` study · `a` add · `b` browse · `y` sync · `s` stats · `o` options · `n`/`r`/`d` deck CRUD · `q` quit |
| Study | `Space` show · `1`–`4` rate · `m` replay audio · `Esc` back |
| Add/Edit | `Tab` fields · Edit: `Enter` newline · `Ctrl+i`/`Alt+i` image (yazi) · `Ctrl+s` save · `Shift+D` delete · Add: `Enter` save · `Esc` cancel |
| Browse | `/` search · `Enter` edit · `s` suspend |

## AnkiWeb sync

Press `y` on the deck browser.

| Key | Action |
|-----|--------|
| F1 | Login |
| **F5** | **Incremental sync** (collection delta + media) |
| **F6** | Media only |
| F2 | Full download — replace local + media |
| F3 | Full upload — replace AnkiWeb + media |
| F4 | Logout |

### How it works

- A shadow `collection.anki2` is stored under the app data dir (`…/anki-tui/anki/`).
- **F5** runs Anki’s delta protocol against that shadow, re-imports into the TUI DB, then syncs media.
- If schemas diverge or sanity fails, F5 asks you to use F2/F3.
- Media files: `…/anki-tui/anki/collection.media/` with `collection.media.db2`.

### Limits

- Notes still map to Basic Front/Back in the TUI.
- Local FSRS scheduling may be reset when a delta re-imports cards.
- Notetype/deck structural changes on schema-18 may force a full sync.
