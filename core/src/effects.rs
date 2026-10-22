//! The microphone effect chain: an ordered list of LADSPA, LV2 or PipeWire builtin effects, saved in
//! effects.toml and run by the service as its own PipeWire filter-chain.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Node names of the chain the service runs.
pub const CAPTURE: &str = "capture.fluxdeck_efectos";
pub const SOURCE: &str = "fluxdeck_efectos";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Builtin,
    Ladspa,
    Lv2,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Effect {
    /// Unique within the chain; controls are addressed as "<id>:<control>".
    pub id: String,
    /// Shown in the editor.
    pub title: String,
    pub kind: Kind,
    /// LADSPA: path of the .so; LV2: plugin URI; builtin: empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    /// LADSPA or builtin label (bq_peaking, sc4m…); empty for LV2.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Audio ports to link through; found when the effect is added.
    pub input: String,
    pub output: String,
    pub enabled: bool,
    pub controls: BTreeMap<String, f64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Chain {
    /// Name apps show for the processed microphone.
    pub description: String,
    /// The microphone to process (a PipeWire source node name).
    pub target: String,
    pub effects: Vec<Effect>,
}

pub fn path() -> PathBuf {
    crate::paths::config().join("effects.toml")
}

impl Chain {
    pub fn load() -> Option<Chain> {
        toml::from_str(&std::fs::read_to_string(path()).ok()?).ok()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path().with_extension("toml.tmp");
        std::fs::create_dir_all(crate::paths::config())?;
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path())
    }

    /// Same effects in the same order, ignoring control values: changing only values needs no restart.
    pub fn same_shape(&self, other: &Chain) -> bool {
        let shape = |c: &Chain| {
            (c.target.clone(), c.description.clone(), c.effects.iter().filter(|e| e.enabled).map(|e| (e.id.clone(), e.kind, e.plugin.clone(), e.label.clone())).collect::<Vec<_>>())
        };
        shape(self) == shape(other)
    }

    pub fn new_id(&self, base: &str) -> String {
        let clean: String = base.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let clean = clean.trim_matches('_').to_string();
        let clean = if clean.is_empty() { "efecto".to_string() } else { clean };
        (1..).map(|n| if n == 1 { clean.clone() } else { format!("{clean}_{n}") }).find(|id| !self.effects.iter().any(|e| &e.id == id)).unwrap_or(clean)
    }

    /// A PipeWire config that runs this chain as its own process (`pipewire -c <file>`).
    pub fn to_conf(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let active: Vec<&Effect> = self.effects.iter().filter(|e| e.enabled).collect();
        let mut nodes = String::new();
        for e in &active {
            let controls: Vec<String> = e.controls.iter().map(|(k, v)| format!("{} = {v}", q(k))).collect();
            let (kind, plugin, label) = match e.kind {
                Kind::Builtin => ("builtin", String::new(), format!(" label = {}", q(&e.label))),
                Kind::Ladspa => ("ladspa", format!(" plugin = {}", q(&e.plugin)), format!(" label = {}", q(&e.label))),
                Kind::Lv2 => ("lv2", format!(" plugin = {}", q(&e.plugin)), String::new()),
            };
            nodes += &format!("                    {{ type = {kind} name = {}{plugin}{label} control = {{ {} }} }}\n", q(&e.id), controls.join(" "));
        }
        // An empty chain still needs one node to pass sound through.
        if active.is_empty() {
            nodes += "                    { type = builtin name = \"paso\" label = \"copy\" }\n";
        }
        let links: Vec<String> = active.windows(2).map(|w| format!("{{ output = {} input = {} }}", q(&format!("{}:{}", w[0].id, w[0].output)), q(&format!("{}:{}", w[1].id, w[1].input)))).collect();
        let (first, last) = match (active.first(), active.last()) {
            (Some(f), Some(l)) => (format!("{}:{}", f.id, f.input), format!("{}:{}", l.id, l.output)),
            _ => ("paso:In".to_string(), "paso:Out".to_string()),
        };
        format!(
            r#"# Written by the service from effects.toml; edits here are replaced.
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
    {{ name = libpipewire-module-filter-chain
        args = {{
            node.description = {desc}
            media.name       = {desc}
            filter.graph = {{
                nodes = [
{nodes}                ]
                links = [ {links} ]
                inputs  = [ {first} ]
                outputs = [ {last} ]
            }}
            audio.rate     = 48000
            audio.position = [ MONO ]
            capture.props = {{
                node.name     = "{CAPTURE}"
                node.passive  = true
                target.object = {target}
            }}
            playback.props = {{
                node.name        = "{SOURCE}"
                node.description = {desc}
                media.class      = Audio/Source
            }}
        }}
    }}
]
"#,
            desc = q(&self.description),
            links = links.join(" "),
            first = q(&first),
            last = q(&last),
            target = q(&self.target),
        )
    }
}

