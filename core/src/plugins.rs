//! Stream Deck SDK plugins (Elgato and OpenAction): where they live, their manifests, images and saved settings.
use image::RgbaImage;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `$XDG_DATA_HOME/fluxdeck/plugins`, one `<uuid>.sdPlugin` folder each.
pub fn dir() -> PathBuf {
    crate::paths::data().join("plugins")
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Manifest {
    #[serde(alias = "Name")]
    pub name: String,
    #[serde(alias = "Author")]
    pub author: String,
    #[serde(alias = "Version")]
    pub version: String,
    #[serde(alias = "Icon")]
    pub icon: String,
    #[serde(alias = "Category")]
    pub category: String,
    #[serde(alias = "Description")]
    pub description: String,
    #[serde(alias = "Actions")]
    pub actions: Vec<ActionDef>,
    #[serde(alias = "OS")]
    pub os: Vec<Os>,
    #[serde(alias = "CodePath")]
    pub code_path: Option<String>,
    #[serde(alias = "CodePathLin")]
    pub code_path_linux: Option<String>,
    #[serde(alias = "CodePathWin")]
    pub code_path_windows: Option<String>,
    #[serde(alias = "CodePaths")]
    pub code_paths: Option<HashMap<String, String>>,
    #[serde(alias = "PropertyInspectorPath")]
    pub property_inspector: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Os {
    #[serde(alias = "Platform")]
    pub platform: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ActionDef {
    #[serde(alias = "UUID")]
    pub uuid: String,
    #[serde(alias = "Name")]
    pub name: String,
    #[serde(alias = "Icon")]
    pub icon: String,
    #[serde(alias = "Tooltip")]
    pub tooltip: String,
    /// "Keypad" and/or "Encoder"; Keypad when missing.
    #[serde(alias = "Controllers")]
    pub controllers: Vec<String>,
    #[serde(alias = "States")]
    pub states: Vec<StateDef>,
    #[serde(alias = "PropertyInspectorPath")]
    pub property_inspector: Option<String>,
    #[serde(alias = "Encoder")]
    pub encoder: Option<EncoderDef>,
    /// When false, a key with several states moves to the next one on each press.
    #[serde(alias = "DisableAutomaticStates")]
    pub disable_automatic_states: bool,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct StateDef {
    #[serde(alias = "Image")]
    pub image: String,
    #[serde(alias = "Title")]
    pub title: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct EncoderDef {
    #[serde(alias = "Icon")]
    pub icon: String,
    #[serde(alias = "background")]
    pub background: String,
}

impl ActionDef {
    pub fn on_keys(&self) -> bool {
        self.controllers.is_empty() || self.controllers.iter().any(|c| c == "Keypad")
    }
    pub fn on_dials(&self) -> bool {
        self.controllers.iter().any(|c| c == "Encoder")
    }
}

/// How a plugin's code is started on this machine.
#[derive(Debug, Clone, PartialEq)]
pub enum Runtime {
    Native(PathBuf),
    Node(PathBuf),
    Wine(PathBuf),
    /// Plugins whose code is a web page need a browser engine FluxDeck does not ship.
    Unsupported(String),
}

#[derive(Debug, Clone)]
pub struct Plugin {
    /// Folder name without ".sdPlugin"; the uuid the plugin registers with.
    pub uuid: String,
    pub path: PathBuf,
    pub manifest: Manifest,
}

pub fn read(path: &Path) -> Result<Plugin, String> {
    let text = std::fs::read_to_string(path.join("manifest.json")).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut manifest: Manifest = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))?;
    if manifest.category.is_empty() {
        manifest.category = manifest.name.clone();
    }
    let folder = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    Ok(Plugin { uuid: folder.trim_end_matches(".sdPlugin").to_string(), path: path.to_path_buf(), manifest })
}

/// Installed plugins the user has not switched off in the config.
pub fn enabled() -> Vec<Plugin> {
    let off = crate::Config::load().map(|c| c.disabled_plugins).unwrap_or_default();
    installed().into_iter().filter(|p| !off.contains(&p.uuid)).collect()
}

/// Every installed plugin that has a readable manifest, sorted by name.
pub fn installed() -> Vec<Plugin> {
    let mut out: Vec<Plugin> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| read(&e.path()).map_err(|err| eprintln!("fluxdeck: skipping plugin {err}")).ok())
        .collect();
    out.sort_by_key(|p| p.manifest.name.to_lowercase());
    out
}

pub fn find_action<'a>(plugins: &'a [Plugin], uuid: &str) -> Option<(&'a Plugin, &'a ActionDef)> {
    plugins.iter().find_map(|p| p.manifest.actions.iter().find(|a| a.uuid == uuid).map(|a| (p, a)))
}

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

impl Plugin {
    pub fn runtime(&self) -> Runtime {
        let m = &self.manifest;
        let linux = m.os.iter().any(|o| o.platform == "linux");
        let windows = m.os.iter().any(|o| o.platform == "windows");
        let target = format!("{}-unknown-linux-gnu", std::env::consts::ARCH);
        let own = m.code_paths.as_ref().and_then(|p| p.get(&target).cloned()).or_else(|| m.code_path_linux.clone());
        let code = if linux { own.or_else(|| m.code_path.clone()) } else { own };
        let pick = |rel: &str| self.path.join(rel);
        match code {
            Some(c) if c.ends_with(".js") || c.ends_with(".mjs") || c.ends_with(".cjs") => Runtime::Node(pick(&c)),
            Some(c) if c.ends_with(".html") || c.ends_with(".htm") => Runtime::Unsupported("web".into()),
            Some(c) => Runtime::Native(pick(&c)),
            None => match m.code_path.clone() {
                // Node plugins often list only Windows and macOS but run anywhere.
                Some(c) if c.ends_with(".js") || c.ends_with(".mjs") || c.ends_with(".cjs") => Runtime::Node(pick(&c)),
                Some(c) if c.ends_with(".html") || c.ends_with(".htm") => Runtime::Unsupported("web".into()),
                _ if windows => m.code_path_windows.clone().or_else(|| m.code_path.clone()).map_or(Runtime::Unsupported("windows".into()), |c| Runtime::Wine(pick(&c))),
                _ => Runtime::Unsupported("platform".into()),
            },
        }
    }

    /// What is missing to run this plugin here, if anything.
    pub fn missing(&self) -> Option<&'static str> {
        match self.runtime() {
            Runtime::Node(_) if !has("node") => Some("node"),
            Runtime::Wine(_) if !has("wine") => Some("wine"),
            Runtime::Unsupported(_) => Some("unsupported"),
            _ => None,
        }
    }

    /// Property inspector page for an action, if it has one.
    pub fn inspector(&self, action: &ActionDef) -> Option<PathBuf> {
        action.property_inspector.clone().or_else(|| self.manifest.property_inspector.clone()).map(|p| self.path.join(p))
    }
}

