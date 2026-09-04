//! Study-time media: mpv for audio, Kitty graphics protocol for images.

use anyhow::{bail, Context, Result};
use base64::Engine;
use regex::Regex;
use std::io::{stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use crate::sync::paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaRef {
    Sound(String),
    Image(String),
}

#[derive(Debug, Clone)]
pub struct ParsedCardText {
    /// Text with media tags replaced by short placeholders.
    pub display: String,
    pub media: Vec<MediaRef>,
}

impl ParsedCardText {
    pub fn sound_count(&self) -> usize {
        self.media
            .iter()
            .filter(|m| matches!(m, MediaRef::Sound(_)))
            .count()
    }

    pub fn has_sound(&self) -> bool {
        self.sound_count() > 0
    }
}

/// Nerd Font md-play (U+F040A). Requires a Nerd Font in the terminal.
pub const NF_PLAY: &str = "󰐊";

pub fn parse_card_text(raw: &str) -> ParsedCardText {
    let sound_re = sound_re();
    let img_re = img_re();

    let mut media = Vec::new();
    let mut display = raw.to_string();

    for cap in sound_re.captures_iter(raw) {
        let fname = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        if !fname.is_empty() {
            media.push(MediaRef::Sound(fname));
        }
    }
    // Inline play button (Nerd Font) where [sound:…] was
    display = sound_re
        .replace_all(&display, format!(" {NF_PLAY} "))
        .into_owned();

    for cap in img_re.captures_iter(raw) {
        let fname = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        if !fname.is_empty() {
            media.push(MediaRef::Image(fname));
        }
    }
    display = img_re.replace_all(&display, "[img]").into_owned();

    // Collapse leftover HTML tags lightly for terminal readability
    let html_re = html_tag_re();
    display = html_re.replace_all(&display, "").into_owned();
    display = display.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");

    ParsedCardText { display, media }
}

fn sound_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\[sound:([^\]]+)\]").expect("sound regex"))
}

fn img_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<img[^>]+src\s*=\s*["']([^"']+)["'][^>]*>"#).expect("img regex")
    })
}

fn html_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)</?(br|div|span|b|i|u|p|anki)[^>]*>").expect("html regex"))
}

pub fn media_path(fname: &str) -> Result<PathBuf> {
    let folder = paths::media_folder()?;
    // Anki media names are flat; reject path traversal
    let name = Path::new(fname)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| fname.to_string());
    let path = folder.join(&name);
    if !path.exists() {
        bail!("media file missing: {name} (sync media with F6)");
    }
    Ok(path)
}

pub fn kitty_supported() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM")
            .map(|t| t.contains("kitty") || t.contains("xterm-kitty"))
            .unwrap_or(false)
        || std::env::var("TERM_PROGRAM")
            .map(|t| t.eq_ignore_ascii_case("WezTerm") || t.eq_ignore_ascii_case("kitty"))
            .unwrap_or(false)
}