/// PipeWire's own effects, always available.
pub fn builtins() -> Vec<(&'static str, &'static str, Vec<(&'static str, f64)>)> {
    let bq = |g: bool| if g { vec![("Freq", 1000.0), ("Q", 0.707), ("Gain", 0.0)] } else { vec![("Freq", 1000.0), ("Q", 0.707)] };
    vec![
        ("bq_highpass", "Pasa altos", bq(false)),
        ("bq_lowpass", "Pasa bajos", bq(false)),
        ("bq_lowshelf", "Graves (estante)", bq(true)),
        ("bq_highshelf", "Agudos (estante)", bq(true)),
        ("bq_peaking", "Ecualizador de banda", bq(true)),
        ("bq_notch", "Quitar una frecuencia", bq(false)),
    ]
}

/// Reads a filter-chain config (SPA JSON) and returns the chain it describes, if it has one.
pub fn import(conf: &str) -> Option<Chain> {
    let v = spa_json(conf)?;
    let module = v["context.modules"].as_array()?.iter().find(|m| m["name"] == "libpipewire-module-filter-chain")?;
    let args = &module["args"];
    let graph = &args["filter.graph"];
    let mut effects = Vec::new();
    for n in graph["nodes"].as_array()? {
        let s = |k: &str| n[k].as_str().unwrap_or_default().to_string();
        let kind = match s("type").as_str() {
            "ladspa" => Kind::Ladspa,
            "lv2" => Kind::Lv2,
            _ => Kind::Builtin,
        };
        let controls = n["control"].as_object().map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect()).unwrap_or_default();
        effects.push(Effect { id: s("name"), title: s("name"), kind, plugin: s("plugin"), label: s("label"), enabled: true, controls, ..Default::default() });
    }
    // Order and port names come from the links: follow them from the graph's input.
    let links: Vec<(String, String)> = graph["links"].as_array().into_iter().flatten().map(|l| (l["output"].as_str().unwrap_or_default().to_string(), l["input"].as_str().unwrap_or_default().to_string())).collect();
    let input = graph["inputs"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or_default().to_string();
    let output = graph["outputs"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or_default().to_string();
    let split = |p: &str| p.split_once(':').map(|(a, b)| (a.to_string(), b.to_string()));
    let mut ordered = Vec::new();
    let mut at = split(&input);
    while let Some((id, port_in)) = at.take() {
        let Some(i) = effects.iter().position(|e: &Effect| e.id == id) else { break };
        let mut e = effects.remove(i);
        e.input = port_in;
        let next = links.iter().find(|(o, _)| split(o).is_some_and(|(n, _)| n == e.id));
        e.output = match next {
            Some((o, _)) => split(o).map(|(_, p)| p).unwrap_or_default(),
            None => split(&output).filter(|(n, _)| *n == e.id).map(|(_, p)| p).unwrap_or_default(),
        };
        at = next.and_then(|(_, i)| split(i));
        ordered.push(e);
    }
    if ordered.is_empty() {
        return None;
    }
    for e in &mut ordered {
        e.title = title_for(e);
    }
    Some(Chain {
        description: args["node.description"].as_str().unwrap_or("Micrófono con efectos").to_string(),
        target: args["capture.props"]["target.object"].as_str().unwrap_or_default().to_string(),
        effects: ordered,
    })
}

/// The name a config gave the node reads best ("presencia" → "Presencia"); RNNoise gets its role.
fn title_for(e: &Effect) -> String {
    if matches!(e.label.as_str(), "noise_suppressor_mono" | "noise_suppressor_stereo") {
        return crate::i18n::t("Supresión de ruido").into();
    }
    let s = e.id.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Finds the filter-chain config files PipeWire loads for the user.
pub fn system_configs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/pipewire/filter-chain.conf.d"), PathBuf::from("/etc/pipewire/filter-chain.conf.d")];
    if let Some(h) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))) {
        dirs.push(h.join("pipewire/filter-chain.conf.d"));
    }
    let mut out: Vec<PathBuf> = dirs.iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "conf")).collect();
    out.sort();
    out
}

/// The system chain that feeds `capture`, read from its config file.
pub fn import_system(capture: &str) -> Option<Chain> {
    system_configs().iter().filter_map(|p| std::fs::read_to_string(p).ok()).find(|t| t.contains(capture)).and_then(|t| import(&t))
}

pub fn conf_path() -> PathBuf {
    crate::paths::runtime("effects.conf")
}

