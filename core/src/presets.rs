//! Ready-made actions for the editor's library. Each group only appears when what it needs is installed.
use crate::i18n::{t, tf};
use crate::{Dial, Key, corsair, headset, openrgb, plugins, pw, upower};
use std::path::Path;

#[derive(Clone)]
pub enum Item {
    Key(Key),
    Dial(Dial),
}

pub struct Preset {
    pub group: String,
    pub name: String,
    pub item: Item,
}

/// What this machine has, found once when the editor starts.
#[derive(Default)]
pub struct Env {
    pub hyprland: bool,
    /// Hyprland configured in Lua, where dispatchers are written as hl.dsp calls.
    pub hypr_lua: bool,
    pub playerctl: bool,
    pub screenshot: bool,
    pub terminal: Option<String>,
    /// PipeWire source of an Elgato XLR Dock.
    pub dock: Option<String>,
    /// (filter capture node, filtered source) for each RNNoise filter-chain.
    pub filters: Vec<(String, String)>,
    /// Corsair devices seen by OpenLinkHub.
    pub corsair: Vec<corsair::Device>,
    /// Lights OpenRGB controls.
    pub rgb: Vec<openrgb::Device>,
    /// Wireless peripherals with a battery, of any brand.
    pub batteries: Vec<upower::Battery>,
    /// Headsets HeadsetControl sees.
    pub headsets: Vec<headset::Headset>,
    /// Installed plugins that can run here.
    pub plugins: Vec<plugins::Plugin>,
}

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

impl Env {
    /// `modules` are the optional modules switched on in the config.
    pub fn detect(modules: &[String]) -> Env {
        let nodes = pw::nodes();
        Env {
            hyprland: std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some(),
            hypr_lua: crate::config::dir().with_file_name("hypr").join("hyprland.lua").exists(),
            playerctl: has("playerctl"),
            screenshot: has("grim") && has("slurp") && has("wl-copy"),
            terminal: ["kitty", "alacritty", "foot", "wezterm", "ghostty", "konsole", "gnome-terminal", "xterm"]
                .into_iter()
                .find(|t| has(t))
                .map(String::from),
            dock: Path::new("/proc/asound/Dock")
                .exists()
                .then(|| nodes.iter().find(|n| n.class == "Audio/Source" && n.name.contains("XLR_Dock")).map(|n| n.name.clone()))
                .flatten(),
            // The service's own effect chain is edited in the Microphone view, not offered here.
            filters: pw::rnnoise_filters().into_iter().filter(|(f, _)| f.name != crate::effects::CAPTURE).filter_map(|(f, s)| Some((f.name, s?.name))).collect(),
            plugins: plugins::enabled().into_iter().filter(|p| p.missing().is_none()).collect(),
            corsair: if modules.iter().any(|m| m == "corsair") { corsair::devices() } else { Vec::new() },
            rgb: if modules.iter().any(|m| m == "openrgb") { openrgb::devices() } else { Vec::new() },
            batteries: upower::batteries(),
            headsets: if modules.iter().any(|m| m == "headset") { headset::headsets() } else { Vec::new() },
        }
    }
}

const RED: &str = "#b3261e";
const AMBER: &str = "#8a5a00";

fn key(label: &'static str, icon: char, command: &str, status: &str, interval: f32) -> Item {
    Item::Key(Key { label: t(label).into(), icon: icon.to_string(), command: command.into(), status: status.into(), interval, ..Default::default() })
}

fn dial(label: &'static str, icon: char, [right, left, press]: [String; 3], status: String) -> Dial {
    Dial { label: t(label).into(), icon: icon.to_string(), right, left, press, status, ..Default::default() }
}

fn wpctl_dial(label: &'static str, icon: char, target: &str) -> Item {
    Item::Dial(dial(
        label,
        icon,
        [
            format!("wpctl set-volume -l 1 {target} $((FLUXDECK_STEPS*2))%+"),
            format!("wpctl set-volume {target} $((FLUXDECK_STEPS*2))%-"),
            format!("wpctl set-mute {target} toggle"),
        ],
        format!("wpctl get-volume {target} | awk '{{ if ($3 == \"[MUTED]\") print \"mute||{RED}\"; else print int($2*100 + 0.5) \"%\" }}'"),
    ))
}

