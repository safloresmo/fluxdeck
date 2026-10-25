//! Where the program keeps its files. Every folder and runtime file name comes from NAME.
use std::path::PathBuf;

/// The program's name in paths, binaries and the editor's app id.
pub const NAME: &str = "fluxdeck";

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
        .unwrap_or_default()
        .join(NAME)
}

/// `~/.config/<name>`: config.toml, theme.toml, plugin settings.
pub fn config() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// `~/.local/share/<name>`: installed plugins.
pub fn data() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// `~/.local/state/<name>`: plugin logs.
pub fn state() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

/// `~/.cache/<name>`: catalogues and icons.
pub fn cache() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_RUNTIME_DIR/<name>-<file>`: files shared between the service and the editor while they run.
pub fn runtime(file: &str) -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(format!("{NAME}-{file}"))
}

/// True when the service is up: it answers on its plugin port.
pub fn service_running() -> bool {
    let Some(port) = std::fs::read_to_string(runtime("plugins.port")).ok().and_then(|s| s.trim().parse::<u16>().ok()) else { return false };
    std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), std::time::Duration::from_millis(150)).is_ok()
}

fn systemctl(args: &[&str]) -> bool {
    std::process::Command::new("systemctl").arg("--user").args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
}

/// The systemd user unit is installed (dist/install.sh puts it there).
pub fn service_installed() -> bool {
    systemctl(&["cat", &format!("{NAME}.service")])
}

/// Whether the service starts with the session.
pub fn autostart() -> bool {
    systemctl(&["is-enabled", "--quiet", &format!("{NAME}.service")])
}

pub fn set_autostart(on: bool) -> bool {
    systemctl(&[if on { "enable" } else { "disable" }, &format!("{NAME}.service")])
}

/// How the service starts with the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Startup {
    /// The systemd user unit, enabled.
    Systemd,
    /// The desktop's own startup list (Ryoku's Sesión → Al iniciar, or an XDG autostart entry).
    Desktop,
    /// Nothing starts it; the editor or the user does.
    Manual,
}

/// True when the desktop's startup list runs the service: Ryoku's list or ~/.config/autostart.
pub fn in_desktop_startup() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let ryoku = std::fs::read_to_string(config.join("ryoku/desktop.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let in_ryoku = ryoku.is_some_and(|v| {
        v["desktop"]["autostart"].as_array().into_iter().flatten().any(|e| {
            let cmd = e["command"].as_str().unwrap_or_default();
            cmd.split_whitespace().next().is_some_and(|p| p.rsplit('/').next() == Some(NAME))
        })
    });
    in_ryoku || autostart_entry().is_file()
}

pub fn startup() -> Startup {
    if autostart() {
        Startup::Systemd
    } else if in_desktop_startup() {
        Startup::Desktop
    } else {
        Startup::Manual
    }
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".config"))
}

/// Ryoku keeps its own startup list, changed only from its Hub.
pub fn ryoku_desktop() -> bool {
    config_home().join("ryoku/desktop.json").is_file()
}

fn autostart_entry() -> PathBuf {
    config_home().join("autostart").join(format!("{NAME}.desktop"))
}

/// Switches how the service starts. On desktops that read ~/.config/autostart (GNOME, KDE, XFCE…)
/// the "desktop" choice writes that entry; on Ryoku the user adds it in the Hub.
pub fn set_startup(s: Startup) -> bool {
    let systemd_ok = set_autostart(s == Startup::Systemd) || s != Startup::Systemd;
    let entry = autostart_entry();
    if s == Startup::Desktop && !ryoku_desktop() {
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=FluxDeck\nComment=Stream Deck, audio y periféricos\nExec={}\nIcon={NAME}\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            service_bin().display()
        );
        let written = std::fs::create_dir_all(config_home().join("autostart")).and_then(|_| std::fs::write(&entry, text)).is_ok();
        return systemd_ok && written;
    }
    // Leaving the desktop choice takes back only the entry this program wrote.
    if s != Startup::Desktop && entry.is_file() {
        let _ = std::fs::remove_file(&entry);
    }
    systemd_ok
}

/// Starts the service: through systemd when its unit is installed, else as a detached process.
pub fn start_service() -> Result<(), String> {
    let unit = format!("{NAME}.service");
    let has_unit = std::process::Command::new("systemctl").args(["--user", "cat", &unit]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    if has_unit {
        let ok = std::process::Command::new("systemctl").args(["--user", "start", &unit]).status().is_ok_and(|s| s.success());
        return if ok { Ok(()) } else { Err(format!("systemctl --user start {unit}")) };
    }
    use std::os::unix::process::CommandExt;
    std::process::Command::new(service_bin())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // Its own process group, so it outlives the editor.
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The service binary next to the running one, or on PATH.
pub fn service_bin() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(NAME))).filter(|p| p.is_file()).unwrap_or_else(|| NAME.into())
}
