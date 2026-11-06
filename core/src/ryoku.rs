//! The FluxDeck widget for a Ryoku desktop's bar. The installer copies its source from its own repo, and FluxDeck
//! installs, updates, shows or hides it only through Ryoku's own commands.
use std::path::PathBuf;
use std::process::{Command, Stdio};

const WIDGET: &str = "fluxdeck";

/// What FluxDeck knows about its widget, read once when the settings page opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    /// The version FluxDeck ships, if its copy is there.
    pub shipped: Option<String>,
    /// The version Ryoku has installed, if any.
    pub installed: Option<String>,
    /// Shown on the bar; None when it is not on the bar.
    pub shown: Option<bool>,
}

impl Widget {
    pub fn read() -> Widget {
        let installed = version(&ryoku_plugins().join(WIDGET));
        Widget { shipped: version(&source()), shown: if installed.is_some() { bar_widget() } else { None }, installed }
    }

    pub fn outdated(&self) -> bool {
        matches!((&self.shipped, &self.installed), (Some(a), Some(b)) if a != b)
    }
}

/// Where the installer puts the widget's source.
pub fn source() -> PathBuf {
    crate::paths::data().join("ryoku-widget")
}

fn ryoku_plugins() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/share")).join("ryoku/plugins")
}

fn version(dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["version"].as_str().map(String::from)
}

/// Installs the shipped copy and puts it on the bar; an update removes the old one first, which
/// Ryoku does keeping its place and settings.
pub fn install(update: bool) -> Result<(), String> {
    if update {
        run(&["plugin", "remove", WIDGET])?;
    }
    let src = source();
    run(&["plugin", "add", &src.to_string_lossy(), "--bar", "--yes"])
}

pub fn remove() -> Result<(), String> {
    run(&["plugin", "remove", WIDGET])
}

fn run(args: &[&str]) -> Result<(), String> {
    let out = Command::new("ryoku").args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    Err(said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("ryoku plugin failed").to_string())
}

/// Whether the widget is shown on the bar; None when the bar has no FluxDeck widget.
fn bar_widget() -> Option<bool> {
    let out = Command::new("ryoku-shell").args(["bar", "list"]).stderr(Stdio::null()).output().ok()?;
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Lines look like `right  13  fluxdeck  plugin  shown  FluxDeck`.
fn parse(list: &str) -> Option<bool> {
    list.lines().map(|l| l.split_whitespace().collect::<Vec<_>>()).find(|w| w.get(2) == Some(&WIDGET)).map(|w| w.get(4) == Some(&"shown"))
}

pub fn set_bar_widget(shown: bool) -> bool {
    Command::new("ryoku-shell").args(["bar", if shown { "show" } else { "hide" }, WIDGET]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_widget_state() {
        let list = "left    0  launcher     builtin  shown   Launcher\nright  13  fluxdeck     plugin   hidden  FluxDeck\n";
        assert_eq!(parse(list), Some(false));
        assert_eq!(parse(&list.replace("hidden", "shown")), Some(true));
        assert_eq!(parse("left 0 launcher builtin shown Launcher"), None);
    }
}
