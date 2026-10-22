//! Audio mixer: the output and microphone in use, and every app playing or recording, each with
//! its fader, mute and device.
use crate::mic::card_frame;
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::groups;
use fluxdeck_core::mixer::{self, Channel, Kind, Snapshot};
use fluxdeck_core::pw;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use std::collections::HashMap;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
enum Op {
    Volume(u32, f32),
    Mute(u32, bool),
    Default(u32),
    Move(u32, String),
    Group(String, groups::Level, Vec<groups::Group>),
}

impl Op {
    /// Writes with the same key replace each other, so a fast drag sends only its last value.
    fn key(&self) -> String {
        match self {
            Op::Volume(id, _) => format!("v{id}"),
            Op::Mute(id, _) => format!("m{id}"),
            Op::Default(_) => "default".into(),
            Op::Move(id, _) => format!("t{id}"),
            Op::Group(id, _, _) => format!("g{id}"),
        }
    }

    fn apply(&self) {
        match self {
            Op::Volume(id, v) => drop(mixer::set_volume(*id, *v)),
            Op::Mute(id, m) => drop(mixer::set_mute(*id, *m)),
            Op::Default(id) => drop(mixer::set_default(*id)),
            Op::Move(id, dev) => drop(mixer::move_stream(*id, dev)),
            Op::Group(id, level, all) => {
                groups::set_level(id, *level);
                groups::apply(all, &mixer::snapshot());
            }
        }
    }
}

/// What a button in the mixer asks the window to do.
pub enum Go {
    Effects,
}

pub struct Mixer {
    latest: Arc<Mutex<Option<Snapshot>>>,
    shown: Snapshot,
    active: Arc<AtomicBool>,
    writes: Sender<Op>,
    edited: Instant,
    /// Live levels of the output and input in use, and what they measure.
    meters: Vec<Child>,
    metered: (String, String),
    levels: [Arc<Mutex<f32>>; 2],
    shown_levels: [f32; 2],
    /// A working copy of the config's hidden devices.
    hidden: Vec<String>,
    group_levels: std::collections::BTreeMap<String, groups::Level>,
    levels_read: Instant,
}