/// Converts SPA JSON (PipeWire's relaxed JSON: `=` or `:`, optional commas and quotes, # comments) to JSON.
pub fn spa_json(text: &str) -> Option<Value> {
    let mut out = String::with_capacity(text.len() * 2);
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    // Whether a value has been written at the current level, so a separator is needed before the next.
    let mut need_comma: Vec<bool> = vec![false];
    let mut in_object: Vec<bool> = vec![true];
    let mut expecting_value = false;
    out.push('{');
    let bare_end = |c: char| c.is_whitespace() || matches!(c, '{' | '}' | '[' | ']' | '=' | ':' | ',' | '"' | '#');
    while i < chars.len() {
        let c = chars[i];
        match c {
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            c if c.is_whitespace() || c == ',' => {}
            '=' | ':' => {
                out.push(':');
                expecting_value = true;
            }
            '}' | ']' => {
                out.push(c);
                need_comma.pop();
                in_object.pop();
                *need_comma.last_mut()? = true;
                expecting_value = false;
            }
            _ => {
                if !expecting_value && *need_comma.last()? {
                    out.push(',');
                }
                let is_key = *in_object.last()? && !expecting_value;
                match c {
                    '{' | '[' => {
                        out.push(c);
                        need_comma.push(false);
                        in_object.push(c == '{');
                        expecting_value = false;
                        i += 1;
                        continue;
                    }
                    '"' => {
                        let start = i;
                        i += 1;
                        while i < chars.len() && chars[i] != '"' {
                            if chars[i] == '\\' {
                                i += 1;
                            }
                            i += 1;
                        }
                        out.extend(&chars[start..=i.min(chars.len() - 1)]);
                    }
                    _ => {
                        let start = i;
                        while i < chars.len() && !bare_end(chars[i]) {
                            i += 1;
                        }
                        let word: String = chars[start..i].iter().collect();
                        let literal = !is_key && (word == "true" || word == "false" || word == "null" || word.parse::<f64>().is_ok());
                        if literal {
                            // JSON has no leading "+" or bare ".5".
                            out.push_str(&word.parse::<f64>().map(|n| n.to_string()).unwrap_or(word));
                        } else {
                            out.push_str(&serde_json::to_string(&word).ok()?);
                        }
                        i -= 1;
                    }
                }
                if is_key {
                    *need_comma.last_mut()? = false;
                } else {
                    *need_comma.last_mut()? = true;
                    expecting_value = false;
                }
            }
        }
        i += 1;
    }
    out.push('}');
    serde_json::from_str(&out).ok()
}

/// Microphones the chain can take: real sources, not the output of another filter.
pub fn microphones() -> Vec<crate::pw::Node> {
    let nodes = crate::pw::nodes();
    let filtered = |name: &str| nodes.iter().any(|n| n.name == format!("capture.{name}"));
    nodes.iter().filter(|n| n.class == "Audio/Source" && n.name != SOURCE && !filtered(&n.name)).cloned().collect()
}

/// A set of effects the system package manager can install.
pub struct Pack {
    pub name: &'static str,
    /// Spanish source text; shown through i18n::t.
    pub about: &'static str,
    /// Package name for each package manager; a missing one means that distro has no package.
    pub packages: &'static [(&'static str, &'static str)],
}

pub const PACKS: &[Pack] = &[
    Pack {
        name: "LSP Plugins",
        about: "Calidad de estudio: ecualizador paramétrico, compuerta, de-esser, compresores y limitadores.",
        packages: &[("pacman", "lsp-plugins-lv2"), ("apt", "lsp-plugins-lv2"), ("dnf", "lsp-plugins-lv2"), ("zypper", "lsp-plugins-lv2")],
    },
    Pack {
        name: "x42",
        about: "Ecualizador, compresor y medidores precisos, muy ligeros.",
        packages: &[("pacman", "x42-plugins-lv2"), ("apt", "x42-plugins"), ("dnf", "x42-plugins"), ("zypper", "x42-plugins")],
    },
    Pack {
        name: "Calf",
        about: "Compresores, de-esser, compuerta y ecualizadores clásicos.",
        packages: &[("pacman", "calf"), ("apt", "calf-plugins"), ("dnf", "lv2-calf-plugins"), ("zypper", "calf")],
    },
    Pack {
        name: "SWH",
        about: "Más de cien efectos LADSPA pequeños: compuerta, compresores, limitadores, ecualizador de 15 bandas.",
        packages: &[("pacman", "swh-plugins"), ("apt", "swh-plugins"), ("dnf", "ladspa-swh-plugins"), ("zypper", "ladspa-swh-plugins")],
    },
    Pack {
        name: "RNNoise",
        about: "Supresión de ruido con inteligencia artificial: deja pasar solo la voz.",
        packages: &[("pacman", "noise-suppression-for-voice")],
    },
];

