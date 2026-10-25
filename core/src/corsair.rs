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
    /// Has a screen (Nautilus LCD Cap and pump LCDs).
    pub lcd: bool,
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
    /// Screen modes, the mode in use and the rotation, for devices with a screen.
    pub lcd: Option<(Vec<(u8, String)>, u8, u8)>,
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
        lcd: (d["HasLCD"] == true).then(|| lcd(serial)).flatten(),
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
    let has_lcd = |serial: &str| http("GET", &format!("/api/devices/{serial}"), None).is_some_and(|d| d["device"]["HasLCD"] == true);
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
                lcd: has_lcd(serial),
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

/// A screen's modes (number, OpenLinkHub's name), sorted, and the one in use.
pub fn lcd(serial: &str) -> Option<(Vec<(u8, String)>, u8, u8)> {
    let d = http("GET", &format!("/api/devices/{serial}"), None)?;
    let d = &d["device"];
    let mut modes: Vec<(u8, String)> = d["LCDModes"].as_object()?.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_str()?.to_string()))).collect();
    modes.sort();
    let p = &d["DeviceProfile"];
    Some((modes, p["LCDMode"].as_u64().unwrap_or(0) as u8, p["LCDRotation"].as_u64().unwrap_or(0) as u8))
}

pub fn set_lcd(serial: &str, mode: u8) -> Result<(), String> {
    post("/api/lcd", json!({ "deviceId": serial, "channelId": 0, "mode": mode }))
}

/// 0, 1, 2 or 3 quarter turns.
pub fn set_lcd_rotation(serial: &str, rotation: u8) -> Result<(), String> {
    post("/api/lcd/rotation", json!({ "deviceId": serial, "channelId": 0, "rotation": rotation }))
}

/// Keyboard keys with a G name (G1, G2…), by their id in the keyboard's active layout.
pub fn g_keys(serial: &str) -> Vec<(String, u64, bool)> {
    let Some(d) = http("GET", &format!("/api/devices/{serial}"), None) else { return Vec::new() };
    let profile = &d["device"]["DeviceProfile"];
    let layout = profile["Profile"].as_str().unwrap_or("default");
    let rows = profile["Keyboards"][layout]["row"].as_object().cloned().unwrap_or_default();
    let mut out: Vec<(String, u64, bool)> = rows
        .values()
        .flat_map(|r| r["keys"].as_object().cloned().unwrap_or_default())
        .filter_map(|(id, k)| {
            let name = k["keyName"].as_str()?.to_string();
            let id: u64 = id.parse().ok()?;
            (name.starts_with('G') && name[1..].parse::<u8>().is_ok()).then(|| (name, id, k["default"] != false))
        })
        .collect();
    out.sort_by_key(|(n, _, _)| n[1..].parse::<u8>().unwrap_or(0));
    out
}

/// Makes a key send F13 + `offset` (OpenLinkHub's "Keyboard" action), or gives it back its own job.
pub fn set_g_key(serial: &str, key: u64, offset: Option<u16>) -> Result<(), String> {
    let body = match offset {
        Some(o) => json!({ "deviceId": serial, "keyIndex": key, "enabled": false, "keyAssignmentType": 3, "keyAssignmentValue": 112 + o,
            "pressAndHold": false, "keyAssignmentModifier": 0, "keyAssignmentOriginal": false, "toggleDelay": 30, "onRelease": false }),
        None => json!({ "deviceId": serial, "keyIndex": key, "enabled": true, "keyAssignmentType": 0, "keyAssignmentValue": 0,
            "pressAndHold": false, "keyAssignmentModifier": 0, "keyAssignmentOriginal": false, "toggleDelay": 30, "onRelease": false }),
    };
    post("/api/keyboard/updateKeyAssignment", body)
}

/// Spanish names for OpenLinkHub's screen modes; unknown ones keep theirs.
pub fn lcd_mode_name(name: &str) -> String {
    let t = |s: &'static str| crate::i18n::t(s).to_string();
    match name {
        "CPU Temperature" => t("Temperatura del CPU"),
        "GPU Temperature" => t("Temperatura de la GPU"),
        "CPU / GPU Temp" => t("Temperatura CPU y GPU"),
        "CPU / GPU Load" => t("Carga CPU y GPU"),
        "CPU / GPU Load/Temp" => t("Carga y temperatura"),
        "Time" => t("Hora"),
        "Image / GIF" => t("Imagen o GIF"),
        "Arc" => t("Arco"),
        "Double Arc" => t("Arco doble"),
        "Animation" => t("Animación"),
        other => other.to_string(),
    }
}

/// A readable name for an OpenLinkHub light effect id; unknown ones are shown capitalised.
pub fn effect_name(id: &str) -> String {
    let t = |s: &'static str| crate::i18n::t(s).to_string();
    match id {
        "off" => t("Apagado"),
        "static" => t("Color fijo"),
        "custom" => t("Personalizado"),
        "rainbow" => t("Arcoíris"),
        "rainbowwave" => t("Ola arcoíris"),
        "pastelrainbow" => t("Arcoíris pastel"),
        "spiralrainbow" => t("Espiral arcoíris"),
        "pastelspiralrainbow" => t("Espiral arcoíris pastel"),
        "colorwave" => t("Ola de color"),
        "colorpulse" => t("Pulso de color"),
        "colorshift" => t("Cambio de color"),
        "colorwarp" => t("Color deformado"),
        "circle" => t("Círculo"),
        "circleshift" => t("Círculo cambiante"),
        "arc" => t("Arco"),
        "flickering" => t("Parpadeo"),
        "gradient" => t("Degradado"),
        "marquee" => t("Marquesina"),
        "nebula" => t("Nebulosa"),
        "rain" => t("Lluvia"),
        "storm" => t("Tormenta"),
        "watercolor" => t("Acuarela"),
        "rotator" => t("Giro"),
        "rotarystack" => t("Pila giratoria"),
        "spinner" => t("Remolino"),
        "sequential" => t("Secuencial"),
        "visor" => t("Visor"),
        "keyboard" => t("Teclado"),
        "mouse" => t("Ratón"),
        "mousepad" => t("Alfombrilla"),
        "headset" => t("Audífonos"),
        "stand" => t("Base"),
        "controller" => t("Control"),
        "led" => t("LED"),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
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
    fn effect_names_are_readable() {
        crate::i18n::init("es");
        assert_eq!(effect_name("colorwave"), "Ola de color");
        assert_eq!(effect_name("tlk"), "Tlk");
        assert_eq!(effect_name(""), "");
    }

    #[test]
    fn levels_round_trip() {
        for (i, (mode, _)) in LEVELS.iter().enumerate() {
            assert_eq!(level_index(*mode as u64), i);
        }
        assert_eq!(level_index(0), 3);
    }
}
