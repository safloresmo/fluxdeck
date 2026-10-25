//! Home: is everything working? Service, deck, microphone, devices and plugins at a glance.
use crate::mic::card_frame;
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::{State, corsair, effects, plugins, pw};
use eframe::egui::{self, Color32, RichText, Sense, vec2};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a card's button leads.
#[derive(Clone, Copy, PartialEq)]
pub enum Go {
    Deck,
    Audio,
    Devices,
    Plugins,
    StartService,
}

#[derive(Clone, Default)]
struct Summary {
    /// Description of the microphone apps use, and how it is processed.
    mic: Option<String>,
    mic_mode: &'static str,
    effects: usize,
    dock: bool,
    batteries: Vec<(String, Option<u64>, u64)>,
    olh_running: bool,
    plugins: usize,
    plugins_off: usize,
    plugins_broken: Vec<String>,
}

fn gather(corsair_on: bool) -> Summary {
    let nodes = pw::nodes();
    let default = pw::default_source();
    let desc = |name: &str| nodes.iter().find(|n| n.name == name).map(|n| if n.description.is_empty() { n.name.clone() } else { n.description.clone() });
    let filtered = |name: &str| nodes.iter().any(|n| n.name == format!("capture.{name}"));
    let mic_mode = match default.as_deref() {
        Some(effects::SOURCE) => "Con los efectos de FluxDeck",
        Some(n) if filtered(n) => "Con el filtro del sistema",
        Some(_) => "Directo, sin procesar",
        None => "",
    };
    let chain = effects::Chain::load();
    let all = plugins::installed();
    let on = plugins::enabled();
    let (batteries, olh_running) = if corsair_on { (corsair::batteries(), corsair::running()) } else { (Vec::new(), false) };
    Summary {
        mic: default.as_deref().and_then(desc),
        mic_mode,
        effects: chain.map_or(0, |c| c.effects.iter().filter(|e| e.enabled).count()),
        dock: std::path::Path::new("/proc/asound/Dock").exists(),
        batteries,
        olh_running,
        plugins: all.len(),
        plugins_off: all.len() - on.len(),
        plugins_broken: on.iter().filter(|p| p.missing().is_some()).map(|p| p.manifest.name.clone()).collect(),
    }
}

pub struct Home {
    latest: Arc<Mutex<Summary>>,
    active: Arc<AtomicBool>,
    corsair: Arc<AtomicBool>,
}

impl Home {
    pub fn new() -> Home {
        let latest: Arc<Mutex<Summary>> = Arc::default();
        let active = Arc::new(AtomicBool::new(true));
        let corsair = Arc::new(AtomicBool::new(false));
        {
            let (latest, active, corsair) = (latest.clone(), active.clone(), corsair.clone());
            std::thread::spawn(move || {
                loop {
                    if active.load(Ordering::Relaxed) {
                        let s = gather(corsair.load(Ordering::Relaxed));
                        *latest.lock().unwrap() = s;
                    }
                    std::thread::sleep(Duration::from_secs(3));
                }
            });
        }
        Home { latest, active, corsair }
    }

    pub fn set_active(&self, on: bool, corsair_on: bool) {
        self.active.store(on, Ordering::Relaxed);
        self.corsair.store(corsair_on, Ordering::Relaxed);
    }

