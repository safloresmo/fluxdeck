use crate::config::{Dial, Key, Theme};
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::{Rgba, RgbaImage, imageops};
use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;

/// Draws keys and dial segments. Shared by the daemon and the editor preview.
pub struct Painter {
    /// First font is the text font; the rest are fallbacks for icons and missing glyphs.
    fonts: Vec<FontVec>,
    images: RefCell<HashMap<String, Option<RgbaImage>>>,
}

const FALLBACKS: [&str; 3] = ["Symbols Nerd Font Mono", "Symbols Nerd Font", "JetBrainsMono Nerd Font"];

/// Font file fontconfig picks for `family`.
pub fn font_file(family: &str) -> Option<String> {
    let out = Command::new("fc-match").args(["-f", "%{file}", family]).output().ok()?;
    let file = String::from_utf8(out.stdout).ok()?;
    (!file.is_empty()).then_some(file)
}

/// Path of an installed Nerd Font, used for icons.
pub fn nerd_font_file() -> Option<String> {
    FALLBACKS.iter().filter_map(|f| font_file(f)).find(|file| file.contains("Nerd"))
}

fn load_font(file: &str) -> Option<FontVec> {
    FontVec::try_from_vec_and_index(std::fs::read(file).ok()?, 0).ok()
}

pub fn parse_color(s: &str) -> Option<Rgba<u8>> {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, 255])),
        8 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
        _ => None,
    }
}

fn color(s: &str, fallback: Rgba<u8>) -> Rgba<u8> {
    parse_color(s).unwrap_or(fallback)
}

/// "42%", "42" or "0.42" style output as 0..=1.
pub fn level(status: &str) -> Option<f32> {
    let v: f32 = status.trim().trim_end_matches('%').trim().parse().ok()?;
    let v = if status.contains('%') || v > 1.0 { v / 100.0 } else { v };
    Some(v.clamp(0.0, 1.0))
}

fn mix_rgba(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgba([m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), 255])
}

/// A bar with round ends.
fn round_bar(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: Rgba<u8>) {
    let r = h as f32 / 2.0;
    for py in y..y + h {
        for px in x..x + w {
            let dx = if (px as f32) < x as f32 + r { x as f32 + r - px as f32 - 0.5 } else if (px as f32) > (x + w) as f32 - r { px as f32 + 0.5 - ((x + w) as f32 - r) } else { 0.0 };
            let dy = py as f32 + 0.5 - (y as f32 + r);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r && px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                blend(img.get_pixel_mut(px as u32, py as u32), c, (r - d).clamp(0.0, 1.0));
            }
        }
    }
}

fn luma(c: Rgba<u8>) -> f32 {
    (0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32) / 255.0
}

/// The text colour, or black or white when it would barely show on `bg` (a key's own colour, a status colour).
fn readable(fg: Rgba<u8>, bg: Rgba<u8>) -> Rgba<u8> {
    if (luma(fg) - luma(bg)).abs() >= 0.4 {
        fg
    } else if luma(bg) > 0.55 {
        Rgba([20, 20, 24, 255])
    } else {
        Rgba([250, 250, 252, 255])
    }
}

fn blend(px: &mut Rgba<u8>, c: Rgba<u8>, cov: f32) {
    let a = cov.clamp(0.0, 1.0) * c[3] as f32 / 255.0;
    for i in 0..3 {
        px[i] = (c[i] as f32 * a + px[i] as f32 * (1.0 - a)).round() as u8;
    }
}

fn fill(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: Rgba<u8>) {
    for py in y.max(0)..(y + h).min(img.height() as i32) {
        for px in x.max(0)..(x + w).min(img.width() as i32) {
            blend(img.get_pixel_mut(px as u32, py as u32), c, 1.0);
        }
    }
}

