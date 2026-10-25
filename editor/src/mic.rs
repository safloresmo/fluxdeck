//! Wave Link-style view: the XLR Dock's hardware controls and the microphone's filter chain.
use fluxdeck_core::i18n::t;
use fluxdeck_core::effects::{self, CAPTURE, SOURCE};
use fluxdeck_core::pw;
use crate::effects::EffectsEditor;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Default, PartialEq)]
struct Snapshot {
    /// (dB, on) of the Dock's preamp and headphone output.
    gain: Option<(f32, bool)>,
    phones: Option<(f32, bool)>,
    /// None when the Dock's USB control is not reachable (udev rule missing).
    phantom: Option<bool>,
    lowz: Option<bool>,
    filtered: bool,
    /// Node name of the default source.
    default: Option<String>,
    params: Vec<(String, f64)>,
}

/// Writes are keyed so a fast slider drag only applies its latest value.
#[derive(Clone)]
enum Write {
    Amixer(&'static str, String),
    Xlr(&'static str, bool),
    Default(String),
    Param(String, f64),
}

impl Write {
    fn key(&self) -> String {
        match self {
            Write::Amixer(c, v) => format!("a{c}{}", v.ends_with("dB")),
            Write::Xlr(w, _) => format!("x{w}"),
            Write::Default(_) => "default".into(),
            Write::Param(n, _) => format!("p{n}"),
        }
    }

    fn apply(&self, xlr: &std::path::Path) {
        match self {
            Write::Amixer(ctl, v) => drop(Command::new("amixer").args(["-q", "-c", "Dock", "sset", ctl, v]).status()),
            Write::Xlr(what, on) => drop(Command::new(xlr).args(["xlr", what, if *on { "on" } else { "off" }]).stdout(Stdio::null()).status()),
            Write::Default(n) => drop(pw::set_default(n)),
            Write::Param(n, v) => {
                if let Some((node, _)) = n.split_once('\u{0}') {
                    pw::set_param(node, &n[node.len() + 1..], *v);
                }
            }
        }
    }
}

pub struct Mic {
    dock: Option<String>,
    /// (filter capture node, filtered source)
    filter: Option<(String, String)>,
    shown: Snapshot,
    initial: Option<Vec<(String, f64)>>,
    latest: Arc<Mutex<Option<Snapshot>>>,
    active: Arc<AtomicBool>,
    edited: Instant,
    writes: Sender<Write>,
    meters: Vec<Child>,
    levels: [Arc<Mutex<f32>>; 2],
    shown_levels: [f32; 2],
    /// The service's own effect chain, when effects.toml exists.
    effects: Option<EffectsEditor>,
    /// Any microphone at all, so the view is offered even without a Dock or a filter.
    any_mic: bool,
    /// The loopback that lets you hear yourself.
    monitor: Option<Child>,
    /// When 48 V was asked for once; a second click soon after turns it on.
    phantom_armed: Option<Instant>,
    pub notice: String,
}

fn fluxdeck_bin() -> std::path::PathBuf {
    fluxdeck_core::paths::service_bin()
}

/// Reads "[75.00dB] [on]" from `amixer sget`.
fn amixer(ctl: &str) -> Option<(f32, bool)> {
    let out = Command::new("amixer").args(["-c", "Dock", "sget", ctl]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.contains("dB]"))?;
    let fields: Vec<&str> = line.split('[').skip(1).filter_map(|f| f.split(']').next()).collect();
    let db = fields.iter().find(|f| f.ends_with("dB"))?.trim_end_matches("dB").parse().ok()?;
    Some((db, !fields.contains(&"off")))
}

fn xlr(what: &str) -> Option<bool> {
    let out = Command::new(fluxdeck_bin()).args(["xlr", what]).stderr(Stdio::null()).output().ok()?;
    match String::from_utf8_lossy(&out.stdout).trim() {
        "on" => Some(true),
        "off" => Some(false),
        _ => None,
    }
}

impl Mic {
    pub fn new(dock: Option<String>, filter: Option<(String, String)>) -> Mic {
        let latest = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        {
            let (latest, active, dock, filter) = (latest.clone(), active.clone(), dock.is_some(), filter.clone());
            std::thread::spawn(move || {
                loop {
                    if active.load(Ordering::Relaxed) {
                        let snap = Snapshot {
                            gain: dock.then(|| amixer("Mic")).flatten(),
                            phones: dock.then(|| amixer("PCM")).flatten(),
                            phantom: dock.then(|| xlr("phantom")).flatten(),
                            lowz: dock.then(|| xlr("lowz")).flatten(),
                            filtered: filter.as_ref().is_some_and(|(_, src)| pw::default_source().as_deref() == Some(src)),
                            default: pw::default_source(),
                            params: filter.as_ref().map(|(node, _)| pw::params(node)).unwrap_or_default(),
                        };
                        *latest.lock().unwrap() = Some(snap);
                    }
                    std::thread::sleep(Duration::from_millis(700));
                }
            });
        }
        let (writes, rx) = channel::<Write>();
        std::thread::spawn(move || {
            let bin = fluxdeck_bin();
            while let Ok(first) = rx.recv() {
                let mut batch: Vec<Write> = vec![first];
                batch.extend(rx.try_iter());
                let mut last: HashMap<String, Write> = HashMap::new();
                for w in batch {
                    last.insert(w.key(), w);
                }
                for w in last.values() {
                    w.apply(&bin);
                }
            }
        });
        Mic {
            dock,
            filter,
            shown: Snapshot::default(),
            initial: None,
            latest,
            active,
            edited: Instant::now() - Duration::from_secs(10),
            writes,
            meters: Vec::new(),
            levels: [Arc::new(Mutex::new(0.0)), Arc::new(Mutex::new(0.0))],
            shown_levels: [0.0; 2],
            effects: EffectsEditor::load(),
            any_mic: !effects::microphones().is_empty(),
            monitor: None,
            phantom_armed: None,
            notice: String::new(),
        }
    }

    pub fn available(&self) -> bool {
        self.dock.is_some() || self.filter.is_some() || self.effects.is_some() || self.any_mic
    }

    /// The processed microphone the second meter follows: the effect chain when there is one.
    fn processed(&self) -> Option<String> {
        if self.effects.is_some() { Some(SOURCE.into()) } else { self.filter.as_ref().map(|f| f.1.clone()) }
    }

    /// Notices effects.toml appearing or going away.
    fn follow_chain(&mut self) {
        let exists = effects::path().exists();
        if exists != self.effects.is_some() {
            self.effects = if exists { EffectsEditor::load() } else { None };
            // The second meter switches source.
            if !self.meters.is_empty() {
                self.set_active(false);
                self.set_active(true);
            }
        }
    }

    /// Starts the effect chain as a copy of the system filter, or empty when there is none.
    fn create_chain(&mut self) {
        let copied = self.filter.as_ref().and_then(|(node, _)| effects::import_system(node));
        let first = || self.dock.clone().or_else(|| effects::microphones().first().map(|m| m.name.clone())).unwrap_or_default();
        let base = copied.unwrap_or_else(|| effects::Chain { target: first(), ..Default::default() });
        let chain = effects::Chain { description: t("Micrófono con efectos").into(), ..base };
        match chain.save() {
            Ok(()) => self.notice = t("Cadena creada: es una copia de la del sistema").into(),
            Err(e) => self.notice = e.to_string(),
        }
        self.follow_chain();
    }

    /// Starts polling and meters while the view is open, stops them when it closes.
    pub fn set_active(&mut self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
        if on && self.meters.is_empty() {
            let sources = [self.dock.clone(), self.processed()];
            for (i, src) in sources.into_iter().enumerate() {
                let Some(src) = src else { continue };
                let level = self.levels[i].clone();
                if let Some(c) = pw::meter(&src, move |l| {
                    *level.lock().unwrap() = l;
                    true
                }) {
                    self.meters.push(c);
                }
            }
        } else if !on {
            for mut c in self.meters.drain(..) {
                let _ = c.kill();
                let _ = c.wait();
            }
            // Hearing yourself only while this view is open, so it is never left on by mistake.
            self.listen(false);
        }
    }

    /// Plays the processed microphone into the default output, to hear changes as they are made.
    fn listen(&mut self, on: bool) {
        if let Some(mut c) = self.monitor.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if !on {
            return;
        }
        let Some(src) = self.processed().or_else(|| self.dock.clone()) else { return };
        let name = format!("{}-escucha", fluxdeck_core::paths::NAME);
        self.monitor = Command::new("pw-loopback")
            .args(["-n", &name, "-C", &src, "-l", "10"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        if self.monitor.is_none() {
            self.notice = t("No se pudo iniciar la escucha (falta pw-loopback).").into();
        }
    }

    fn listen_button(&mut self, ui: &mut egui::Ui) {
        let on = self.monitor.is_some();
        ui.horizontal(|ui| {
            if toggle(ui, on, &format!("\u{f02cb}  {}", t("Escucharme")), Color32::from_rgb(46, 125, 50)) {
                self.listen(!on);
            }
            ui.label(RichText::new(t("Con audífonos: en bocinas se acopla. Se apaga al salir de esta vista.")).weak().size(12.0));
        });
        ui.add_space(6.0);
    }

    fn send(&mut self, w: Write) {
        self.edited = Instant::now();
        let _ = self.writes.send(w);
    }

    fn sync(&mut self, ctx: &egui::Context) {
        // Keep what the user is moving; take the hardware's values once they let go.
        let busy = ctx.dragged_id().is_some() || self.edited.elapsed() < Duration::from_millis(1500);
        if let Some(s) = self.latest.lock().unwrap().take()
            && !busy
        {
            if self.initial.is_none() && !s.params.is_empty() {
                self.initial = Some(s.params.clone());
            }
            self.shown = s;
        }
        for i in 0..2 {
            let l = *self.levels[i].lock().unwrap();
            self.shown_levels[i] = l.max(self.shown_levels[i] - 0.02);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        self.follow_chain();
        self.sync(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(33));
        ui.add_space(20.0);
        if !self.available() {
            ui.vertical_centered(|ui| ui.label(RichText::new(t("No encontré un XLR Dock ni un filtro de micrófono.")).weak()));
            return;
        }
        let w = ((ui.available_width() - 60.0) / 2.0).max(320.0);
        ui.horizontal_top(|ui| {
            ui.add_space(20.0);
            if self.dock.is_some() {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(w - 40.0);
                    ui.vertical(|ui| self.dock_ui(ui, accent));
                });
            }
            ui.add_space(12.0);
            if self.filter.is_some() || self.effects.is_some() || self.dock.is_some() {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(w - 40.0);
                    egui::ScrollArea::vertical().id_salt("chain").show(ui, |ui| {
                        ui.vertical(|ui| if self.effects.is_some() { self.effects_ui(ui, accent) } else { self.chain_ui(ui, accent) })
                    });
                });
            }
        });
    }

    fn dock_ui(&mut self, ui: &mut egui::Ui, accent: Color32) {
        ui.label(RichText::new("XLR Dock").size(18.0).strong());
        ui.add_space(8.0);
        ui.label(t("Entrada"));
        level_bar(ui, self.shown_levels[0], accent);
        ui.add_space(10.0);
        if let Some((mut db, on)) = self.shown.gain {
            ui.label(t("Ganancia"));
            if ui.add(egui::Slider::new(&mut db, 0.0..=75.0).step_by(0.5).suffix(" dB")).changed() {
                self.shown.gain = Some((db, on));
                self.send(Write::Amixer("Mic", format!("{db}dB")));
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if let Some((db, on)) = self.shown.gain
                && toggle(ui, !on, t("Mute"), Color32::from_rgb(179, 38, 30))
            {
                self.shown.gain = Some((db, !on));
                self.send(Write::Amixer("Mic", if on { "nocap" } else { "cap" }.into()));
            }
            // 48 V only suits condenser mics, so turning it on takes a second click; off is immediate.
            let armed = self.phantom_armed.is_some_and(|at| at.elapsed() < Duration::from_secs(4));
            if armed {
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
            match self.shown.phantom {
                Some(p) if toggle(ui, p || armed, if armed { t("¿Encender 48 V?") } else { "48 V" }, Color32::from_rgb(138, 90, 0)) => {
                    if p || armed {
                        self.phantom_armed = None;
                        self.shown.phantom = Some(!p);
                        self.send(Write::Xlr("phantom", !p));
                    } else {
                        self.phantom_armed = Some(Instant::now());
                    }
                }
                None => drop(ui.add_enabled(false, egui::Button::new("48 V")).on_disabled_hover_text(t(NO_USB))),
                _ => {}
            }
            match self.shown.lowz {
                Some(z) if toggle(ui, z, t("Baja Z"), Color32::from_rgb(138, 90, 0)) => {
                    self.shown.lowz = Some(!z);
                    self.send(Write::Xlr("lowz", !z));
                }
                None => drop(ui.add_enabled(false, egui::Button::new(t("Baja Z"))).on_disabled_hover_text(t(NO_USB))),
                _ => {}
            }
        });
        if self.shown.phantom.is_none() {
            ui.label(RichText::new(t("48 V y Baja Z necesitan la regla udev de dist/.")).weak().size(12.5));
        }
        ui.add_space(14.0);
        if let Some((mut db, on)) = self.shown.phones {
            ui.label(t("Audífonos"));
            ui.horizontal(|ui| {
                if ui.add(egui::Slider::new(&mut db, -60.0..=0.0).step_by(0.5).suffix(" dB")).changed() {
                    self.shown.phones = Some((db, on));
                    self.send(Write::Amixer("PCM", format!("{db}dB")));
                }
                if toggle(ui, !on, t("Mute"), Color32::from_rgb(179, 38, 30)) {
                    self.shown.phones = Some((db, !on));
                    self.send(Write::Amixer("PCM", if on { "mute" } else { "unmute" }.into()));
                }
            });
        }
    }

    /// Which microphone apps get: straight from the Dock, the system filter, or the effect chain.
    fn source_picker(&mut self, ui: &mut egui::Ui, accent: Color32) {
        let mut options: Vec<(String, &str)> = Vec::new();
        if let Some(d) = &self.dock {
            options.push((d.clone(), t("Directo")));
        }
        if let Some((_, f)) = &self.filter {
            options.push((f.clone(), t("Filtro del sistema")));
        }
        if self.effects.is_some() {
            options.push((SOURCE.into(), t("Efectos")));
        }
        if options.len() < 2 {
            return;
        }
        ui.label(t("Micrófono que usan las apps"));
        ui.horizontal(|ui| {
            for (node, name) in options {
                let on = self.shown.default.as_deref() == Some(&node);
                if toggle(ui, on, name, accent) && !on {
                    self.shown.default = Some(node.clone());
                    self.shown.filtered = self.filter.as_ref().is_some_and(|f| f.1 == node);
                    self.send(Write::Default(node));
                }
            }
        });
        ui.add_space(8.0);
    }

    fn effects_ui(&mut self, ui: &mut egui::Ui, accent: Color32) {
        ui.label(RichText::new(t("Efectos")).size(18.0).strong());
        ui.add_space(8.0);
        self.source_picker(ui, accent);
        ui.label(t("Voz procesada"));
        level_bar(ui, self.shown_levels[1], accent);
        self.listen_button(ui);
        ui.add_space(8.0);
        let Some(fx) = self.effects.as_mut() else { return };
        let live = fx.ui(ui, accent);
        let notice = std::mem::take(&mut fx.notice);
        for (name, v) in live {
            self.send(Write::Param(format!("{CAPTURE}\u{0}{name}"), v));
        }
        if !notice.is_empty() {
            self.notice = notice;
        }
    }

    fn chain_ui(&mut self, ui: &mut egui::Ui, accent: Color32) {
        if self.filter.is_none() {
            ui.label(RichText::new(t("Procesamiento")).size(18.0).strong());
            ui.add_space(8.0);
            ui.label(RichText::new(t("No hay filtro de micrófono. Puedes crear una cadena de efectos desde cero.")).weak());
            if ui.button(t("Crear cadena de efectos")).clicked() {
                self.create_chain();
            }
            return;
        }
        let Some((node, filtered)) = self.filter.clone() else { return };
        ui.label(RichText::new(t("Procesamiento")).size(18.0).strong());
        ui.add_space(8.0);
        let mut on = self.shown.filtered;
        if ui.checkbox(&mut on, t("Usar el micrófono con filtro")).changed()
            && let Some(raw) = self.dock.clone()
        {
            self.shown.filtered = on;
            self.send(Write::Default(if on { filtered.clone() } else { raw }));
        }
        ui.label(t("Voz procesada"));
        level_bar(ui, self.shown_levels[1], accent);
        self.listen_button(ui);

        let mut group = String::new();
        let mut changes = Vec::new();
        for (name, value) in self.shown.params.iter_mut() {
            let Some((stage, control)) = name.split_once(':') else { continue };
            let Some((label, range, suffix, log)) = describe(stage, control) else { continue };
            if stage != group {
                group = stage.to_string();
                ui.add_space(10.0);
                ui.label(RichText::new(stage_name(stage)).strong());
            }
            let mut v = *value as f32;
            let resp = ui.horizontal(|ui| {
                ui.add_sized([120.0, 20.0], egui::Label::new(label));
                ui.add(egui::Slider::new(&mut v, range).logarithmic(log).suffix(suffix))
            });
            if resp.inner.changed() {
                *value = v as f64;
                changes.push(Write::Param(format!("{node}\u{0}{name}"), v as f64));
            }
        }
        for c in changes {
            self.send(c);
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.initial.is_some(), egui::Button::new(t("Restaurar"))).on_hover_text(t("Vuelve a los valores que había al abrir esta vista")).clicked()
                && let Some(init) = self.initial.clone()
            {
                for (n, v) in &init {
                    self.send(Write::Param(format!("{node}\u{0}{n}"), *v));
                }
                self.shown.params = init;
                self.notice = t("Valores restaurados").into();
            }
            if ui.button(t("Copiar valores")).on_hover_text(t("Para pegarlos en el archivo del filtro y que duren")).clicked() {
                ui.ctx().copy_text(conf_text(&self.shown.params));
                self.notice = t("Valores copiados").into();
            }
        });
        ui.label(RichText::new(t("Los cambios se aplican al momento y duran hasta que se reinicia el audio.")).weak().size(12.5));
        ui.add_space(10.0);
        if ui.button(t("Editar efectos")).on_hover_text(t("Crea una cadena propia a partir de esta: agrega, quita y reordena efectos sin tocar la del sistema")).clicked() {
            self.create_chain();
        }
    }
}

