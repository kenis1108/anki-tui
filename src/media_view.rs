//! Study-time media: mpv for audio, ratatui-image (Kitty/Sixel/halfblocks) for images.

use anyhow::{bail, Context, Result};
use image::ImageReader;
use ratatui::Frame;
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
    Resize, StatefulImage,
};
use regex::Regex;
use std::io::Cursor;
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

/// Ordered pieces of a card field — preserves `<img>` position relative to text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowItem {
    Text(String),
    Image(String),
}

/// Split `raw` into text/image items in document order (img tags become Image items).
pub fn parse_flow(raw: &str) -> Vec<FlowItem> {
    let img_re = img_re();
    let mut items = Vec::new();
    let mut last = 0usize;
    for cap in img_re.captures_iter(raw) {
        let full = cap.get(0).expect("img match");
        let fname = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        if full.start() > last {
            push_flow_text(
                &mut items,
                clean_text_segment(&raw[last..full.start()]),
                true,
            );
        }
        if !fname.is_empty() {
            items.push(FlowItem::Image(fname));
        }
        last = full.end();
    }
    if last < raw.len() {
        // After an <img>, keep blank lines that separate it from following text.
        push_flow_text(&mut items, clean_text_segment(&raw[last..]), false);
    }
    items
}

/// `trim_all`: true for text before an image (drop surrounding blank lines).
/// false for text after an image — keep blank lines that visually separate.
fn push_flow_text(items: &mut Vec<FlowItem>, chunk: String, trim_all: bool) {
    let chunk = if trim_all {
        chunk.trim().to_string()
    } else {
        // The char right after `>` is usually a newline ending the img line —
        // drop only that one; keep further blank lines as spacing under the image.
        let rest = chunk.strip_prefix('\n').unwrap_or(chunk.as_str());
        rest.trim_start_matches([' ', '\t'])
            .trim_end_matches([' ', '\t', '\r'])
            .to_string()
    };
    if chunk.is_empty() {
        return;
    }
    // Keep pure blank-line spacers (e.g. only `\n` left after the strip above).
    if trim_all || !chunk.trim().is_empty() || chunk.contains('\n') {
        items.push(FlowItem::Text(chunk));
    }
}

fn clean_text_segment(raw: &str) -> String {
    let sound_re = sound_re();
    let mut display = sound_re
        .replace_all(raw, format!(" {NF_PLAY} "))
        .into_owned();
    static STYLE: OnceLock<Regex> = OnceLock::new();
    let style_re = STYLE.get_or_init(|| {
        Regex::new(r"(?is)<(?:style|script)\b[^>]*>.*?</(?:style|script)>").expect("style regex")
    });
    display = style_re.replace_all(&display, "").into_owned();
    // Keep structural line breaks before stripping the remaining HTML tags.
    static BREAK: OnceLock<Regex> = OnceLock::new();
    let break_re = BREAK.get_or_init(|| {
        Regex::new(r"(?i)<(?:br|hr)\s*/?>|</(?:div|p|li|tr|h[1-6])\s*>").expect("break regex")
    });
    display = break_re.replace_all(&display, "\n").into_owned();
    let html_re = html_tag_re();
    display = html_re.replace_all(&display, "").into_owned();
    display
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&rarr;", "→")
}

pub fn parse_card_text(raw: &str) -> ParsedCardText {
    let flow = parse_flow(raw);
    let mut media = Vec::new();
    // Also collect sounds from original (parse_flow only keeps images as items).
    for cap in sound_re().captures_iter(raw) {
        let fname = cap.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
        if !fname.is_empty() {
            media.push(MediaRef::Sound(fname));
        }
    }

    for item in flow {
        match item {
            FlowItem::Text(_) => {}
            FlowItem::Image(f) => {
                media.push(MediaRef::Image(f));
            }
        }
    }

    ParsedCardText { media }
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
    RE.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("html regex"))
}

pub fn media_path(fname: &str) -> Result<PathBuf> {
    let folder = paths::media_folder()?;
    let name = Path::new(fname)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| fname.to_string());
    let path = folder.join(&name);
    if !path.exists() {
        bail!("media file missing: {name} under {}", folder.display());
    }
    Ok(path)
}

