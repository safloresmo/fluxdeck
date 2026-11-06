//! Runs Stream Deck SDK plugins and speaks their WebSocket protocol.
//! Plugins and their property inspectors (settings pages, opened in the browser) connect to `port`;
//! the inspectors' files are served over HTTP on `port + 1`.
use crate::Msg;
use fluxdeck_core::plugins::{self, Plugin, Runtime};
use fluxdeck_core::Config;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tungstenite::Message;

/// Where a placed plugin action sits, for building event payloads.
#[derive(Clone, Debug)]
pub struct Placed {
    pub action: String,
    pub plugin: String,
    pub dial: bool,
    pub pos: u8,
}

/// The deck as plugins see it.
#[derive(Clone, Debug, Default)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub kind: u8,
    pub cols: u8,
    pub rows: u8,
}

struct Shared {
    port: u16,
    /// Open sockets: "p:<plugin uuid>" or "i:<instance>" for an open property inspector.
    /// Each with a connection number, so a stale connection cannot drop a newer one.
    sockets: Mutex<HashMap<String, (u64, Sender<String>)>>,
    placed: Mutex<HashMap<String, Placed>>,
    device: Mutex<Device>,
    settings: Mutex<Map<String, Value>>,
    plugins: Mutex<Vec<Plugin>>,
    children: Mutex<Vec<Child>>,
    /// Plugins already told about the current deck.
    greeted: Mutex<std::collections::HashSet<String>>,
    tx: Sender<Msg>,
}

#[derive(Clone)]
pub struct Host(Arc<Shared>);

pub fn port_file() -> PathBuf {
    fluxdeck_core::paths::runtime("plugins.port")
}

impl Host {
    pub fn start(tx: Sender<Msg>) -> Option<Host> {
        let (ws, http, port) = (28196..28296).find_map(|p| Some((TcpListener::bind(("127.0.0.1", p)).ok()?, TcpListener::bind(("127.0.0.1", p + 1)).ok()?, p)))?;
        let _ = std::fs::write(port_file(), port.to_string());
        let host = Host(Arc::new(Shared {
            port,
            sockets: Mutex::new(HashMap::new()),
            placed: Mutex::new(HashMap::new()),
            device: Mutex::new(Device::default()),
            settings: Mutex::new(plugins::load_settings()),
            plugins: Mutex::new(Vec::new()),
            children: Mutex::new(Vec::new()),
            greeted: Mutex::new(Default::default()),
            tx,
        }));
        let h = host.clone();
        thread::spawn(move || {
            for s in ws.incoming().flatten() {
                let h = h.clone();
                thread::spawn(move || h.socket(s));
            }
        });
        let h = host.clone();
        thread::spawn(move || {
            for s in http.incoming().flatten() {
                let h = h.clone();
                thread::spawn(move || h.http(s));
            }
        });
        host.launch();
        Some(host)
    }

    /// (Re)starts every installed plugin that can run here.
    pub fn launch(&self) {
        self.stop();
        self.0.greeted.lock().unwrap().clear();
        let all = plugins::enabled();
        let logs = fluxdeck_core::paths::state().join("plugins");
        let _ = std::fs::create_dir_all(&logs);
        let mut children = self.0.children.lock().unwrap();
        for p in &all {
            match self.spawn(p, &logs) {
                Ok(c) => children.push(c),
                Err(e) => eprintln!("fluxdeck: plugin {} not started: {e}", p.uuid),
            }
        }
        *self.0.plugins.lock().unwrap() = all;
    }

