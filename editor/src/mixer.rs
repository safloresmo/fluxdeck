//! Audio mixer: outputs as columns, and the microphone, groups and apps as rows, each with its
//! fader and mute; a dot marks the output a row plays on.
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::groups;
use fluxdeck_core::mixer::{self, Channel, Kind, Snapshot};
use fluxdeck_core::pw;
use fluxdeck_core::streammix::{self, StreamMix};
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
    Route(String, String, Vec<groups::Group>),
    Stream(StreamMix),
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
            Op::Route(id, _, _) => format!("r{id}"),
            Op::Stream(_) => "stream".into(),
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
            Op::Route(id, output, all) => {
                groups::set_route(id, output);
                groups::apply(all, &mixer::snapshot());
            }
            Op::Stream(mix) => mix.save(),
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
    /// A working copy of the config's short output names, and the one being renamed with its text.
    names: std::collections::BTreeMap<String, String>,
    renaming: Option<(String, String)>,
    group_levels: std::collections::BTreeMap<String, groups::Level>,
    routes: std::collections::BTreeMap<String, String>,
    stream: StreamMix,
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
            names: Default::default(),
            renaming: None,
            group_levels: groups::levels(),
            routes: groups::routes(),
            stream: StreamMix::load(),
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

    /// `hidden` lists devices the user took out of the menus and `names` their short names; changing
    /// either is a config edit.
    /// Laid out like Wave Link: a row per channel, a column per output, and a dot where each row plays.
    pub fn ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32, hidden: &mut Vec<String>, names: &mut std::collections::BTreeMap<String, String>, all_groups: &[groups::Group]) -> Option<Go> {
        self.sync(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(40));
        let mut go = None;
        self.names = names.clone();
        let mut snap = self.shown.clone();
        // Outputs show their short name everywhere: columns and device menus.
        for c in snap.channels.iter_mut().filter(|c| c.kind == Kind::Output) {
            if let Some(n) = self.names.get(&c.name) {
                c.label = n.clone();
            }
        }
        if snap.channels.is_empty() {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(t("Leyendo el audio…")).weak()));
            return None;
        }
        self.hidden = hidden.clone();
        if self.levels_read.elapsed() > Duration::from_secs(2) && self.edited.elapsed() > Duration::from_secs(2) {
            self.group_levels = groups::levels();
            self.routes = groups::routes();
            self.stream = StreamMix::load();
            self.levels_read = Instant::now();
        }
        let outputs: Vec<Channel> = snap.of(Kind::Output).filter(|c| !c.internal).cloned().collect();
        let columns: Vec<Channel> = outputs.iter().filter(|o| !self.hidden.contains(&o.name)).cloned().collect();
        let inputs: Vec<Channel> = snap.of(Kind::Input).cloned().collect();
        let default_out = snap.default_of(Kind::Output).map(|c| c.id);
        // An app on a filter (an equaliser) or on a hidden output is shown on the default one.
        let lands = |dev: Option<u32>| dev.filter(|d| columns.iter().any(|c| c.id == *d)).or(default_out);
        // Apps in a group are controlled by the group's row.
        let playing: Vec<Channel> = snap.of(Kind::Playback).filter(|c| groups::group_of(all_groups, c).is_none()).cloned().collect();
        let recording: Vec<Channel> = snap.of(Kind::Record).cloned().collect();

        egui::ScrollArea::both().show(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal_top(|ui| {
                ui.add_space(20.0);
                egui::Grid::new("matrix").spacing(vec2(8.0, 8.0)).show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(NAME_W);
                        ui.label(RichText::new(t("Canales")).size(22.0).strong());
                        let on = self.stream.on;
                        let b = egui::Button::new(RichText::new(t("Mezcla para stream")).color(if on { crate::mic::ink(accent) } else { ui.visuals().text_color() })).min_size(vec2(72.0, 30.0));
                        if ui.add(if on { b.fill(accent) } else { b })
                            .on_hover_text(t("Crea la salida «FluxDeck Stream» para OBS, con volumen propio para el micrófono y cada grupo. Al apagarla desaparece."))
                            .clicked()
                        {
                            self.stream.on = !self.stream.on;
                            self.send(Op::Stream(self.stream.clone()));
                        }
                    });
                    if self.stream.on {
                        let sink = snap.channels.iter().find(|c| c.name == streammix::SINK).cloned();
                        self.stream_header(ui, sink.as_ref(), card, accent);
                    }
                    for o in &columns {
                        let level = (default_out == Some(o.id)).then_some(self.shown_levels[0]);
                        self.output_header(ui, o, level, card, accent);
                    }
                    let hidden_here: Vec<String> = outputs.iter().filter(|o| self.hidden.contains(&o.name)).map(|o| o.name.clone()).collect();
                    if !hidden_here.is_empty() && ui.button(tf("Mostrar {} ocultas", &[&hidden_here.len().to_string()])).clicked() {
                        self.hidden.retain(|h| !hidden_here.contains(h));
                    }
                    ui.end_row();

                    if let Some(mic) = snap.default_of(Kind::Input).cloned() {
                        let mut effects = false;
                        let mut pick = None;
                        let (vol, mute) = channel_cell(ui, card, accent, icon_for(&mic), &mic.label, mic.volume, mic.muted, Some(self.shown_levels[1]), |ui| {
                            pick = self.device_menu(ui, mic.id, Some(&mic), &inputs, "—");
                            effects = ui.small_button(t("Efectos")).clicked();
                        });
                        if effects {
                            go = Some(Go::Effects);
                        }
                        if let Some(d) = pick {
                            self.send(Op::Default(d.id));
                        }
                        self.channel_changes(&mic, vol, mute);
                        self.send_cell(ui, card, accent, streammix::MIC);
                        blank_cells(ui, card, columns.len());
                        ui.end_row();
                    }

                    for g in all_groups {
                        let members: Vec<&Channel> = snap.of(Kind::Playback).filter(|c| groups::group_of(all_groups, c).is_some_and(|x| x.id == g.id)).collect();
                        let mut level = self.group_levels.get(&g.id).copied().unwrap_or_default();
                        let names: Vec<&str> = members.iter().map(|m| m.label.as_str()).collect();
                        let (vol, mute) = channel_cell(ui, card, accent, &g.icon, &g.name, level.volume, level.muted, None, |ui| {
                            let text = if names.is_empty() { t("Nada sonando").to_string() } else { names.join(", ") };
                            ui.add(egui::Label::new(RichText::new(text).weak().size(11.5)).truncate()).on_hover_text(names.join("\n"));
                        });
                        if vol.is_some() || mute {
                            level.volume = vol.unwrap_or(level.volume);
                            level.muted ^= mute;
                            self.group_levels.insert(g.id.clone(), level);
                            self.send(Op::Group(g.id.clone(), level, all_groups.to_vec()));
                        }
                        self.send_cell(ui, card, accent, &g.id);
                        let routed = self.routes.get(&g.id).and_then(|n| columns.iter().find(|c| &c.name == n)).map(|c| c.id);
                        let here = routed.or_else(|| members.first().and_then(|m| lands(m.device))).or(default_out);
                        for o in &columns {
                            if route_cell(ui, card, accent, here == Some(o.id)) {
                                self.routes.insert(g.id.clone(), o.name.clone());
                                self.send(Op::Route(g.id.clone(), o.name.clone(), all_groups.to_vec()));
                            }
                        }
                        ui.end_row();
                    }

                    for c in &playing {
                        let (vol, mute) = channel_cell(ui, card, accent, icon_for(c), &c.label, c.volume, c.muted, None, |ui| {
                            ui.label(RichText::new(&c.binary).weak().size(11.5));
                        });
                        self.channel_changes(c, vol, mute);
                        blank_cells(ui, card, self.stream.on as usize);
                        let here = lands(c.device);
                        for o in &columns {
                            if route_cell(ui, card, accent, here == Some(o.id)) {
                                let id = o.id;
                                self.set_shown(c.id, |x| x.device = Some(id));
                                self.send(Op::Move(c.id, o.name.clone()));
                            }
                        }
                        ui.end_row();
                    }

                    for c in &recording {
                        let mut pick = None;
                        let current = c.device.and_then(|d| inputs.iter().find(|x| x.id == d));
                        let (vol, mute) = channel_cell(ui, card, accent, icon_for(c), &c.label, c.volume, c.muted, None, |ui| {
                            pick = self.device_menu(ui, c.id, current, &inputs, t("Predeterminada"));
                        });
                        if let Some(d) = pick {
                            self.send(Op::Move(c.id, d.name));
                        }
                        self.channel_changes(c, vol, mute);
                        blank_cells(ui, card, columns.len() + self.stream.on as usize);
                        ui.end_row();
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                let hint = if all_groups.is_empty() && playing.is_empty() { t("Ninguna app está sonando.") } else { t("Haz clic en un cruce para mandar ese canal a esa salida. Cada grupo recuerda la suya.") };
                ui.label(RichText::new(hint).weak().size(12.5));
            });
        });
        if self.hidden != *hidden {
            *hidden = self.hidden.clone();
        }
        if self.names != *names {
            *names = self.names.clone();
        }
        go
    }

    /// The home page's quick controls: the microphone in use and every group, each with its mute
    /// and fader; changes go out as on the full mixer.
    pub fn mini(&mut self, ui: &mut egui::Ui, accent: Color32, all_groups: &[groups::Group]) {
        self.sync(ui.ctx());
        ui.ctx().request_repaint_after(Duration::from_millis(60));
        if self.levels_read.elapsed() > Duration::from_secs(2) && self.edited.elapsed() > Duration::from_secs(2) {
            self.group_levels = groups::levels();
            self.levels_read = Instant::now();
        }
        let snap = self.shown.clone();
        if snap.channels.is_empty() {
            ui.label(RichText::new(t("Leyendo el audio…")).weak());
            return;
        }
        let row = |ui: &mut egui::Ui, icon: &str, name: &str, sub: &str, muted: bool| {
            ui.add_sized([28.0, 28.0], egui::Label::new(RichText::new(icon).size(20.0).color(if muted { ui.visuals().weak_text_color() } else { accent })));
            ui.vertical(|ui| {
                ui.set_width(170.0);
                ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                if !sub.is_empty() {
                    ui.add(egui::Label::new(RichText::new(sub).weak().size(11.5)).truncate()).on_hover_text(sub);
                }
            });
        };
        if let Some(mic) = snap.default_of(Kind::Input).cloned() {
            let (vol, mute) = ui
                .horizontal(|ui| {
                    row(ui, icon_for(&mic), t("Micrófono"), &mic.label, mic.muted);
                    fader(ui, mic.volume, mic.muted, Some(self.shown_levels[1]), accent, 240.0)
                })
                .inner;
            self.channel_changes(&mic, vol, mute);
        }
        for g in all_groups {
            let mut level = self.group_levels.get(&g.id).copied().unwrap_or_default();
            let playing: Vec<&str> = snap.of(Kind::Playback).filter(|c| groups::group_of(all_groups, c).is_some_and(|x| x.id == g.id)).map(|c| c.label.as_str()).collect();
            let sub = if playing.is_empty() { t("Nada sonando").to_string() } else { playing.join(", ") };
            let (vol, mute) = ui
                .horizontal(|ui| {
                    row(ui, &g.icon, &g.name, &sub, level.muted);
                    fader(ui, level.volume, level.muted, None, accent, 240.0)
                })
                .inner;
            if vol.is_some() || mute {
                level.volume = vol.unwrap_or(level.volume);
                level.muted ^= mute;
                self.group_levels.insert(g.id.clone(), level);
                self.send(Op::Group(g.id.clone(), level, all_groups.to_vec()));
            }
        }
    }

    /// Sends a fader or mute change made on a device or app row.
    fn channel_changes(&mut self, c: &Channel, volume: Option<f32>, mute: bool) {
        if let Some(v) = volume {
            self.set_shown(c.id, |x| x.volume = v);
            self.send(Op::Volume(c.id, v));
        }
        if mute {
            let m = !c.muted;
            self.set_shown(c.id, |x| x.muted = m);
            self.send(Op::Mute(c.id, m));
        }
    }

    /// An output's column head: its name, whether it is the default, its fader and mute.
    fn output_header(&mut self, ui: &mut egui::Ui, o: &Channel, level: Option<f32>, card: Color32, accent: Color32) {
        let is_default = level.is_some();
        let stroke = Stroke::new(1.5, if is_default { accent } else { Color32::TRANSPARENT });
        cell_frame(card).stroke(stroke).show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(COL_W - 24.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon_for(o)).size(20.0).color(if o.muted { ui.visuals().weak_text_color() } else { accent }));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("\u{f0209}").on_hover_text(t("Ocultar esta salida")).clicked() {
                        self.hidden.push(o.name.clone());
                    }
                    if is_default {
                        ui.label(RichText::new("\u{f04ce}").color(accent)).on_hover_text(t("Predeterminada"));
                    } else if ui.small_button("\u{f04d2}").on_hover_text(t("Usar como predeterminada")).clicked() {
                        self.send(Op::Default(o.id));
                    }
                    match &mut self.renaming {
                        Some((name, text)) if *name == o.name => {
                            let r = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width()).hint_text(t("Nombre corto")));
                            r.request_focus();
                            if r.lost_focus() {
                                let short = text.trim().to_string();
                                if short.is_empty() {
                                    self.names.remove(&o.name);
                                } else {
                                    self.names.insert(o.name.clone(), short);
                                }
                                self.renaming = None;
                            }
                        }
                        _ => {
                            let r = ui.add(egui::Label::new(RichText::new(&o.label).size(13.5).strong()).truncate().sense(Sense::click()));
                            if r.on_hover_cursor(egui::CursorIcon::Text).on_hover_text(tf("{} · clic para ponerle un nombre corto", &[&o.label])).clicked() {
                                let current = self.names.get(&o.name).cloned().unwrap_or_default();
                                self.renaming = Some((o.name.clone(), current));
                            }
                        }
                    }
                });
            });
            let (vol, mute) = fader(ui, o.volume, o.muted, level, accent, COL_W - 24.0 - 84.0);
            self.channel_changes(o, vol, mute);
        }));
    }

    /// The stream mix's column head: its own fader, once the service has made the output.
    fn stream_header(&mut self, ui: &mut egui::Ui, sink: Option<&Channel>, card: Color32, accent: Color32) {
        cell_frame(card).show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(COL_W - 24.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("\u{f0003}").size(20.0).color(accent));
                ui.label(RichText::new(t("Stream")).size(13.5).strong());
            })
            .response
            .on_hover_text(t("En OBS, captura la salida «FluxDeck Stream»."));
            match sink {
                Some(o) => {
                    let (vol, mute) = fader(ui, o.volume, o.muted, None, accent, COL_W - 24.0 - 84.0);
                    self.channel_changes(o, vol, mute);
                }
                None if !fluxdeck_core::paths::service_running() => drop(ui.label(RichText::new(t("Necesita el servicio encendido.")).weak().size(11.5))),
                None => drop(ui.label(RichText::new(t("Preparando la salida…")).weak().size(11.5))),
            }
        }));
    }

    /// How loud a channel goes into the stream; muted means it is not sent. Hidden while the mix is off.
    fn send_cell(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32, key: &str) {
        if !self.stream.on {
            return;
        }
        let mut level = self.stream.send(key);
        let h = row_height(ui);
        let mut changed = false;
        cell_frame(card).show(ui, |ui| ui.vertical(|ui| {
            ui.set_width(COL_W - 24.0);
            ui.set_min_height(h - 24.0);
            ui.add_space(((h - 24.0 - 26.0) / 2.0).max(0.0));
            let (vol, mute) = fader(ui, level.volume, level.muted, None, accent, COL_W - 24.0 - 84.0);
            if let Some(v) = vol {
                // Moving a send that is off turns it on.
                (level.volume, level.muted, changed) = (v, false, true);
            }
            if mute {
                (level.muted, changed) = (!level.muted, true);
            }
        }));
        if changed {
            self.stream.sends.insert(key.to_string(), level);
            self.send(Op::Stream(self.stream.clone()));
        }
    }

    /// A device picker that can hide devices from itself; returns the one picked.
    fn device_menu(&mut self, ui: &mut egui::Ui, salt: u32, current: Option<&Channel>, devices: &[Channel], none: &str) -> Option<Channel> {
        let shown = current.map_or(none.to_string(), |d| d.label.clone());
        let mut pick = None;
        let mut hide = None;
        let mut unhide = false;
        egui::ComboBox::from_id_salt(("dev", salt)).selected_text(RichText::new(shown).size(12.0)).width(140.0).truncate().show_ui(ui, |ui| {
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
        pick
    }

    fn set_shown(&mut self, id: u32, f: impl FnOnce(&mut Channel)) {
        if let Some(c) = self.shown.channels.iter_mut().find(|c| c.id == id) {
            f(c);
        }
    }
}

const NAME_W: f32 = 400.0;
const COL_W: f32 = 200.0;
const ROW_H: f32 = 68.0;

fn cell_frame(card: Color32) -> egui::Frame {
    egui::Frame::new().fill(card).corner_radius(14).inner_margin(12)
}

/// A row's head: icon, name, whatever `extra` adds under the name, then its fader and mute.
/// Returns the new volume if it moved and whether mute was clicked.
#[allow(clippy::too_many_arguments)]
fn channel_cell(ui: &mut egui::Ui, card: Color32, accent: Color32, icon: &str, name: &str, volume: f32, muted: bool, level: Option<f32>, extra: impl FnOnce(&mut egui::Ui)) -> (Option<f32>, bool) {
    let mut out = (None, false);
    cell_frame(card).show(ui, |ui| {
        ui.set_width(NAME_W - 24.0);
        ui.set_min_height(ROW_H - 24.0);
        ui.horizontal(|ui| {
            ui.add_sized([32.0, 32.0], egui::Label::new(RichText::new(icon).size(26.0).color(if muted { ui.visuals().weak_text_color() } else { accent })));
            ui.vertical(|ui| {
                ui.set_width(150.0);
                ui.add(egui::Label::new(RichText::new(name).size(14.0).strong()).truncate()).on_hover_text(name);
                extra(ui);
            });
            out = fader(ui, volume, muted, level, accent, 110.0);
        });
    });
    out
}

/// A horizontal fader with its percent, mute button and, when given, a level bar under it.
fn fader(ui: &mut egui::Ui, volume: f32, muted: bool, level: Option<f32>, accent: Color32, width: f32) -> (Option<f32>, bool) {
    let mut out = (None, false);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().slider_width = width;
            let mut v = volume;
            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                out.0 = Some(v);
            }
            if let Some(l) = level {
                let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(rect, 2, ui.visuals().extreme_bg_color);
                let mut fill = rect;
                fill.set_right(rect.left() + rect.width() * l.clamp(0.0, 1.0));
                p.rect_filled(fill, 2, if l > 0.92 { Color32::from_rgb(229, 72, 77) } else { accent });
            }
        });
        ui.add_sized([34.0, 20.0], egui::Label::new(RichText::new(format!("{:.0}%", out.0.unwrap_or(volume) * 100.0)).size(12.5)));
        let (icon, tip) = if muted { ("\u{f0581}", t("Activar sonido")) } else { ("\u{f057e}", t("Silenciar")) };
        let b = egui::Button::new(RichText::new(icon).size(15.0).color(if muted { Color32::WHITE } else { ui.visuals().text_color() })).min_size(vec2(34.0, 26.0));
        let b = if muted { b.fill(Color32::from_rgb(179, 38, 30)) } else { b };
        out.1 = ui.add(b).on_hover_text(tip).clicked();
    });
    out
}

/// The height of the row being drawn, as the grid measured it last frame.
fn row_height(ui: &egui::Ui) -> f32 {
    ui.available_height().clamp(ROW_H, 160.0)
}

/// Where a row meets an output: a filled dot if it plays there; clicking an empty one sends it there.
fn route_cell(ui: &mut egui::Ui, card: Color32, accent: Color32, on: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(COL_W, row_height(ui)), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 14, card);
    if on {
        p.circle_filled(rect.center(), 9.0, accent);
    } else {
        p.circle_stroke(rect.center(), 8.0, Stroke::new(1.5, if resp.hovered() { accent } else { ui.visuals().weak_text_color() }));
    }
    let resp = if on { resp } else { resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t("Mandar aquí")) };
    resp.clicked() && !on
}

/// Cells for a row that has no route to pick (inputs and apps recording).
fn blank_cells(ui: &mut egui::Ui, card: Color32, n: usize) {
    for _ in 0..n {
        let (rect, _) = ui.allocate_exact_size(vec2(COL_W, row_height(ui)), Sense::hover());
        ui.painter().rect_filled(rect, 14, card.gamma_multiply(0.45));
    }
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
