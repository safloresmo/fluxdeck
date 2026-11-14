//! Peripherals view: one card per device, with the sections that OpenLinkHub, OpenRGB, HeadsetControl,
//! ratbagd and UPower know about it.
use crate::mic::{card_frame, toggle};
use fluxdeck_core::corsair::{self, LEVELS, Live};
use fluxdeck_core::presets::{Env, title};
use fluxdeck_core::helpers::{self, Fix, Need, State};
use fluxdeck_core::peripherals::{self, Peripheral};
use fluxdeck_core::{headset, openrgb, ratbag, upower};
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
    MouseDpi(String, u32),
    MouseProfile(String, u32),
}

impl Write {
    /// Writes of one kind to one device replace each other: only the last one in a burst is sent.
    fn target(&self) -> (std::mem::Discriminant<Write>, &str) {
        let id = match self {
            Write::Lcd(id, _) | Write::Rotation(id, _) | Write::Anc(id, _) | Write::Brightness(id, _) | Write::Effect(id, _) => id,
            Write::RgbEffect(id, _) | Write::RgbBrightness(id, _) | Write::RgbColor(id, _) => id,
            Write::Sidetone(id, _) | Write::HeadsetLights(id, _) | Write::MouseDpi(id, _) | Write::MouseProfile(id, _) => id,
        };
        (std::mem::discriminant(self), id)
    }
}

#[derive(Default)]
struct Others {
    rgb: Vec<openrgb::Device>,
    batteries: Vec<upower::Battery>,
    headsets: Vec<headset::Headset>,
    mice: Vec<ratbag::Mouse>,
}

impl Others {
    fn peripherals(&self, corsair: &[corsair::Device]) -> Vec<Peripheral> {
        peripherals::merge(corsair, &self.rgb, &self.headsets, &self.mice, &self.batteries)
    }
}

/// Every helper program and its state; None until the first check ends. Shared so that a new `Devices`
/// starts from the last known result instead of an empty list.
static HELPERS: Mutex<Option<Vec<Need>>> = Mutex::new(None);

fn known_helpers() -> Option<Vec<Need>> {
    HELPERS.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    modules: Vec<String>,
    /// A module the user asked to switch on from a suggestion, for the app to apply.
    pub enable: Option<&'static str>,
    watching: Arc<AtomicBool>,
}