    fn spawn(&self, p: &Plugin, logs: &Path) -> Result<Child, String> {
        let mut cmd = match p.runtime() {
            Runtime::Native(code) => {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&code, std::fs::Permissions::from_mode(0o755));
                Command::new(code)
            }
            Runtime::Node(code) => {
                let mut c = Command::new("node");
                c.arg(code);
                c
            }
            Runtime::Wine(code) => {
                let mut c = Command::new("wine");
                c.arg(code);
                c
            }
            Runtime::Unsupported(why) => return Err(format!("unsupported ({why})")),
        };
        let info = self.info(p);
        let log = std::fs::File::create(logs.join(format!("{}.log", p.uuid))).map_err(|e| e.to_string())?;
        cmd.current_dir(&p.path)
            .args(["-port", &self.0.port.to_string(), "-pluginUUID", &p.uuid, "-registerEvent", "registerPlugin", "-info", &info.to_string()])
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        // Plugins die with the daemon instead of lingering.
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }
        cmd.spawn().map_err(|e| e.to_string())
    }

    pub fn stop(&self) {
        for mut c in self.0.children.lock().unwrap().drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn info(&self, p: &Plugin) -> Value {
        let d = self.0.device.lock().unwrap().clone();
        let lang = if fluxdeck_core::i18n::english() { "en" } else { "es" };
        json!({
            "application": { "font": "sans-serif", "language": lang, "platform": "linux", "platformVersion": "", "version": "6.8.0" },
            "plugin": { "uuid": p.uuid, "version": p.manifest.version },
            "devicePixelRatio": 2,
            "colors": {
                "buttonPressedBackgroundColor": "#303030FF", "buttonPressedBorderColor": "#646464FF", "buttonPressedTextColor": "#969696FF",
                "disabledColor": "#F7821B59", "highlightColor": "#F7821BFF", "mouseDownColor": "#CF6304FF"
            },
            "devices": if d.id.is_empty() { json!([]) } else { json!([{ "id": d.id, "name": d.name, "type": d.kind, "size": { "columns": d.cols, "rows": d.rows } }]) },
        })
    }

    /// Records where plugin actions sit; called when the layout loads or changes.
    pub fn set_layout(&self, cfg: &Config) {
        let plugins = self.0.plugins.lock().unwrap();
        let mut placed = HashMap::new();
        for page in &cfg.pages {
            let keys = page.keys.iter().map(|k| (&k.action, &k.instance, false, k.pos));
            let dials = page.dials.iter().map(|d| (&d.action, &d.instance, true, d.pos));
            for (action, instance, dial, pos) in keys.chain(dials) {
                if action.is_empty() || instance.is_empty() {
                    continue;
                }
                let plugin = plugins::find_action(&plugins, action).map(|(p, _)| p.uuid.clone()).unwrap_or_default();
                placed.insert(instance.clone(), Placed { action: action.clone(), plugin, dial, pos });
            }
        }
        *self.0.placed.lock().unwrap() = placed;
    }

    pub fn placed(&self, instance: &str) -> Option<Placed> {
        self.0.placed.lock().unwrap().get(instance).cloned()
    }

    pub fn set_device(&self, d: Device) {
        *self.0.device.lock().unwrap() = d;
        self.0.greeted.lock().unwrap().clear();
        let uuids: Vec<String> = self.0.plugins.lock().unwrap().iter().map(|p| p.uuid.clone()).collect();
        for u in uuids {
            self.hello(&u);
        }
    }

    fn send(&self, to: &str, v: &Value) {
        if debug() {
            eprintln!("fluxdeck: → {to} {}", cut(&v.to_string()));
        }
        if let Some((_, s)) = self.0.sockets.lock().unwrap().get(to) {
            let _ = s.send(v.to_string());
        }
    }

    fn settings_of(&self, key: &str) -> Value {
        self.0.settings.lock().unwrap().get(key).cloned().unwrap_or_else(|| json!({}))
    }

    fn store(&self, key: &str, v: Value) {
        let mut all = self.0.settings.lock().unwrap();
        all.insert(key.to_string(), v);
        plugins::save_settings(&all);
    }

    fn coordinates(&self, p: &Placed) -> Value {
        let cols = self.0.device.lock().unwrap().cols.max(1);
        if p.dial { json!({ "column": p.pos, "row": 0 }) } else { json!({ "column": p.pos % cols, "row": p.pos / cols }) }
    }

    /// Sends an action event (keyDown, willAppear, dialRotate…) with the payload fields every event carries.
    pub fn event(&self, event: &str, instance: &str, extra: Value) {
        let Some(p) = self.placed(instance) else { return };
        let mut payload = json!({
            "settings": self.settings_of(instance),
            "coordinates": self.coordinates(&p),
            "controller": if p.dial { "Encoder" } else { "Keypad" },
            "isInMultiAction": false,
        });
        if let (Some(m), Value::Object(e)) = (payload.as_object_mut(), extra) {
            m.extend(e);
        }
        let device = self.0.device.lock().unwrap().id.clone();
        self.send(&format!("p:{}", p.plugin), &json!({ "event": event, "action": p.action, "context": instance, "device": device, "payload": payload }));
    }

    fn did_receive_settings(&self, to: &str, instance: &str) {
        let Some(p) = self.placed(instance) else { return };
        let device = self.0.device.lock().unwrap().id.clone();
        let payload = json!({ "settings": self.settings_of(instance), "coordinates": self.coordinates(&p), "isInMultiAction": false });
        self.send(to, &json!({ "event": "didReceiveSettings", "action": p.action, "context": instance, "device": device, "payload": payload }));
    }

    fn global(&self, plugin: &str) -> Value {
        json!({ "event": "didReceiveGlobalSettings", "payload": { "settings": self.settings_of(&format!("global:{plugin}")) } })
    }

    /// Open inspectors that belong to `plugin`.
    fn inspectors_of(&self, plugin: &str) -> Vec<String> {
        let placed = self.0.placed.lock().unwrap();
        let sockets = self.0.sockets.lock().unwrap();
        sockets.keys().filter_map(|k| k.strip_prefix("i:")).filter(|i| placed.get(*i).is_some_and(|p| p.plugin == plugin)).map(|i| format!("i:{i}")).collect()
    }

    fn handle(&self, from: &str, v: Value) {
        if debug() {
            eprintln!("fluxdeck: ← {from} {}", cut(&v.to_string()));
        }
        let event = v["event"].as_str().unwrap_or_default().to_string();
        let ctx = v["context"].as_str().unwrap_or_default().to_string();
        let payload = v["payload"].clone();
        let inspector = from.starts_with("i:");
        let plugin = if inspector { self.placed(&from[2..]).map(|p| p.plugin).unwrap_or_default() } else { from[2..].to_string() };
        match event.as_str() {
            "setSettings" => {
                let instance = if inspector { from[2..].to_string() } else { ctx };
                self.store(&instance, payload);
                // The other side learns about the change.
                let other = if inspector { format!("p:{plugin}") } else { format!("i:{instance}") };
                self.did_receive_settings(&other, &instance);
            }
            "getSettings" => {
                let instance = if inspector { from[2..].to_string() } else { ctx };
                self.did_receive_settings(from, &instance);
            }
            "setGlobalSettings" => {
                self.store(&format!("global:{plugin}"), payload);
                let msg = self.global(&plugin);
                let mut to = self.inspectors_of(&plugin);
                to.push(format!("p:{plugin}"));
                for t in to.iter().filter(|t| *t != from) {
                    self.send(t, &msg);
                }
            }
            "getGlobalSettings" => self.send(from, &self.global(&plugin)),
            "openUrl" => {
                if let Some(url) = payload["url"].as_str().filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
                    let _ = Command::new("xdg-open").arg(url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                }
            }
            "logMessage" => eprintln!("fluxdeck: [{plugin}] {}", payload["message"].as_str().unwrap_or_default()),
            "sendToPropertyInspector" => {
                let action = self.placed(&ctx).map(|p| p.action).unwrap_or_default();
                self.send(&format!("i:{ctx}"), &json!({ "event": "sendToPropertyInspector", "action": action, "context": ctx, "payload": payload }));
            }
            "sendToPlugin" if inspector => {
                let instance = &from[2..];
                let action = self.placed(instance).map(|p| p.action).unwrap_or_default();
                self.send(&format!("p:{plugin}"), &json!({ "event": "sendToPlugin", "action": action, "context": instance, "payload": payload }));
            }
            "setTitle" | "setImage" | "setState" | "setFeedback" | "setFeedbackLayout" | "showOk" | "showAlert" if !inspector => {
                let _ = self.0.tx.send(Msg::Plugin(v));
            }
            _ => {}
        }
    }

    fn socket(&self, stream: TcpStream) {
        // Plugins send no Origin; inspectors come from our own pages. Other websites are turned away.
        let ours = format!("http://127.0.0.1:{}", self.0.port + 1);
        let check = |req: &tungstenite::handshake::server::Request, res| match req.headers().get("origin").and_then(|o| o.to_str().ok()) {
            None => Ok(res),
            Some(o) if o == ours => Ok(res),
            Some(_) => Err(tungstenite::http::Response::builder().status(403).body(None).unwrap_or_default()),
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { return };
        let who = match ws.read() {
            Ok(Message::Text(t)) => serde_json::from_str::<Value>(&t).unwrap_or_default(),
            _ => return,
        };
        let uuid = who["uuid"].as_str().unwrap_or_default().to_string();
        let key = match who["event"].as_str() {
            Some("registerPlugin") => format!("p:{uuid}"),
            Some("registerPropertyInspector") => format!("i:{uuid}"),
            _ => return,
        };
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = channel::<String>();
        self.0.sockets.lock().unwrap().insert(key.clone(), (id, tx));
        // The session answers both: it knows which actions are on screen.
        let _ = self.0.tx.send(if key.starts_with("p:") { Msg::PluginReady(uuid.clone()) } else { Msg::Inspector(uuid.clone(), true) });
        let _ = ws.get_mut().set_read_timeout(Some(Duration::from_millis(20)));
        'conn: loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                        self.handle(&key, v);
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
            for m in rx.try_iter() {
                if ws.send(Message::text(m)).is_err() {
                    break 'conn;
                }
            }
        }
        {
            let mut sockets = self.0.sockets.lock().unwrap();
            if sockets.get(&key).is_some_and(|(n, _)| *n == id) {
                sockets.remove(&key);
            }
        }
        if key.starts_with("i:") {
            let _ = self.0.tx.send(Msg::Inspector(uuid, false));
        }
    }

    /// Serves `/pi/<instance>` (an action's settings page, wired to this host) and `/files/<plugin>/<path>`.
    fn http(&self, mut s: TcpStream) {
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
        let path = plugins::percent(&path);
        let (status, mime, body) = self.route(&path).unwrap_or((404, "text/plain", b"not found".to_vec()));
        let head = format!("HTTP/1.1 {status} OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
        let _ = s.write_all(head.as_bytes());
        let _ = s.write_all(&body);
    }

    fn route(&self, path: &str) -> Option<(u16, &'static str, Vec<u8>)> {
        let plugins = self.0.plugins.lock().unwrap().clone();
        if let Some(instance) = path.strip_prefix("/pi/") {
            let p = self.placed(instance)?;
            let (plugin, action) = plugins::find_action(&plugins, &p.action)?;
            let page = plugin.inspector(action)?;
            let html = std::fs::read_to_string(&page).ok()?;
            let rel = page.parent()?.strip_prefix(&plugin.path).ok()?.to_string_lossy().to_string();
            let base = format!("/files/{}/{}{}", plugin.uuid, rel, if rel.is_empty() { "" } else { "/" });
            return Some((200, "text/html; charset=utf-8", self.wire(&html, &base, instance, plugin, &p).into_bytes()));
        }
        let rest = path.strip_prefix("/files/")?;
        let (uuid, file) = rest.split_once('/')?;
        let plugin = plugins.iter().find(|p| p.uuid == uuid)?;
        let full = plugin.path.join(file).canonicalize().ok()?;
        if !full.starts_with(plugin.path.canonicalize().ok()?) {
            return Some((403, "text/plain", b"forbidden".to_vec()));
        }
        Some((200, mime(&full), std::fs::read(&full).ok()?))
    }

    /// The inspector page with a <base> for its relative files and a script that connects it here.
    fn wire(&self, html: &str, base: &str, instance: &str, plugin: &Plugin, p: &Placed) -> String {
        let device = self.0.device.lock().unwrap().id.clone();
        let action_info = json!({
            "action": p.action, "context": instance, "device": device,
            "payload": { "settings": self.settings_of(instance), "coordinates": self.coordinates(p) }
        });
        let args = json!([self.0.port, instance, "registerPropertyInspector", self.info(plugin).to_string(), action_info.to_string()]);
        let script = format!(
            r#"<script>(function(){{const a={args};function go(){{const f=window.connectOpenActionSocket||window.connectElgatoStreamDeckSocket;if(typeof f==="function")f(...a);else setTimeout(go,50)}}if(document.readyState==="complete")go();else addEventListener("load",go)}})();</script>"#
        );
        let style = "<style>html{background:#2d2d2d;color-scheme:dark}body{max-width:420px;margin:24px auto}</style>";
        let head = format!("<base href=\"{base}\">{style}");
        // ASCII only: full lowercasing can change byte lengths and shift the indexes used on `html`.
        let lower = html.to_ascii_lowercase();
        let mut out = match lower.find("<head>") {
            Some(i) => format!("{}{head}{}", &html[..i + 6], &html[i + 6..]),
            None => format!("{head}{html}"),
        };
        match out.to_ascii_lowercase().rfind("</body>") {
            Some(i) => out.insert_str(i, &script),
            None => out.push_str(&script),
        }
        out
    }

    pub fn plugin_of(&self, action: &str) -> Option<(Plugin, plugins::ActionDef)> {
        let plugins = self.0.plugins.lock().unwrap();
        plugins::find_action(&plugins, action).map(|(p, a)| (p.clone(), a.clone()))
    }

    /// Sends deviceDidConnect to a plugin, once per device.
    pub fn hello(&self, plugin: &str) {
        let d = self.0.device.lock().unwrap().clone();
        let connected = self.0.sockets.lock().unwrap().contains_key(&format!("p:{plugin}"));
        if !d.id.is_empty() && connected && self.0.greeted.lock().unwrap().insert(plugin.to_string()) {
            let v = json!({ "event": "deviceDidConnect", "device": d.id, "deviceInfo": { "name": d.name, "type": d.kind, "size": { "columns": d.cols, "rows": d.rows } } });
            self.send(&format!("p:{plugin}"), &v);
        }
    }
}

/// FLUXDECK_DEBUG=1 logs every plugin message.
fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("FLUXDECK_DEBUG").is_some())
}

fn cut(s: &str) -> String {
    if s.len() > 300 { format!("{}…", &s[..s.floor_char_boundary(300)]) } else { s.to_string() }
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}
