//! Corsair devices through the local OpenLinkHub service (https://github.com/jurkovic-nikola/OpenLinkHub).
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const ADDR: &str = "127.0.0.1:27003";

#[derive(Debug, Clone)]
pub struct Device {
    pub serial: String,
    pub product: String,
    pub battery: bool,
    /// OpenLinkHub's kind: 0 keyboard, 1 mouse, 2 headset.
    pub kind: Option<u64>,
    pub anc: bool,
    pub lights: bool,
}

fn http(method: &str, path: &str, body: Option<Value>) -> Option<Value> {
    let mut s = TcpStream::connect_timeout(&ADDR.parse().ok()?, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    write!(s, "{method} {path} HTTP/1.0\r\nHost: {ADDR}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    serde_json::from_str(out.split_once("\r\n\r\n")?.1).ok()
}

fn post(path: &str, body: Value) -> Result<(), String> {
    let r = http("POST", path, Some(body)).ok_or("OpenLinkHub is not running")?;
    if r["status"] == 1 { Ok(()) } else { Err(r["message"].as_str().unwrap_or("OpenLinkHub refused the change").to_string()) }
}

/// The device's saved settings: NoiseCancellation, Brightness, RGBProfile…
pub fn profile(serial: &str) -> Option<Value> {
    let mut d = http("GET", &format!("/api/devices/{serial}"), None)?;
    Some(d["device"]["DeviceProfile"].take()).filter(|p| p.is_object())
}

fn effects_of(device: &Value) -> Vec<String> {
    device["Rgb"]["profiles"].as_object().map(|m| m.keys().filter(|k| !k.ends_with("temperature")).cloned().collect()).unwrap_or_default()
}

/// Light effects the device offers, sorted by name.
pub fn effects(serial: &str) -> Vec<String> {
    effects_of(&http("GET", &format!("/api/devices/{serial}"), None).unwrap_or_default()["device"])
}

pub fn running() -> bool {
    http("GET", "/api/devices/", None).is_some()
}

/// What a device is set to right now, read in one go.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub battery: Option<u64>,
    pub anc: Option<i64>,
    /// Index in LEVELS.
    pub brightness: Option<usize>,
    pub effect: Option<String>,
    pub effects: Vec<String>,
}

pub fn live(serial: &str) -> Live {
    let d = http("GET", &format!("/api/devices/{serial}"), None).unwrap_or_default();
    let (d, p) = (&d["device"], &d["device"]["DeviceProfile"]);
    Live {
        battery: battery(serial),
        anc: p["NoiseCancellation"].as_i64(),
        brightness: p["Brightness"].as_u64().map(level_index),
        effect: p["RGBProfile"].as_str().map(String::from),
        effects: effects_of(d),
    }
}

/// (product, kind, level) of every device that reports a battery, in one request.
pub fn batteries() -> Vec<(String, Option<u64>, u64)> {
    let Some(v) = http("GET", "/api/batteryStats", None) else { return Vec::new() };
    let mut out: Vec<_> = v["data"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, d)| Some((d["Device"].as_str()?.to_string(), d["DeviceType"].as_u64(), d["Level"].as_u64()?)))
        .collect();
    out.sort();
    out
}

pub fn battery(serial: &str) -> Option<u64> {
    http("GET", "/api/batteryStats", None)?["data"][serial]["Level"].as_u64()
}

/// Visible devices; empty when OpenLinkHub is not running.
pub fn devices() -> Vec<Device> {
    let Some(list) = http("GET", "/api/devices/", None) else { return Vec::new() };
    let batteries = http("GET", "/api/batteryStats", None).unwrap_or_default();
    let mut out: Vec<Device> = list["devices"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, d)| d["Hidden"] == false)
        .map(|(serial, d)| {
            let p = profile(serial).unwrap_or_default();
            Device {
                serial: serial.clone(),
                product: d["Product"].as_str().unwrap_or(serial).to_string(),
                battery: batteries["data"][serial]["Level"].is_u64(),
                kind: batteries["data"][serial]["DeviceType"].as_u64(),
                anc: p["NoiseCancellation"].is_i64(),
                lights: p["RGBProfile"].is_string(),
            }
        })
        .collect();
    out.sort_by(|a, b| a.product.cmp(&b.product));
    out
}

/// Nerd Font glyph for the kind of device.
pub fn icon(kind: Option<u64>) -> char {
    match kind {
        Some(0) => '\u{f030c}',
        Some(1) => '\u{f037d}',
        Some(2) => '\u{f02ce}',
        _ => '\u{f0079}',
    }
}

/// 0 off, 1 on, 2 transparency.
pub fn set_anc(serial: &str, mode: i64) -> Result<(), String> {
    post("/api/headset/anc", json!({ "deviceId": serial, "noiseCancellation": mode }))
}

/// OpenLinkHub modes: 0 follows the effect (full), 1 33 %, 2 66 %, 3 100 %, 4 off.
pub fn set_brightness(serial: &str, mode: u8) -> Result<(), String> {
    post("/api/brightness", json!({ "deviceId": serial, "brightness": mode }))
}

pub fn set_effect(serial: &str, effect: &str) -> Result<(), String> {
    post("/api/color", json!({ "deviceId": serial, "channelId": 0, "profile": effect }))
}

/// Brightness modes from dark to bright, with their percentage.
pub const LEVELS: [(u8, u64); 4] = [(4, 0), (1, 33), (2, 66), (3, 100)];

/// Position of a brightness mode in LEVELS.
pub fn level_index(mode: u64) -> usize {
    match mode {
        4 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_round_trip() {
        for (i, (mode, _)) in LEVELS.iter().enumerate() {
            assert_eq!(level_index(*mode as u64), i);
        }
        assert_eq!(level_index(0), 3);
    }
}