const NO_USB: &str = "Instala dist/70-fluxdeck.rules para controlar esto";


pub fn card_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).corner_radius(18).inner_margin(20)
}

pub fn toggle(ui: &mut egui::Ui, on: bool, text: &str, color: Color32) -> bool {
    let b = egui::Button::new(RichText::new(text).color(if on { Color32::WHITE } else { ui.visuals().text_color() })).min_size(vec2(72.0, 30.0));
    ui.add(if on { b.fill(color) } else { b }).clicked()
}

fn level_bar(ui: &mut egui::Ui, level: f32, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 7, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_width(rect.width() * level);
    p.rect_filled(fill, 7, if level > 0.92 { Color32::from_rgb(229, 72, 77) } else { accent });
    for db in [-48.0, -36.0, -24.0, -12.0, -6.0] {
        let x = rect.left() + rect.width() * ((db + 60.0) / 60.0);
        p.line_segment([egui::pos2(x, rect.top() + 3.0), egui::pos2(x, rect.bottom() - 3.0)], Stroke::new(1.0, Color32::from_black_alpha(90)));
    }
    let db = level * 60.0 - 60.0;
    ui.label(RichText::new(if level <= 0.0 { t("silencio").into() } else { format!("{db:.0} dBFS") }).weak().size(12.0));
}