pub fn kitty_supported() -> bool {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    if std::env::var_os("KITTY_PID").is_some() {
        return true;
    }
    if std::env::var_os("WEZTERM_EXECUTABLE").is_some() {
        return true;
    }
    if std::env::var("TERM")
        .map(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("kitty") || t.contains("wezterm")
        })
        .unwrap_or(false)
    {
        return true;
    }
    std::env::var("TERM_PROGRAM")
        .map(|t| {
            let t = t.to_ascii_lowercase();
            t.contains("kitty") || t.contains("wezterm")
        })
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
    picker: Option<Picker>,
    images: Vec<(String, StatefulProtocol)>,
    last_key: Option<(i64, bool)>,
    last_image_sources: Vec<String>,
    pub last_image_status: Option<String>,
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
            picker: None,
            images: Vec::new(),
            last_key: None,
            last_image_sources: Vec::new(),
            last_image_status: None,
        }
    }

    /// Detect terminal graphics capability. Call once after `ratatui::init()`.
    pub fn init_picker(&mut self) {
        if self.picker.is_some() {
            return;
        }
        let picker = match Picker::from_query_stdio() {
            Ok(p) => p,
            Err(_) => {
                // Fallback when capability query fails (e.g. nested TERM).
                let mut p = Picker::halfblocks();
                if kitty_supported() {
                    p.set_protocol_type(ProtocolType::Kitty);
                }
                p
            }
        };
        self.picker = Some(picker);
    }

    /// Drop detected picker (e.g. after TUI suspend/restore).
    pub fn picker_reset(&mut self) {
        self.picker = None;
        self.clear_images();
        self.last_key = None;
    }

    pub fn has_ready_image_for(&self, source: &str) -> bool {
        self.images.iter().any(|(name, _)| name == source)
    }

    pub fn stop_audio(&mut self) {
        if let Some(mut child) = self.player.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn clear_images(&mut self) {
        self.images.clear();
        self.last_image_sources.clear();
    }

    pub fn reset(&mut self) {
        self.stop_audio();
        self.clear_images();
        self.last_key = None;
        self.last_image_status = None;
    }

    /// Load / refresh image + autoplay audio when the visible card side changes.
    pub fn prepare_card(
        &mut self,
        card_id: i64,
        answer_shown: bool,
        answer_includes_question: bool,
        front: &str,
        back: &str,
    ) -> Result<()> {
        self.init_picker();

        let key = (card_id, answer_shown);
        let text = if answer_shown && !answer_includes_question {
            format!("{front}\n{back}")
        } else if answer_shown {
            back.to_string()
        } else {
            front.to_string()
        };
        let parsed = parse_card_text(&text);
        let image_sources: Vec<String> = parsed
            .media
            .iter()
            .filter_map(|media| match media {
                MediaRef::Image(source) => Some(source.clone()),
                MediaRef::Sound(_) => None,
            })
            .collect();

        let side_changed = self.last_key != Some(key);
        self.last_key = Some(key);

        if side_changed {
            self.stop_audio();
            if let Some(MediaRef::Sound(fname)) = parsed
                .media
                .iter()
                .find(|m| matches!(m, MediaRef::Sound(_)))
            {
                if let Ok(path) = media_path(fname) {
                    let _ = self.play_mpv(&path);
                }
            }
        }

        if image_sources == self.last_image_sources {
            return Ok(());
        }

        self.images.clear();
        self.last_image_sources = image_sources.clone();
        self.last_image_status = None;
        for source in image_sources {
            match self.load_image(&source) {
                Ok(protocol) => self.images.push((source, protocol)),
                Err(error) => {
                    if self.last_image_status.is_none() {
                        self.last_image_status = Some(format!("img: {error:#}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn load_image(&self, fname: &str) -> Result<StatefulProtocol> {
        let Some(picker) = self.picker.as_ref() else {
            bail!("picker not ready");
        };
        let path = media_path(fname)?;
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let dyn_img = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .context("guess image format")
            .and_then(|reader| reader.decode().context("decode image"))?;
        Ok(picker.new_resize_protocol(dyn_img))
    }

    pub fn image_cell_size_for(
        &self,
        source: &str,
        max: ratatui::layout::Size,
    ) -> Option<ratatui::layout::Size> {
        let (_, proto) = self.images.iter().find(|(name, _)| name == source)?;
        self.protocol_cell_size(proto, max)
    }

    fn protocol_cell_size(
        &self,
        proto: &StatefulProtocol,
        max: ratatui::layout::Size,
    ) -> Option<ratatui::layout::Size> {
        // Cap so Fit does not upscale into a giant empty placement.
        let capped = ratatui::layout::Size::new(max.width.clamp(4, 48), max.height.clamp(4, 18));
        Some(proto.size_for(Resize::Fit(None), capped))
    }

    pub fn render_image_for(
        &mut self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        source: &str,
    ) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        let needed = self
            .image_cell_size_for(source, ratatui::layout::Size::new(area.width, area.height))
            .unwrap_or(ratatui::layout::Size::new(area.width, area.height));
        let w = needed.width.min(area.width).max(1);
        let h = needed.height.min(area.height).max(1);
        let centered = ratatui::layout::Rect {
            x: area.x + (area.width.saturating_sub(w)) / 2,
            y: area.y + (area.height.saturating_sub(h)) / 2,
            width: w,
            height: h,
        };
        let Some((_, proto)) = self.images.iter_mut().find(|(name, _)| name == source) else {
            return;
        };
        let widget = StatefulImage::new().resize(Resize::Fit(None));
        frame.render_stateful_widget(widget, centered, proto);
    }

    pub fn replay_sounds(
        &mut self,
        front: &str,
        back: &str,
        answer_shown: bool,
        answer_includes_question: bool,
    ) -> Result<String> {
        if !mpv_available() {
            bail!("mpv not found — install mpv to play audio");
        }
        let text = if answer_shown && !answer_includes_question {
            format!("{front}\n{back}")
        } else if answer_shown {
            back.to_string()
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
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        self.stop_audio();
        self.clear_images();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sound_and_img() {
        let p = parse_card_text(r#"Hello [sound:foo.mp3]<br><img src="bar.png" alt="x"> world"#);
        assert!(p.media.contains(&MediaRef::Sound("foo.mp3".into())));
        assert!(p.media.contains(&MediaRef::Image("bar.png".into())));
        assert!(p.has_sound());
        assert_eq!(p.sound_count(), 1);
    }

    #[test]
    fn flow_keeps_img_before_following_text() {
        let flow = parse_flow("<img src=\"apple.jpeg\">\n苹果");
        assert_eq!(
            flow,
            vec![
                FlowItem::Image("apple.jpeg".into()),
                FlowItem::Text("苹果".into()),
            ]
        );
    }

    #[test]
    fn flow_keeps_blank_line_under_img() {
        // Field: <img…>\n\n苹果  → one blank line under the image
        let flow = parse_flow("<img src=\"apple.jpeg\">\n\n苹果");
        assert_eq!(
            flow,
            vec![
                FlowItem::Image("apple.jpeg".into()),
                FlowItem::Text("\n苹果".into()),
            ]
        );
    }
}