impl Painter {
    pub fn new(family: &str) -> Painter {
        let mut files: Vec<String> = Vec::new();
        for f in std::iter::once(family).chain(FALLBACKS) {
            if let Some(file) = font_file(f).filter(|f| !files.contains(f)) {
                files.push(file);
            }
        }
        let fonts: Vec<FontVec> = files.iter().filter_map(|f| load_font(f)).collect();
        if fonts.is_empty() {
            eprintln!("fluxdeck: no font found through fc-match, text will not be drawn");
        }
        Painter { fonts, images: RefCell::default() }
    }

    fn font_for(&self, c: char) -> Option<&FontVec> {
        self.fonts.iter().find(|f| f.glyph_id(c).0 != 0).or(self.fonts.first())
    }

    fn width(&self, text: &str, px: f32) -> f32 {
        text.chars()
            .filter_map(|c| Some(self.font_for(c)?.as_scaled(PxScale::from(px)).h_advance(self.font_for(c)?.glyph_id(c))))
            .sum()
    }

    /// Shrinks then truncates `text` until it fits in `max_w`.
    fn fit(&self, text: &str, px: f32, max_w: f32) -> (String, f32) {
        let mut px = px;
        let min = (px * 0.6).max(10.0);
        while px > min && self.width(text, px) > max_w {
            px *= 0.92;
        }
        let mut chars: Vec<char> = text.chars().collect();
        let mut s: String = text.into();
        while self.width(&s, px) > max_w && chars.len() > 1 {
            chars.pop();
            s = chars.iter().collect::<String>() + "…";
        }
        (s, px)
    }

