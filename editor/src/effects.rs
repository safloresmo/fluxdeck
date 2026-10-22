//! Editor for the service's microphone effect chain (effects.toml): add, remove, reorder, switch
//! effects on and off, and move their values live.
use fluxdeck_core::effects::{self, Chain, Effect, Kind};
use fluxdeck_core::i18n::{t, tf};
use eframe::egui::{self, Color32, RichText};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Control {
    pub name: String,
    pub title: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub log: bool,
    pub int: bool,
    pub toggle: bool,
}

/// An installed effect, from `fluxdeck effects scan` or PipeWire's builtins.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Available {
    pub kind: Kind,
    pub plugin: String,
    pub label: String,
    pub name: String,
    pub maker: String,
    pub input: String,
    pub output: String,
    pub controls: Vec<Control>,
}

fn builtin_catalogue() -> Vec<Available> {
    effects::builtins()
        .into_iter()
        .map(|(label, name, controls)| Available {
            kind: Kind::Builtin,
            label: label.into(),
            name: t(name).into(),
            maker: "PipeWire".into(),
            input: "In".into(),
            output: "Out".into(),
            controls: controls.into_iter().map(|(n, d)| builtin_control(n, d)).collect(),
            ..Default::default()
        })
        .collect()
}

fn builtin_control(name: &str, default: f64) -> Control {
    let (min, max, log) = match name {
        "Freq" => (20.0, 20000.0, true),
        "Q" => (0.1, 10.0, true),
        _ => (-18.0, 18.0, false),
    };
    Control { name: name.into(), min, max, default, log, ..Default::default() }
}

/// Scans in the background, from a cached copy first so the list shows at once.
fn load_catalogue(into: Arc<Mutex<Vec<Available>>>) {
    let cache = fluxdeck_core::paths::cache().join("effects.json");
    let parse = |text: &str| serde_json::from_str::<Vec<Available>>(text).unwrap_or_default();
    let fresh = |list: Vec<Available>| {
        let mut all = builtin_catalogue();
        all.extend(list);
        all
    };
    *into.lock().unwrap() = fresh(parse(&std::fs::read_to_string(&cache).unwrap_or_default()));
    std::thread::spawn(move || {
        let out = std::process::Command::new(fluxdeck_core::paths::service_bin()).args(["effects", "scan"]).output();
        if let Ok(o) = out
            && o.status.success()
        {
            let text = String::from_utf8_lossy(&o.stdout);
            let _ = std::fs::create_dir_all(fluxdeck_core::paths::cache());
            let _ = std::fs::write(&cache, text.as_bytes());
            *into.lock().unwrap() = fresh(parse(&text));
        }
    });
}

pub struct EffectsEditor {
    pub chain: Chain,
    catalogue: Arc<Mutex<Vec<Available>>>,
    open: HashSet<String>,
    adding: bool,
    search: String,
    /// Values changed since the last save; saved once the user stops moving them.
    unsaved_since: Option<Instant>,
    seen: Option<SystemTime>,
    confirm_drop: bool,
    mics: Vec<fluxdeck_core::pw::Node>,
    /// (installed, checked) per pack, filled in the background.
    packs: Arc<Mutex<Vec<Option<bool>>>>,
    packs_checked: Option<Instant>,
    pub notice: String,
}

fn mtime() -> Option<SystemTime> {
    std::fs::metadata(effects::path()).and_then(|m| m.modified()).ok()
}

impl EffectsEditor {
    pub fn load() -> Option<EffectsEditor> {
        let chain = Chain::load()?;
        let catalogue = Arc::new(Mutex::new(Vec::new()));
        load_catalogue(catalogue.clone());
        Some(EffectsEditor {
            chain,
            catalogue,
            open: HashSet::new(),
            adding: false,
            search: String::new(),
            unsaved_since: None,
            seen: mtime(),
            confirm_drop: false,
            mics: effects::microphones(),
            packs: Arc::new(Mutex::new(vec![None; effects::PACKS.len()])),
            packs_checked: None,
            notice: String::new(),
        })
    }

    /// Picks up edits made elsewhere (by hand, or by a deck dial) when nothing here is pending.
    fn sync(&mut self) {
        if self.unsaved_since.is_none()
            && mtime() != self.seen
            && let Some(c) = Chain::load()
        {
            self.chain = c;
            self.seen = mtime();
        }
        if self.unsaved_since.is_some_and(|t| t.elapsed() > Duration::from_millis(800)) {
            self.save();
        }
    }

    fn save(&mut self) {
        match self.chain.save() {
            Ok(()) => self.notice.clear(),
            Err(e) => self.notice = tf("No se pudo guardar: {}", &[&e.to_string()]),
        }
        self.seen = mtime();
        self.unsaved_since = None;
    }

