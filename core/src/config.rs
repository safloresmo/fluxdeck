use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{fs, io};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub brightness: u8,
    /// Font family resolved through fontconfig; Nerd Font glyphs fall back to any installed Nerd Font.
    pub font: String,
    pub theme: Theme,
    /// A Material You palette (matugen-style colors.json); when set, colours follow it live.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub palette: String,
    /// Interface language: "es", "en", or empty to follow the system.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub language: String,
    /// Editor size: 0.9 compact, 1.0 normal, 1.15 comfortable.
    #[serde(skip_serializing_if = "is_one")]
    pub ui_scale: f32,
    /// Profiles that switch on with the focused app.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<crate::profiles::Profile>,
    /// App groups (Wave Link-style channels): a volume and mute for several apps at once.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<crate::groups::Group>,
    /// Audio devices left out of the mixer's menus, by node name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden_devices: Vec<String>,
    /// Installed plugins that are switched off, by uuid.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disabled_plugins: Vec<String>,
    /// Optional device modules that are switched on, such as "corsair".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    pub pages: Vec<Page>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Theme {
    pub background: String,
    pub key: String,
    pub text: String,
    pub accent: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Page {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Key>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dials: Vec<Dial>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Key {
    pub pos: u8,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// A Nerd Font glyph, or a path to a PNG/JPEG image.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Background colour; the theme's key colour when empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Page to open when pressed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub page: String,
    /// Command whose first output line replaces the label, run every `interval` seconds.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub interval: f32,
    /// A plugin action's UUID; the plugin then draws this key and handles its presses.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// Id of this placed plugin action, kept when the key moves so its settings follow it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instance: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Dial {
    pub pos: u8,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Commands get FLUXDECK_STEPS with how many detents the dial moved.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub right: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub left: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub press: String,
    /// Output like "42%" or "42" also draws a level bar.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub interval: f32,
    /// PipeWire node whose live input level fills the bar.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meter: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub instance: String,
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Default for Config {
    fn default() -> Self {
        Config {
            brightness: 60,
            font: "sans-serif".into(),
            theme: Theme::default(),
            palette: String::new(),
            language: String::new(),
            ui_scale: 1.0,
            disabled_plugins: Vec::new(),
            hidden_devices: Vec::new(),
            groups: Vec::new(),
            profiles: Vec::new(),
            modules: Vec::new(),
            pages: vec![Page { name: "inicio".into(), keys: vec![], dials: vec![] }],
        }
    }
}

impl Theme {
    pub fn dark(accent: &str) -> Theme {
        Theme { accent: accent.into(), ..Theme::default() }
    }

    pub fn light(accent: &str) -> Theme {
        Theme { background: "#f4f1f6".into(), key: "#e4dee9".into(), text: "#1c1a20".into(), accent: accent.into() }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            background: "#101014".into(),
            key: "#1d1d24".into(),
            text: "#e8e6ee".into(),
            accent: "#9b8cff".into(),
        }
    }
}

impl Key {
    pub fn interval(&self) -> f32 {
        if self.interval > 0.0 { self.interval } else { 2.0 }
    }
}

impl Dial {
    pub fn interval(&self) -> f32 {
        if self.interval > 0.0 { self.interval } else { 2.0 }
    }
}

impl Config {
    pub fn module(&self, name: &str) -> bool {
        self.modules.iter().any(|m| m == name)
    }

    pub fn set_module(&mut self, name: &str, on: bool) {
        self.modules.retain(|m| m != name);
        if on {
            self.modules.push(name.to_string());
        }
    }
}

impl Page {
    pub fn key(&self, pos: u8) -> Option<&Key> {
        self.keys.iter().find(|k| k.pos == pos)
    }
    pub fn dial(&self, pos: u8) -> Option<&Dial> {
        self.dials.iter().find(|d| d.pos == pos)
    }
}

pub fn dir() -> PathBuf {
    crate::paths::config()
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

/// Optional file written by a theme generator (e.g. matugen); overrides `[theme]`.
pub fn theme_path() -> PathBuf {
    dir().join("theme.toml")
}

/// Ryoku's live palette, offered by the editor when present.
pub fn desktop_palette() -> Option<String> {
    let p = "~/.cache/ryoku/colors.json";
    expand(p).exists().then(|| p.to_string())
}

pub fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// Maps Material You roles to the deck: surface, raised surface, text on surface, primary.
pub fn palette_theme(path: &str) -> Option<Theme> {
    if path.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(expand(path)).ok()?).ok()?;
    let get = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k)?.as_str().map(String::from));
    Some(Theme {
        background: get(&["background", "surface"])?,
        key: get(&["surfaceContainerHigh", "surface_container_high", "surfaceVariant"])?,
        text: get(&["onSurface", "on_surface", "foreground"])?,
        accent: get(&["primary"])?,
    })
}

pub fn file_theme() -> Option<Theme> {
    toml::from_str(&fs::read_to_string(theme_path()).ok()?).ok()
}

impl Config {
    /// Missing file gives the default; a broken file is an error so callers keep what they had.
    pub fn load() -> Result<Config, String> {
        let mut cfg = match fs::read_to_string(path()) {
            Ok(s) => toml::from_str::<Config>(&s).map_err(|e| format!("{}: {e}", path().display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(format!("{}: {e}", path().display())),
        };
        if cfg.pages.is_empty() {
            cfg.pages = Config::default().pages;
        }
        cfg.brightness = cfg.brightness.min(100);
        Ok(cfg)
    }

    /// The theme from theme.toml when present, else the config's own.
    pub fn effective_theme(&self) -> Theme {
        file_theme().or_else(|| palette_theme(&self.palette)).unwrap_or_else(|| self.theme.clone())
    }

    /// Write through a temp file so the daemon never reads half a config.
    /// A symlinked config (dotfiles) is written at its target so the link survives.
    pub fn save(&self) -> io::Result<()> {
        fs::create_dir_all(dir())?;
        let target = fs::canonicalize(path()).unwrap_or_else(|_| path());
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = target.with_file_name(".config.toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, target)
    }

    pub fn page_index(&self, name: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_everything() {
        let mut cfg = Config::default();
        cfg.pages[0].dials.push(Dial { pos: 1, right: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 2%+".into(), meter: "mic".into(), ..Default::default() });
        cfg.pages[0].keys.push(Key { pos: 3, label: "Term".into(), command: "kitty".into(), ..Default::default() });
        let back: Config = toml::from_str(&toml::to_string_pretty(&cfg).unwrap()).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn palette_roles() {
        let p = std::env::temp_dir().join("fluxdeck-palette-test.json");
        fs::write(&p, r##"{"background":"#000001","surfaceContainerHigh":"#000002","onSurface":"#000003","primary":"#000004"}"##).unwrap();
        let t = palette_theme(p.to_str().unwrap()).unwrap();
        assert_eq!((t.background.as_str(), t.key.as_str(), t.text.as_str(), t.accent.as_str()), ("#000001", "#000002", "#000003", "#000004"));
        assert!(palette_theme("").is_none());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[[pages]]\nname = \"a\"\n[[pages.keys]]\npos = 2\n").unwrap();
        assert_eq!(cfg.brightness, 60);
        assert_eq!(cfg.pages[0].key(2).unwrap().interval(), 2.0);
    }
}