/// The app a dial or key from "Volumen de una app" controls, read back from its status command.
pub fn app_of(status: &str) -> Option<String> {
    status.strip_prefix("fluxdeck mix app '")?.split('\'').next().map(String::from)
}

fn app_clean(app: &str) -> String {
    app.chars().filter(|c| !matches!(c, '\'' | '"' | '\\' | '$' | '`')).collect()
}

/// A "Carpeta" key's page before it is placed: placing it makes the folder page.
pub const NEW_FOLDER: &str = "\u{0}carpeta";

/// The app id an "Abrir una app" key starts.
pub fn launched_app(command: &str) -> Option<String> {
    command.strip_prefix("fluxdeck launch '")?.split('\'').next().map(String::from)
}

/// A key that opens an installed app, with its name and icon.
pub fn launch_key(app: Option<&crate::apps::App>) -> Key {
    match app {
        Some(a) => Key {
            label: a.name.clone(),
            icon: crate::apps::icon_file(&a.icon).map(|p| p.display().to_string()).unwrap_or_else(|| '\u{f08c6}'.to_string()),
            command: format!("fluxdeck launch '{}'", app_clean(&a.id)),
            ..Default::default()
        },
        None => Key { label: t("App").into(), icon: '\u{f08c6}'.to_string(), command: "fluxdeck launch ''".into(), ..Default::default() },
    }
}

/// The group id a "Volumen de un grupo" slot controls.
pub fn group_id_of(status: &str) -> Option<String> {
    status.strip_prefix("fluxdeck mix group '")?.split('\'').next().map(String::from)
}

/// A dial for an app group: turn to change its volume, press to mute it.
pub fn group_dial(g: Option<&crate::groups::Group>) -> Dial {
    let (id, name, icon) = g.map_or((String::new(), t("Grupo").to_string(), '\u{f075a}'.to_string()), |g| (g.id.clone(), g.name.clone(), g.icon.clone()));
    let base = format!("fluxdeck mix group '{}'", app_clean(&id));
    Dial { label: name, icon, right: format!("{base} +$((2*FLUXDECK_STEPS))"), left: format!("{base} -$((2*FLUXDECK_STEPS))"), press: format!("{base} toggle"), status: format!("{base} short"), ..Default::default() }
}

/// A key that mutes and unmutes an app group.
pub fn group_key(g: Option<&crate::groups::Group>) -> Key {
    let (id, name, icon) = g.map_or((String::new(), t("Grupo").to_string(), '\u{f075a}'.to_string()), |g| (g.id.clone(), g.name.clone(), g.icon.clone()));
    let base = format!("fluxdeck mix group '{}'", app_clean(&id));
    Key { label: name, icon, command: format!("{base} toggle"), status: base, interval: 2.0, ..Default::default() }
}

/// A dial for one app's volume: turn to change it, press to mute.
pub fn app_dial(app: &str) -> Dial {
    let a = app_clean(app);
    let base = format!("fluxdeck mix app '{a}'");
    Dial {
        label: if a.is_empty() { t("App").into() } else { a.clone() },
        icon: '\u{f075a}'.to_string(),
        right: format!("{base} +$((2*FLUXDECK_STEPS))"),
        left: format!("{base} -$((2*FLUXDECK_STEPS))"),
        press: format!("{base} toggle"),
        status: format!("{base} short"),
        ..Default::default()
    }
}

/// A key that mutes and unmutes one app.
pub fn app_key(app: &str) -> Key {
    let a = app_clean(app);
    let base = format!("fluxdeck mix app '{a}'");
    Key { label: if a.is_empty() { t("App").into() } else { a.clone() }, icon: '\u{f075a}'.to_string(), command: format!("{base} toggle"), status: base, interval: 2.0, ..Default::default() }
}

