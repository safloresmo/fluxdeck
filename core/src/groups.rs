//! App groups, like Wave Link's channels: a group has a volume and a mute that FluxDeck applies to
//! every app in it, including apps opened later. No virtual devices are created, so nothing new shows
//! up in the system's sound settings.
use crate::mixer::{self, Kind, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// A Nerd Font glyph.
    pub icon: String,
    /// App names (or parts of them, any case) that belong to this group.
    pub apps: Vec<String>,
}

impl Group {
    pub fn matches(&self, app: &str, binary: &str) -> bool {
        let (app, binary) = (app.to_lowercase(), binary.to_lowercase());
        self.apps.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).any(|a| app.contains(&a) || binary.contains(&a))
    }
}

/// A group's volume (0..1) and mute.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
}

impl Default for Level {
    fn default() -> Self {
        Level { volume: 1.0, muted: false }
    }
}

/// Levels live apart from config.toml: dials change them often and the layout should not reload each time.
fn levels_path() -> PathBuf {
    crate::paths::state().join("groups.json")
}

pub fn levels() -> BTreeMap<String, Level> {
    std::fs::read_to_string(levels_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn set_level(id: &str, level: Level) {
    let mut all = levels();
    all.insert(id.to_string(), level);
    let path = levels_path();
    let _ = std::fs::create_dir_all(crate::paths::state());
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&all).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// The group an app belongs to: the first that lists it.
pub fn group_of<'a>(groups: &'a [Group], app: &mixer::Channel) -> Option<&'a Group> {
    groups.iter().find(|g| g.matches(&app.label, &app.binary))
}

/// Brings every playing app to its group's volume and mute; returns how many streams changed.
pub fn apply(groups: &[Group], snap: &Snapshot) -> usize {
    let levels = levels();
    let mut changed = 0;
    for app in snap.of(Kind::Playback) {
        let Some(g) = group_of(groups, app) else { continue };
        let want = levels.get(&g.id).copied().unwrap_or_default();
        if (app.volume - want.volume).abs() > 0.005 {
            mixer::set_volume(app.id, want.volume);
            changed += 1;
        }
        if app.muted != want.muted {
            mixer::set_mute(app.id, want.muted);
            changed += 1;
        }
    }
    changed
}

/// The groups offered on first use.
pub fn starters() -> Vec<Group> {
    let g = |id: &str, name: &'static str, icon: char, apps: &[&str]| Group { id: id.into(), name: crate::i18n::t(name).into(), icon: icon.to_string(), apps: apps.iter().map(|a| a.to_string()).collect() };
    vec![
        g("juegos", "Juegos", '\u{f0297}', &["steam", "proton", "wine", "gamescope", "lutris", "heroic", "minecraft"]),
        g("musica", "Música", '\u{f075a}', &["spotify", "cider", "rhythmbox", "elisa", "amberol", "tidal"]),
        g("navegador", "Navegador", '\u{f059f}', &["firefox", "chrom", "helium", "brave", "zen", "vivaldi"]),
        g("chat", "Chat", '\u{f066f}', &["discord", "vesktop", "equibop", "telegram", "signal", "teamspeak", "mumble"]),
    ]
}

/// A fresh id from a name: lowercase letters and digits, unique among `groups`.
pub fn new_id(groups: &[Group], name: &str) -> String {
    let base: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let base = if base.is_empty() { "grupo".to_string() } else { base };
    (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}{n}") }).find(|id| !groups.iter().any(|g| &g.id == id)).unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_by_name_or_binary_any_case() {
        let g = Group { apps: vec!["Spotify".into(), "chrom".into(), " ".into()], ..Default::default() };
        assert!(g.matches("spotify", ""));
        assert!(g.matches("Google Chrome", ""));
        assert!(g.matches("", "chromium"));
        assert!(!g.matches("Firefox", "firefox"));
    }

    #[test]
    fn ids_are_clean_and_unique() {
        let gs = starters();
        assert_eq!(new_id(&gs, "Música"), "msica");
        assert_eq!(new_id(&gs, "Juegos"), "juegos2");
        assert_eq!(new_id(&gs, "¡!"), "grupo");
    }
}