    fn find<'a>(list: &'a [Available], e: &Effect) -> Option<&'a Available> {
        list.iter().find(|a| a.kind == e.kind && a.plugin == e.plugin && a.label == e.label)
    }

    /// Returns (control, value) changes to apply live.
    pub fn ui(&mut self, ui: &mut egui::Ui, accent: Color32) -> Vec<(String, f64)> {
        self.sync();
        let mut live = Vec::new();
        self.mic_ui(ui);
        ui.add_space(6.0);
        let catalogue = self.catalogue.lock().unwrap().clone();
        let mut structural = false;
        let mut action: Option<(usize, i32)> = None;
        let n = self.chain.effects.len();
        for i in 0..n {
            let id = self.chain.effects[i].id.clone();
            let open = self.open.contains(&id);
            ui.horizontal(|ui| {
                let e = &mut self.chain.effects[i];
                if ui.checkbox(&mut e.enabled, "").on_hover_text(t("Encender o apagar este efecto")).changed() {
                    structural = true;
                }
                let arrow = if open { "▾" } else { "▸" };
                let title = RichText::new(format!("{arrow} {}", e.title)).strong().color(if e.enabled { ui.visuals().text_color() } else { ui.visuals().weak_text_color() });
                if ui.add(egui::Label::new(title).sense(egui::Sense::click())).clicked() {
                    if open {
                        self.open.remove(&id);
                    } else {
                        self.open.insert(id.clone());
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text(t("Quitar efecto")).clicked() {
                        action = Some((i, 0));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                        action = Some((i, 1));
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                        action = Some((i, -1));
                    }
                });
            });
            if open {
                let known = Self::find(&catalogue, &self.chain.effects[i]).cloned();
                let e = &mut self.chain.effects[i];
                let names: Vec<String> = e.controls.keys().cloned().collect();
                ui.indent(("fx", i), |ui| {
                    for name in names {
                        let spec = known.as_ref().and_then(|k| k.controls.iter().find(|c| c.name == name));
                        let Some((label, range, suffix, log)) = describe(&e.label, &name, spec) else { continue };
                        let v = e.controls.get_mut(&name).unwrap();
                        let mut f = *v as f32;
                        let changed = ui
                            .horizontal(|ui| {
                                ui.add_sized([130.0, 20.0], egui::Label::new(label).truncate());
                                if spec.is_some_and(|s| s.toggle) {
                                    let mut on = f >= 0.5;
                                    let r = ui.checkbox(&mut on, "");
                                    f = if on { 1.0 } else { 0.0 };
                                    r.changed()
                                } else {
                                    ui.add(egui::Slider::new(&mut f, range).logarithmic(log).suffix(suffix)).changed()
                                }
                            })
                            .inner;
                        if changed {
                            *v = f as f64;
                            if e.enabled {
                                live.push((format!("{}:{name}", e.id), *v));
                            }
                            self.unsaved_since = Some(Instant::now());
                        }
                    }
                    if e.controls.is_empty() {
                        ui.label(RichText::new(t("Este efecto no tiene ajustes.")).weak().size(12.5));
                    }
                });
                ui.add_space(4.0);
            }
        }
        match action {
            Some((i, 0)) => {
                self.chain.effects.remove(i);
                structural = true;
            }
            Some((i, d)) => {
                let j = (i as i32 + d) as usize;
                self.chain.effects.swap(i, j);
                structural = true;
            }
            None => {}
        }

        ui.add_space(8.0);
        let label = if self.adding { t("Cerrar la lista") } else { t("+ Agregar efecto") };
        if ui.button(label).clicked() {
            self.adding = !self.adding;
        }
        if self.adding && self.add_ui(ui, &catalogue, accent) {
            structural = true;
        }
        if self.adding {
            self.packs_ui(ui, accent);
        }
        if structural {
            self.save();
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t("Los valores cambian al momento. Agregar, quitar, mover o apagar un efecto corta el micrófono un instante.")).weak().size(12.5));
        ui.add_space(4.0);
        let drop_label = if self.confirm_drop { t("¿Seguro? Pulsa otra vez") } else { t("Dejar de usar esta cadena") };
        if ui.small_button(drop_label).on_hover_text(t("Borra effects.toml y apaga el micrófono con efectos")).clicked() {
            if self.confirm_drop {
                let _ = std::fs::remove_file(effects::path());
                self.notice = t("Cadena quitada").into();
            }
            self.confirm_drop = !self.confirm_drop;
        }
        live
    }

    /// Which microphone the chain processes: any source works, USB, headset or built-in.
    fn mic_ui(&mut self, ui: &mut egui::Ui) {
        let current = self.mics.iter().find(|m| m.name == self.chain.target).map(|m| if m.description.is_empty() { m.name.clone() } else { m.description.clone() });
        let shown = current.unwrap_or_else(|| if self.chain.target.is_empty() { "—".into() } else { tf("{} (desconectado)", &[&self.chain.target]) });
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.label(t("Micrófono de entrada"));
            let resp = egui::ComboBox::from_id_salt("fx_mic").selected_text(shown).width(280.0).show_ui(ui, |ui| {
                for m in &self.mics {
                    let name = if m.description.is_empty() { &m.name } else { &m.description };
                    if ui.selectable_label(m.name == self.chain.target, name).clicked() {
                        pick = Some(m.name.clone());
                    }
                }
            });
            if resp.response.clicked() {
                self.mics = effects::microphones();
            }
        });
        if let Some(m) = pick.filter(|m| *m != self.chain.target) {
            self.chain.target = m;
            self.save();
        }
    }

    /// Effect packs from the system's package manager, installed in a terminal so the user types the password.
    fn packs_ui(&mut self, ui: &mut egui::Ui, accent: Color32) {
        if self.packs_checked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.packs_checked = Some(Instant::now());
            let (packs, catalogue) = (self.packs.clone(), self.catalogue.clone());
            std::thread::spawn(move || {
                let now: Vec<Option<bool>> = effects::PACKS.iter().map(|p| Some(p.installed())).collect();
                let mut old = packs.lock().unwrap();
                // A pack that just got installed brings new effects to the list.
                let newly = old.iter().zip(&now).any(|(o, n)| *o == Some(false) && *n == Some(true));
                *old = now;
                drop(old);
                if newly {
                    load_catalogue(catalogue);
                }
            });
        }
        ui.add_space(10.0);
        ui.label(RichText::new(t("Paquetes de efectos")).strong());
        if effects::package_manager().is_none() {
            ui.label(RichText::new(t("No reconozco el gestor de paquetes de este sistema; instala los paquetes a mano.")).weak().size(12.5));
            return;
        }
        let state = self.packs.lock().unwrap().clone();
        for (i, p) in effects::PACKS.iter().enumerate() {
            let Some(cmd) = p.install_command() else { continue };
            ui.horizontal(|ui| {
                match state[i] {
                    Some(true) => drop(ui.label(RichText::new(t("Instalado")).color(accent).size(12.5))),
                    Some(false) => {
                        if ui.small_button(t("Instalar")).on_hover_text(&cmd).clicked() {
                            match effects::run_in_terminal(&cmd) {
                                Ok(()) => self.notice = t("Escribe tu contraseña en la terminal que se abrió.").into(),
                                Err(e) => self.notice = e,
                            }
                        }
                    }
                    None => drop(ui.spinner()),
                }
                ui.label(RichText::new(p.name).strong());
            });
            ui.label(RichText::new(t(p.about)).weak().size(12.0));
        }
    }

    /// The list of installed effects; true when one was added.
    fn add_ui(&mut self, ui: &mut egui::Ui, catalogue: &[Available], accent: Color32) -> bool {
        ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(t("Buscar efectos… (compresor, gate, eq)")).desired_width(f32::INFINITY));
        let q = self.search.trim().to_lowercase();
        let mut added = None;
        egui::ScrollArea::vertical().id_salt("add_fx").max_height(260.0).show(ui, |ui| {
            for a in catalogue.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.maker.to_lowercase().contains(&q) || a.label.to_lowercase().contains(&q)) {
                ui.horizontal(|ui| {
                    if ui.small_button("+").clicked() {
                        added = Some(a.clone());
                    }
                    ui.label(&a.name);
                    let kind = match a.kind {
                        Kind::Builtin => "PipeWire",
                        Kind::Ladspa => "LADSPA",
                        Kind::Lv2 => "LV2",
                    };
                    ui.label(RichText::new(format!("{kind} · {}", a.maker)).weak().size(11.5).color(if a.kind == Kind::Builtin { accent } else { ui.visuals().weak_text_color() }));
                });
            }
        });
        let Some(a) = added else { return false };
        let id = self.chain.new_id(&a.name);
        let controls = a.controls.iter().map(|c| (c.name.clone(), c.default)).collect();
        self.chain.effects.push(Effect { id: id.clone(), title: a.name.clone(), kind: a.kind, plugin: a.plugin.clone(), label: a.label.clone(), input: a.input.clone(), output: a.output.clone(), enabled: true, controls });
        self.open.insert(id);
        self.notice = tf("«{}» agregado", &[&a.name]);
        true
    }
}

/// Label, range, suffix and log scale for a control; None hides internals (biquad coefficients, placeholders).
fn describe(label: &str, control: &str, spec: Option<&Control>) -> Option<(String, std::ops::RangeInclusive<f32>, &'static str, bool)> {
    let known = crate::mic::describe(label, control);
    // A control mic::describe does not know comes back under its own name; the catalogue describes it better.
    if let (Some(k), Some(_)) = (&known, spec)
        && k.0 == control
    {
        return describe_spec(spec?);
    }
    if let Some(known) = known {
        // The catalogue's range wins when it knows one; the Spanish label and suffix stay.
        return Some(match spec {
            Some(s) if s.min < s.max => (known.0, s.min as f32..=s.max as f32, known.2, known.3 || s.log),
            _ => known,
        });
    }
    describe_spec(spec?)
}

fn describe_spec(s: &Control) -> Option<(String, std::ops::RangeInclusive<f32>, &'static str, bool)> {
    if matches!(s.name.as_str(), "Placeholder" | "Dry Mix" | "freeWheeling") {
        return None;
    }
    let title = if s.title.is_empty() { s.name.clone() } else { s.title.clone() };
    Some((title, s.min as f32..=s.max.max(s.min + 1e-3) as f32, "", s.log))
}