/// Finds an image the SDK names without extension: `img/key` may be key.png, key@2x.png or key.svg.
pub fn image_file(base: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let p = base.join(rel);
    ["", "@2x.png", ".png", ".svg", "@2x.jpg", ".jpg"]
        .iter()
        .map(|ext| if ext.is_empty() { p.clone() } else { PathBuf::from(format!("{}{ext}", p.display())) })
        .find(|c| c.is_file())
}

fn svg(data: &[u8], size: u32) -> Option<RgbaImage> {
    // Loading system fonts takes a moment, so it happens once and only when an SVG shows up.
    static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    let fonts = || {
        FONTS.get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            std::sync::Arc::new(db)
        })
    };
    // Fonts take tens of megabytes; only SVGs with text need them.
    let has_text = data.windows(5).any(|w| w == b"<text");
    let opt = if has_text { resvg::usvg::Options { fontdb: fonts().clone(), ..Default::default() } } else { resvg::usvg::Options::default() };
    let tree = resvg::usvg::Tree::from_data(data, &opt).ok()?;
    let mut pix = resvg::tiny_skia::Pixmap::new(size, size)?;
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pix.as_mut());
    // tiny-skia stores premultiplied alpha.
    let mut img = RgbaImage::from_raw(size, size, pix.take())?;
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(img)
}

/// Decodes image bytes (PNG, JPEG or SVG) into a square of `size` pixels.
pub fn decode(bytes: &[u8], size: u32) -> Option<RgbaImage> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]).to_lowercase();
    let img = if head.contains("<svg") || head.contains("<?xml") { svg(bytes, size)? } else { image::load_from_memory(bytes).ok()?.to_rgba8() };
    Some(image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3))
}

pub fn load_image(path: &Path, size: u32) -> Option<RgbaImage> {
    decode(&std::fs::read(path).ok()?, size)
}

/// Images sent by plugins: `data:image/png;base64,…`, `data:image/svg+xml;charset=utf8,<svg…>` or a bare SVG.
pub fn data_url(url: &str, size: u32) -> Option<RgbaImage> {
    let Some(rest) = url.strip_prefix("data:") else { return decode(url.as_bytes(), size) };
    let (meta, body) = rest.split_once(',')?;
    if meta.ends_with(";base64") { decode(&base64(body)?, size) } else { decode(percent(body).as_bytes(), size) }
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let digits: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').map(val).collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, d)| n | (*d as u32) << (18 - 6 * i));
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Some(out)
}

fn percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Settings plugins save per placed action (by instance id) and per plugin ("global:<uuid>").
pub fn settings_path() -> PathBuf {
    crate::config::dir().join("plugin-settings.json")
}

pub fn load_settings() -> Map<String, Value> {
    std::fs::read_to_string(settings_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_settings(all: &Map<String, Value>) {
    let path = settings_path();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(all).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// A fresh id for a placed plugin action.
pub fn new_instance() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:x}{:x}{:x}", t, std::process::id(), COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
    }

    #[test]
    fn percent_decodes() {
        assert_eq!(percent("%3Csvg%20a%3D%221%22%3E"), "<svg a=\"1\">");
    }

    #[test]
    fn svg_data_url_renders() {
        let url = "data:image/svg+xml;charset=utf8,<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><rect width='10' height='10' fill='red'/></svg>";
        let img = data_url(url, 20).unwrap();
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255]);
    }

    #[test]
    fn runtime_from_manifest() {
        let p = |json: &str| Plugin { uuid: "x".into(), path: "/p".into(), manifest: serde_json::from_str(json).unwrap() };
        assert_eq!(p(r#"{"CodePath":"bin/plugin.js","OS":[{"Platform":"windows"}]}"#).runtime(), Runtime::Node("/p/bin/plugin.js".into()));
        assert_eq!(p(r#"{"CodePathLin":"x","CodePath":"x.exe","OS":[{"Platform":"linux"}]}"#).runtime(), Runtime::Native("/p/x".into()));
        assert_eq!(p(r#"{"CodePath":"x.exe","OS":[{"Platform":"windows"}]}"#).runtime(), Runtime::Wine("/p/x.exe".into()));
    }
}
