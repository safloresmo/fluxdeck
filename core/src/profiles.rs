//! Profiles: what changes when a given app has the focus — a deck page, group volumes, a command —
//! and goes back when it loses it.
use crate::groups::Level;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// A Nerd Font glyph.
    pub icon: String,
    /// Window classes or titles (or parts of them, any case) that turn the profile on.
    pub apps: Vec<String>,
    /// Deck page to show; empty leaves the page alone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub page: String,
    /// Group volumes to set, by group id.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub levels: BTreeMap<String, Level>,
    /// A command to run when the profile turns on.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    /// Put the page and volumes back when another app takes the focus.
    pub restore: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { id: String::new(), name: String::new(), icon: '\u{f0297}'.to_string(), apps: Vec::new(), page: String::new(), levels: BTreeMap::new(), command: String::new(), restore: true }
    }
}

impl Profile {
    pub fn matches(&self, class: &str, title: &str) -> bool {
        let (class, title) = (class.to_lowercase(), title.to_lowercase());
        self.apps.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).any(|a| class.contains(&a) || title.contains(&a))
    }
}

/// The first profile for a focused window.
pub fn for_window<'a>(profiles: &'a [Profile], class: &str, title: &str) -> Option<&'a Profile> {
    profiles.iter().find(|p| p.matches(class, title))
}

/// Which profile is on right now, shared with the editor.
pub fn active_path() -> std::path::PathBuf {
    crate::paths::runtime("profile")
}

pub fn active() -> Option<String> {
    std::fs::read_to_string(active_path()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A profile to start from: games, with chat up and music down.
pub fn starter() -> Profile {
    let mut levels = BTreeMap::new();
    levels.insert("chat".to_string(), Level { volume: 1.0, muted: false });
    levels.insert("musica".to_string(), Level { volume: 0.4, muted: false });
    Profile {
        id: "juegos".into(),
        name: crate::i18n::t("Juegos").into(),
        apps: ["steam_app", "gamescope", "lutris", "heroic", "minecraft"].map(String::from).to_vec(),
        levels,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matching_profile_wins() {
        let games = starter();
        let code = Profile { id: "code".into(), apps: vec!["zed".into(), "Code".into()], ..Default::default() };
        let all = vec![games, code];
        assert_eq!(for_window(&all, "steam_app_1245620", "ELDEN RING").map(|p| p.id.as_str()), Some("juegos"));
        assert_eq!(for_window(&all, "dev.zed.Zed", "main.rs").map(|p| p.id.as_str()), Some("code"));
        assert!(for_window(&all, "kitty", "zsh").is_none());
    }

    #[test]
    fn reads_old_configs_without_restore() {
        let p: Profile = toml::from_str("id = \"a\"\nname = \"A\"\napps = [\"x\"]").unwrap();
        assert!(p.restore);
    }
}
