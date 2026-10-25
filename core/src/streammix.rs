//! The stream mix: an output for OBS ("FluxDeck Stream") fed by sends from the microphone and the
//! app groups, each at its own volume. It exists only while it is switched on and the service runs.
use crate::groups::Level;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const SINK: &str = "fluxdeck_stream";
/// The send key for the microphone; groups use their id.
pub const MIC: &str = "mic";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct StreamMix {
    pub on: bool,
    /// By "mic" or group id; a muted send is not sent at all.
    pub sends: BTreeMap<String, Level>,
}

fn path() -> PathBuf {
    crate::paths::state().join("stream-mix.json")
}

impl StreamMix {
    pub fn load() -> StreamMix {
        std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::paths::state());
        let tmp = path().with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, path());
        }
    }

    /// A send as shown: off (muted) at full volume until the user turns it on.
    pub fn send(&self, key: &str) -> Level {
        self.sends.get(key).copied().unwrap_or(Level { volume: 1.0, muted: true })
    }

    /// The sends that carry sound, sorted by key.
    pub fn active(&self) -> Vec<(String, Level)> {
        self.sends.iter().filter(|(_, l)| !l.muted).map(|(k, l)| (k.clone(), *l)).collect()
    }
}

/// Node names of a send: where sound goes in, and where it leaves towards the stream output.
pub fn send_in(key: &str) -> String {
    format!("{SINK}_send_{key}")
}

pub fn send_out(key: &str) -> String {
    format!("{SINK}_send_{key}_out")
}

/// The pw-cli line that makes the stream output; it lives as long as that pw-cli does.
pub fn sink_command() -> String {
    format!(
        "create-node adapter {{ factory.name=support.null-audio-sink node.name={SINK} node.description=\"FluxDeck Stream\" media.class=Audio/Sink audio.position=[FL FR] priority.session=0 priority.driver=0 monitor.channel-volumes=true object.linger=false }}\n"
    )
}

/// A `pipewire -c` config with one loopback per send. The microphone's follows `mic`; a group's
/// waits for its apps to be linked in.
pub fn to_conf(keys: &[String], mic: &str) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut loops = String::new();
    for key in keys {
        let source = if key == MIC { format!("target.object = {}", q(mic)) } else { "node.autoconnect = false".to_string() };
        loops += &format!(
            r#"    {{ name = libpipewire-module-loopback
        args = {{
            node.description = {desc}
            audio.position = [ FL FR ]
            capture.props = {{ node.name = {cap} node.passive = true {source} }}
            playback.props = {{ node.name = {play} target.object = "{SINK}" node.dont-reconnect = true }}
        }}
    }}
"#,
            desc = q(&format!("FluxDeck Stream: {key}")),
            cap = q(&send_in(key)),
            play = q(&send_out(key)),
        );
    }
    format!(
        r#"# Written by the service from stream-mix.json; edits here are replaced.
context.properties = {{ log.level = 0 }}
context.spa-libs = {{
    audio.convert.* = audioconvert/libspa-audioconvert
    support.*       = support/libspa-support
}}
context.modules = [
    {{ name = libpipewire-module-rt args = {{ }} flags = [ ifexists nofail ] }}
    {{ name = libpipewire-module-protocol-native }}
    {{ name = libpipewire-module-client-node }}
    {{ name = libpipewire-module-adapter }}
{loops}]
"#
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub id: u32,
    pub node: u32,
    pub output: bool,
    pub channel: String,
}

/// Audio ports, leaving out monitor ports.
pub fn ports(all: &[Value]) -> Vec<Port> {
    all.iter()
        .filter(|o| o["type"] == "PipeWire:Interface:Port" && o["info"]["props"]["port.monitor"] != true)
        .filter_map(|o| {
            Some(Port {
                id: o["id"].as_u64()? as u32,
                node: o["info"]["props"]["node.id"].as_u64()? as u32,
                output: o["info"]["direction"] == "output",
                channel: o["info"]["props"]["audio.channel"].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// Existing links, as (output port, input port).
pub fn links(all: &[Value]) -> Vec<(u32, u32)> {
    all.iter()
        .filter(|o| o["type"] == "PipeWire:Interface:Link")
        .filter_map(|o| Some((o["info"]["output-port-id"].as_u64()? as u32, o["info"]["input-port-id"].as_u64()? as u32)))
        .collect()
}

/// The node id of a node name.
pub fn node_id(all: &[Value], name: &str) -> Option<u32> {
    all.iter().find(|o| o["type"] == "PipeWire:Interface:Node" && o["info"]["props"]["node.name"] == name).and_then(|o| o["id"].as_u64()).map(|i| i as u32)
}

/// Links still missing from an app's outputs to a send's inputs: same channel to same channel,
/// and a mono app to both sides.
pub fn missing(app: u32, send: u32, ports: &[Port], existing: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let ins: Vec<&Port> = ports.iter().filter(|p| p.node == send && !p.output).collect();
    let mut out = Vec::new();
    for o in ports.iter().filter(|p| p.node == app && p.output) {
        for i in ins.iter().filter(|i| o.channel == "MONO" || i.channel == o.channel) {
            if !existing.contains(&(o.id, i.id)) {
                out.push((o.id, i.id));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(id: u32, node: u32, output: bool, channel: &str) -> Port {
        Port { id, node, output, channel: channel.into() }
    }

    #[test]
    fn links_channels_and_skips_existing() {
        let ports = vec![port(1, 10, true, "FL"), port(2, 10, true, "FR"), port(3, 20, false, "FL"), port(4, 20, false, "FR"), port(5, 30, true, "MONO")];
        assert_eq!(missing(10, 20, &ports, &[(1, 3)]), vec![(2, 4)]);
        assert_eq!(missing(30, 20, &ports, &[]), vec![(5, 3), (5, 4)]);
        assert!(missing(10, 99, &ports, &[]).is_empty());
    }

    #[test]
    fn sends_start_off_and_only_live_ones_run() {
        let mut m = StreamMix::default();
        assert!(m.send("chat").muted);
        m.sends.insert("chat".into(), Level { volume: 0.5, muted: false });
        m.sends.insert("musica".into(), Level { volume: 1.0, muted: true });
        assert_eq!(m.active(), vec![("chat".to_string(), Level { volume: 0.5, muted: false })]);
        let conf = to_conf(&["mic".into(), "chat".into()], "mic\"x");
        assert!(conf.contains(r#"target.object = "mic\"x""#));
        assert!(conf.contains("fluxdeck_stream_send_chat_out"));
    }
}
