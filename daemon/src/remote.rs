//! The deck on a phone: a small web server on the local network, on only while switched on. It
//! hands out a page, the deck's current images (the same ones the hardware shows) and takes taps,
//! which the deck session handles exactly like presses on the hardware. Every request must carry
//! the secret key from the QR code and come from a local or private network.
use crate::Msg;
use fluxdeck_core::config::Theme;
use fluxdeck_core::image::{ImageFormat, RgbaImage};
use fluxdeck_core::remote::{self, Remote};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;

const PAGE: &str = include_str!("remote.html");

/// What the phone asks the deck to do.
pub enum Input {
    Key(u8),
    Turn(u8, i8),
    Press(u8),
    Page(usize),
}

#[derive(Default)]
struct View {
    /// Bumped on any change, so waiting phones wake up.
    version: u64,
    layout: Value,
    /// Each slot's PNG and the version it changed at.
    frames: HashMap<String, (u64, Arc<Vec<u8>>)>,
}

static VIEW: LazyLock<(Mutex<View>, Condvar)> = LazyLock::new(Default::default);
static SERVING: AtomicBool = AtomicBool::new(false);
/// Set when serving starts, so the session sends every image once.
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn serving() -> bool {
    SERVING.load(Ordering::Relaxed)
}

pub fn take_refresh() -> bool {
    REFRESH.swap(false, Ordering::Relaxed)
}

/// The deck's shape, page and colours; phones redraw when it changes.
pub fn set_layout(cols: u8, rows: u8, dials: u8, page: usize, pages: Vec<String>, theme: &Theme) {
    let layout = json!({ "cols": cols, "rows": rows, "dials": dials, "page": page, "pages": pages,
        "bg": theme.background, "key": theme.key, "text": theme.text, "accent": theme.accent });
    let (view, wake) = &*VIEW;
    let mut v = view.lock().unwrap();
    if v.layout != layout {
        v.layout = layout;
        v.version += 1;
        wake.notify_all();
    }
}

/// A slot's new image ("k3", "d1").
pub fn set_frame(id: &str, img: &RgbaImage) {
    let mut png = Vec::new();
    if img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png).is_err() {
        return;
    }
    let (view, wake) = &*VIEW;
    let mut v = view.lock().unwrap();
    v.version += 1;
    let version = v.version;
    v.frames.insert(id.to_string(), (version, Arc::new(png)));
    wake.notify_all();
}

