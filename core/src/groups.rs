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
    save(&levels_path(), &all);
}

/// Which output each group plays on, by device name; a group not listed follows the default output.
fn routes_path() -> PathBuf {
    crate::paths::state().join("group-outputs.json")
}

pub fn routes() -> BTreeMap<String, String> {
    std::fs::read_to_string(routes_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn set_route(id: &str, output: &str) {
    let mut all = routes();
    all.insert(id.to_string(), output.to_string());
    save(&routes_path(), &all);
}

fn save<T: Serialize>(path: &std::path::Path, value: &T) {
    let _ = std::fs::create_dir_all(crate::paths::state());
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(value).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// The group an app belongs to: the first that lists it.
pub fn group_of<'a>(groups: &'a [Group], app: &mixer::Channel) -> Option<&'a Group> {
    groups.iter().find(|g| g.matches(&app.label, &app.binary))
}

/// The output an app should move to so it plays where its group's route says, if it is elsewhere.
/// A route to a device that is not plugged in, or an app not linked yet, is left alone.
pub fn misrouted<'a>(route: Option<&'a String>, app: &mixer::Channel, snap: &Snapshot) -> Option<&'a String> {
    let route = route?;
    let target = snap.of(Kind::Output).find(|o| &o.name == route)?;
    (app.device? != target.id).then_some(route)
}

/// Brings every playing app to its group's volume, mute and output; returns how many streams changed.
/// While the stream mix is on, a group's apps play at full volume into the group's own output
/// instead, and the group's volume and mute are set on that output (see `streammix`).
pub fn apply(groups: &[Group], snap: &Snapshot) -> usize {
    let levels = levels();
    let routes = routes();
    let mixing = crate::streammix::StreamMix::load().on;
    let mut changed = 0;
    if mixing {
        for g in groups {
            let Some(sink) = snap.of(Kind::Output).find(|o| o.name == crate::streammix::group_sink(&g.id)) else { continue };
            let want = levels.get(&g.id).copied().unwrap_or_default();
            changed += set(sink, want);
        }
    }
    for app in snap.of(Kind::Playback) {
        let Some(g) = group_of(groups, app) else { continue };
        if mixing {
            // Until the group's output exists, its apps are left as they are.
            let name = crate::streammix::group_sink(&g.id);
            let Some(sink) = snap.of(Kind::Output).find(|o| o.name == name) else { continue };
            if app.device.is_some_and(|d| d != sink.id) {
                mixer::move_stream(app.id, &name);
                changed += 1;
            }
            changed += set(app, Level::default());
            continue;
        }
        if let Some(output) = misrouted(routes.get(&g.id), app, snap) {
            mixer::move_stream(app.id, output);
            changed += 1;
        }
        changed += set(app, levels.get(&g.id).copied().unwrap_or_default());
    }
    changed
}

/// Sets a channel's volume and mute where they differ; returns how many changed.
fn set(c: &mixer::Channel, want: Level) -> usize {
    let mut changed = 0;
    if (c.volume - want.volume).abs() > 0.005 {
        mixer::set_volume(c.id, want.volume);
        changed += 1;
    }
    if c.muted != want.muted {
        mixer::set_mute(c.id, want.muted);
        changed += 1;
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
    fn moves_only_apps_on_another_plugged_in_output() {
        let ch = |id, name: &str, kind, device| mixer::Channel { id, name: name.into(), label: String::new(), icon: String::new(), binary: String::new(), kind, volume: 1.0, muted: false, device, internal: false };
        let snap = Snapshot { channels: vec![ch(1, "phones", Kind::Output, None), ch(2, "speakers", Kind::Output, None)], ..Default::default() };
        let (phones, gone) = ("phones".to_string(), "usb".to_string());
        assert_eq!(misrouted(Some(&phones), &ch(9, "", Kind::Playback, Some(2)), &snap), Some(&phones));
        assert_eq!(misrouted(Some(&phones), &ch(9, "", Kind::Playback, Some(1)), &snap), None);
        assert_eq!(misrouted(Some(&phones), &ch(9, "", Kind::Playback, None), &snap), None);
        assert_eq!(misrouted(Some(&gone), &ch(9, "", Kind::Playback, Some(2)), &snap), None);
        assert_eq!(misrouted(None, &ch(9, "", Kind::Playback, Some(2)), &snap), None);
    }

    #[test]
    fn ids_are_clean_and_unique() {
        let gs = starters();
        assert_eq!(new_id(&gs, "Música"), "msica");
        assert_eq!(new_id(&gs, "Juegos"), "juegos2");
        assert_eq!(new_id(&gs, "¡!"), "grupo");
    }
}
