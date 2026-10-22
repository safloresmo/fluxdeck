//! Peripherals view: one card per Corsair device seen by OpenLinkHub.
use crate::mic::{card_frame, toggle};
use fluxdeck_core::corsair::{self, LEVELS, Live};
use fluxdeck_core::i18n::t;
use eframe::egui::{self, Color32, RichText, Sense, vec2};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

enum Write {
    Anc(String, i64),
    Brightness(String, u8),
    Effect(String, String),
}

pub struct Devices {
    list: Vec<corsair::Device>,
    shown: HashMap<String, Live>,
    latest: Arc<Mutex<Option<HashMap<String, Live>>>>,
    active: Arc<AtomicBool>,
    edited: Instant,
    writes: Sender<Write>,
    errors: Arc<Mutex<Vec<String>>>,
    pub running: bool,
    pub notice: String,
}

impl Devices {
    pub fn new(list: &[corsair::Device]) -> Devices {
        let latest = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));
        {
            let (latest, active, serials) = (latest.clone(), active.clone(), list.iter().map(|d| d.serial.clone()).collect::<Vec<_>>());
            std::thread::spawn(move || {
                // Ends with the view's owner: a new Devices replaces this one after a re-detect.
                while Arc::strong_count(&active) > 1 {
                    if active.load(Ordering::Relaxed) {
                        let all = serials.iter().map(|s| (s.clone(), corsair::live(s))).collect();
                        *latest.lock().unwrap() = Some(all);
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                }
            });
        }
        let (writes, rx) = channel::<Write>();
        {
            let errors = errors.clone();
            std::thread::spawn(move || {
                while let Ok(w) = rx.recv() {
                    let r = match &w {
                        Write::Anc(id, m) => corsair::set_anc(id, *m),
                        Write::Brightness(id, b) => corsair::set_brightness(id, *b),
                        Write::Effect(id, e) => corsair::set_effect(id, e),
                    };
                    if let Err(e) = r {
                        errors.lock().unwrap().push(e);
                    }
                }
            });
        }
        Devices {
            list: list.to_vec(),
            shown: HashMap::new(),
            latest,
            active,
            edited: Instant::now() - Duration::from_secs(10),
            writes,
            errors,
            running: !list.is_empty() || corsair::running(),
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
        ui.ctx().request_repaint_after(Duration::from_millis(500));
        ui.add_space(20.0);
        if self.list.is_empty() {
            let msg = if self.running { t("OpenLinkHub no ve ningún aparato Corsair.") } else { t("OpenLinkHub no está corriendo. Instálalo y arráncalo: github.com/jurkovic-nikola/OpenLinkHub") };
            ui.vertical_centered(|ui| ui.label(RichText::new(msg).weak()));
            return;
        }
        let list = self.list.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(20.0);
                for d in &list {
                    card_frame(card).show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.vertical(|ui| self.card(ui, d, accent));
                    });
                    ui.add_space(12.0);
                }
            });
        });
    }

    fn card(&mut self, ui: &mut egui::Ui, d: &corsair::Device, accent: Color32) {
        let mut live = self.shown.get(&d.serial).cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new(corsair::icon(d.kind).to_string()).size(22.0));
            ui.label(RichText::new(&d.product).size(17.0).strong());
        });
        ui.add_space(8.0);
        if let Some(b) = live.battery {
            ui.label(t("Batería"));
            bar(ui, b as f32 / 100.0, if b <= 15 { Color32::from_rgb(229, 72, 77) } else { accent });
            ui.label(RichText::new(format!("{b}%")).weak().size(12.0));
            ui.add_space(8.0);
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
            egui::ComboBox::from_id_salt(("effect", &d.serial)).selected_text(&cur).width(220.0).show_ui(ui, |ui| {
                for e in &live.effects {
                    if ui.selectable_label(*e == cur, e).clicked() && *e != cur {
                        live.effect = Some(e.clone());
                        self.send(Write::Effect(d.serial.clone(), e.clone()));
                    }
                }
            });
        }
        if live.battery.is_none() && live.anc.is_none() && live.brightness.is_none() && live.effect.is_none() {
            ui.label(RichText::new(t("Sin ajustes que controlar desde aquí.")).weak().size(12.5));
        }
        self.shown.insert(d.serial.clone(), live);
    }
}

fn bar(ui: &mut egui::Ui, level: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_width(rect.width() * level.clamp(0.0, 1.0));
    p.rect_filled(fill, 6, color);
}
