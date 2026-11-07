//! Headsets of many brands (Logitech, SteelSeries, HyperX, Corsair…) through HeadsetControl
//! (https://github.com/Sapd/HeadsetControl). Every write names one headset: without `-d`,
//! HeadsetControl changes all of them.
use serde_json::{Map, Value};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Headset {
    /// "vendor:product", as HeadsetControl's `-d` takes it.
    pub id: String,
    pub vendor: String,
    pub product: String,
    pub battery: Option<u64>,
    pub charging: bool,
    pub has_sidetone: bool,
    pub has_lights: bool,
    /// 0–128: read back when the headset can, else the last level set from FluxDeck.
    pub sidetone: Option<u8>,
    /// Lights can only be set, so this is the last state set from FluxDeck.
    pub lights: Option<bool>,
}

fn run(args: &[&str]) -> Option<Value> {
    let o = Command::new("headsetcontrol").args(args).args(["-o", "json"]).output().ok()?;
    serde_json::from_slice(&o.stdout).ok()
}

fn parse(v: &Value, saved: &Value) -> Vec<Headset> {
    v["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["status"] == "success")
        .filter_map(|d| {
            let id = format!("{}:{}", d["id_vendor"].as_str()?, d["id_product"].as_str()?);
            let caps: Vec<&str> = d["capabilities_str"].as_array()?.iter().filter_map(Value::as_str).collect();
            let b = &d["battery"];
            let s = &saved[&id];
            Some(Headset {
                vendor: d["vendor"].as_str().unwrap_or_default().to_string(),
                product: d["product"].as_str().or(d["device"].as_str()).unwrap_or(&id).to_string(),
                battery: b["level"].as_u64().filter(|_| b["status"] != "BATTERY_UNAVAILABLE"),
                charging: b["status"] == "BATTERY_CHARGING",
                has_sidetone: caps.contains(&"sidetone"),
                has_lights: caps.contains(&"lights"),
                sidetone: d["sidetone"]["level"].as_u64().or(s["sidetone"].as_u64()).map(|l| l.min(128) as u8),
                lights: s["lights"].as_bool(),
                id,
            })
        })
        .collect()
}

fn saved_path() -> std::path::PathBuf {
    crate::paths::state().join("headset.json")
}

fn saved() -> Value {
    std::fs::read_to_string(saved_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Object(Map::new()))
}

fn remember(id: &str, key: &str, value: Value) {
    let mut all = saved();
    if !all[id].is_object() {
        all[id] = Value::Object(Map::new());
    }
    all[id][key] = value;
    let path = saved_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(&path));
    let _ = std::fs::write(path, all.to_string());
}

/// Headsets HeadsetControl sees; empty when it is not installed.
pub fn headsets() -> Vec<Headset> {
    run(&[]).map(|v| parse(&v, &saved())).unwrap_or_default()
}

pub fn headset(id: &str) -> Option<Headset> {
    let v = run(&["-d", id])?;
    parse(&v, &saved()).into_iter().find(|h| h.id == id)
}

pub fn installed() -> bool {
    Command::new("headsetcontrol").arg("--version").output().is_ok()
}

/// Runs one change on one headset and reports HeadsetControl's own error.
fn act(id: &str, args: &[&str]) -> Result<(), String> {
    let mut all = vec!["-d", id];
    all.extend(args);
    let v = run(&all).ok_or("HeadsetControl is not installed")?;
    let action = v["actions"].as_array().and_then(|a| a.first()).ok_or_else(|| format!("HeadsetControl does not see {id}"))?;
    if action["status"] == "success" { Ok(()) } else { Err(action["error_message"].as_str().unwrap_or("HeadsetControl refused the change").to_string()) }
}

/// How much of the microphone the headset plays back, 0–128.
pub fn set_sidetone(id: &str, level: u8) -> Result<(), String> {
    let level = level.min(128);
    act(id, &["-s", &level.to_string()])?;
    remember(id, "sidetone", level.into());
    Ok(())
}

pub fn set_lights(id: &str, on: bool) -> Result<(), String> {
    act(id, &["-l", if on { "1" } else { "0" }])?;
    remember(id, "lights", on.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn reads_headsets() {
        let v = json!({ "devices": [
            { "status": "success", "vendor": "Logitech", "product": "G535", "id_vendor": "0x046d", "id_product": "0x0ac4",
              "capabilities_str": ["sidetone", "battery"], "battery": { "status": "BATTERY_CHARGING", "level": 40 } },
            { "status": "success", "vendor": "X", "product": "Y", "id_vendor": "0xf00b", "id_product": "0xa00c",
              "capabilities_str": ["sidetone", "lights", "battery"], "battery": { "status": "BATTERY_UNAVAILABLE", "level": -1 },
              "sidetone": { "level": 85 } },
            { "status": "failure", "id_vendor": "0x1", "id_product": "0x2", "capabilities_str": [] }
        ]});
        let saved = json!({ "0x046d:0x0ac4": { "sidetone": 32, "lights": true }, "0xf00b:0xa00c": { "sidetone": 10 } });
        let h = super::parse(&v, &saved);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].id.as_str(), h[0].battery, h[0].charging, h[0].sidetone, h[0].has_lights), ("0x046d:0x0ac4", Some(40), true, Some(32), false));
        assert_eq!((h[1].battery, h[1].sidetone, h[1].lights, h[1].has_lights), (None, Some(85), None, true));
    }
}