    /// Draws one line centred on (cx, cy).
    fn text(&self, img: &mut RgbaImage, text: &str, px: f32, cx: f32, cy: f32, max_w: f32, c: Rgba<u8>) {
        let Some(main) = self.fonts.first() else { return };
        let (text, px) = self.fit(text, px, max_w);
        let scale = PxScale::from(px);
        let sm = main.as_scaled(scale);
        let baseline = cy + (sm.ascent() + sm.descent()) / 2.0;
        let mut x = cx - self.width(&text, px) / 2.0;
        for ch in text.chars() {
            let Some(font) = self.font_for(ch) else { continue };
            let sf = font.as_scaled(scale);
            let id = font.glyph_id(ch);
            let glyph = id.with_scale_and_position(scale, ab_glyph::point(x, baseline));
            x += sf.h_advance(id);
            let Some(outline) = font.outline_glyph(glyph) else { continue };
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    blend(img.get_pixel_mut(px as u32, py as u32), c, cov);
                }
            });
        }
    }

    pub fn image(&self, path: &str, size: u32) -> Option<RgbaImage> {
        let mut cache = self.images.borrow_mut();
        let img = cache
            .entry(path.into())
            .or_insert_with(|| {
                let full = crate::config::expand(path);
                // Plugin manifests name images without their extension.
                let file = crate::plugins::image_file(std::path::Path::new(""), &full.to_string_lossy())?;
                crate::plugins::load_image(&file, 288)
            })
            .as_ref()?;
        Some(imageops::resize(img, size, size, imageops::FilterType::Lanczos3))
    }

    /// An SVG drawn in `currentColor` (the icon store's single-colour icons) painted in `c`; other
    /// images as they are.
    fn tinted(&self, path: &str, c: Rgba<u8>, size: u32) -> Option<RgbaImage> {
        if !path.ends_with(".svg") {
            return self.image(path, size);
        }
        let key = format!("{path}#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        let mut cache = self.images.borrow_mut();
        let img = cache
            .entry(key)
            .or_insert_with(|| {
                let text = std::fs::read_to_string(crate::config::expand(path)).ok()?;
                let text = text.replace("currentColor", &format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
                crate::plugins::data_url(&text, 288)
            })
            .as_ref()?;
        Some(imageops::resize(img, size, size, imageops::FilterType::Lanczos3))
    }

    /// Paints an icon: a path to an image, or glyph text.
    fn icon(&self, img: &mut RgbaImage, icon: &str, size: f32, cx: f32, cy: f32, c: Rgba<u8>) {
        if icon.contains('/') {
            if let Some(pic) = self.tinted(icon, c, size as u32) {
                let (x, y) = ((cx - size / 2.0) as i64, (cy - size / 2.0) as i64);
                imageops::overlay(img, &pic, x, y);
            }
        } else {
            self.text(img, icon, size, cx, cy, img.width() as f32 * 0.9, c);
        }
    }

    pub fn key(&self, key: Option<&Key>, status: Option<&str>, theme: &Theme, size: u32) -> RgbaImage {
        let bg = color(&theme.background, Rgba([0, 0, 0, 255]));
        let fg = color(&theme.text, Rgba([255, 255, 255, 255]));
        let accent = color(&theme.accent, fg);
        let Some(key) = key else { return RgbaImage::from_pixel(size, size, bg) };
        let st = parse_status(status.unwrap_or(""));
        let fill_color = st.color.unwrap_or_else(|| color(&key.color, color(&theme.key, bg)));
        let fg = readable(fg, fill_color);
        let mut img = RgbaImage::from_pixel(size, size, fill_color);
        let s = size as f32;
        let text = if st.text.is_empty() { key.label.as_str() } else { st.text };

        if key.icon.is_empty() {
            let lines: Vec<&str> = text.lines().take(3).collect();
            let px = s * if lines.len() > 1 { 0.18 } else { 0.22 };
            let top = s / 2.0 - (lines.len() as f32 - 1.0) * px * 0.6;
            for (i, line) in lines.iter().enumerate() {
                self.text(&mut img, line, px, s / 2.0, top + i as f32 * px * 1.2, s * 0.88, fg);
            }
        } else if text.is_empty() {
            self.icon(&mut img, &key.icon, s * 0.5, s / 2.0, s / 2.0, fg);
        } else {
            self.icon(&mut img, &key.icon, s * 0.42, s / 2.0, s * 0.4, fg);
            self.text(&mut img, text.lines().next().unwrap_or(""), s * 0.16, s / 2.0, s * 0.8, s * 0.9, fg);
        }
        if let Some(l) = st.level {
            let (x, h) = ((s * 0.12) as i32, (s * 0.04).max(2.0) as i32);
            let w = size as i32 - x * 2;
            fill(&mut img, x, size as i32 - h * 2, w, h, bg);
            fill(&mut img, x, size as i32 - h * 2, (w as f32 * l) as i32, h, accent);
        }
        if !key.page.is_empty() {
            let d = (s * 0.07) as i32;
            fill(&mut img, size as i32 - d * 2, d, d, d, accent);
        }
        img
    }

    /// A key drawn by a plugin: its image fills the key and its title sits on top, as on Elgato's software.
    pub fn plugin_key(&self, image: Option<&RgbaImage>, title: &str, theme: &Theme, size: u32) -> RgbaImage {
        let bg = color(&theme.key, color(&theme.background, Rgba([0, 0, 0, 255])));
        let fg = color(&theme.text, Rgba([255, 255, 255, 255]));
        let mut img = RgbaImage::from_pixel(size, size, bg);
        if let Some(pic) = image {
            imageops::overlay(&mut img, &imageops::resize(pic, size, size, imageops::FilterType::Lanczos3), 0, 0);
        }
        let s = size as f32;
        let lines: Vec<&str> = title.lines().take(3).collect();
        let px = s * 0.16;
        let top = if image.is_some() { s * 0.82 - (lines.len() as f32 - 1.0) * px * 1.15 } else { s / 2.0 - (lines.len() as f32 - 1.0) * px * 0.6 };
        let shadow = Rgba([0, 0, 0, 200]);
        for (i, line) in lines.iter().enumerate() {
            let y = top + i as f32 * px * 1.15;
            if image.is_some() {
                self.text(&mut img, line, px, s / 2.0 + 1.0, y + 1.0, s * 0.92, shadow);
            }
            self.text(&mut img, line, px, s / 2.0, y, s * 0.92, fg);
        }
        img
    }

    /// One dial's part of the touch strip. `meter` is a live input level that wins over the status level.
    pub fn dial(&self, dial: Option<&Dial>, status: Option<&str>, meter: Option<f32>, theme: &Theme, w: u32, h: u32) -> RgbaImage {
        let bg = color(&theme.background, Rgba([0, 0, 0, 255]));
        let fg = color(&theme.text, Rgba([255, 255, 255, 255]));
        let accent = color(&theme.accent, fg);
        let mut img = RgbaImage::from_pixel(w, h, bg);
        let Some(dial) = dial else { return img };
        let (wf, hf) = (w as f32, h as f32);
        let pad = (hf * 0.06) as i32;
        fill(&mut img, pad, pad, w as i32 - pad * 2, h as i32 - pad * 2, color(&theme.key, bg));

        let title = [dial.icon.as_str(), dial.label.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("  ");
        let st = parse_status(status.unwrap_or(""));
        let lvl = meter.or(st.level);
        // Name on top, the value large in the middle, a thick bar at the bottom.
        let (title_y, value_y) = match (st.text.is_empty(), lvl.is_some()) {
            (true, false) => (hf / 2.0, 0.0),
            (true, true) => (hf * 0.34, 0.0),
            _ => (hf * 0.24, hf * 0.53),
        };
        let soft = mix_rgba(fg, color(&theme.key, bg), 0.25);
        self.text(&mut img, &title, hf * 0.21, wf / 2.0, title_y, wf * 0.88, soft);
        if !st.text.is_empty() {
            let muted = st.text.eq_ignore_ascii_case("mute");
            let c = st.color.unwrap_or(if muted { accent } else { fg });
            self.text(&mut img, st.text, hf * 0.36, wf / 2.0, value_y, wf * 0.88, c);
        }
        if let Some(l) = lvl {
            let (bx, by, bw, bh) = (pad * 3, (hf * 0.79) as i32, w as i32 - pad * 6, (hf * 0.1).max(4.0) as i32);
            let hot = Rgba([229, 72, 77, 255]);
            round_bar(&mut img, bx, by, bw, bh, bg);
            let filled = (bw as f32 * l) as i32;
            if filled > 0 {
                round_bar(&mut img, bx, by, filled.max(bh), bh, if meter.is_some() && l > 0.92 { hot } else { accent });
            }
        }
        img
    }
}

/// A status line: `text`, optionally followed by `|level` (0..1 or 42%) and `|#color`.
pub struct Status<'a> {
    pub text: &'a str,
    pub level: Option<f32>,
    pub color: Option<Rgba<u8>>,
}

pub fn parse_status(line: &str) -> Status<'_> {
    let mut parts = line.split('|');
    let text = parts.next().unwrap_or("").trim();
    let lvl = parts.next().map(str::trim).filter(|s| !s.is_empty());
    let col = parts.next().and_then(parse_color);
    Status { text, level: lvl.and_then(level).or_else(|| level(text)), color: col }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(level("42%"), Some(0.42));
        assert_eq!(level("0.5"), Some(0.5));
        assert_eq!(level("150"), Some(1.0));
        assert_eq!(level("mute"), None);
    }

    #[test]
    fn status_fields() {
        let st = parse_status("42 dB|0.5|#ff0000");
        assert_eq!((st.text, st.level, st.color), ("42 dB", Some(0.5), Some(Rgba([255, 0, 0, 255]))));
        let st = parse_status("Mute||#c0392b");
        assert_eq!((st.text, st.level), ("Mute", None));
        assert_eq!(parse_status("30%").level, Some(0.3));
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff0080"), Some(Rgba([255, 0, 128, 255])));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn draws_text_on_key() {
        let p = Painter::new("sans-serif");
        let key = Key { label: "Hola".into(), ..Default::default() };
        let theme = Theme::default();
        let img = p.key(Some(&key), None, &theme, 72);
        let bg = parse_color(&theme.key).unwrap();
        assert!(img.pixels().any(|px| *px != bg), "label left no pixels");
    }
}
