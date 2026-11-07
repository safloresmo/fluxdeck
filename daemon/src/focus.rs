//! The focused window on whatever desktop is running: Hyprland, Sway and i3, niri, KDE Plasma,
//! GNOME (with the Focused Window D-Bus extension) and any X11 desktop. Each source calls back
//! with (class, title) on every change and returns when its connection ends.
use fluxdeck_core::paths;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Which source is following the focus, for the editor: a name, or "gnome" when GNOME lacks the
/// extension, or "none".
fn report(source: &str) {
    let _ = std::fs::write(paths::runtime("focus"), source);
}

/// Windows used lately, newest first, one per class, for the editor to offer when making a profile.
fn remember(class: &str, title: &str) {
    if class.is_empty() {
        return;
    }
    let path = paths::runtime("windows");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let title = title.replace(['\t', '\n'], " ");
    let mut lines = vec![format!("{class}\t{title}")];
    lines.extend(old.lines().filter(|l| l.split('\t').next() != Some(class)).take(39).map(String::from));
    let _ = std::fs::write(path, lines.join("\n"));
}

/// A source: calls back on every focus change and returns when its connection ends.
type Source = fn(&mut dyn FnMut(&str, &str)) -> Option<()>;

/// Follows the focus with the first source that works here, until it stops; false when none does.
pub fn follow(on: &mut dyn FnMut(&str, &str)) -> bool {
    let sources: [(&str, Source); 6] =
        [("hyprland", hyprland), ("sway", sway), ("niri", niri), ("kde", kde), ("gnome", gnome), ("x11", x11)];
    for (name, source) in sources {
        if let Some(()) = source(&mut |class, title| {
            report(name);
            remember(class, title);
            on(class, title)
        }) {
            return true;
        }
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
    report(if desktop.contains("gnome") { "gnome-missing" } else { "none" });
    false
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
}

/// The newest file in `dir` whose name passes `matches`: a compositor that restarted leaves old sockets.
fn newest(dir: PathBuf, matches: impl Fn(&str) -> bool) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| matches(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn hyprland(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    let run = runtime_dir()?.join("hypr");
    let from_env = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").map(|s| run.join(s).join(".socket2.sock")).filter(|p| p.exists());
    // The service may start before the session exports the signature: take the newest instance.
    let path = from_env.or_else(|| newest(run.clone(), |_| true).map(|d| d.join(".socket2.sock")).filter(|p| p.exists()))?;
    let stream = UnixStream::connect(path).ok()?;
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        if let Some(rest) = line.strip_prefix("activewindow>>") {
            let (class, title) = rest.split_once(',').unwrap_or((rest, ""));
            on(class, title);
        }
    }
    Some(())
}

/// One i3 IPC message: "i3-ipc", length, type, payload.
fn ipc_send(s: &mut UnixStream, kind: u32, payload: &str) -> Option<()> {
    let mut m = b"i3-ipc".to_vec();
    m.extend((payload.len() as u32).to_ne_bytes());
    m.extend(kind.to_ne_bytes());
    m.extend(payload.as_bytes());
    s.write_all(&m).ok()
}

fn ipc_read(s: &mut UnixStream) -> Option<(u32, Value)> {
    let mut head = [0u8; 14];
    s.read_exact(&mut head).ok()?;
    let len = u32::from_ne_bytes(head[6..10].try_into().ok()?) as usize;
    let kind = u32::from_ne_bytes(head[10..14].try_into().ok()?);
    let mut body = vec![0; len];
    s.read_exact(&mut body).ok()?;
    Some((kind, serde_json::from_slice(&body).ok()?))
}

/// Sway and i3 share one IPC protocol.
fn sway(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    let env = ["SWAYSOCK", "I3SOCK"].iter().find_map(std::env::var_os).map(PathBuf::from).filter(|p| p.exists());
    let path = env.or_else(|| newest(runtime_dir()?, |n| n.starts_with("sway-ipc.") && n.ends_with(".sock")))?;
    sway_at(UnixStream::connect(path).ok()?, on)
}

fn sway_at(mut s: UnixStream, on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    ipc_send(&mut s, 2, r#"["window"]"#)?; // SUBSCRIBE
    ipc_read(&mut s)?;
    while let Some((kind, ev)) = ipc_read(&mut s) {
        // Events have the high bit set; 3 is "window".
        if kind == 0x8000_0003 && ev["change"] == "focus" {
            let c = &ev["container"];
            let class = c["app_id"].as_str().or(c["window_properties"]["class"].as_str()).unwrap_or_default();
            on(class, c["name"].as_str().unwrap_or_default());
        }
    }
    Some(())
}

fn niri(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    let env = std::env::var_os("NIRI_SOCKET").map(PathBuf::from).filter(|p| p.exists());
    let path = env.or_else(|| newest(runtime_dir()?, |n| n.starts_with("niri.") && n.ends_with(".sock")))?;
    niri_at(UnixStream::connect(path).ok()?, on)
}

fn niri_at(mut s: UnixStream, on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    s.write_all(b"\"EventStream\"\n").ok()?;
    let mut windows: HashMap<u64, (String, String)> = HashMap::new();
    let mut focused: Option<u64> = None;
    let read = |w: &Value| (w["app_id"].as_str().unwrap_or_default().to_string(), w["title"].as_str().unwrap_or_default().to_string());
    for line in BufReader::new(s).lines().map_while(Result::ok) {
        let Ok(ev) = serde_json::from_str::<Value>(&line) else { continue };
        let before = focused.and_then(|id| windows.get(&id).cloned());
        if let Some(list) = ev["WindowsChanged"]["windows"].as_array() {
            windows = list.iter().filter_map(|w| Some((w["id"].as_u64()?, read(w)))).collect();
            focused = list.iter().find(|w| w["is_focused"] == true).and_then(|w| w["id"].as_u64());
        } else if let Some(w) = ev["WindowOpenedOrChanged"]["window"].as_object().map(|_| &ev["WindowOpenedOrChanged"]["window"]) {
            let Some(id) = w["id"].as_u64() else { continue };
            windows.insert(id, read(w));
            if w["is_focused"] == true {
                focused = Some(id);
            }
        } else if let Some(id) = ev["WindowClosed"]["id"].as_u64() {
            windows.remove(&id);
        } else if ev.get("WindowFocusChanged").is_some() {
            focused = ev["WindowFocusChanged"]["id"].as_u64();
        } else {
            continue;
        }
        let now = focused.and_then(|id| windows.get(&id).cloned());
        if now != before {
            let (class, title) = now.unwrap_or_default();
            on(&class, &title);
        }
    }
    Some(())
}

const BUS_NAME: &str = "io.github.safloresmo.fluxdeck";

struct KwinFocus(std::sync::mpsc::Sender<(String, String)>);

#[zbus::interface(name = "io.github.safloresmo.fluxdeck.Focus")]
impl KwinFocus {
    fn activated(&self, class: String, title: String) {
        let _ = self.0.send((class, title));
    }
}

/// KWin has no focus feed of its own: a small KWin script calls back over D-Bus.
fn kde(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    let session = zbus::blocking::Connection::session().ok()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&session).ok()?;
    if !dbus.name_has_owner("org.kde.KWin".try_into().ok()?).ok()? {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let _server = zbus::blocking::connection::Builder::session().ok()?.name(BUS_NAME).ok()?.serve_at("/focus", KwinFocus(tx)).ok()?.build().ok()?;
    let script = paths::runtime("focus.js");
    std::fs::write(
        &script,
        format!(
            r#"function send(w) {{ if (w) callDBus("{BUS_NAME}", "/focus", "{BUS_NAME}.Focus", "Activated", String(w.resourceClass), String(w.caption)); }}
if (workspace.windowActivated) workspace.windowActivated.connect(send); else workspace.clientActivated.connect(send);
send(workspace.activeWindow || workspace.activeClient);
"#
        ),
    )
    .ok()?;
    let scripting = zbus::blocking::Proxy::new(&session, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting").ok()?;
    let _: Result<bool, _> = scripting.call("unloadScript", &("fluxdeck-focus",));
    let id: i32 = scripting.call("loadScript", &(script.to_string_lossy().as_ref(), "fluxdeck-focus")).ok()?;
    // Plasma 6 puts the script at /Scripting/ScriptN, Plasma 5 at /N.
    let started = [format!("/Scripting/Script{id}"), format!("/{id}")]
        .iter()
        .any(|path| zbus::blocking::Proxy::new(&session, "org.kde.KWin", path.as_str(), "org.kde.kwin.Script").is_ok_and(|p| p.call::<_, _, ()>("run", &()).is_ok()));
    if !started {
        return None;
    }
    while let Ok((class, title)) = rx.recv() {
        on(&class, &title);
    }
    Some(())
}

/// GNOME on Wayland keeps the focus to itself; the Focused Window D-Bus extension shares it.
fn gnome(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    let session = zbus::blocking::Connection::session().ok()?;
    let proxy = zbus::blocking::Proxy::new(&session, "org.gnome.Shell", "/org/gnome/shell/extensions/FocusedWindow", "org.gnome.shell.extensions.FocusedWindow").ok()?;
    let get = || proxy.call::<_, _, String>("Get", &()).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
    get()?;
    // shortcut: polled once a second; the extension's change signal is not documented.
    let mut last = None;
    loop {
        let now = get().map(|w| (w["wm_class"].as_str().unwrap_or_default().to_string(), w["title"].as_str().unwrap_or_default().to_string()));
        if now != last {
            let (class, title) = now.clone().unwrap_or_default();
            on(&class, &title);
            last = now;
        }
        std::thread::sleep(Duration::from_secs(1));
        // The extension was switched off or GNOME restarted.
        if session.inner().is_closed() {
            return Some(());
        }
    }
}

/// Any X11 desktop (XFCE, Cinnamon, MATE, i3…), through xprop.
fn x11(on: &mut dyn FnMut(&str, &str)) -> Option<()> {
    // Under Wayland, X11 only sees the XWayland windows.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let mut spy = Command::new("xprop").args(["-root", "-spy", "_NET_ACTIVE_WINDOW"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let out = spy.stdout.take()?;
    for line in BufReader::new(out).lines().map_while(Result::ok) {
        let Some(id) = line.rsplit(' ').next().filter(|id| id.starts_with("0x") && *id != "0x0") else { continue };
        let props = Command::new("xprop").args(["-id", id, "WM_CLASS", "_NET_WM_NAME"]).output().ok()?;
        let (class, title) = parse_xprop(&String::from_utf8_lossy(&props.stdout));
        on(&class, &title);
    }
    let _ = spy.wait();
    Some(())
}

/// `WM_CLASS(STRING) = "instance", "Class"` and `_NET_WM_NAME(UTF8_STRING) = "title"`.
fn parse_xprop(out: &str) -> (String, String) {
    let quoted = |line: &str| line.split('"').skip(1).step_by(2).map(String::from).collect::<Vec<_>>();
    let class = out.lines().find(|l| l.starts_with("WM_CLASS")).and_then(|l| quoted(l).pop()).unwrap_or_default();
    let title = out.lines().find(|l| l.starts_with("_NET_WM_NAME")).and_then(|l| quoted(l).first().cloned()).unwrap_or_default();
    (class, title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(f: impl FnOnce(&mut dyn FnMut(&str, &str))) -> Vec<(String, String)> {
        let mut seen = Vec::new();
        f(&mut |c, t| seen.push((c.to_string(), t.to_string())));
        seen
    }

    #[test]
    fn follows_sway() {
        let (mut server, client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let mut head = [0u8; 14];
            server.read_exact(&mut head).unwrap();
            let mut body = vec![0; u32::from_ne_bytes(head[6..10].try_into().unwrap()) as usize];
            server.read_exact(&mut body).unwrap();
            assert_eq!(body, br#"["window"]"#);
            let reply = |kind: u32, json: &str| {
                let mut m = b"i3-ipc".to_vec();
                m.extend((json.len() as u32).to_ne_bytes());
                m.extend(kind.to_ne_bytes());
                m.extend(json.as_bytes());
                m
            };
            server.write_all(&reply(2, r#"{"success":true}"#)).unwrap();
            server.write_all(&reply(0x8000_0003, r#"{"change":"title","container":{"app_id":"x","name":"y"}}"#)).unwrap();
            server.write_all(&reply(0x8000_0003, r#"{"change":"focus","container":{"app_id":"foot","name":"~"}}"#)).unwrap();
            server.write_all(&reply(0x8000_0003, r#"{"change":"focus","container":{"app_id":null,"window_properties":{"class":"Steam"},"name":"Steam"}}"#)).unwrap();
        });
        let seen = collect(|on| drop(sway_at(client, on)));
        assert_eq!(seen, [("foot".into(), "~".into()), ("Steam".into(), "Steam".into())]);
    }

    #[test]
    fn follows_niri() {
        let (mut server, client) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let mut req = [0u8; 14];
            server.read_exact(&mut req).unwrap();
            assert_eq!(&req, b"\"EventStream\"\n");
            let lines = [
                r#"{"Ok":"Handled"}"#,
                r#"{"WindowsChanged":{"windows":[{"id":1,"app_id":"firefox","title":"Web","is_focused":true},{"id":2,"app_id":"kitty","title":"sh","is_focused":false}]}}"#,
                r#"{"WindowFocusChanged":{"id":2}}"#,
                r#"{"WindowOpenedOrChanged":{"window":{"id":2,"app_id":"kitty","title":"vim","is_focused":true}}}"#,
                r#"{"WindowClosed":{"id":2}}"#,
                r#"{"WindowFocusChanged":{"id":null}}"#,
            ];
            for l in lines {
                server.write_all(format!("{l}\n").as_bytes()).unwrap();
            }
        });
        let seen = collect(|on| drop(niri_at(client, on)));
        let pairs: Vec<(String, String)> = [("firefox", "Web"), ("kitty", "sh"), ("kitty", "vim"), ("", "")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(seen, pairs);
    }
    #[test]
    fn reads_xprop() {
        let out = "WM_CLASS(STRING) = \"navigator\", \"firefox\"\n_NET_WM_NAME(UTF8_STRING) = \"Inicio — Mozilla Firefox\"\n";
        assert_eq!(super::parse_xprop(out), ("firefox".into(), "Inicio — Mozilla Firefox".into()));
        assert_eq!(super::parse_xprop("WM_CLASS:  not found.\n"), (String::new(), String::new()));
    }
}
