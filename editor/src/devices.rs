//! Peripherals view: one card per Corsair device seen by OpenLinkHub, per light OpenRGB controls and
//! per wireless peripheral UPower sees.
use crate::mic::{card_frame, toggle};
use fluxdeck_core::corsair::{self, LEVELS, Live};
use fluxdeck_core::presets::{Env, title};
use fluxdeck_core::{headset, openrgb, upower};
use fluxdeck_core::i18n::{t, tf};
use eframe::egui::{self, Color32, RichText, Sense, vec2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Write {
    Lcd(String, u8),
    Rotation(String, u8),
    Anc(String, i64),
    Brightness(String, u8),
    Effect(String, String),
    RgbEffect(String, String),
    RgbBrightness(String, u8),
    RgbColor(String, String),
    Sidetone(String, u8),
    HeadsetLights(String, bool),
}

impl Write {
    /// Writes of one kind to one device replace each other: only the last one in a burst is sent.
    fn target(&self) -> (std::mem::Discriminant<Write>, &str) {
        let id = match self {
            Write::Lcd(id, _) | Write::Rotation(id, _) | Write::Anc(id, _) | Write::Brightness(id, _) | Write::Effect(id, _) => id,
            Write::RgbEffect(id, _) | Write::RgbBrightness(id, _) | Write::RgbColor(id, _) => id,
            Write::Sidetone(id, _) | Write::HeadsetLights(id, _) => id,
        };
        (std::mem::discriminant(self), id)
    }
}

#[derive(Default)]
struct Others {
    rgb: Vec<openrgb::Device>,
    batteries: Vec<upower::Battery>,
    headsets: Vec<headset::Headset>,
}

pub struct Devices {
    list: Vec<corsair::Device>,
    shown: HashMap<String, Live>,
    latest: Arc<Mutex<Option<HashMap<String, Live>>>>,
    others: Others,
    latest_others: Arc<Mutex<Option<Others>>>,
    corsair_on: bool,
    active: Arc<AtomicBool>,
    edited: Instant,
    writes: Sender<Write>,
    errors: Arc<Mutex<Vec<String>>>,
    pub running: bool,
    pub notice: String,
}

impl Devices {
    /// `modules` are the optional modules switched on in the config.
    pub fn new(env: &Env, modules: &[String]) -> Devices {
        let list = &env.corsair;
        let rgb_on = modules.iter().any(|m| m == "openrgb");
        let headset_on = modules.iter().any(|m| m == "headset");
        let latest = Arc::new(Mutex::new(None));
        let latest_others = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));
        {
            let (latest, latest_others, active, serials) = (latest.clone(), latest_others.clone(), active.clone(), list.iter().map(|d| d.serial.clone()).collect::<Vec<_>>());
            std::thread::spawn(move || {
                // Ends with the view's owner: a new Devices replaces this one after a re-detect.
                while Arc::strong_count(&active) > 1 {
                    if active.load(Ordering::Relaxed) {
                        let all = serials.iter().map(|s| (s.clone(), corsair::live(s))).collect();
                        *latest.lock().unwrap() = Some(all);
                        let rgb = if rgb_on { openrgb::devices() } else { Vec::new() };
                        let headsets = if headset_on { headset::headsets() } else { Vec::new() };
                        *latest_others.lock().unwrap() = Some(Others { rgb, batteries: upower::batteries(), headsets });
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                }
            });
        }
        let (writes, rx) = channel::<Write>();
        {
            let errors = errors.clone();
            std::thread::spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut burst: Vec<Write> = std::iter::once(first).chain(rx.try_iter()).collect();
                    let mut i = 0;
                    while i < burst.len() {
                        if burst[i + 1..].iter().any(|later| later.target() == burst[i].target()) {
                            burst.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                    for w in burst {
                    let r = match &w {
                        Write::Lcd(id, m) => corsair::set_lcd(id, *m),
                        Write::Rotation(id, r) => corsair::set_lcd_rotation(id, *r),
                        Write::Anc(id, m) => corsair::set_anc(id, *m),
                        Write::Brightness(id, b) => corsair::set_brightness(id, *b),
                        Write::Effect(id, e) => corsair::set_effect(id, e),
                        Write::RgbEffect(id, e) => openrgb::set_effect(id, e),
                        Write::RgbBrightness(id, b) => openrgb::set_brightness(id, *b),
                        Write::RgbColor(id, c) => openrgb::set_color(id, c),
                        Write::Sidetone(id, l) => headset::set_sidetone(id, *l),
                        Write::HeadsetLights(id, on) => headset::set_lights(id, *on),
                    };
                    if let Err(e) = r {
                        errors.lock().unwrap().push(e);
                    }
                    }
                }
            });
        }
        Devices {
            list: list.to_vec(),
            shown: HashMap::new(),
            latest,
            others: Others { rgb: env.rgb.clone(), batteries: env.batteries.clone(), headsets: env.headsets.clone() },
            latest_others,
            corsair_on: modules.iter().any(|m| m == "corsair"),
            active,
            edited: Instant::now() - Duration::from_secs(10),
            writes,
            errors,
            running: !list.is_empty() || modules.iter().any(|m| m == "corsair") && corsair::running(),
            notice: String::new(),
        }
    }

    pub fn set_active(&mut self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
    }

    fn send(&mut self, w: Write) {
        self.edited = Instant::now();
        let _ = self.writes.send(w);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        if let Some(e) = self.errors.lock().unwrap().pop() {
            self.notice = e;
        }
        let busy = ui.ctx().dragged_id().is_some() || self.edited.elapsed() < Duration::from_millis(2500);
        if let Some(s) = self.latest.lock().unwrap().take()
            && !busy
        {
            self.shown = s;
        }
        if let Some(o) = self.latest_others.lock().unwrap().take()
            && !busy
        {
            self.others = o;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        ui.add_space(20.0);
        if self.corsair_on && self.list.is_empty() {
            let msg = if self.running { t("OpenLinkHub no ve ningún aparato Corsair.") } else { t("OpenLinkHub no está corriendo. Instálalo y arráncalo: github.com/jurkovic-nikola/OpenLinkHub") };
            ui.vertical_centered(|ui| ui.label(RichText::new(msg).weak()));
            ui.add_space(12.0);
        }
        if self.list.is_empty() && self.others.rgb.is_empty() && self.others.batteries.is_empty() && self.others.headsets.is_empty() {
            if !self.corsair_on {
                ui.vertical_centered(|ui| ui.label(RichText::new(t("No hay periféricos que mostrar. Los módulos de cada marca se activan en Ajustes → Extensiones.")).weak()));
            }
            return;
        }
        let list = self.list.clone();
        let (rgb, batteries, headsets) = (self.others.rgb.clone(), self.others.batteries.clone(), self.others.headsets.clone());
        ui.horizontal_wrapped(|ui| {
            ui.add_space(20.0);
            for d in &list {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical(|ui| self.card(ui, d, accent));
                });
                ui.add_space(12.0);
            }
            for d in &rgb {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical(|ui| self.rgb_card(ui, d));
                });
                ui.add_space(12.0);
            }
            for h in &headsets {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical(|ui| self.headset_card(ui, h, accent));
                });
                ui.add_space(12.0);
            }
            for b in &batteries {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical(|ui| {
                        header(ui, corsair::icon(b.kind_code()), &title(&b.model));
                        battery(ui, b.level, accent);
                    });
                });
                ui.add_space(12.0);
            }
        });
    }

    fn headset_card(&mut self, ui: &mut egui::Ui, h: &headset::Headset, accent: Color32) {
        let mut live = h.clone();
        header(ui, corsair::icon(Some(2)), &title(&h.product));
        if let Some(b) = h.battery {
            battery(ui, b, accent);
        }
        if h.has_sidetone {
            ui.label(t("Retorno de voz"));
            let mut level = h.sidetone.unwrap_or(0);
            if ui.add(egui::Slider::new(&mut level, 0..=128).custom_formatter(|v, _| format!("{}%", v as u32 * 100 / 128))).changed() {
                live.sidetone = Some(level);
                self.send(Write::Sidetone(h.id.clone(), level));
            }
            ui.add_space(8.0);
        }
        if h.has_lights {
            ui.label(t("Luces"));
            ui.horizontal(|ui| {
                for (on, name) in [(false, t("Apagadas")), (true, t("Encendidas"))] {
                    if toggle(ui, h.lights == Some(on), name, accent) && h.lights != Some(on) {
                        live.lights = Some(on);
                        self.send(Write::HeadsetLights(h.id.clone(), on));
                    }
                }
            });
        }
        if let Some(slot) = self.others.headsets.iter_mut().find(|x| x.id == h.id) {
            *slot = live;
        }
    }

    fn rgb_card(&mut self, ui: &mut egui::Ui, d: &openrgb::Device) {
        let mut live = d.clone();
        header(ui, '\u{f0335}', &title(&d.name));
        if let Some(mut b) = d.brightness() {
            ui.label(t("Brillo"));
            if ui.add(egui::Slider::new(&mut b, 0..=100).suffix("%")).changed() {
                self.send(Write::RgbBrightness(d.id.clone(), b));
            }
            ui.add_space(8.0);
        }
        if let Some(cur) = d.effect().map(String::from) {
            ui.label(t("Efecto de luz"));
            egui::ComboBox::from_id_salt(("rgb", &d.id)).selected_text(openrgb::effect_name(&cur)).width(220.0).show_ui(ui, |ui| {
                for (i, e) in d.effects().iter().enumerate() {
                    if ui.selectable_label(*e == cur, openrgb::effect_name(e)).clicked() && *e != cur {
                        live.active = i;
                        self.send(Write::RgbEffect(d.id.clone(), e.clone()));
                    }
                }
            });
        }
        if d.takes_color() {
            ui.add_space(8.0);
            let hex = d.color().unwrap_or_else(|| "ffffff".into());
            let mut rgb = [0u8; 3];
            for (i, c) in rgb.iter_mut().enumerate() {
                *c = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(255);
            }
            ui.horizontal(|ui| {
                ui.label(t("Color"));
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    self.send(Write::RgbColor(d.id.clone(), format!("{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])));
                }
            });
        }
        if d.modes.is_empty() {
            ui.label(RichText::new(t("Sin ajustes que controlar desde aquí.")).weak().size(12.5));
        }
        // The new effect shows until OpenRGB's next reading.
        if let Some(slot) = self.others.rgb.iter_mut().find(|r| r.id == d.id) {
            *slot = live;
        }
    }

    /// The keyboard with G keys, if OpenLinkHub sees one.
    pub fn keyboard(&self) -> Option<String> {
        self.list.iter().find(|d| d.kind == Some(0)).map(|d| d.serial.clone())
    }

    fn card(&mut self, ui: &mut egui::Ui, d: &corsair::Device, accent: Color32) {
        let mut live = self.shown.get(&d.serial).cloned().unwrap_or_default();
        header(ui, corsair::icon(d.kind), &d.product);
        if let Some(b) = live.battery {
            battery(ui, b, accent);
        }
        if let Some(anc) = live.anc {
            ui.label(t("Cancelación de ruido"));
            ui.horizontal(|ui| {
                for (mode, name) in [(0, t("Apagada")), (1, t("Activa")), (2, t("Transparencia"))] {
                    if toggle(ui, anc == mode, name, accent) && anc != mode {
                        live.anc = Some(mode);
                        self.send(Write::Anc(d.serial.clone(), mode));
                    }
                }
            });
            ui.add_space(8.0);
        }
        if let Some(mut i) = live.brightness {
            ui.label(t("Brillo"));
            let max = LEVELS.len() - 1;
            if ui.add(egui::Slider::new(&mut i, 0..=max).custom_formatter(|v, _| format!("{}%", LEVELS[v as usize].1)).show_value(true)).changed() {
                live.brightness = Some(i);
                self.send(Write::Brightness(d.serial.clone(), LEVELS[i].0));
            }
            ui.add_space(8.0);
        }
        if let Some(cur) = live.effect.clone() {
            ui.label(t("Efecto de luz"));
            egui::ComboBox::from_id_salt(("effect", &d.serial)).selected_text(corsair::effect_name(&cur)).width(220.0).show_ui(ui, |ui| {
                for e in &live.effects {
                    if ui.selectable_label(*e == cur, corsair::effect_name(e)).clicked() && *e != cur {
                        live.effect = Some(e.clone());
                        self.send(Write::Effect(d.serial.clone(), e.clone()));
                    }
                }
            });
        }
        if let Some((modes, mode, rotation)) = live.lcd.clone() {
            ui.add_space(8.0);
            ui.label(t("Pantalla"));
            ui.horizontal(|ui| {
                let shown = modes.iter().find(|(m, _)| *m == mode).map_or("—".to_string(), |(_, n)| corsair::lcd_mode_name(n));
                egui::ComboBox::from_id_salt(("lcd", &d.serial)).selected_text(shown).width(190.0).show_ui(ui, |ui| {
                    for (m, name) in &modes {
                        if ui.selectable_label(*m == mode, corsair::lcd_mode_name(name)).clicked() && *m != mode {
                            live.lcd = Some((modes.clone(), *m, rotation));
                            self.send(Write::Lcd(d.serial.clone(), *m));
                        }
                    }
                });
                if ui.button(format!("↻ {}", t("Girar"))).on_hover_text(format!("{}°", rotation as u32 * 90)).clicked() {
                    let r = (rotation + 1) % 4;
                    live.lcd = Some((modes.clone(), mode, r));
                    self.send(Write::Rotation(d.serial.clone(), r));
                }
            });
        }
        if live.battery.is_none() && live.anc.is_none() && live.brightness.is_none() && live.effect.is_none() && live.lcd.is_none() {
            ui.label(RichText::new(t("Sin ajustes que controlar desde aquí.")).weak().size(12.5));
        }
        self.shown.insert(d.serial.clone(), live);
    }
}

fn header(ui: &mut egui::Ui, icon: char, name: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon.to_string()).size(22.0));
        ui.label(RichText::new(name).size(17.0).strong());
    });
    ui.add_space(8.0);
}

fn battery(ui: &mut egui::Ui, b: u64, accent: Color32) {
    ui.label(t("Batería"));
    let color = crate::home::battery_color(b, accent);
    bar(ui, b as f32 / 100.0, color);
    if b <= 30 {
        ui.label(RichText::new(tf("{}% · batería baja", &[&b.to_string()])).color(color).size(12.0));
    } else {
        ui.label(RichText::new(format!("{b}%")).weak().size(12.0));
    }
    ui.add_space(8.0);
}

fn bar(ui: &mut egui::Ui, level: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_width(rect.width() * level.clamp(0.0, 1.0));
    p.rect_filled(fill, 6, color);
}