impl Mixer {
    pub fn new() -> Mixer {
        let latest = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        {
            let (latest, active) = (latest.clone(), active.clone());
            std::thread::spawn(move || {
                loop {
                    if active.load(Ordering::Relaxed) {
                        *latest.lock().unwrap() = Some(mixer::snapshot());
                    }
                    std::thread::sleep(Duration::from_millis(600));
                }
            });
        }
        let (writes, rx) = channel::<Op>();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut last: HashMap<String, Op> = HashMap::new();
                for op in std::iter::once(first).chain(rx.try_iter()) {
                    last.insert(op.key(), op);
                }
                for op in last.values() {
                    op.apply();
                }
            }
        });
        Mixer {
            latest,
            shown: Snapshot::default(),
            active,
            writes,
            edited: Instant::now() - Duration::from_secs(10),
            meters: Vec::new(),
            metered: Default::default(),
            levels: [Arc::new(Mutex::new(0.0)), Arc::new(Mutex::new(0.0))],
            shown_levels: [0.0; 2],
            hidden: Vec::new(),
            group_levels: groups::levels(),
            levels_read: Instant::now(),
        }
    }

    pub fn set_active(&mut self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
        if !on {
            self.stop_meters();
        }
    }

    fn stop_meters(&mut self) {
        for mut c in self.meters.drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.metered = Default::default();
    }

    /// Meters follow the default output and input; restarted when either changes.
    fn follow_meters(&mut self) {
        let want = (self.shown.default_output.clone(), self.shown.default_input.clone());
        if want == self.metered || want.0.is_empty() {
            return;
        }
        self.stop_meters();
        for (i, node) in [want.0.clone(), want.1.clone()].into_iter().enumerate() {
            if node.is_empty() {
                continue;
            }
            let level = self.levels[i].clone();
            let on = move |l| {
                *level.lock().unwrap() = l;
                true
            };
            let child = if i == 0 { pw::meter_output(&node, on) } else { pw::meter(&node, on) };
            self.meters.extend(child);
        }
        self.metered = want;
    }

    fn send(&mut self, op: Op) {
        self.edited = Instant::now();
        let _ = self.writes.send(op);
    }

    fn sync(&mut self, ctx: &egui::Context) {
        // Hold what the user is moving; take PipeWire's values once they let go.
        let busy = ctx.dragged_id().is_some() || self.edited.elapsed() < Duration::from_millis(1200);
        if let Some(s) = self.latest.lock().unwrap().take()
            && !busy
        {
            self.shown = s;
        }
        for i in 0..2 {
            let l = *self.levels[i].lock().unwrap();
            self.shown_levels[i] = l.max(self.shown_levels[i] - 0.02);
        }
        self.follow_meters();
    }

    /// `hidden` lists devices the user took out of the menus; changing it is a config edit.
    pub fn ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32, hidden: &mut Vec<String>, all_groups: &[groups::Group]) -> Option<Go> {
        self.sync(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(40));
        let mut go = None;
        let snap = self.shown.clone();
        if snap.channels.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(t("Leyendo el audio…")).weak()));
            return None;
        }
        let outputs: Vec<Channel> = snap.of(Kind::Output).filter(|c| !c.internal).cloned().collect();
        let inputs: Vec<Channel> = snap.of(Kind::Input).cloned().collect();
        self.hidden = hidden.clone();
        // Apps in a group are controlled by the group's fader.
        let playing: Vec<Channel> = snap.of(Kind::Playback).filter(|c| groups::group_of(all_groups, c).is_none()).cloned().collect();
        if self.levels_read.elapsed() > Duration::from_secs(2) && self.edited.elapsed() > Duration::from_secs(2) {
            self.group_levels = groups::levels();
            self.levels_read = Instant::now();
        }
        let recording: Vec<Channel> = snap.of(Kind::Record).cloned().collect();

        egui::ScrollArea::both().show(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal_top(|ui| {
                ui.add_space(20.0);
                ui.spacing_mut().item_spacing.x = 12.0;
                if let Some(out) = snap.default_of(Kind::Output).cloned() {
                    let level = Some(self.shown_levels[0]);
                    group(ui, card, t("Salida"), |ui| {
                        self.strip(ui, &out, &outputs, level, accent, None);
                    });
                }
                if let Some(mic) = snap.default_of(Kind::Input).cloned() {
                    let level = Some(self.shown_levels[1]);
                    group(ui, card, t("Micrófono"), |ui| {
                        if self.strip(ui, &mic, &inputs, level, accent, Some(t("Efectos"))) {
                            go = Some(Go::Effects);
                        }
                    });
                }
                if !all_groups.is_empty() {
                    group(ui, card, t("Grupos"), |ui| {
                        ui.horizontal_top(|ui| {
                            for g in all_groups {
                                let members: Vec<String> = snap.of(Kind::Playback).filter(|c| groups::group_of(all_groups, c).is_some_and(|x| x.id == g.id)).map(|c| c.label.clone()).collect();
                                self.group_strip(ui, g, &members, all_groups, accent);
                            }
                        });
                    });
                }
                group(ui, card, t("Apps"), |ui| {
                    if playing.is_empty() {
                        empty(ui, t("Ninguna app está sonando."));
                    }
                    ui.horizontal_top(|ui| {
                        for c in &playing {
                            self.strip(ui, c, &outputs, None, accent, None);
                        }
                    });
                });
                if !recording.is_empty() {
                    group(ui, card, t("Usan el micrófono"), |ui| {
                        ui.horizontal_top(|ui| {
                            for c in &recording {
                                self.strip(ui, c, &inputs, None, accent, None);
                            }
                        });
                    });
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new(t("Cada app puede ir a otra salida con el menú bajo su barra. Los cambios son al momento.")).weak().size(12.5));
            });
        });
        if self.hidden != *hidden {
            *hidden = self.hidden.clone();
        }
        go
    }

    /// One channel: icon, name, fader with its level, percent, mute and device. Returns true when `extra` was clicked.
    fn strip(&mut self, ui: &mut egui::Ui, c: &Channel, devices: &[Channel], level: Option<f32>, accent: Color32, extra: Option<&str>) -> bool {
        let mut clicked = false;
        let app = matches!(c.kind, Kind::Playback | Kind::Record);
        ui.vertical(|ui| {
            ui.set_width(118.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon_for(c)).size(24.0).color(if c.muted { ui.visuals().weak_text_color() } else { accent }));
                ui.add(egui::Label::new(RichText::new(&c.label).size(13.0).strong()).truncate()).on_hover_text(&c.label);
            });
            ui.horizontal(|ui| {
                ui.add_space(28.0);
                let mut v = c.volume;
                let resp = ui.add_sized([28.0, 190.0], egui::Slider::new(&mut v, 0.0..=1.0).vertical().show_value(false));
                if resp.changed() {
                    self.set_shown(c.id, |x| x.volume = v);
                    self.send(Op::Volume(c.id, v));
                }
                if let Some(l) = level {
                    meter(ui, l, accent);
                }
            });
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("{:.0}%", c.volume * 100.0)).size(13.0));
                let (icon, tip) = if c.muted { ("\u{f0581}", t("Activar sonido")) } else { ("\u{f057e}", t("Silenciar")) };
                let b = egui::Button::new(RichText::new(icon).size(16.0).color(if c.muted { Color32::WHITE } else { ui.visuals().text_color() })).min_size(vec2(44.0, 28.0));
                let b = if c.muted { b.fill(Color32::from_rgb(179, 38, 30)) } else { b };
                if ui.add(b).on_hover_text(tip).clicked() {
                    let m = !c.muted;
                    self.set_shown(c.id, |x| x.muted = m);
                    self.send(Op::Mute(c.id, m));
                }
            });
            // Which device: the default for output and microphone, the target for an app.
            let current = if app { c.device.and_then(|d| devices.iter().find(|x| x.id == d)) } else { Some(c) };
            // An app going through a filter (an equaliser) ends up on the default output.
            let shown = current.map_or(if app { t("Predeterminada").to_string() } else { "—".to_string() }, |d| d.label.clone());
            let mut pick = None;
            let mut hide = None;
            let mut unhide = false;
            egui::ComboBox::from_id_salt(("dev", c.id)).selected_text(RichText::new(shown).size(12.0)).width(112.0).truncate().show_ui(ui, |ui| {
                let hidden_here = devices.iter().filter(|d| self.hidden.contains(&d.name)).count();
                for d in devices.iter().filter(|d| !self.hidden.contains(&d.name) || current.is_some_and(|x| x.id == d.id)) {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(current.is_some_and(|x| x.id == d.id), &d.label).clicked() {
                            pick = Some(d.clone());
                        }
                        if ui.small_button("\u{f0209}").on_hover_text(t("Ocultar de esta lista")).clicked() {
                            hide = Some(d.name.clone());
                        }
                    });
                }
                if hidden_here > 0 {
                    ui.separator();
                    if ui.button(tf("Mostrar {} ocultos", &[&hidden_here.to_string()])).clicked() {
                        unhide = true;
                    }
                }
            });
            if let Some(h) = hide {
                self.hidden.push(h);
            }
            if unhide {
                let names: Vec<String> = devices.iter().map(|d| d.name.clone()).collect();
                self.hidden.retain(|h| !names.contains(h));
            }
            if let Some(d) = pick {
                if app {
                    self.send(Op::Move(c.id, d.name.clone()));
                } else {
                    self.send(Op::Default(d.id));
                }
            }
            if let Some(label) = extra {
                ui.vertical_centered(|ui| {
                    if ui.button(label).clicked() {
                        clicked = true;
                    }
                });
            }
        });
        clicked
    }

    /// A group's fader: its own volume and mute, applied to every app in it.
    fn group_strip(&mut self, ui: &mut egui::Ui, g: &groups::Group, members: &[String], all: &[groups::Group], accent: Color32) {
        let mut level = self.group_levels.get(&g.id).copied().unwrap_or_default();
        ui.vertical(|ui| {
            ui.set_width(118.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(&g.icon).size(24.0).color(if level.muted { ui.visuals().weak_text_color() } else { accent }));
                ui.add(egui::Label::new(RichText::new(&g.name).size(13.0).strong()).truncate());
            });
            ui.horizontal(|ui| {
                ui.add_space(36.0);
                if ui.add_sized([28.0, 190.0], egui::Slider::new(&mut level.volume, 0.0..=1.0).vertical().show_value(false)).changed() {
                    self.group_levels.insert(g.id.clone(), level);
                    self.send(Op::Group(g.id.clone(), level, all.to_vec()));
                }
            });
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(format!("{:.0}%", level.volume * 100.0)).size(13.0));
                let (icon, tip) = if level.muted { ("\u{f0581}", t("Activar sonido")) } else { ("\u{f057e}", t("Silenciar")) };
                let b = egui::Button::new(RichText::new(icon).size(16.0).color(if level.muted { Color32::WHITE } else { ui.visuals().text_color() })).min_size(vec2(44.0, 28.0));
                let b = if level.muted { b.fill(Color32::from_rgb(179, 38, 30)) } else { b };
                if ui.add(b).on_hover_text(tip).clicked() {
                    level.muted = !level.muted;
                    self.group_levels.insert(g.id.clone(), level);
                    self.send(Op::Group(g.id.clone(), level, all.to_vec()));
                }
                let text = if members.is_empty() { t("Nada sonando").to_string() } else { members.join(", ") };
                ui.add(egui::Label::new(RichText::new(text).weak().size(11.5)).truncate()).on_hover_text(members.join("\n"));
            });
        });
    }

    fn set_shown(&mut self, id: u32, f: impl FnOnce(&mut Channel)) {
        if let Some(c) = self.shown.channels.iter_mut().find(|c| c.id == id) {
            f(c);
        }
    }
}

