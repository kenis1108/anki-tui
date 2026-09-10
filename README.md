# anki-tui

Terminal Anki client written in Rust (ratatui) and backed by Anki's official scheduler.

## Features

- **Deck hierarchy and daily counts** from Anki's official `deck_due_tree()`
- **Study session** with Anki's exact queue, answer states, intervals, and revlog
- **Add / edit / delete** notes without flattening multi-field note types
- **Browse & search** cards; suspend / unsuspend
- **Deck options** — new cards/day & review cards/day
- **Stats** — collection overview and review activity
- **AnkiWeb collection and media sync** through Anki's official backend
- The shadow `collection.anki2` is authoritative in synced mode
- Study media: **audio via mpv**, **images via Kitty graphics protocol**

## Dependencies

### Build

- Rust toolchain (`cargo`)

### Runtime (media)

| Tool | Used for | Notes |
|------|----------|--------|
| **Python 3 + `anki` package** | Official collection, scheduler, and sync backend | Required for exact Anki mode. Keep its version aligned with Desktop; verify with `python3 -c 'from anki.collection import Collection'`. |
| **[mpv](https://mpv.io/)** | Audio playback (`[sound:…]`) | Required to hear card audio. Install e.g. `brew install mpv` |
| **Nerd Font** (e.g. JetBrainsMono NF) | Play icon `󰐊` on cards with audio | Optional; without it the glyph may look like a box |
| **[yazi](https://yazi-rs.github.io/)** | Pick images in Add/Edit (`Ctrl+o` / `Alt+o`) | Optional. Install yazi; without it, insert image shows an error. |
| **[Kitty](https://sw.kovidgoyal.net/kitty/)** (or a Kitty-protocol terminal) | Inline images (`<img src="…">`) | Requires [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/). WezTerm also works. Plain terminals show `[img]` placeholders only. |

Without mpv, study still works but audio is skipped. Without Kitty protocol, images show as `[img]` placeholders only.

## Run

```bash
cargo run --release
```

Prefer running inside **Kitty** (or WezTerm) so images render.

Data dir (all platforms): `~/.local/share/anki-tui/`  
(or `$XDG_DATA_HOME/anki-tui` if set)

- `anki/collection.anki2` — authoritative collection in official mode
- `anki/collection.media/` — media files
- `anki/collection.media.db2` — official media index
- `collection.db` — standalone `rs-fsrs` fallback, used only before official mode is initialized

Once official mode is initialized, the app refuses to fall back to `collection.db` if the Python `anki` package is missing. Showing or modifying a different collection would silently diverge from Desktop.

## Keys

Press `?` inside the app for the full help screen.

| Screen | Keys |
|--------|------|
| Decks | `Enter` study · `a` add · `b` browse · `y` sync · `s` stats · `o` options · `n`/`r`/`d` deck CRUD · `q` quit |
| Study | `Space` show · `1`–`4` rate · `m` replay audio · `Esc` back |
| Add/Edit | `Tab` fields · Edit: `Enter` newline · `Ctrl+o`/`Alt+o` image (yazi) · `Ctrl+s` save · `Shift+D` delete · Add: `Enter` save · `Esc` cancel |
| Browse | `/` search · `Enter` edit · `s` suspend |

## AnkiWeb sync

Press `y` on the deck browser.

| Key | Action |
|-----|--------|
| F1 | Official Anki sync login, then sync an initialized collection |
| **F5** | **Normal bidirectional sync** (collection + media) |
| **F6** | Media only |
| F2 | Full download — replace the local collection, then sync media |
| F3 | Full upload — replace the AnkiWeb collection, then sync media |
| F4 | Logout |

Anki currently provides username/password sync authentication, not a browser/OAuth handoff. The password is sent to the official login endpoint; only the returned host key is stored locally.

### Safe migration from an older anki-tui

Older builds could flatten every synced note to two fields. Do **not** press F3 with an old shadow collection.

1. Open Anki Desktop and confirm its notes and templates are still correct.
2. In Anki Desktop, force a one-way **upload to AnkiWeb** so the known-good Desktop collection becomes authoritative.
3. Start this build, press `y`, then use `F1` to sign in.
4. Press `F2` to replace the old TUI shadow with a fresh official download.

Only a successfully opened full download creates `anki/.official-backend-v1`. Until that marker exists, automatic sync and F3 are blocked.
When an unmarked legacy shadow is found, the app still opens on the Decks screen, but blocks data operations until you press `y` and complete a fresh F2 download. This prevents the unrelated fallback collection from being modified by mistake.

### How it works

- A shadow `collection.anki2` is stored under the app data dir (`…/anki-tui/anki/`) and opened directly by Anki's official backend.
- Deck operations, browsing, note edits, scheduling, and review history all modify that same collection; there is no lossy import/export step.
- When signed in, an initialized official collection syncs automatically at startup and before a normal exit.
- **F5** calls the same normal collection sync API as Anki Desktop, followed by official media sync.
- Sync actions run in the background so progress and failures remain visible in the TUI.
- If Anki requests a one-way sync, F5 asks you to choose F2/F3 instead of guessing a direction.
- Media files: `…/anki-tui/anki/collection.media/` with `collection.media.db2`.

### Display limits

Scheduling, stored data, and expanded card templates use the official backend. The study view preserves template order, true-color text/backgrounds, alignment, bold/italic/underline/strike styling, separators, multiple images, and grouped audio buttons. A terminal still cannot reproduce browser-only details such as exact fonts and pixel sizes, rounded corners, CSS animations, MathJax, or arbitrary card JavaScript.