impl Devices {
    /// `modules` are the optional modules switched on in the config.
    pub fn new(env: &Env, modules: &[String]) -> Devices {
        let list = &env.corsair;
        let rgb_on = modules.iter().any(|m| m == "openrgb");
        let headset_on = modules.iter().any(|m| m == "headset");
        let mouse_on = modules.iter().any(|m| m == "mouse");
        let latest = Arc::new(Mutex::new(None));
        let latest_others = Arc::new(Mutex::new(None));
        let active = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(Vec::new()));
        let watching = Arc::new(AtomicBool::new(false));
        {
            let watching = watching.clone();
            std::thread::spawn(move || {
                let mut last = Instant::now() - Duration::from_secs(60);
                while Arc::strong_count(&watching) > 1 {
                    if watching.load(Ordering::Relaxed) && last.elapsed() > Duration::from_secs(3) {
                        // A thread, because HeadsetControl can take seconds.
                        *HELPERS.lock().unwrap_or_else(|e| e.into_inner()) = Some(helpers::all(headset_on));
                        last = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
            });
        }
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
                        let mice = if mouse_on { ratbag::mice() } else { Vec::new() };
                        *latest_others.lock().unwrap() = Some(Others { rgb, batteries: upower::batteries(), headsets, mice });
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
                        Write::MouseDpi(id, dpi) => ratbag::mouse(id).ok_or_else(|| format!("ratbagd does not see {id}")).and_then(|m| ratbag::set_dpi(&m, *dpi)),
                        Write::MouseProfile(id, i) => ratbag::mouse(id).ok_or_else(|| format!("ratbagd does not see {id}")).and_then(|m| ratbag::set_profile(&m, *i).map(|_| ())),
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
            others: Others { rgb: env.rgb.clone(), batteries: env.batteries.clone(), headsets: env.headsets.clone(), mice: env.mice.clone() },
            modules: modules.to_vec(),
            enable: None,
            watching,
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

    /// `helpers` is on while a screen shows the helper programs' state.
    pub fn set_active(&mut self, on: bool, helpers: bool) {
        self.active.store(on, Ordering::Relaxed);
        self.watching.store(helpers, Ordering::Relaxed);
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
        let checked = known_helpers();
        let needs: Vec<Need> = checked.iter().flatten().filter(|n| !n.devices.is_empty() && (n.state != State::Ready || !self.modules.iter().any(|m| m == n.helper.module()))).cloned().collect();
        for n in &needs {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                card_frame(card).show(ui, |ui| {
                    ui.set_width((ui.available_width() - 20.0).min(760.0));
                    ui.vertical(|ui| self.need_card(ui, n, accent));
                });
            });
            ui.add_space(12.0);
        }
        let olh_suggested = needs.iter().any(|n| n.helper == helpers::Helper::OpenLinkHub);
        let mut told = olh_suggested;
        // Nothing about Corsair unless a Corsair device is plugged in.
        if self.corsair_on && self.corsair_connected() && self.list.is_empty() && !olh_suggested {
            let msg = if self.running { t("OpenLinkHub no ve ningún aparato Corsair.") } else { t("OpenLinkHub no está corriendo. Instálalo y arráncalo: github.com/jurkovic-nikola/OpenLinkHub") };
            ui.vertical_centered(|ui| ui.label(RichText::new(msg).weak()));
            ui.add_space(12.0);
            told = true;
        }
        let all = self.others.peripherals(&self.list);
        if all.is_empty() {
            if checked.is_none() {
                ui.vertical_centered(|ui| ui.label(RichText::new(t("Comprobando…")).weak()));
            } else if !told {
                ui.vertical_centered(|ui| ui.label(RichText::new(t("No hay periféricos que mostrar. Los módulos de cada marca se activan en Ajustes → Extensiones.")).weak()));
            }
            return;
        }
        // Rows of as many cards as fit, all the same width: wrapping a row of frames lets the last ones run off screen.
        // A card is 40 wider than its content (margins) and 20 apart from the next; the spare width is shared out like the plugin grid does.
        let room = ui.available_width() - 48.0;
        let per_row = (((room + 20.0) / 360.0) as usize).max(1);
        let width = ((room - 20.0 * (per_row as f32 - 1.0)) / per_row as f32).floor() - 40.0;
        for row in all.chunks(per_row) {
            ui.horizontal_top(|ui| {
                ui.add_space(20.0);
                for p in row {
                    card_frame(card).show(ui, |ui| {
                        ui.set_width(width);
                        // The slider and its percentage must fit inside the card.
                        ui.spacing_mut().slider_width = width - 70.0;
                        ui.vertical(|ui| self.peripheral_card(ui, p, accent));
                    });
                    ui.add_space(12.0);
                }
            });
            ui.add_space(12.0);
        }
    }

    /// Whether a Corsair device is plugged in, which is what OpenLinkHub is needed for.
    pub fn corsair_connected(&self) -> bool {
        known_helpers().is_some_and(|h| h.iter().any(|n| n.helper == helpers::Helper::OpenLinkHub && !n.devices.is_empty()))
    }

    fn peripheral_card(&mut self, ui: &mut egui::Ui, p: &Peripheral, accent: Color32) {
        let name = if p.corsair.is_some() { p.name.clone() } else { title(&p.name) };
        header(ui, p.icon(), &name);
        let live = p.corsair.as_ref().and_then(|d| self.shown.get(&d.serial)).cloned().unwrap_or_default();
        let level = live.battery.or(p.headset.as_ref().and_then(|h| h.battery)).or(p.battery.as_ref().map(|b| b.level));
        let corsair_lights = live.effect.is_some() || live.brightness.is_some();
        // OpenLinkHub gives no color: with its lights, OpenRGB only adds the color.
        let rgb = p.rgb.as_ref().filter(|d| d.takes_color() || !corsair_lights && (d.brightness().is_some() || d.effect().is_some()));
        let headset = p.headset.as_ref().filter(|h| h.has_sidetone || h.has_lights);
        let mouse = p.mouse.as_ref().filter(|m| !m.dpis().is_empty() || m.profiles.len() > 1);
        if level.is_none() && live.anc.is_none() && !corsair_lights && live.lcd.is_none() && rgb.is_none() && headset.is_none() && mouse.is_none() {
            ui.label(RichText::new(t("Sin ajustes que controlar desde aquí.")).weak().size(12.5));
        }
        if let Some(b) = level {
            battery(ui, b, accent);
        }
        if let Some(d) = &p.corsair {
            self.corsair_sections(ui, d, accent);
        }
        if let Some(d) = rgb {
            self.rgb_sections(ui, d, corsair_lights);
        }
        if let Some(h) = headset {
            self.headset_sections(ui, h, accent);
        }
        if let Some(m) = mouse {
            self.mouse_sections(ui, m);
        }
    }