fn stage_name(stage: &str) -> String {
    if stage == "rnnoise" {
        return t("Supresión de ruido").into();
    }
    let s = stage.replace('_', " ");
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Label, range, suffix and log scale for known filter-chain controls; None hides internals.
pub fn describe(stage: &str, control: &str) -> Option<(String, std::ops::RangeInclusive<f32>, &'static str, bool)> {
    let s = |l: &'static str| t(l).to_string();
    Some(match control {
        "VAD Threshold (%)" => (s("Umbral de voz"), 0.0..=99.0, " %", false),
        "VAD Grace Period (ms)" => (s("Retención"), 0.0..=1000.0, " ms", false),
        "Retroactive VAD Grace (ms)" => (s("Retención previa"), 0.0..=200.0, " ms", false),
        "Freq" => (s("Frecuencia"), 20.0..=20000.0, " Hz", true),
        "Q" => (s("Q"), 0.1..=10.0, "", true),
        "Gain" if stage.contains("alto") || stage.contains("high") || stage.contains("low") && !stage.contains("shelf") => return None,
        "Gain" => (s("Ganancia"), -18.0..=18.0, " dB", false),
        "Attack time (ms)" => (s("Ataque"), 1.5..=400.0, " ms", true),
        "Release time (ms)" => (s("Liberación"), 2.0..=800.0, " ms", true),
        "Threshold level (dB)" => (s("Umbral"), -60.0..=0.0, " dB", false),
        "Ratio (1:n)" => (s("Proporción"), 1.0..=20.0, ":1", false),
        "Knee radius (dB)" => (s("Rodilla"), 1.0..=10.0, " dB", false),
        "Makeup gain (dB)" => (s("Compensación"), 0.0..=24.0, " dB", false),
        "RMS/peak" => (s("RMS ↔ pico"), 0.0..=1.0, "", false),
        "Input gain (dB)" => (s("Entrada"), -20.0..=20.0, " dB", false),
        "Limit (dB)" => (s("Techo"), -20.0..=0.0, " dB", false),
        "Release time (s)" => (s("Liberación"), 0.01..=2.0, " s", true),
        "Placeholder" | "Dry Mix" | "b0" | "b1" | "b2" | "a0" | "a1" | "a2" => return None,
        other => (other.to_string(), -100.0..=100.0, "", false),
    })
}

/// The values grouped by stage, ready to paste into a filter-chain `control = { … }` block.
fn conf_text(params: &[(String, f64)]) -> String {
    let mut out = String::new();
    let mut group = "";
    for (name, v) in params {
        let Some((stage, control)) = name.split_once(':') else { continue };
        if describe(stage, control).is_none() {
            continue;
        }
        if stage != group {
            out.push_str(&format!("\n# {stage}\n"));
            group = stage;
        }
        out.push_str(&format!("\"{control}\" = {}\n", (v * 1000.0).round() / 1000.0));
    }
    out.trim_start().to_string()
}

impl Drop for Mic {
    fn drop(&mut self) {
        self.listen(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_internals_and_formats_conf() {
        assert!(describe("graves", "b0").is_none());
        assert!(describe("pasa_altos", "Gain").is_none());
        assert!(describe("graves", "Gain").is_some());
        let text = conf_text(&[("graves:Gain".into(), 4.5), ("graves:b0".into(), 1.0), ("compresor:Ratio (1:n)".into(), 3.0)]);
        assert_eq!(text, "# graves\n\"Gain\" = 4.5\n\n# compresor\n\"Ratio (1:n)\" = 3\n");
    }
}
