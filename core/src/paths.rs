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
