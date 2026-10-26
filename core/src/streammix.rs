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

/// The output a group's apps play into while the mix is on.
pub fn group_sink(id: &str) -> String {
    format!("fluxdeck_group_{id}")
}

/// A group's own output while the mix is on: its name, and the device it goes on to for you
/// (empty: the default one).
#[derive(Clone, Debug, PartialEq)]
pub struct GroupSink {
    pub id: String,
    pub name: String,
    pub output: String,
}

/// A `pipewire -c` config for the mix. Each group gets an output its apps play into, which goes on
/// to your device at the group's volume; each send takes its group's sound from that output's
/// monitor, before that volume, so what you hear and what the stream gets are set apart. The
/// microphone's send follows `mic`.
pub fn to_conf(keys: &[String], mic: &str, groups: &[GroupSink]) -> String {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut loops = String::new();
    for g in groups {
        // An explicit device falls back to the default one when it is unplugged.
        let to = if g.output.is_empty() { String::new() } else { format!("target.object = {}", q(&g.output)) };
        loops += &format!(
            r#"    {{ name = libpipewire-module-loopback
        args = {{
            node.description = {desc}
            audio.position = [ FL FR ]
            capture.props = {{ node.name = {sink} media.class = Audio/Sink priority.session = 0 priority.driver = 0 }}
            playback.props = {{ node.name = {out} {to} }}
        }}
    }}
"#,
            desc = q(&format!("FluxDeck · {}", g.name)),
            sink = q(&group_sink(&g.id)),
            out = q(&format!("{}_out", group_sink(&g.id))),
        );
    }
    for key in keys {
        let source = if key == MIC { format!("target.object = {}", q(mic)) } else { format!("target.object = {} stream.capture.sink = true node.dont-reconnect = true", q(&group_sink(key))) };
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

/// The node id of a node name.
pub fn node_id(all: &[Value], name: &str) -> Option<u32> {
    all.iter().find(|o| o["type"] == "PipeWire:Interface:Node" && o["info"]["props"]["node.name"] == name).and_then(|o| o["id"].as_u64()).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_start_off_and_only_live_ones_run() {
        let mut m = StreamMix::default();
        assert!(m.send("chat").muted);
        m.sends.insert("chat".into(), Level { volume: 0.5, muted: false });
        m.sends.insert("musica".into(), Level { volume: 1.0, muted: true });
        assert_eq!(m.active(), vec![("chat".to_string(), Level { volume: 0.5, muted: false })]);
        let groups = [GroupSink { id: "chat".into(), name: "Chat".into(), output: String::new() }, GroupSink { id: "juegos".into(), name: "Juegos".into(), output: "phones".into() }];
        let conf = to_conf(&["mic".into(), "chat".into()], "mic\"x", &groups);
        assert!(conf.contains(r#"target.object = "mic\"x""#));
        assert!(conf.contains("fluxdeck_stream_send_chat_out"));
        // The chat send listens to the chat group's output; games go to their own device.
        assert!(conf.contains(r#"target.object = "fluxdeck_group_chat" stream.capture.sink = true"#));
        assert!(conf.contains(r#"node.name = "fluxdeck_group_juegos_out" target.object = "phones""#));
        assert!(conf.contains(r#"node.description = "FluxDeck · Chat""#));
    }
}
