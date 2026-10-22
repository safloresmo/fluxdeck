//! Small PipeWire helpers over pw-cli and wpctl.
use std::io::Read;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub id: u32,
    pub name: String,
    pub class: String,
    pub description: String,
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn quoted(line: &str) -> String {
    line.split('"').nth(1).unwrap_or("").to_string()
}

pub fn nodes() -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for line in run("pw-cli", &["ls", "Node"]).unwrap_or_default().lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("id ") {
            let id = rest.split(',').next().and_then(|n| n.parse().ok()).unwrap_or(0);
            out.push(Node { id, ..Default::default() });
        } else if let Some(n) = out.last_mut() {
            if t.starts_with("node.name ") {
                n.name = quoted(t);
            } else if t.starts_with("media.class ") {
                n.class = quoted(t);
            } else if t.starts_with("node.description ") {
                n.description = quoted(t);
            }
        }
    }
    out
}

/// Current value of a filter-chain control such as "rnnoise:VAD Threshold (%)".
pub fn param(node: &str, name: &str) -> Option<f64> {
    let text = run("pw-cli", &["enum-params", node, "Props"])?;
    let needle = format!("String \"{name}\"");
    let mut lines = text.lines().skip_while(|l| !l.contains(&needle)).skip(1);
    lines.next()?.split_whitespace().nth(1)?.parse().ok()
}

/// All numeric filter-chain controls of a node, as ("node:Control", value), in graph order.
pub fn params(node: &str) -> Vec<(String, f64)> {
    let text = run("pw-cli", &["enum-params", node, "Props"]).unwrap_or_default();
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut lines = text.lines().map(str::trim).peekable();
    while let Some(line) = lines.next() {
        let Some(name) = line.strip_prefix("String \"").and_then(|r| r.strip_suffix('"')) else { continue };
        if !name.contains(':') {
            continue;
        }
        let value = lines.peek().and_then(|v| {
            let mut it = v.split_whitespace();
            matches!(it.next(), Some("Int" | "Float")).then(|| it.next()?.parse().ok()).flatten()
        });
        if let Some(v) = value
            && !out.iter().any(|(n, _)| n == name)
        {
            out.push((name.to_string(), v));
        }
    }
    out
}

/// Streams a node's input level (0..1 on a -60..0 dBFS scale) every 50 ms until `f` returns false
/// or the returned child is killed.
/// Node name of the level meters, so mixers can leave them out.
pub fn meter_name() -> String {
    format!("{}-meter", crate::paths::NAME)
}

pub fn meter(node: &str, f: impl FnMut(f32) -> bool + Send + 'static) -> Option<Child> {
    meter_with(node, false, f)
}

/// Like `meter`, for what an output device plays.
pub fn meter_output(node: &str, f: impl FnMut(f32) -> bool + Send + 'static) -> Option<Child> {
    meter_with(node, true, f)
}

fn meter_with(node: &str, output: bool, mut f: impl FnMut(f32) -> bool + Send + 'static) -> Option<Child> {
    let props = format!("{{ node.name = \"{}\" stream.capture.sink = {output} }}", meter_name());
    let mut child = Command::new("pw-record")
        .args(["-P", &props])
        .args(["--raw", "--target", node, "--rate", "16000", "--channels", "1", "--format", "s16", "--latency", "20ms", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 1600];
        while out.read_exact(&mut buf).is_ok() {
            let peak = buf.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max().unwrap_or(0);
            let db = 20.0 * (peak.max(1) as f32 / 32768.0).log10();
            if !f(((db + 60.0) / 60.0).clamp(0.0, 1.0)) {
                break;
            }
        }
    });
    Some(child)
}

pub fn set_param(node: &str, name: &str, value: f64) -> bool {
    let v = if value.fract() == 0.0 { format!("{}", value as i64) } else { format!("{value}") };
    let props = format!("{{ params = [ \"{name}\" {v} ] }}");
    run("pw-cli", &["set-param", node, "Props", &props]).is_some()
}

pub fn default_source() -> Option<String> {
    let text = run("wpctl", &["inspect", "@DEFAULT_AUDIO_SOURCE@"])?;
    text.lines().find(|l| l.contains("node.name =")).map(quoted)
}

pub fn set_default(name: &str) -> bool {
    match nodes().into_iter().find(|n| n.name == name) {
        Some(n) => run("wpctl", &["set-default", &n.id.to_string()]).is_some(),
        None => false,
    }
}

/// Filter-chain capture nodes running RNNoise, with the source they feed.
pub fn rnnoise_filters() -> Vec<(Node, Option<Node>)> {
    let all = nodes();
    all.iter()
        .filter(|n| n.class == "Stream/Input/Audio" && param(&n.name, "rnnoise:VAD Threshold (%)").is_some())
        .map(|n| {
            let source = all.iter().find(|s| s.class == "Audio/Source" && Some(s.name.as_str()) == n.name.strip_prefix("capture.")).cloned();
            (n.clone(), source)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn quoted_value() {
        assert_eq!(super::quoted(r#"node.name = "wave_dx""#), "wave_dx");
    }
}