    fn need_card(&mut self, ui: &mut egui::Ui, n: &Need, accent: Color32) {
        let name = n.helper.name();
        let devices = n.devices.iter().map(|d| title(d)).collect::<Vec<_>>().join(", ");
        ui.label(RichText::new(t("Conectado")).size(15.0).strong());
        ui.label(RichText::new(devices).weak());
        let what = match n.state {
            State::Missing => tf("Para controlarlo hace falta {}, que no está instalado.", &[name]),
            State::Stopped => tf("Para controlarlo hace falta {}, que está instalado pero no está corriendo.", &[name]),
            State::Ready => tf("{} está listo. Activa su módulo para controlarlo desde aquí.", &[name]),
        };
        ui.label(what);
        ui.add_space(6.0);
        if let Some(Fix::Terminal(cmd)) = &n.fix {
            ui.label(RichText::new(cmd).monospace().color(accent));
            ui.add_space(4.0);
        }
        self.fix_buttons(ui, n, true);
    }

    /// The button that fixes a program: a command for a terminal, the program, or its page.
    fn fix_buttons(&mut self, ui: &mut egui::Ui, n: &Need, module_button: bool) {
        let name = n.helper.name();
        ui.horizontal(|ui| {
            match &n.fix {
                Some(Fix::Terminal(cmd)) => {
                    if ui.button(t("Abrir terminal con el comando")).on_hover_text(t("Se ejecuta cuando pulses Enter; ahí escribes tu contraseña")).clicked()
                        && let Err(e) = fluxdeck_core::effects::run_in_terminal(cmd)
                    {
                        self.notice = e;
                    }
                    if ui.button(t("Copiar comando")).clicked() {
                        ui.ctx().copy_text(cmd.clone());
                        self.notice = t("Comando copiado: pégalo en una terminal").into();
                    }
                }
                Some(Fix::Launch(args)) => {
                    if ui.button(tf("Abrir {}", &[name])).clicked()
                        && let Err(e) = std::process::Command::new(&args[0]).args(&args[1..]).spawn()
                    {
                        self.notice = e.to_string();
                    }
                }
                Some(Fix::Page(url)) => {
                    if ui.button(t("Ver cómo instalarlo")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                None => {}
            }
            let module = n.helper.module();
            if module_button && !self.modules.iter().any(|m| m == module) && ui.button(t("Activar módulo")).clicked() {
                self.enable = Some(module);
            }
        });
    }

    /// One card per helper program with its state and the way to fix it, for Settings.
    pub fn helpers_ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
        let Some(all) = known_helpers() else {
            ui.label(RichText::new(t("Comprobando…")).weak());
            return;
        };
        for n in all {
            // Not installed and no connected device needs it: nothing is wrong.
            let idle = n.devices.is_empty() && n.state == State::Missing;
            card_frame(card).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(n.helper.name()).size(15.0).strong());
                    let (text, color) = match n.state {
                        State::Missing if idle => (t("No lo necesitas: no hay aparatos conectados que lo usen"), ui.visuals().weak_text_color()),
                        State::Ready => (t("Instalado y listo"), accent),
                        State::Stopped => (t("Instalado pero parado"), Color32::from_rgb(217, 164, 65)),
                        State::Missing => (t("Falta"), Color32::from_rgb(229, 72, 77)),
                    };
                    ui.label(RichText::new(text).color(color));
                });
                if !idle {
                    let used = if n.devices.is_empty() { t("Ninguno de tus aparatos lo necesita ahora.").to_string() } else { tf("Lo necesitan: {}", &[&n.devices.iter().map(|d| title(d)).collect::<Vec<_>>().join(", ")]) };
                    ui.label(RichText::new(used).weak().size(12.5));
                }
                if let Some(Fix::Terminal(cmd)) = &n.fix {
                    ui.add_space(4.0);
                    ui.label(RichText::new(cmd).monospace().color(if idle { ui.visuals().weak_text_color() } else { accent }));
                }
                if n.fix.is_some() {
                    ui.add_space(4.0);
                    self.fix_buttons(ui, &n, false);
                }
            });
            ui.add_space(8.0);
        }
    }

    fn headset_sections(&mut self, ui: &mut egui::Ui, h: &headset::Headset, accent: Color32) {
        let mut live = h.clone();
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

    fn mouse_sections(&mut self, ui: &mut egui::Ui, m: &ratbag::Mouse) {
        let mut live = m.clone();
        if let Some(cur) = m.dpi().filter(|_| !m.dpis().is_empty()) {
            ui.label("DPI");
            egui::ComboBox::from_id_salt(("dpi", &m.id)).selected_text(cur.to_string()).width(220.0).show_ui(ui, |ui| {
                for d in m.dpis() {
                    if ui.selectable_label(*d == cur, d.to_string()).clicked() && *d != cur {
                        if let Some(r) = live.resolutions.iter_mut().find(|r| r.active) {
                            r.dpi = *d;
                        }
                        self.send(Write::MouseDpi(m.id.clone(), *d));
                    }
                }
            });
            ui.add_space(8.0);
        }
        let enabled: Vec<&ratbag::Profile> = m.profiles.iter().filter(|p| !p.disabled).collect();
        if let (true, Some(cur)) = (enabled.len() > 1, m.active_profile()) {
            let label = |p: &ratbag::Profile| if p.name.is_empty() { format!("{} {}", t("Perfil"), p.index + 1) } else { p.name.clone() };
            ui.label(t("Perfil"));
            egui::ComboBox::from_id_salt(("profile", &m.id)).selected_text(label(cur)).width(220.0).show_ui(ui, |ui| {
                for p in enabled {
                    if ui.selectable_label(p.active, label(p)).clicked() && !p.active {
                        for q in live.profiles.iter_mut() {
                            q.active = q.index == p.index;
                        }
                        self.send(Write::MouseProfile(m.id.clone(), p.index));
                    }
                }
            });
            ui.add_space(8.0);
        }
        ui.label(RichText::new(t("Sin probar en hardware: cuéntanos si funciona con tu ratón")).weak().size(12.0));
        // The new values show until ratbagd's next reading.
        if let Some(slot) = self.others.mice.iter_mut().find(|x| x.id == m.id) {
            *slot = live;
        }
    }

    fn rgb_sections(&mut self, ui: &mut egui::Ui, d: &openrgb::Device, color_only: bool) {
        let mut live = d.clone();
        if let Some(mut b) = d.brightness().filter(|_| !color_only) {
            ui.label(t("Brillo"));
            if ui.add(egui::Slider::new(&mut b, 0..=100).suffix("%")).changed() {
                self.send(Write::RgbBrightness(d.id.clone(), b));
            }
            ui.add_space(8.0);
        }
        if let Some(cur) = d.effect().filter(|_| !color_only).map(String::from) {
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
        // The new effect shows until OpenRGB's next reading.
        if let Some(slot) = self.others.rgb.iter_mut().find(|r| r.id == d.id) {
            *slot = live;
        }
    }

    /// The keyboard with G keys, if OpenLinkHub sees one.
    pub fn keyboard(&self) -> Option<String> {
        self.list.iter().find(|d| d.kind == Some(0)).map(|d| d.serial.clone())
    }

    fn corsair_sections(&mut self, ui: &mut egui::Ui, d: &corsair::Device, accent: Color32) {
        let mut live = self.shown.get(&d.serial).cloned().unwrap_or_default();
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
        self.shown.insert(d.serial.clone(), live);
    }
}

fn header(ui: &mut egui::Ui, icon: char, name: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(format!("{icon} ")).size(22.0));
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