fn group(ui: &mut egui::Ui, card: Color32, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    card_frame(card).inner_margin(16).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(15.0).strong());
            ui.add_space(8.0);
            add(ui);
        });
    });
}

fn empty(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui(vec2(200.0, 60.0), |ui| ui.label(RichText::new(text).weak()));
}

/// A thin vertical level bar beside a fader.
fn meter(ui: &mut egui::Ui, level: f32, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 190.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4, ui.visuals().extreme_bg_color);
    let mut fill = rect;
    fill.set_top(rect.bottom() - rect.height() * level.clamp(0.0, 1.0));
    p.rect_filled(fill, 4, if level > 0.92 { Color32::from_rgb(229, 72, 77) } else { accent });
    p.rect_stroke(rect, 4, Stroke::new(1.0, Color32::from_black_alpha(60)), egui::StrokeKind::Inside);
}

/// A Nerd Font glyph for well-known apps, else a generic one by kind.
fn icon_for(c: &Channel) -> &'static str {
    let name = format!("{} {}", c.icon, c.label).to_lowercase();
    let known = [
        ("spotify", "\u{f04c7}"),
        ("firefox", "\u{f0239}"),
        ("discord", "\u{f066f}"),
        ("vesktop", "\u{f066f}"),
        ("equibop", "\u{f066f}"),
        ("chrom", "\u{f02af}"),
        ("helium", "\u{f02af}"),
        ("steam", "\u{f04d3}"),
        ("youtube", "\u{f05c3}"),
        ("cider", "\u{f075a}"),
        ("music", "\u{f075a}"),
    ];
    if let Some((_, g)) = known.iter().find(|(k, _)| name.contains(k)) {
        return g;
    }
    match c.kind {
        Kind::Output => "\u{f04c3}",
        Kind::Input => "\u{f036c}",
        Kind::Playback => "\u{f08c6}",
        Kind::Record => "\u{f044b}",
    }
}
