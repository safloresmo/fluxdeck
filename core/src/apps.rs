//! Installed desktop apps (.desktop files): names, icons and how to start them.
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct App {
    /// File name without ".desktop", e.g. "firefox".
    pub id: String,
    pub name: String,
    pub exec: String,
    /// The icon name or path the file gives.
    pub icon: String,
}

fn data_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))];
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    dirs.push(home.join(".local/share/flatpak/exports/share"));
    dirs.push("/var/lib/flatpak/exports/share".into());
    dirs.dedup();
    dirs
}

/// Reads the [Desktop Entry] group, taking the name in the interface language when there is one.
fn parse(id: &str, text: &str) -> Option<App> {
    let lang = if crate::i18n::english() { "en" } else { "es" };
    let (mut name, mut local_name, mut exec, mut icon) = (None, None, None, String::new());
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "Type" if v.trim() != "Application" => return None,
            "NoDisplay" | "Hidden" if v.trim() == "true" => return None,
            "Name" => name = Some(v.trim().to_string()),
            k if k == format!("Name[{lang}]") => local_name = Some(v.trim().to_string()),
            "Exec" => exec = Some(v.trim().to_string()),
            "Icon" => icon = v.trim().to_string(),
            _ => {}
        }
    }
    Some(App { id: id.into(), name: local_name.or(name)?, exec: exec?, icon })
}

/// Visible apps, one per id (the user's own file wins), sorted by name.
pub fn installed() -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for dir in data_dirs() {
        for entry in std::fs::read_dir(dir.join("applications")).into_iter().flatten().flatten() {
            let path = entry.path();
            let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")) else { continue };
            if out.iter().any(|a| a.id == id) {
                continue;
            }
            if let Some(app) = std::fs::read_to_string(&path).ok().and_then(|t| parse(id, &t)) {
                out.push(app);
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

pub fn find(id: &str) -> Option<App> {
    let id = id.trim_end_matches(".desktop");
    data_dirs().iter().find_map(|d| {
        let path = d.join("applications").join(format!("{id}.desktop"));
        std::fs::read_to_string(path).ok().and_then(|t| parse(id, &t))
    })
}

/// The command line from Exec, without the %f/%u… placeholders a launcher fills with files.
pub fn command(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if chars.next() == Some('%') {
                out.push('%');
            }
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Starts an app on its own, so it outlives whoever launched it.
pub fn launch(app: &App) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new("sh")
        .args(["-c", &command(&app.exec)])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

/// The icon theme in use and the ones it inherits from, ending in hicolor.
fn themes() -> Vec<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let configured = ["gtk-4.0", "gtk-3.0"]
        .iter()
        .filter_map(|g| std::fs::read_to_string(home.join(".config").join(g).join("settings.ini")).ok())
        .find_map(|t| t.lines().find_map(|l| l.trim().strip_prefix("gtk-icon-theme-name=").map(|v| v.trim().to_string())));
    let mut chain: Vec<String> = Vec::new();
    let mut next = configured;
    while let Some(theme) = next.take() {
        if chain.contains(&theme) || chain.len() > 6 {
            break;
        }
        let index = data_dirs().iter().map(|d| d.join("icons").join(&theme).join("index.theme")).chain([home.join(".icons").join(&theme).join("index.theme")]).find_map(|p| std::fs::read_to_string(p).ok());
        next = index.and_then(|t| t.lines().find_map(|l| l.strip_prefix("Inherits=").map(|v| v.split(',').next().unwrap_or("").trim().to_string())));
        chain.push(theme);
    }
    chain.push("hicolor".into());
    chain
}

/// A file for an icon name: the theme's largest, else pixmaps. Paths are returned as they are.
pub fn icon_file(icon: &str) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    if Path::new(icon).is_absolute() {
        return Path::new(icon).is_file().then(|| icon.into());
    }
    let sizes = ["scalable/apps", "512x512/apps", "256x256/apps", "128x128/apps", "96x96/apps", "64x64/apps", "48x48/apps", "apps/scalable", "apps/64"];
    for theme in themes() {
        for base in data_dirs().iter().map(|d| d.join("icons").join(&theme)) {
            for size in sizes {
                for ext in ["svg", "png"] {
                    let p = base.join(size).join(format!("{icon}.{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    ["svg", "png", "xpm"].iter().map(|ext| PathBuf::from(format!("/usr/share/pixmaps/{icon}.{ext}"))).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_field_codes() {
        assert_eq!(command("firefox %u"), "firefox");
        assert_eq!(command("env A=1 app --x %F --pct=50%%"), "env A=1 app --x --pct=50%");
    }

    #[test]
    fn reads_the_entry_group_only() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus %U\nIcon=org.gnome.Nautilus\n[Desktop Action new]\nName=New\nExec=nautilus --new";
        let a = parse("org.gnome.Nautilus", text).unwrap();
        assert_eq!((a.name.as_str(), a.exec.as_str(), a.icon.as_str()), ("Files", "nautilus %U", "org.gnome.Nautilus"));
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true").is_none());
    }
}
