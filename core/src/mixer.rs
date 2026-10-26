//! What sounds where: outputs, inputs and the apps playing or recording, read from `pw-dump`,
//! changed through `wpctl` (volume, mute, defaults) and `pw-metadata` (which output an app uses).
use serde_json::Value;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A speaker, headphones or other output device.
    Output,
    /// A microphone or other input.
    Input,
    /// An app playing sound.
    Playback,
    /// An app recording.
    Record,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub id: u32,
    pub name: String,
    pub label: String,
    /// The app's icon name, when it says one.
    pub icon: String,
    /// The program behind an app stream (firefox, steam…).
    pub binary: String,
    pub kind: Kind,
    /// 0..1 on the same scale wpctl and desktop volume sliders use.
    pub volume: f32,
    pub muted: bool,
    /// For apps: the device they are linked to.
    pub device: Option<u32>,
    /// An output that is half of a filter (an equaliser in front of the real device): not offered in menus.
    pub internal: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub channels: Vec<Channel>,
    pub default_output: String,
    pub default_input: String,
}

impl Snapshot {
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Channel> {
        self.channels.iter().filter(move |c| c.kind == kind)
    }

    pub fn default_of(&self, kind: Kind) -> Option<&Channel> {
        let name = if kind == Kind::Output { &self.default_output } else { &self.default_input };
        self.channels.iter().find(|c| c.kind == kind && &c.name == name)
    }
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn snapshot() -> Snapshot {
    let Ok(out) = Command::new("pw-dump").stderr(Stdio::null()).output() else { return Snapshot::default() };
    let Ok(all) = serde_json::from_slice::<Vec<Value>>(&out.stdout) else { return Snapshot::default() };
    parse(&all)
}

pub fn parse(all: &[Value]) -> Snapshot {
    let mut snap = Snapshot::default();
    let mut links: Vec<(u64, u64)> = Vec::new();
    for o in all {
        match o["type"].as_str().unwrap_or_default() {
            "PipeWire:Interface:Link" => {
                let i = &o["info"];
                if let (Some(a), Some(b)) = (i["output-node-id"].as_u64(), i["input-node-id"].as_u64()) {
                    links.push((a, b));
                }
            }
            "PipeWire:Interface:Metadata" if o["props"]["metadata.name"] == "default" => {
                for m in o["metadata"].as_array().into_iter().flatten() {
                    let name = s(&m["value"]["name"]);
                    match m["key"].as_str() {
                        Some("default.audio.sink") => snap.default_output = name,
                        Some("default.audio.source") => snap.default_input = name,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    for o in all.iter().filter(|o| o["type"] == "PipeWire:Interface:Node") {
        let p = &o["info"]["props"];
        let kind = match p["media.class"].as_str() {
            Some("Audio/Sink") => Kind::Output,
            Some("Audio/Source") => Kind::Input,
            Some("Stream/Output/Audio") => Kind::Playback,
            Some("Stream/Input/Audio") => Kind::Record,
            _ => continue,
        };
        let is_stream = matches!(kind, Kind::Playback | Kind::Record);
        // Halves of a filter-chain and level meters are plumbing, not apps.
        if is_stream && (p["node.link-group"].is_string() || p["node.name"].as_str() == Some(&crate::pw::meter_name()) || p["node.passive"] == true) {
            continue;
        }
        let props = o["info"]["params"]["Props"].as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
        let vols: Vec<f64> = props["channelVolumes"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        let linear = if vols.is_empty() { props["volume"].as_f64().unwrap_or(1.0) } else { vols.iter().sum::<f64>() / vols.len() as f64 };
        let id = o["id"].as_u64().unwrap_or(0);
        let app = s(&p["application.name"]);
        let label = if is_stream {
            let media = s(&p["media.name"]);
            if app.is_empty() { media } else { app.clone() }
        } else {
            let d = s(&p["node.description"]);
            if d.is_empty() { s(&p["node.name"]) } else { d }
        };
        let device = is_stream.then(|| links.iter().find_map(|&(a, b)| if kind == Kind::Playback && a == id { Some(b) } else if kind == Kind::Record && b == id { Some(a) } else { None })).flatten();
        snap.channels.push(Channel {
            id: id as u32,
            name: s(&p["node.name"]),
            label,
            icon: [&p["application.icon-name"], &p["application.process.binary"]].iter().map(|v| s(v)).find(|v| !v.is_empty()).unwrap_or_default(),
            binary: s(&p["application.process.binary"]),
            kind,
            volume: linear.max(0.0).cbrt() as f32,
            muted: props["mute"].as_bool().unwrap_or(false),
            device: device.map(|d| d as u32),
            // The stream mix's own outputs get columns of their own, not places among the devices.
            internal: kind == Kind::Output
                && (p["node.link-group"].is_string() || p["node.name"] == crate::streammix::SINK || p["node.name"].as_str().is_some_and(|n| n.starts_with("fluxdeck_group_"))),
        });
    }
    snap.channels.sort_by_key(|c| (c.kind as u8, c.label.to_lowercase()));
    snap
}

fn wpctl(args: &[&str]) -> bool {
    Command::new("wpctl").args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

pub fn set_volume(id: u32, v: f32) -> bool {
    wpctl(&["set-volume", &id.to_string(), &format!("{:.3}", v.clamp(0.0, 1.5))])
}

pub fn set_mute(id: u32, muted: bool) -> bool {
    wpctl(&["set-mute", &id.to_string(), if muted { "1" } else { "0" }])
}

pub fn set_default(id: u32) -> bool {
    wpctl(&["set-default", &id.to_string()])
}

/// Sends an app's sound to another output (or records it from another input).
pub fn move_stream(stream: u32, device: &str) -> bool {
    Command::new("pw-metadata").args([&stream.to_string(), "target.object", device]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_devices_apps_and_defaults() {
        let dump = vec![
            json!({"id": 50, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "speakers", "node.description": "Bocinas"},
                "params": {"Props": [{"channelVolumes": [0.125, 0.125], "mute": false}]}}}),
            json!({"id": 60, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "spotify", "application.name": "Spotify", "application.icon-name": "spotify"},
                "params": {"Props": [{"channelVolumes": [1.0, 1.0], "mute": true}]}}}),
            json!({"id": 61, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "eq_out", "node.link-group": "filter-chain-1"}}}),
            json!({"id": 70, "type": "PipeWire:Interface:Link", "info": {"output-node-id": 60, "input-node-id": 50}}),
            json!({"id": 0, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"}, "metadata": [{"key": "default.audio.sink", "value": {"name": "speakers"}}]}),
        ];
        let s = parse(&dump);
        assert_eq!(s.channels.len(), 2, "filter-chain halves are left out");
        let out = s.default_of(Kind::Output).unwrap();
        assert_eq!((out.label.as_str(), (out.volume * 100.0).round()), ("Bocinas", 50.0));
        let app = s.of(Kind::Playback).next().unwrap();
        assert_eq!((app.label.as_str(), app.muted, app.device), ("Spotify", true, Some(50)));
    }
}