/// Serves while remote.json says so; checks once a second.
pub fn start(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut listener: Option<(TcpListener, Remote)> = None;
        loop {
            if !remote::enabled() {
                if listener.take().is_some() {
                    SERVING.store(false, Ordering::Relaxed);
                    eprintln!("fluxdeck: deck on the phone off");
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let want = Remote::load();
            let same = listener.as_ref().is_some_and(|(_, r)| r.token == want.token && r.port == want.port);
            if (!want.on || !same) && listener.take().is_some() {
                SERVING.store(false, Ordering::Relaxed);
                eprintln!("fluxdeck: deck on the phone off");
            }
            if want.on && listener.is_none() {
                listener = bind(want);
            }
            if let Some((l, r)) = &listener {
                // Accept for a second, then look at the settings again.
                for _ in 0..20 {
                    match l.accept() {
                        Ok((stream, peer)) if remote::local_peer(peer.ip()) => {
                            let (tx, token) = (tx.clone(), r.token.clone());
                            std::thread::spawn(move || handle(stream, &token, &tx));
                        }
                        Ok(_) => {}
                        Err(_) => std::thread::sleep(Duration::from_millis(50)),
                    }
                }
            } else {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

/// Listens on the saved port, or the next free one (saved for the QR code).
fn bind(mut r: Remote) -> Option<(TcpListener, Remote)> {
    let first = r.port;
    for port in first..first.saturating_add(20) {
        if let Ok(l) = TcpListener::bind(("0.0.0.0", port)) {
            let _ = l.set_nonblocking(true);
            if port != r.port {
                r.port = port;
                r.save();
            }
            SERVING.store(true, Ordering::Relaxed);
            REFRESH.store(true, Ordering::Relaxed);
            eprintln!("fluxdeck: deck on the phone at port {port}");
            return Some((l, r));
        }
    }
    eprintln!("fluxdeck: no free port for the deck on the phone");
    None
}

fn handle(stream: TcpStream, token: &str, tx: &Sender<Msg>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().unwrap_or(0).min(4096);
        }
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let q: HashMap<&str, &str> = query.split('&').filter_map(|p| p.split_once('=')).collect();
    let mut out = &stream;
    if !remote::same_key(q.get("k").copied().unwrap_or(""), token) {
        let body = "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><body style='font:16px sans-serif;background:#111;color:#eee;padding:2em'>Esta dirección necesita la clave del QR. Vuelve a escanearlo desde FluxDeck.";
        reply(&mut out, "403 Forbidden", "text/html; charset=utf-8", body.as_bytes());
        return;
    }
    match (method, path) {
        ("GET", "/") => reply(&mut out, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()),
        ("GET", "/manifest.webmanifest") => {
            let m = json!({ "name": "FluxDeck", "short_name": "FluxDeck", "display": "fullscreen", "start_url": format!("/?k={token}"), "background_color": "#101014", "theme_color": "#101014" });
            reply(&mut out, "200 OK", "application/manifest+json", m.to_string().as_bytes());
        }
        ("GET", "/state") => {
            let seen: u64 = q.get("v").and_then(|v| v.parse().ok()).unwrap_or(0);
            let (view, wake) = &*VIEW;
            // Long poll: answer at once if something changed, else when it does (or after 25 s).
            let guard = view.lock().unwrap();
            let (v, _) = wake.wait_timeout_while(guard, Duration::from_secs(25), |v| v.version == seen).unwrap();
            let frames: serde_json::Map<String, Value> = v.frames.iter().map(|(id, (ver, _))| (id.clone(), json!(ver))).collect();
            let body = json!({ "version": v.version, "layout": v.layout, "frames": frames });
            drop(v);
            reply(&mut out, "200 OK", "application/json", body.to_string().as_bytes());
        }
        ("GET", "/img") => {
            let frame = VIEW.0.lock().unwrap().frames.get(q.get("id").copied().unwrap_or("")).map(|(_, png)| png.clone());
            match frame {
                Some(png) => reply(&mut out, "200 OK", "image/png", &png),
                None => reply(&mut out, "404 Not Found", "text/plain", b""),
            }
        }
        ("POST", "/input") => {
            let mut body = vec![0u8; length];
            if reader.read_exact(&mut body).is_ok()
                && let Some(input) = serde_json::from_slice::<Value>(&body).ok().as_ref().and_then(parse_input)
            {
                let _ = tx.send(Msg::Remote(input));
            }
            reply(&mut out, "204 No Content", "text/plain", b"");
        }
        _ => reply(&mut out, "404 Not Found", "text/plain", b""),
    }
}

/// `{"t":"key","i":3}`, `{"t":"turn","i":1,"d":-1}`, `{"t":"press","i":0}`, `{"t":"page","i":2}`.
fn parse_input(v: &Value) -> Option<Input> {
    let i = v["i"].as_u64()?;
    Some(match v["t"].as_str()? {
        "key" => Input::Key(u8::try_from(i).ok()?),
        "turn" => Input::Turn(u8::try_from(i).ok()?, v["d"].as_i64()?.clamp(-10, 10) as i8),
        "press" => Input::Press(u8::try_from(i).ok()?),
        "page" => Input::Page(i as usize),
        _ => return None,
    })
}

fn reply(out: &mut &TcpStream, status: &str, kind: &str, body: &[u8]) {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    let _ = out.write_all(head.as_bytes()).and_then(|_| out.write_all(body));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_taps_and_drops_nonsense() {
        assert!(matches!(parse_input(&json!({"t": "key", "i": 3})), Some(Input::Key(3))));
        assert!(matches!(parse_input(&json!({"t": "turn", "i": 1, "d": -50})), Some(Input::Turn(1, -10))));
        assert!(matches!(parse_input(&json!({"t": "page", "i": 2})), Some(Input::Page(2))));
        assert!(parse_input(&json!({"t": "key", "i": 900})).is_none());
        assert!(parse_input(&json!({"t": "rm", "i": 1})).is_none());
        assert!(parse_input(&json!({"t": "key"})).is_none());
    }
}