    pub fn ui(&self, ui: &mut egui::Ui, card: Color32, accent: Color32, service_up: bool, live: Option<&State>, corsair_on: bool) -> Option<Go> {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let s = self.latest.lock().unwrap().clone();
        let red = Color32::from_rgb(229, 72, 77);
        let amber = Color32::from_rgb(229, 160, 72);
        let deck = live.filter(|l| !l.model.is_empty());
        let mut go = None;

        // Things that need attention, most serious first.
        let mut alerts: Vec<(Color32, String, Option<(&str, Go)>)> = Vec::new();
        if !service_up {
            alerts.push((red, t("El servicio está apagado: el deck, los plugins y los efectos no funcionan.").into(), Some((t("Iniciar servicio"), Go::StartService))));
        } else if deck.is_none() {
            alerts.push((amber, t("No hay ningún Stream Deck conectado.").into(), None));
        }
        if corsair_on && !s.olh_running {
            alerts.push((amber, t("OpenLinkHub no está corriendo: los periféricos Corsair no responden.").into(), Some((t("Ver periféricos"), Go::Devices))));
        }
        for (name, _, level) in s.batteries.iter().filter(|b| b.2 <= 20) {
            alerts.push((amber, tf("Batería baja en {}: {}%", &[name, &level.to_string()]), None));
        }
        for p in &s.plugins_broken {
            alerts.push((amber, tf("El plugin «{}» no puede correr aquí.", &[p]), Some((t("Ver plugins"), Go::Plugins))));
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    let (mark, text, color) = if alerts.is_empty() {
                        ("\u{f05e0}", t("Todo en orden").to_string(), accent)
                    } else {
                        ("\u{f0026}", tf("{} cosas necesitan atención", &[&alerts.len().to_string()]), amber)
                    };
                    ui.label(RichText::new(format!("{mark}  {text}")).color(color).size(15.0));
                });
            });
            ui.add_space(14.0);

            if !alerts.is_empty() {
                pad(ui, |ui| {
                    card_frame(card).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for (color, text, action) in &alerts {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").color(*color));
                                ui.label(text);
                                if let Some((label, target)) = action {
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button(*label).clicked() {
                                            go = Some(*target);
                                        }
                                    });
                                }
                            });
                        }
                    });
                });
                ui.add_space(14.0);
            }

            // Three summary cards, in as many columns as fit.
            pad(ui, |ui| {
                let avail = ui.available_width();
                let cols = if avail >= 760.0 { 3 } else if avail >= 500.0 { 2 } else { 1 };
                let w = (avail - (cols as f32 - 1.0) * 14.0) / cols as f32;
                let deck_text = match deck {
                    Some(l) => (tf("{} · página «{}»", &[&crate::model_name(&l.model), &l.page]), t("Conectado").to_string()),
                    None => (t("Sin conectar").to_string(), String::new()),
                };
                let mic = s.mic.clone().unwrap_or_else(|| t("Ninguno").into());
                let detail = if s.effects > 0 { format!("{} · {}", t(s.mic_mode), tf("{} efectos", &[&s.effects.to_string()])) } else { t(s.mic_mode).to_string() };
                let plug = tf("{} instalados", &[&s.plugins.to_string()]);
                let off = if s.plugins_off > 0 { tf("{} desactivados", &[&s.plugins_off.to_string()]) } else { String::new() };
                let cards = [("\u{f0570}", t("Deck"), deck_text.0.clone(), deck_text.1.clone(), Go::Deck), ("\u{f036c}", t("Micrófono"), mic, detail, Go::Audio), ("\u{f0431}", t("Plugins"), plug, off, Go::Plugins)];
                for row in cards.chunks(cols) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for (icon, title, main, detail, target) in row {
                            if summary(ui, card, w, icon, title, main, detail, accent) {
                                go = Some(*target);
                            }
                        }
                    });
                    ui.add_space(14.0);
                }
            });

            // Connected devices.
            pad(ui, |ui| {
                card_frame(card).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(t("Dispositivos")).size(16.0).strong());
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(28.0, 14.0);
                        if let Some(l) = deck {
                            device(ui, "\u{f0570}", &crate::model_name(&l.model), None, accent);
                        }
                        if s.dock {
                            device(ui, "\u{f036c}", "Elgato XLR Dock", None, accent);
                        }
                        for (name, kind, level) in &s.batteries {
                            device(ui, &corsair::icon(*kind).to_string(), &fluxdeck_core::presets::title(name), Some(*level), battery_color(*level, accent));
                        }
                        if deck.is_none() && !s.dock && s.batteries.is_empty() {
                            ui.label(RichText::new(t("Ninguno detectado.")).weak());
                        }
                    });
                });
            });
            ui.add_space(20.0);
        });
        go
    }
}

fn pad(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width() - 24.0);
            add(ui);
        });
    });
}

/// A clickable summary card; true when clicked.
#[allow(clippy::too_many_arguments)]
fn summary(ui: &mut egui::Ui, card: Color32, w: f32, icon: &str, title: &str, main: &str, detail: &str, accent: Color32) -> bool {
    let r = card_frame(card)
        .show(ui, |ui| {
            ui.set_width(w - 40.0);
            ui.set_min_height(96.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).size(24.0).color(accent));
                    ui.label(RichText::new(title).size(16.0).strong());
                });
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(main).size(14.5)).truncate());
                if !detail.is_empty() {
                    ui.add(egui::Label::new(RichText::new(detail).weak().size(12.5)).truncate());
                }
            });
        })
        .response
        .interact(Sense::click());
    if r.hovered() {
        ui.painter().rect_stroke(r.rect, 18, egui::Stroke::new(1.5, accent), egui::StrokeKind::Inside);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Battery colour everywhere: red at 15 % or less, amber at 30 % or less, else the accent.
pub fn battery_color(level: u64, accent: Color32) -> Color32 {
    match level {
        0..=15 => Color32::from_rgb(229, 72, 77),
        16..=30 => Color32::from_rgb(217, 164, 65),
        _ => accent,
    }
}

fn device(ui: &mut egui::Ui, icon: &str, name: &str, battery: Option<u64>, color: Color32) {
    ui.vertical(|ui| {
        ui.set_width(150.0);
        ui.label(RichText::new(icon).size(30.0));
        ui.add(egui::Label::new(RichText::new(name).size(13.0)).truncate());
        if let Some(b) = battery {
            let (rect, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
            ui.painter().rect_filled(rect, 3, ui.visuals().extreme_bg_color);
            let mut fill = rect;
            fill.set_width(rect.width() * b as f32 / 100.0);
            ui.painter().rect_filled(fill, 3, color);
            ui.label(RichText::new(format!("{b}%")).weak().size(12.0));
        } else {
            ui.label(RichText::new(t("Conectado")).weak().size(12.0));
        }
    });
}