fn on_path(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// The system's package manager, when it is one FluxDeck knows.
pub fn package_manager() -> Option<&'static str> {
    ["pacman", "apt", "dnf", "zypper"].into_iter().find(|m| on_path(m))
}

impl Pack {
    pub fn package(&self) -> Option<&'static str> {
        let pm = package_manager()?;
        self.packages.iter().find(|(m, _)| *m == pm).map(|(_, p)| *p)
    }

    pub fn installed(&self) -> bool {
        let Some(pkg) = self.package() else { return false };
        let (cmd, args): (&str, Vec<&str>) = match package_manager() {
            Some("pacman") => ("pacman", vec!["-Q", pkg]),
            Some("apt") => ("dpkg", vec!["-s", pkg]),
            _ => ("rpm", vec!["-q", pkg]),
        };
        std::process::Command::new(cmd).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success())
    }

    /// The command that installs it, to run in a terminal where the user types their password.
    pub fn install_command(&self) -> Option<String> {
        let pkg = self.package()?;
        Some(match package_manager()? {
            "pacman" => format!("sudo pacman -S --needed {pkg}"),
            "apt" => format!("sudo apt install {pkg}"),
            "dnf" => format!("sudo dnf install {pkg}"),
            _ => format!("sudo zypper install {pkg}"),
        })
    }
}

/// Opens the user's terminal running `cmd`, so they can read it and type their password.
pub fn run_in_terminal(cmd: &str) -> Result<(), String> {
    let wrapped = format!("{cmd}; echo; read -r -p '{}' _", crate::i18n::t("Listo. Pulsa Enter para cerrar."));
    let terms: [(&str, &[&str]); 8] = [
        ("kitty", &[]),
        ("foot", &[]),
        ("alacritty", &["-e"]),
        ("ghostty", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("konsole", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("xterm", &["-e"]),
    ];
    let (term, pre) = terms.iter().find(|(t, _)| on_path(t)).ok_or_else(|| crate::i18n::t("No encontré una terminal para instalarlo.").to_string())?;
    std::process::Command::new(term).args(*pre).args(["sh", "-c", &wrapped]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = r#"
# comment
context.modules = [
    { name = libpipewire-module-filter-chain
        flags = [ nofail ]
        args = {
            node.description = "Wave DX sin ruido"
            filter.graph = {
                nodes = [
                    { type = ladspa name = rnnoise plugin = /usr/lib/ladspa/librnnoise_ladspa.so label = noise_suppressor_mono
                      control = { "VAD Threshold (%)" = 50.0 } }
                    { type = builtin name = pasa_altos label = bq_highpass control = { "Freq" = 70.0 "Q" = 0.707 } }
                    { type = ladspa name = limitador plugin = /usr/lib/ladspa/fast_lookahead_limiter_1913.so label = fastLookaheadLimiter
                      control = { "Limit (dB)" = -1.5 } }
                ]
                links = [
                    { output = "pasa_altos:Out" input = "limitador:Input 1" }
                    { output = "rnnoise:Output" input = "pasa_altos:In" }
                ]
                inputs  = [ "rnnoise:Input" ]
                outputs = [ "limitador:Output 1" ]
            }
            capture.props = { node.name = "capture.x" target.object = "alsa_input.dock" }
        }
    }
]"#;

    #[test]
    fn imports_a_chain_in_link_order() {
        let c = import(CONF).unwrap();
        assert_eq!(c.target, "alsa_input.dock");
        let ids: Vec<&str> = c.effects.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["rnnoise", "pasa_altos", "limitador"]);
        assert_eq!((c.effects[2].input.as_str(), c.effects[2].output.as_str()), ("Input 1", "Output 1"));
        assert_eq!(c.effects[1].controls["Freq"], 70.0);
        assert_eq!(c.effects[2].controls["Limit (dB)"], -1.5);
    }

    #[test]
    fn writes_a_config_that_reads_back() {
        let c = import(CONF).unwrap();
        let again = import(&c.to_conf()).unwrap();
        assert_eq!(again.effects, c.effects);
        assert!(c.same_shape(&again));
    }

    #[test]
    fn disabled_effects_are_left_out() {
        let mut c = import(CONF).unwrap();
        c.effects[1].enabled = false;
        let again = import(&c.to_conf()).unwrap();
        assert_eq!(again.effects.len(), 2);
        assert!(!c.same_shape(&import(CONF).unwrap()));
    }

    #[test]
    fn new_ids_do_not_clash() {
        let c = import(CONF).unwrap();
        assert_eq!(c.new_id("Pasa altos"), "pasa_altos_2");
        assert_eq!(c.new_id("Gate"), "gate");
    }
}