pub fn mpv_available() -> bool {
    Command::new("mpv")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Active media playback / display session for the study screen.
pub struct MediaSession {
    player: Option<Child>,
    shown_image_ids: Vec<u32>,
    last_key: Option<(i64, bool)>, // (card_id, answer_shown)
}

impl Default for MediaSession {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaSession {
    pub fn new() -> Self {
        Self {
            player: None,
            shown_image_ids: Vec::new(),
            last_key: None,
        }
    }

    pub fn stop_audio(&mut self) {
        if let Some(mut child) = self.player.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn clear_images(&mut self) {
        if !self.shown_image_ids.is_empty() || kitty_supported() {
            // Delete all Kitty images placed by this process
            let _ = write!(stdout(), "\x1b_Ga=d\x1b\\");
            let _ = stdout().flush();
        }
        self.shown_image_ids.clear();
    }

    pub fn reset(&mut self) {
        self.stop_audio();
        self.clear_images();
        self.last_key = None;
    }

    /// Autoplay sounds / show images when the visible card side changes.
    pub fn on_card_side(
        &mut self,
        card_id: i64,
        answer_shown: bool,
        front: &str,
        back: &str,
        image_area: ratatui::layout::Rect,
    ) -> Result<()> {
        let key = (card_id, answer_shown);
        if self.last_key == Some(key) {
            // Still re-place images each frame (ratatui redraw clears them)
            self.place_images(front, back, answer_shown, image_area)?;
            return Ok(());
        }
        self.last_key = Some(key);
        self.stop_audio();
        self.clear_images();

        let text = if answer_shown {
            format!("{front}\n{back}")
        } else {
            front.to_string()
        };
        let parsed = parse_card_text(&text);

        // Play first sound (Anki-style); user can press `m` to replay all
        if let Some(MediaRef::Sound(fname)) = parsed.media.iter().find(|m| matches!(m, MediaRef::Sound(_)))
        {
            match media_path(fname) {
                Ok(path) => {
                    if let Err(e) = self.play_mpv(&path) {
                        // non-fatal
                        let _ = e;
                    }
                }
                Err(_) => {}
            }
        }

        self.place_images(front, back, answer_shown, image_area)?;
        Ok(())
    }

    pub fn replay_sounds(&mut self, front: &str, back: &str, answer_shown: bool) -> Result<String> {
        if !mpv_available() {
            bail!("mpv not found — install mpv to play audio");
        }
        let text = if answer_shown {
            format!("{front}\n{back}")
        } else {
            front.to_string()
        };
        let parsed = parse_card_text(&text);
        let sounds: Vec<_> = parsed
            .media
            .iter()
            .filter_map(|m| match m {
                MediaRef::Sound(f) => Some(f.as_str()),
                _ => None,
            })
            .collect();
        if sounds.is_empty() {
            return Ok("No audio on this side".into());
        }
        // Play all sequentially via one mpv playlist
        let paths: Result<Vec<_>, _> = sounds.iter().map(|f| media_path(f)).collect();
        let paths = paths?;
        self.stop_audio();
        self.play_mpv_playlist(&paths)?;
        Ok(format!("Playing {} file(s) via mpv", paths.len()))
    }

    fn play_mpv(&mut self, path: &Path) -> Result<()> {
        if !mpv_available() {
            bail!("mpv not found");
        }
        self.stop_audio();
        let child = Command::new("mpv")
            .args([
                "--no-terminal",
                "--force-window=no",
                "--audio-display=no",
                "--keep-open=no",
            ])
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn mpv")?;
        self.player = Some(child);
        Ok(())
    }

    fn play_mpv_playlist(&mut self, paths: &[PathBuf]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.stop_audio();
        let mut cmd = Command::new("mpv");
        cmd.args([
            "--no-terminal",
            "--force-window=no",
            "--audio-display=no",
            "--keep-open=no",
        ]);
        for p in paths {
            cmd.arg(p);
        }
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn mpv playlist")?;
        self.player = Some(child);
        Ok(())
    }

    fn place_images(
        &mut self,
        front: &str,
        back: &str,
        answer_shown: bool,
        area: ratatui::layout::Rect,
    ) -> Result<()> {
        if area.width < 4 || area.height < 2 {
            return Ok(());
        }
        if !kitty_supported() {
            return Ok(());
        }
        let text = if answer_shown {
            format!("{front}\n{back}")
        } else {
            front.to_string()
        };
        let parsed = parse_card_text(&text);
        let images: Vec<_> = parsed
            .media
            .iter()
            .filter_map(|m| match m {
                MediaRef::Image(f) => Some(f.as_str()),
                _ => None,
            })
            .collect();
        if images.is_empty() {
            return Ok(());
        }

        // Clear previous placements then draw first image into the content area
        let _ = write!(stdout(), "\x1b_Ga=d\x1b\\");
        self.shown_image_ids.clear();

        let fname = images[0];
        let Ok(path) = media_path(fname) else {
            return Ok(());
        };

        let id = 42u32; // stable id for study image
        let cols = area.width.saturating_sub(2).max(1);
        let rows = area.height.saturating_sub(1).max(1);
        // 1-based cursor position: leave one row for the "Front/Back" label
        let row = area.y.saturating_add(2) + 1;
        let col = area.x.saturating_add(1) + 1;

        let engine = base64::engine::general_purpose::STANDARD;
        let path_b64 = engine.encode(path.to_string_lossy().as_bytes());

        let mut out = stdout();
        // Save cursor, move, transmit+display file via Kitty, restore
        write!(
            out,
            "\x1b[s\x1b[{row};{col}H\x1b_Ga=T,f=100,t=f,i={id},c={cols},r={rows},q=2;{path_b64}\x1b\\\x1b[u"
        )?;
        out.flush()?;
        self.shown_image_ids.push(id);
        Ok(())
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sound_and_img() {
        let p = parse_card_text(
            r#"Hello [sound:foo.mp3]<br><img src="bar.png" alt="x"> world"#,
        );
        assert!(p.display.contains(NF_PLAY));
        assert!(p.display.contains("[img]"));
        assert!(p.media.contains(&MediaRef::Sound("foo.mp3".into())));
        assert!(p.media.contains(&MediaRef::Image("bar.png".into())));
        assert!(p.has_sound());
        assert_eq!(p.sound_count(), 1);
    }
}