pub fn all(env: &Env) -> Vec<Preset> {
    let mut out = Vec::new();
    let mut add = |group: &'static str, name: &'static str, item: Item| out.push(Preset { group: t(group).into(), name: t(name).into(), item });

    if let Some(t) = &env.terminal {
        add("Sistema", "Terminal", key("Terminal", '\u{e795}', t, "", 0.0));
    }
    add("Sistema", "Abrir una app", Item::Key(launch_key(None)));
    add("Sistema", "Carpeta", Item::Key(Key { label: t("Carpeta").into(), icon: '\u{f024b}'.to_string(), page: NEW_FOLDER.into(), ..Default::default() }));
    add(
        "Sistema",
        "Secuencia de pasos",
        Item::Key(Key { label: t("Secuencia").into(), icon: '\u{f027b}'.to_string(), steps: vec![crate::Step::Wait { ms: 300 }], ..Default::default() }),
    );
    add("Sistema", "Carpeta personal", key("Archivos", '\u{f024b}', "xdg-open ~", "", 0.0));
    add("Sistema", "Reloj", key("", '\u{f0150}', "", "date +%H:%M", 1.0));
    add("Sistema", "Uso de CPU", key("CPU", '\u{f0ee0}', "", "top -bn1 | awk '/^%Cpu/ { print int(100 - $8) \"%\" }'", 2.0));
    add("Sistema", "Uso de RAM", key("RAM", '\u{f035b}', "", "free | awk '/^Mem/ { print int($3 / $2 * 100) \"%\" }'", 3.0));
    add("Sistema", "Bloquear sesión", key("Bloquear", '\u{f033e}', "loginctl lock-session", "", 0.0));
    if env.screenshot {
        add("Sistema", "Captura de una zona", key("Captura", '\u{f0e51}', "grim -g \"$(slurp)\" - | wl-copy", "", 0.0));
    }

    add("Audio", "Volumen de salida", wpctl_dial("Volumen", '\u{f057e}', "@DEFAULT_AUDIO_SINK@"));
    add("Audio", "Volumen de una app", Item::Dial(app_dial("")));
    add("Audio", "Silenciar una app", Item::Key(app_key("")));
    add("Audio", "Volumen de un grupo", Item::Dial(group_dial(None)));
    add("Audio", "Silenciar un grupo", Item::Key(group_key(None)));
    add("Audio", "Volumen del micrófono", wpctl_dial("Micrófono", '\u{f036c}', "@DEFAULT_AUDIO_SOURCE@"));
    add(
        "Audio",
        "Silenciar salida",
        key(
            "Sonido",
            '\u{f057e}',
            "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
            &format!("wpctl get-volume @DEFAULT_AUDIO_SINK@ | grep -q MUTED && echo '{}||{RED}' || echo '{}'", t("Silencio"), t("Sonido")),
            1.0,
        ),
    );
    if env.playerctl {
        add(
            "Multimedia",
            "Reproducir / pausar",
            key(
                "",
                '\u{f040e}',
                "playerctl play-pause",
                if crate::i18n::english() { "playerctl status 2>/dev/null" } else { "playerctl status 2>/dev/null | sed 's/Playing/Sonando/; s/Paused/Pausa/; s/Stopped/Detenido/'" },
                1.0,
            ),
        );
        add("Multimedia", "Siguiente", key("Siguiente", '\u{f04ad}', "playerctl next", "", 0.0));
        add("Multimedia", "Anterior", key("Anterior", '\u{f04ae}', "playerctl previous", "", 0.0));
    }

    if env.hyprland {
        let [close, full, float] = if env.hypr_lua {
            ["hl.dsp.window.close()", "hl.dsp.window.fullscreen()", "hl.dsp.window.float({ action = \"toggle\" })"].map(|d| format!("hyprctl dispatch '{d}'"))
        } else {
            ["killactive", "fullscreen", "togglefloating"].map(|d| format!("hyprctl dispatch {d}"))
        };
        add("Ventanas", "Cerrar ventana", key("Cerrar", '\u{f05ad}', &close, "", 0.0));
        add("Ventanas", "Pantalla completa", key("Completa", '\u{f0293}', &full, "", 0.0));
        add("Ventanas", "Flotante", key("Flotante", '\u{f10ac}', &float, "", 0.0));
    }

    if let Some(source) = &env.dock {
        let gain = "amixer -c Dock sget Mic | awk -F'[][]' '/Capture [0-9]/ { db = $4; sw = $6 } END { if (sw == \"off\") print \"Mute||#b3261e\"; else printf \"%g dB|%.2f\\n\", db, db / 75 }'";
        add(
            "XLR Dock",
            "Ganancia (dB)",
            Item::Dial(dial(
                "Ganancia",
                '\u{f036c}',
                ["amixer -q -c Dock sset Mic ${FLUXDECK_STEPS}dB+".into(), "amixer -q -c Dock sset Mic ${FLUXDECK_STEPS}dB-".into(), "amixer -q -c Dock sset Mic toggle".into()],
                gain.into(),
            )),
        );
        let phones = "amixer -c Dock sget PCM | awk -F'[][]' '/Playback [0-9]/ { db = $4; sw = $6 } END { if (sw == \"off\") print \"Mute||#b3261e\"; else printf \"%g dB|%.2f\\n\", db, (db + 60) / 60 }'";
        add(
            "XLR Dock",
            "Audífonos",
            Item::Dial(dial(
                "Audífonos",
                '\u{f02cb}',
                ["amixer -q -c Dock sset PCM ${FLUXDECK_STEPS}dB+".into(), "amixer -q -c Dock sset PCM ${FLUXDECK_STEPS}dB-".into(), "amixer -q -c Dock sset PCM toggle".into()],
                phones.into(),
            )),
        );
        add(
            "XLR Dock",
            "Silenciar micrófono",
            key("Micrófono", '\u{f036c}', "amixer -q -c Dock sset Mic toggle", &format!("amixer -c Dock sget Mic | grep -q '\\[off\\]' && echo '{}||{RED}' || echo '{}'", t("Silenciado"), t("Micrófono")), 1.0),
        );
        add(
            "XLR Dock",
            "Nivel de entrada",
            Item::Dial(Dial { label: t("Nivel").into(), icon: '\u{f147d}'.to_string(), meter: source.clone(), ..Default::default() }),
        );
        add(
            "XLR Dock",
            "Phantom 48 V",
            key("48 V", '\u{f0241}', "fluxdeck xlr phantom toggle", &format!("[ \"$(fluxdeck xlr phantom)\" = on ] && echo '48 V||{AMBER}' || echo '{}'", t("Sin 48 V")), 2.0),
        );
        add(
            "XLR Dock",
            "Baja impedancia",
            key("Baja Z", '\u{f095c}', "fluxdeck xlr lowz toggle", &format!("[ \"$(fluxdeck xlr lowz)\" = on ] && echo '{}||{AMBER}' || echo '{}'", t("Baja Z"), t("Z normal")), 2.0),
        );
    }

    for (filter, filtered) in &env.filters {
        let vad = "rnnoise:VAD Threshold (%)";
        add(
            "Micrófono con filtro",
            "Fuerza de la supresión",
            Item::Dial(dial(
                "Supresión",
                '\u{f0232}',
                [
                    format!("fluxdeck pw param {filter} '{vad}' +$((5*FLUXDECK_STEPS)) 0 99"),
                    format!("fluxdeck pw param {filter} '{vad}' -$((5*FLUXDECK_STEPS)) 0 99"),
                    String::new(),
                ],
                format!("echo \"$(fluxdeck pw param {filter} '{vad}')%\""),
            )),
        );
        if let Some(raw) = &env.dock {
            add(
                "Micrófono con filtro",
                "Con filtro / directo",
                key(
                    "Filtro",
                    '\u{f0ea2}',
                    &format!("fluxdeck pw toggle-default {filtered} {raw}"),
                    &format!("[ \"$(fluxdeck pw default)\" = {filtered} ] && echo '{}' || echo '{}||{AMBER}'", t("Con filtro"), t("Directo")),
                    2.0,
                ),
            );
        }
        add(
            "Micrófono con filtro",
            "Nivel con filtro",
            Item::Dial(Dial { label: t("Voz").into(), icon: '\u{f147d}'.to_string(), meter: filtered.clone(), ..Default::default() }),
        );
    }

    // The service's own effect chain (effects.toml).
    if let Some(chain) = crate::effects::Chain::load() {
        let (cap, src) = (crate::effects::CAPTURE, crate::effects::SOURCE);
        if let Some(raw) = &env.dock {
            add(
                "Efectos del micrófono",
                "Con efectos / directo",
                key(
                    "Efectos",
                    '\u{f0ea2}',
                    &format!("fluxdeck pw toggle-default {src} {raw}"),
                    &format!("[ \"$(fluxdeck pw default)\" = {src} ] && echo '{}' || echo '{}||{AMBER}'", t("Con efectos"), t("Directo")),
                    2.0,
                ),
            );
        }
        if let Some(rn) = chain.effects.iter().find(|e| e.controls.contains_key("VAD Threshold (%)")) {
            let vad = format!("{}:VAD Threshold (%)", rn.id);
            add(
                "Efectos del micrófono",
                "Fuerza de la supresión",
                Item::Dial(dial(
                    "Supresión",
                    '\u{f0232}',
                    [
                        format!("fluxdeck pw param {cap} '{vad}' +$((5*FLUXDECK_STEPS)) 0 99"),
                        format!("fluxdeck pw param {cap} '{vad}' -$((5*FLUXDECK_STEPS)) 0 99"),
                        String::new(),
                    ],
                    format!("echo \"$(fluxdeck pw param {cap} '{vad}')%\""),
                )),
            );
        }
        add("Efectos del micrófono", "Nivel con efectos", Item::Dial(Dial { label: t("Voz").into(), icon: '\u{f147d}'.to_string(), meter: src.into(), ..Default::default() }));
    }

    for d in &env.corsair {
        let (id, name) = (&d.serial, title(&d.product));
        let mut push = |what: &'static str, item: Item| out.push(Preset { group: "Corsair".into(), name: tf(what, &[&name]), item });
        if d.battery {
            push(
                "Batería de {}",
                Item::Key(Key { label: name.clone(), icon: corsair::icon(d.kind).to_string(), status: format!("fluxdeck corsair battery {id}"), interval: 60.0, ..Default::default() }),
            );
        }
        if d.anc {
            push(
                "Cancelación de ruido de {}",
                Item::Key(Key {
                    label: name.clone(),
                    icon: '\u{f07c5}'.to_string(),
                    command: format!("fluxdeck corsair anc {id} cycle"),
                    status: format!("fluxdeck corsair anc {id}"),
                    interval: 5.0,
                    ..Default::default()
                }),
            );
        }
        if d.lcd {
            push(
                "Pantalla de {}",
                Item::Key(Key {
                    label: name.clone(),
                    icon: '\u{f0379}'.to_string(),
                    command: format!("fluxdeck corsair lcd {id} next"),
                    status: format!("fluxdeck corsair lcd {id}"),
                    interval: 10.0,
                    ..Default::default()
                }),
            );
        }
        if d.lights {
            push(
                "Luces de {}",
                Item::Dial(Dial {
                    label: name.clone(),
                    icon: '\u{f0335}'.to_string(),
                    right: format!("fluxdeck corsair brightness {id} up"),
                    left: format!("fluxdeck corsair brightness {id} down"),
                    press: format!("fluxdeck corsair effect {id} next"),
                    status: format!("fluxdeck corsair brightness {id}"),
                    interval: 10.0,
                    ..Default::default()
                }),
            );
        }
    }
    for d in &env.rgb {
        let (id, name) = (sq(&d.id), title(&d.id));
        let mut push = |what: &'static str, item: Item| out.push(Preset { group: "OpenRGB".into(), name: tf(what, &[&name]), item });
        if d.modes.len() > 1 {
            push(
                "Luces de {}",
                Item::Dial(Dial {
                    label: name.clone(),
                    icon: '\u{f0335}'.to_string(),
                    right: format!("fluxdeck rgb effect {id} next"),
                    left: format!("fluxdeck rgb effect {id} prev"),
                    status: format!("fluxdeck rgb effect {id}"),
                    interval: 10.0,
                    ..Default::default()
                }),
            );
        }
        if d.modes.iter().any(|m| m.name.eq_ignore_ascii_case("off")) {
            push("Apagar las luces de {}", Item::Key(Key { label: name.clone(), icon: '\u{f0336}'.to_string(), command: format!("fluxdeck rgb effect {id} off"), ..Default::default() }));
        }
        if d.takes_color() {
            let color = d.color().unwrap_or_else(|| "ffffff".into());
            push(
                "Color de {}",
                Item::Key(Key { label: name.clone(), icon: '\u{f0266}'.to_string(), command: format!("fluxdeck rgb color {id} {color}"), status: format!("fluxdeck rgb color {id}"), interval: 10.0, ..Default::default() }),
            );
        }
        if d.brightness().is_some() {
            push(
                "Brillo de {}",
                Item::Dial(Dial {
                    label: name.clone(),
                    icon: '\u{f00e0}'.to_string(),
                    right: format!("fluxdeck rgb brightness {id} up"),
                    left: format!("fluxdeck rgb brightness {id} down"),
                    status: format!("fluxdeck rgb brightness {id}"),
                    interval: 10.0,
                    ..Default::default()
                }),
            );
        }
    }
    for b in &env.batteries {
        let name = title(&b.model);
        let key = Key { label: name.clone(), icon: corsair::icon(b.kind_code()).to_string(), status: format!("fluxdeck battery {}", sq(&b.model)), interval: 60.0, ..Default::default() };
        out.push(Preset { group: t("Baterías").into(), name: tf("Batería de {}", &[&name]), item: Item::Key(key) });
    }
    for h in &env.headsets {
        let (id, name) = (&h.id, title(&h.product));
        let mut push = |what: &'static str, item: Item| out.push(Preset { group: t("Auriculares").into(), name: tf(what, &[&name]), item });
        if h.battery.is_some() {
            push(
                "Batería de {}",
                Item::Key(Key { label: name.clone(), icon: corsair::icon(Some(2)).to_string(), status: format!("fluxdeck headset battery {id}"), interval: 60.0, ..Default::default() }),
            );
        }
        if h.has_sidetone {
            push(
                "Retorno de voz de {}",
                Item::Dial(Dial {
                    label: name.clone(),
                    icon: '\u{f036c}'.to_string(),
                    right: format!("fluxdeck headset sidetone {id} up"),
                    left: format!("fluxdeck headset sidetone {id} down"),
                    press: format!("fluxdeck headset sidetone {id} 0"),
                    status: format!("fluxdeck headset sidetone {id}"),
                    interval: 10.0,
                    ..Default::default()
                }),
            );
        }
        if h.has_lights {
            push(
                "Luces de {}",
                Item::Key(Key {
                    label: name.clone(),
                    icon: '\u{f0335}'.to_string(),
                    command: format!("fluxdeck headset lights {id} toggle"),
                    status: format!("fluxdeck headset lights {id}"),
                    interval: 30.0,
                    ..Default::default()
                }),
            );
        }
    }
    for p in &env.plugins {
        for a in &p.manifest.actions {
            let icon = plugins::image_file(&p.path, &a.icon).or_else(|| a.states.first().and_then(|s| plugins::image_file(&p.path, &s.image)));
            let icon = icon.map(|i| i.display().to_string()).unwrap_or_default();
            let (action, label, group) = (a.uuid.clone(), a.name.clone(), p.manifest.category.clone());
            if a.on_keys() {
                let item = Item::Key(Key { label: label.clone(), icon: icon.clone(), action: action.clone(), ..Default::default() });
                out.push(Preset { group: group.clone(), name: a.name.clone(), item });
            }
            if a.on_dials() {
                let name = if a.on_keys() { tf("{} (perilla)", &[&a.name]) } else { a.name.clone() };
                out.push(Preset { group, name, item: Item::Dial(Dial { label, icon, action, ..Default::default() }) });
            }
        }
    }
    out
}

/// A shell word that keeps spaces and quotes.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// "VIRTUOSO MAX" → "Virtuoso Max", keeping model codes such as K100 as they are.
pub fn title(product: &str) -> String {
    product
        .split_whitespace()
        .map(|w| if w.chars().any(|c| c.is_ascii_digit()) || w.len() <= 3 { w.to_string() } else { w[..1].to_string() + &w[1..].to_lowercase() })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_controls_read_back() {
        assert_eq!(super::app_of(&super::app_dial("Spotify").status).as_deref(), Some("Spotify"));
        assert_eq!(super::app_of(&super::app_key("it's").status).as_deref(), Some("its"));
        assert_eq!(super::app_of("date"), None);
    }
}
