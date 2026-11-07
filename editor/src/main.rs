mod devices;
mod effects;
mod home;
mod icons;
mod mic;
mod mixer;
mod store;

use fluxdeck_core::image::RgbaImage;
use fluxdeck_core::presets::{self, Item, Preset};
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::{Config, Dial, Key, Page, Painter, State, Theme, config, pw, render};
use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, Id, LayerId, Order, RichText, Sense, Stroke, StrokeKind, TextEdit, TextureHandle, TextureOptions, Vec2, vec2,
};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const KEY_PX: u32 = 144;
const LOGO: &str = include_str!("../../assets/fluxdeck.svg");
const STRIP: (u32, u32) = (200, 100);
/// Stream Deck + layout, used while no deck is connected.
const DEFAULT_LAYOUT: (u8, u8, u8) = (4, 2, 4);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Sel {
    Key(u8),
    Dial(u8),
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Home,
    Deck,
    Mic,
    Devices,
    Plugins,
    Profiles,
    Settings,
}

/// What is being dragged.
#[derive(Clone, Copy)]
enum Drag {
    Slot(Sel),
    Preset(usize),
}

fn main() -> eframe::Result {
    if let Ok(c) = Config::load() {
        fluxdeck_core::i18n::init(&c.language);
    }
    let logo = fluxdeck_core::plugins::decode(LOGO.as_bytes(), 128).map(|img| egui::IconData { rgba: img.into_raw(), width: 128, height: 128 });
    let mut viewport = egui::ViewportBuilder::default();
    if let Some(icon) = logo {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_app_id(format!("{}-editor", fluxdeck_core::paths::NAME))
            .with_title("FluxDeck")
            .with_inner_size([1280.0, 760.0])
            .with_min_inner_size([980.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native("FluxDeck", options, Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))))
}

struct App {
    cfg: Config,
    saved: Config,
    /// Set when config.toml could not be parsed: saving would destroy it.
    blocked: Option<String>,
    /// When the last automatic save failed; it is tried again a little later, not every frame.
    save_failed: Option<Instant>,
    theme: Theme,
    styled: Option<Theme>,
    theme_checked: Instant,
    page: usize,
    sel: Option<Sel>,
    painter: Painter,
    font: String,
    textures: HashMap<Sel, TextureHandle>,
    drawn: Option<(Page, Theme, String, HashMap<Sel, String>, Vec<Option<std::time::SystemTime>>)>,
    /// Status output from t("Probar"), shown while the deck is not on this page.
    tested: HashMap<Sel, String>,
    live: Option<State>,
    live_checked: Instant,
    presets: Vec<Preset>,
    sources: Vec<pw::Node>,
    filter: String,
    glyphs: Vec<(char, String)>,
    icon_query: String,
    new_page: String,
    rename: String,
    notice: String,
    confirm_close: bool,
    closing: bool,
    view: View,
    mic: mic::Mic,
    devices: devices::Devices,
    store: store::Store,
    /// Icons of plugin actions in the library, by file path.
    lib_icons: HashMap<String, Option<egui::TextureHandle>>,
    service_up: bool,
    service_checked: Instant,
    /// Undo history: the config as it was before each change, and what undo took away.
    history: Vec<Config>,
    future: Vec<Config>,
    committed: Config,
    prev_frame: Config,
    changed_at: Instant,
    /// A copied key or dial, for Ctrl+V.
    clip: Option<Item>,
    /// When the current notice appeared; it fades after a few seconds.
    notice_at: (String, Instant),
    home: home::Home,
    mixer: mixer::Mixer,
    /// Audio view: 0 mixer, 1 microphone.
    audio_tab: usize,
    settings_tab: usize,
    startup: fluxdeck_core::paths::Startup,
    /// Backups: whether plugin settings go in, the backups found, the one chosen to import, and a
    /// file being picked with the system's file chooser.
    backup_plugins: bool,
    backups: Option<Vec<std::path::PathBuf>>,
    backup_choice: Option<std::path::PathBuf>,
    backup_pick: Option<std::sync::Arc<std::sync::Mutex<Option<Option<std::path::PathBuf>>>>>,
    /// Which phone's QR code is shown, and the deck shape the deck page is edited for (None: the
    /// Stream Deck's) with the phones that have decks of their own, read now and then.
    phone_qr: Option<String>,
    view_as: Option<String>,
    phone_shapes: (Vec<(String, String, (u8, u8, u8))>, Instant),
    /// The icon store window and the key or dial it picks an icon for.
    icon_store: icons::IconStore,
    icon_target: Option<Sel>,
    /// Installed font families, read when the font menu is first shown, and its search text.
    font_families: Option<Vec<String>>,
    font_query: String,
    /// The FluxDeck widget for a Ryoku bar, read when the settings tab opens.
    ryoku_widget: Option<fluxdeck_core::ryoku::Widget>,
    zoom: f32,
    /// Apps playing, for the app picker, and when that was read.
    apps_now: (Vec<String>, Instant),
    /// What is being typed to add an app, per group.
    new_app: HashMap<String, String>,
    logo: Option<TextureHandle>,
    installed_apps: Option<Vec<fluxdeck_core::apps::App>>,
    profile_sel: usize,
    /// The keyboard's G keys (name, id, still on its own job) and when that was read.
    gkeys: (Vec<(String, u64, bool)>, Instant),
    /// The action library on narrow windows, where it hides behind a button.
    lib_open: bool,
    /// Library groups the user folded, remembered between runs.
    folded: std::collections::BTreeSet<String>,
    /// (class, title) of open windows, for adding apps to a profile, and when that was read.
    windows: (Vec<(String, String)>, Instant),
    active_profile: Option<String>,
    app_search: String,
}

/// What the right-click menu of a slot asked for.
#[derive(Clone, Copy)]
enum SlotAct {
    Copy,
    Paste,
    Duplicate,
    Remove,
}

impl App {
    fn new(ctx: &egui::Context) -> App {
        setup_fonts(ctx);
        let (cfg, blocked) = match Config::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        let nodes = pw::nodes();
        let env = presets::Env::detect(&cfg.modules);
        let devices = devices::Devices::new(&env, &cfg.modules);
        App {
            painter: Painter::new(&cfg.font),
            font: cfg.font.clone(),
            saved: cfg.clone(),
            rename: cfg.pages[0].name.clone(),
            theme: cfg.effective_theme(),
            cfg,
            blocked,
            styled: None,
            theme_checked: Instant::now(),
            page: 0,
            sel: None,
            textures: HashMap::new(),
            drawn: None,
            tested: HashMap::new(),
            live: State::read(),
            live_checked: Instant::now(),
            presets: presets::all(&env),
            mic: mic::Mic::new(env.dock.clone(), env.filters.first().cloned()),
            devices,
            store: store::Store::new(),
            lib_icons: HashMap::new(),
            view: match std::env::args().nth(1).as_deref() {
                Some("--deck") => View::Deck,
                Some("--settings") => View::Settings,
                Some("--profiles") => View::Profiles,
                Some("--mic" | "--audio" | "--groups") => View::Mic,
                Some("--devices") => View::Devices,
                Some("--plugins") => View::Plugins,
                _ => View::Home,
            },
            sources: nodes.into_iter().filter(|n| n.class == "Audio/Source").collect(),
            filter: String::new(),
            glyphs: glyph_names(),
            icon_query: String::new(),
            new_page: String::new(),
            notice: String::new(),
            confirm_close: false,
            closing: false,
            service_up: fluxdeck_core::paths::service_running(),
            service_checked: Instant::now(),
            history: Vec::new(),
            future: Vec::new(),
            committed: Config::default(),
            save_failed: None,
            prev_frame: Config::default(),
            changed_at: Instant::now(),
            clip: None,
            notice_at: (String::new(), Instant::now()),
            home: home::Home::new(),
            mixer: mixer::Mixer::new(),
            audio_tab: match std::env::args().nth(1).as_deref() {
                Some("--mic") => 1,
                Some("--groups") => 2,
                _ => 0,
            },
            // `--settings 2` opens a given tab.
            settings_tab: std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(0),
            startup: fluxdeck_core::paths::startup(),
            backup_plugins: false,
            backups: None,
            backup_choice: None,
            backup_pick: None,
            phone_qr: None,
            view_as: None,
            phone_shapes: (Vec::new(), Instant::now() - Duration::from_secs(60)),
            icon_store: icons::IconStore::new(),
            icon_target: None,
            font_families: None,
            font_query: String::new(),
            ryoku_widget: None,
            zoom: 0.0,
            apps_now: (Vec::new(), Instant::now() - Duration::from_secs(10)),
            new_app: HashMap::new(),
            logo: None,
            installed_apps: None,
            profile_sel: 0,
            gkeys: (Vec::new(), Instant::now() - Duration::from_secs(60)),
            lib_open: false,
            folded: std::fs::read_to_string(fluxdeck_core::paths::state().join("library-folded")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect(),
            windows: (Vec::new(), Instant::now() - Duration::from_secs(10)),
            active_profile: None,
            app_search: String::new(),
        }
        .with_history()
    }

    fn with_history(mut self) -> App {
        self.committed = self.cfg.clone();
        self.prev_frame = self.cfg.clone();
        self
    }

    /// Records a step in the undo history once an edit settles: not mid-drag, not mid-typing.
    fn track_history(&mut self, ctx: &egui::Context) {
        if self.cfg != self.prev_frame {
            self.prev_frame = self.cfg.clone();
            self.changed_at = Instant::now();
        }
        let settled = ctx.dragged_id().is_none() && (!ctx.egui_wants_keyboard_input() || self.changed_at.elapsed() > Duration::from_millis(900));
        if self.cfg != self.committed && settled && self.changed_at.elapsed() > Duration::from_millis(150) {
            self.commit();
        }
        // Every change is saved on its own once it settles; a failed save waits before trying again.
        let retry = self.save_failed.is_none_or(|at| at.elapsed() > Duration::from_secs(5));
        if self.dirty() && settled && self.changed_at.elapsed() > Duration::from_millis(150) && self.blocked.is_none() && retry {
            self.save();
        }
    }

    fn commit(&mut self) {
        if self.cfg != self.committed {
            self.history.push(std::mem::replace(&mut self.committed, self.cfg.clone()));
            self.future.clear();
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
    }

    fn undo(&mut self) {
        self.commit();
        if let Some(prev) = self.history.pop() {
            self.future.push(std::mem::replace(&mut self.cfg, prev));
            self.after_jump(t("Deshecho"));
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.future.pop() {
            self.history.push(std::mem::replace(&mut self.cfg, next));
            self.after_jump(t("Rehecho"));
        }
    }

    fn after_jump(&mut self, msg: &str) {
        self.committed = self.cfg.clone();
        self.prev_frame = self.cfg.clone();
        self.page = self.page.min(self.cfg.pages.len() - 1);
        self.rename = self.cfg.pages[self.page].name.clone();
        self.theme = self.cfg.effective_theme();
        self.notice = msg.into();
    }

    fn slot_act(&mut self, sel: Sel, act: SlotAct) {
        let page = &mut self.cfg.pages[self.page];
        match act {
            SlotAct::Copy => {
                self.clip = match sel {
                    Sel::Key(i) => page.key(i).cloned().map(Item::Key),
                    Sel::Dial(i) => page.dial(i).cloned().map(Item::Dial),
                };
                if self.clip.is_some() {
                    self.notice = t("Copiada").into();
                }
            }
            SlotAct::Paste => match (self.clip.clone(), sel) {
                (Some(Item::Key(k)), Sel::Key(pos)) => {
                    page.keys.retain(|k| k.pos != pos);
                    page.keys.push(Key { pos, instance: instance_for(&k.action), ..k });
                    self.sel = Some(sel);
                }
                (Some(Item::Dial(d)), Sel::Dial(pos)) => {
                    page.dials.retain(|d| d.pos != pos);
                    page.dials.push(Dial { pos, instance: instance_for(&d.action), ..d });
                    self.sel = Some(sel);
                }
                (Some(Item::Key(_)), _) => self.notice = t("Lo copiado es una tecla").into(),
                (Some(Item::Dial(_)), _) => self.notice = t("Lo copiado es una perilla").into(),
                (None, _) => self.notice = t("No hay nada copiado").into(),
            },
            SlotAct::Duplicate => {
                let (cols, rows, dials) = self.layout();
                let page = &mut self.cfg.pages[self.page];
                match sel {
                    Sel::Key(i) => {
                        let Some(k) = page.key(i).cloned() else { return };
                        match (0..cols * rows).find(|p| page.key(*p).is_none()) {
                            Some(pos) => {
                                page.keys.push(Key { pos, instance: instance_for(&k.action), ..k });
                                self.sel = Some(Sel::Key(pos));
                            }
                            None => self.notice = t("No quedan teclas libres en esta página").into(),
                        }
                    }
                    Sel::Dial(i) => {
                        let Some(d) = page.dial(i).cloned() else { return };
                        match (0..dials).find(|p| page.dial(*p).is_none()) {
                            Some(pos) => {
                                page.dials.push(Dial { pos, instance: instance_for(&d.action), ..d });
                                self.sel = Some(Sel::Dial(pos));
                            }
                            None => self.notice = t("No quedan perillas libres en esta página").into(),
                        }
                    }
                }
            }
            SlotAct::Remove => {
                match sel {
                    Sel::Key(i) => page.keys.retain(|k| k.pos != i),
                    Sel::Dial(i) => page.dials.retain(|d| d.pos != i),
                }
                self.tested.remove(&sel);
            }
        }
    }

    /// Keyboard shortcuts; skipped while a text field has the keyboard.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key as K, KeyboardShortcut as S, Modifiers as M};
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let hit = |m: M, k: K| ctx.input_mut(|i| i.consume_shortcut(&S::new(m, k)));
        if hit(M::COMMAND | M::SHIFT, K::Z) || hit(M::COMMAND, K::Y) {
            self.redo();
        } else if hit(M::COMMAND, K::Z) {
            self.undo();
        }
        let views = [(K::Num1, View::Home), (K::Num2, View::Deck), (K::Num3, View::Mic), (K::Num4, View::Devices), (K::Num5, View::Profiles), (K::Num6, View::Plugins), (K::Num7, View::Settings), (K::Comma, View::Settings)];
        for (k, v) in views {
            if hit(M::COMMAND, k) && self.view_available(v) {
                self.view = v;
            }
        }
        if self.view != View::Deck {
            return;
        }
        if hit(M::NONE, K::Escape) {
            self.sel = None;
        }
        let Some(sel) = self.sel else { return };
        if hit(M::COMMAND, K::C) {
            self.slot_act(sel, SlotAct::Copy);
        } else if hit(M::COMMAND, K::V) {
            self.slot_act(sel, SlotAct::Paste);
        } else if hit(M::COMMAND, K::D) {
            self.slot_act(sel, SlotAct::Duplicate);
        } else if hit(M::NONE, K::Delete) || hit(M::NONE, K::Backspace) {
            self.slot_act(sel, SlotAct::Remove);
        }
    }

    fn start_service(&mut self) {
        match fluxdeck_core::paths::start_service() {
            Ok(()) => self.notice = t("Servicio iniciado").into(),
            Err(e) => self.notice = tf("No se pudo iniciar el servicio: {}", &[&e]),
        }
        self.service_checked = Instant::now() - Duration::from_secs(5);
    }

    /// Sidebar entries that apply here: (view, icon, name).
    fn nav(&self) -> Vec<(View, &'static str, &'static str)> {
        [
            (View::Home, "\u{f02dc}", t("Inicio")),
            (View::Deck, "\u{f0570}", t("Deck")),
            (View::Mic, "\u{f147d}", t("Audio")),
            (View::Devices, "\u{f030c}", t("Periféricos")),
            (View::Profiles, "\u{f0328}", t("Perfiles")),
            (View::Plugins, "\u{f0431}", t("Plugins")),
            (View::Settings, "\u{f0493}", t("Ajustes")),
        ]
        .into_iter()
        .filter(|(v, _, _)| self.view_available(*v))
        .collect()
    }

    /// Left navigation; only icons when the window is narrow.
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let narrow = ui.ctx().content_rect().width() < 1360.0;
        let width = if narrow { 64.0 } else { 190.0 };
        let accent = self.accent();
        egui::Panel::left("nav").resizable(false).exact_size(width).show(ui, |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(if narrow { 10.0 } else { 8.0 });
                let logo = self.logo.get_or_insert_with(|| {
                    let img = fluxdeck_core::plugins::decode(LOGO.as_bytes(), 64).unwrap_or_else(|| RgbaImage::new(1, 1));
                    ui.ctx().load_texture("logo", to_egui(&img), TextureOptions::LINEAR)
                });
                ui.add(egui::Image::new(&*logo).fit_to_exact_size(vec2(28.0, 28.0)));
                if !narrow {
                    ui.add_space(4.0);
                    ui.label(RichText::new("FluxDeck").size(19.0).strong());
                }
            });
            ui.add_space(18.0);
            for (v, icon, name) in self.nav() {
                let on = self.view == v;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
                let fill = if on { mix(ui.visuals().panel_fill, accent, 0.22) } else { mix(ui.visuals().panel_fill, ui.visuals().faint_bg_color, hover) };
                ui.painter().rect_filled(rect.shrink2(vec2(6.0, 2.0)), 10, fill);
                let color = if on { accent } else { ui.visuals().text_color() };
                let x = rect.left() + if narrow { rect.width() / 2.0 } else { 26.0 };
                let anchor = if narrow { egui::Align2::CENTER_CENTER } else { egui::Align2::LEFT_CENTER };
                ui.painter().text(egui::pos2(x - if narrow { 0.0 } else { 8.0 }, rect.center().y), anchor, icon, egui::FontId::proportional(19.0), color);
                if !narrow {
                    ui.painter().text(egui::pos2(rect.left() + 48.0, rect.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(15.0), color);
                }
                let resp = if narrow { resp.on_hover_text(name) } else { resp };
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.view = v;
                }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.add_space(14.0);
                let (dot, text) = if self.service_up { (Color32::from_rgb(80, 200, 120), t("Servicio activo")) } else { (Color32::from_rgb(229, 72, 77), t("Servicio apagado")) };
                ui.horizontal(|ui| {
                    ui.add_space(if narrow { 24.0 } else { 16.0 });
                    let r = ui.label(RichText::new("●").color(dot));
                    if narrow {
                        r.on_hover_text(text);
                    } else {
                        ui.label(RichText::new(text).weak().size(12.5));
                    }
                });
            });
        });
    }

    fn view_available(&self, v: View) -> bool {
        match v {
            View::Home | View::Deck | View::Plugins | View::Profiles | View::Settings => true,
            View::Mic => true,
            View::Devices => true,
        }
    }

    fn dirty(&self) -> bool {
        self.cfg != self.saved
    }

    fn layout(&self) -> (u8, u8, u8) {
        if let Some(id) = &self.view_as
            && let Some((_, _, shape)) = self.phone_shapes.0.iter().find(|p| &p.0 == id)
        {
            return *shape;
        }
        match &self.live {
            Some(s) if s.cols > 0 => (s.cols, s.rows, s.dials),
            _ => DEFAULT_LAYOUT,
        }
    }

    fn save(&mut self) -> bool {
        if let Some(e) = &self.blocked {
            self.notice = tf("No guardo: el archivo tiene un error ({}). Corrígelo y pulsa Recargar.", &[e]);
            return false;
        }
        match self.cfg.save() {
            Ok(()) => {
                let plugins_changed = self.saved.disabled_plugins != self.cfg.disabled_plugins;
                self.saved = self.cfg.clone();
                if plugins_changed {
                    self.redetect();
                }
                self.save_failed = None;
                true
            }
            Err(e) => {
                self.notice = tf("No se pudo guardar: {}", &[&e.to_string()]);
                self.save_failed = Some(Instant::now());
                false
            }
        }
    }

    fn reload(&mut self) {
        match Config::load() {
            Ok(c) => {
                self.saved = c.clone();
                self.cfg = c;
                self.blocked = None;
                self.notice = t("Recargado").into();
            }
            Err(e) => {
                self.notice = e.clone();
                self.blocked = Some(e);
            }
        }
        self.redetect();
        self.select_page(self.page.min(self.cfg.pages.len() - 1));
        self.history.clear();
        self.future.clear();
        self.committed = self.cfg.clone();
        self.prev_frame = self.cfg.clone();
    }

    /// Finds devices again, after a reload or a module switched on or off.
    fn redetect(&mut self) {
        let env = presets::Env::detect(&self.cfg.modules);
        self.presets = presets::all(&env);
        self.devices = devices::Devices::new(&env, &self.cfg.modules);
        if self.view == View::Devices && !["corsair", "openrgb", "headset"].iter().any(|m| self.cfg.module(m)) && env.batteries.is_empty() {
            self.view = View::Deck;
        }
    }

    fn select_page(&mut self, i: usize) {
        self.page = i;
        self.sel = None;
        self.tested.clear();
        self.rename = self.cfg.pages[i].name.clone();
    }

    /// Reads the daemon's state and the theme files now and then; cheap enough to do twice a second.
    fn poll(&mut self, ctx: &egui::Context) {
        if self.live_checked.elapsed() > Duration::from_millis(500) {
            self.live = State::read();
            self.live_checked = Instant::now();
        }
        if self.theme_checked.elapsed() > Duration::from_secs(2) {
            self.theme = self.cfg.effective_theme();
            self.theme_checked = Instant::now();
        }
        let scale = if self.cfg.ui_scale > 0.5 { self.cfg.ui_scale } else { 1.0 };
        if scale != self.zoom {
            ctx.set_zoom_factor(scale);
            self.zoom = scale;
        }
        if self.service_checked.elapsed() > Duration::from_secs(2) {
            self.service_up = fluxdeck_core::paths::service_running();
            self.service_checked = Instant::now();
        }
        // Notices fade after a few seconds.
        if self.notice != self.notice_at.0 {
            self.notice_at = (self.notice.clone(), Instant::now());
        } else if !self.notice.is_empty() && self.notice_at.1.elapsed() > Duration::from_secs(6) {
            self.notice.clear();
            self.notice_at.0.clear();
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    /// Statuses to show: the deck's own when it is on this page, else the last t("Probar").
    fn preview(&self) -> HashMap<Sel, String> {
        let page = &self.cfg.pages[self.page].name;
        match &self.live {
            Some(s) if &s.page == page && !s.model.is_empty() => s
                .status
                .iter()
                .filter_map(|(id, text)| {
                    let n: u8 = id.get(1..)?.parse().ok()?;
                    Some((if id.starts_with('k') { Sel::Key(n) } else { Sel::Dial(n) }, text.clone()))
                })
                .collect(),
            _ => self.tested.clone(),
        }
    }

    /// When each plugin key's live image last changed, so previews redraw when a plugin draws.
    fn plugin_images(&self) -> Vec<Option<std::time::SystemTime>> {
        self.cfg.pages[self.page]
            .keys
            .iter()
            .filter(|k| !k.instance.is_empty())
            .map(|k| std::fs::metadata(fluxdeck_core::paths::runtime(&format!("plugin-{}.png", k.instance))).and_then(|m| m.modified()).ok())
            .collect()
    }

    /// Re-renders the previews only when something they show changed.
    fn refresh(&mut self, ctx: &egui::Context) {
        let state = (self.cfg.pages[self.page].clone(), self.theme.clone(), self.cfg.font.clone(), self.preview(), self.plugin_images());
        if self.drawn.as_ref() == Some(&state) {
            return;
        }
        if self.cfg.font != self.font {
            self.painter = Painter::new(&self.cfg.font);
            self.font = self.cfg.font.clone();
        }
        let (cols, rows, dials) = self.layout();
        let (page, theme, _, preview, _) = &state;
        for i in 0..cols * rows {
            let img = match page.key(i) {
                Some(k) if !k.action.is_empty() => match live_plugin_image(&k.instance) {
                    // What the deck shows now, as the plugin drew it.
                    Some(img) => img,
                    None => self.painter.plugin_key(self.painter.image(&k.icon, KEY_PX).as_ref(), &k.label, theme, KEY_PX),
                },
                k => self.painter.key(k, preview.get(&Sel::Key(i)).map(String::as_str), theme, KEY_PX),
            };
            self.textures.insert(Sel::Key(i), ctx.load_texture(format!("key{i}"), to_egui(&img), TextureOptions::LINEAR));
        }
        for i in 0..dials {
            let img = self.painter.dial(page.dial(i), preview.get(&Sel::Dial(i)).map(String::as_str), None, theme, STRIP.0, STRIP.1);
            self.textures.insert(Sel::Dial(i), ctx.load_texture(format!("dial{i}"), to_egui(&img), TextureOptions::LINEAR));
        }
        self.drawn = Some(state);
    }

    fn apply_style(&mut self, ctx: &egui::Context) {
        if self.styled.as_ref() == Some(&self.theme) {
            return;
        }
        let c = |s: &str, d: Color32| render::parse_color(s).map_or(d, |p| Color32::from_rgb(p[0], p[1], p[2]));
        let bg = c(&self.theme.background, Color32::from_rgb(16, 16, 20));
        let key = c(&self.theme.key, Color32::from_rgb(30, 30, 36));
        let text = c(&self.theme.text, Color32::WHITE);
        let accent = c(&self.theme.accent, Color32::LIGHT_BLUE);
        // A light palette needs light widgets: pick the base by how bright the background is.
        let light = (0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32) > 140.0;
        let mut v = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
        v.panel_fill = bg;
        v.window_fill = bg;
        v.extreme_bg_color = if light { mix(bg, Color32::WHITE, 0.7) } else { mix(bg, Color32::BLACK, 0.35) };
        v.faint_bg_color = mix(bg, key, 0.5);
        // Secondary text: as dim as it can be while still reading on a card (WCAG AA 4.5:1, with margin).
        let card = v.faint_bg_color;
        v.weak_text_color = Some((10..=20).map(|i| mix(card, text, i as f32 / 20.0)).find(|c| mic::contrast(*c, card) >= 4.8).unwrap_or(text));
        v.selection.bg_fill = accent;
        // Text on the accent fill (selected tab, highlighted item) uses the surface colour to stay legible.
        v.selection.stroke = Stroke::new(1.0, bg);
        v.hyperlink_color = accent;
        v.window_corner_radius = CornerRadius::same(14);
        for (w, f) in [
            (&mut v.widgets.inactive, key),
            (&mut v.widgets.hovered, mix(key, accent, 0.25)),
            (&mut v.widgets.active, mix(key, accent, 0.45)),
            (&mut v.widgets.open, mix(key, accent, 0.2)),
        ] {
            w.bg_fill = f;
            w.weak_bg_fill = f;
            w.corner_radius = CornerRadius::same(8);
            w.fg_stroke.color = text;
            w.bg_stroke = Stroke::NONE;
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, mix(bg, text, 0.12));
        v.widgets.noninteractive.fg_stroke.color = text;
        ctx.set_visuals(v);
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(8.0, 8.0);
            s.spacing.button_padding = vec2(12.0, 6.0);
            s.spacing.interact_size.y = 28.0;
            s.spacing.slider_width = 240.0;
            s.visuals.slider_trailing_fill = true;
        });
        self.styled = Some(self.theme.clone());
        self.drawn = None;
    }

    fn accent(&self) -> Color32 {
        render::parse_color(&self.theme.accent).map_or(Color32::LIGHT_BLUE, |p| Color32::from_rgb(p[0], p[1], p[2]))
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            // Every page names itself here; the deck shows its page tabs instead.
            if self.view != View::Deck {
                let title = self.nav().into_iter().find(|(v, _, _)| *v == self.view).map(|(_, _, n)| n).unwrap_or_default();
                ui.label(RichText::new(title).size(17.0).strong());
            }
            if self.view == View::Deck {
                self.view_as_menu(ui);
            }
            if self.view == View::Deck && ui.ctx().content_rect().width() < 1100.0 {
                let label = RichText::new(format!("\u{f0570}  {}", t("Acciones"))).size(14.0);
                if ui.selectable_label(self.lib_open, label).on_hover_text(t("Mostrar u ocultar la biblioteca de acciones")).clicked() {
                    self.lib_open = !self.lib_open;
                }
                ui.separator();
            }
            for i in 0..self.cfg.pages.len() {
                if self.view != View::Deck {
                    break;
                }
                let name = self.cfg.pages[i].name.clone();
                let shown = if self.cfg.pages[i].folder { format!("\u{f024b} {name}") } else { name.clone() };
                let resp = ui.selectable_label(self.page == i, RichText::new(shown).size(15.0));
                if resp.clicked() {
                    self.select_page(i);
                }
                if let Some(d) = resp.dnd_release_payload::<Drag>()
                    && let Drag::Slot(sel) = *d
                    && i != self.page
                {
                    self.copy_to_page(sel, i);
                }
                if resp.dnd_hover_payload::<Drag>().is_some() {
                    ui.painter().rect_stroke(resp.rect.expand(2.0), 8, Stroke::new(2.0, self.accent()), StrokeKind::Outside);
                }
            }
            let name = self.new_page.trim().to_string();
            if self.view != View::Deck {
                return self.status_area(ui);
            }
            let entered = ui.add(TextEdit::singleline(&mut self.new_page).hint_text(t("nueva página")).desired_width(120.0)).lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let valid = !name.is_empty() && self.cfg.page_index(&name).is_none();
            if (ui.add_enabled(valid, egui::Button::new("+")).clicked() || entered) && valid {
                self.cfg.pages.push(Page { name, ..Default::default() });
                self.new_page.clear();
                self.select_page(self.cfg.pages.len() - 1);
            }
            self.status_area(ui);
        });
        ui.add_space(8.0);
    }

    fn status_area(&mut self, ui: &mut egui::Ui) {
        if !self.mic.notice.is_empty() {
            self.notice = std::mem::take(&mut self.mic.notice);
        }
        if let Some(m) = self.devices.enable.take() {
            self.cfg.set_module(m, true);
            self.redetect();
        }
        if !self.devices.notice.is_empty() {
            self.notice = std::mem::take(&mut self.devices.notice);
        }
        if !self.store.notice.is_empty() {
            self.notice = std::mem::take(&mut self.store.notice);
        }
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.blocked.is_some() || self.save_failed.is_some() {
                    if ui.button(RichText::new(t("Sin guardar · reintentar")).color(Color32::from_rgb(229, 72, 77))).clicked() {
                        self.save();
                    }
                } else {
                    let (text, tip) = if self.dirty() { (t("Guardando…"), t("Se guarda solo al terminar el cambio")) } else { (t("Guardado ✓"), t("Los cambios se guardan solos")) };
                    ui.label(RichText::new(text).weak().size(12.5)).on_hover_text(tip);
                }
                if ui.button(t("Recargar")).on_hover_text(t("Leer de nuevo el archivo, por si lo editaste a mano")).clicked() {
                    self.reload();
                }
                if ui.add_enabled(!self.future.is_empty(), egui::Button::new(RichText::new("\u{f044f}").size(16.0))).on_hover_text(t("Rehacer (Ctrl+Shift+Z)")).clicked() {
                    self.redo();
                }
                if ui.add_enabled(!self.history.is_empty() || self.cfg != self.committed, egui::Button::new(RichText::new("\u{f054d}").size(16.0))).on_hover_text(t("Deshacer (Ctrl+Z)")).clicked() {
                    self.undo();
                }
                ui.add_space(8.0);
                if !self.service_up {
                    if ui.button(t("Iniciar servicio")).on_hover_text(t("Sin el servicio, el deck, los plugins y los efectos no funcionan")).clicked() {
                        self.start_service();
                    }
                    ui.label(RichText::new(t("Servicio apagado")).color(Color32::from_rgb(229, 72, 77)));
                    ui.label(RichText::new("●").color(Color32::from_rgb(229, 72, 77)));
                } else {
                    let (dot, text) = match &self.live {
                        Some(s) if !s.model.is_empty() => (Color32::from_rgb(80, 200, 120), tf("{} · en «{}»", &[&model_name(&s.model), &s.page])),
                        _ => (Color32::from_gray(120), t("Esperando un Stream Deck").into()),
                    };
                    // In a narrow window the page tabs come first: the status shrinks to its dot.
                    if ui.available_width() > 140.0 {
                        ui.add(egui::Label::new(RichText::new(&text).weak()).truncate()).on_hover_text(&text);
                    }
                    ui.label(RichText::new("●").color(dot)).on_hover_text(&text);
                }
                if ui.available_width() > 120.0 {
                    ui.add(egui::Label::new(RichText::new(&self.notice).weak()).truncate()).on_hover_text(&self.notice);
                }
            });
        }
    }

    /// Copies a key or dial into another page, at the same place if free, else the first free one.
    fn copy_to_page(&mut self, sel: Sel, target: usize) {
        let (cols, rows, dials) = self.layout();
        let src = &self.cfg.pages[self.page];
        let name = self.cfg.pages[target].name.clone();
        match sel {
            Sel::Key(i) => {
                let Some(mut k) = src.key(i).cloned() else { return };
                let dst = &mut self.cfg.pages[target];
                let Some(pos) = std::iter::once(i).chain(0..cols * rows).find(|p| dst.key(*p).is_none()) else {
                    self.notice = tf("«{}» no tiene teclas libres", &[&name]);
                    return;
                };
                k.pos = pos;
                k.instance = instance_for(&k.action);
                dst.keys.push(k);
            }
            Sel::Dial(i) => {
                let Some(mut d) = src.dial(i).cloned() else { return };
                let dst = &mut self.cfg.pages[target];
                let Some(pos) = std::iter::once(i).chain(0..dials).find(|p| dst.dial(*p).is_none()) else {
                    self.notice = tf("«{}» no tiene perillas libres", &[&name]);
                    return;
                };
                d.pos = pos;
                d.instance = instance_for(&d.action);
                dst.dials.push(d);
            }
        }
        self.notice = tf("Copiada a «{}»", &[&name]);
    }

    fn drop_on(&mut self, target: Sel, drag: Drag) {
        let page = &mut self.cfg.pages[self.page];
        match (drag, target) {
            (Drag::Slot(Sel::Key(a)), Sel::Key(b)) if a != b => {
                for k in &mut page.keys {
                    k.pos = if k.pos == a { b } else if k.pos == b { a } else { k.pos };
                }
            }
            (Drag::Slot(Sel::Dial(a)), Sel::Dial(b)) if a != b => {
                for d in &mut page.dials {
                    d.pos = if d.pos == a { b } else if d.pos == b { a } else { d.pos };
                }
            }
            (Drag::Preset(i), _) => self.apply_preset(i, target),
            _ => return,
        }
        self.sel = Some(target);
    }

    fn apply_preset(&mut self, i: usize, target: Sel) {
        // A folder key brings its own page, with a key back to where it was placed.
        if let (Item::Key(k), Sel::Key(pos)) = (&self.presets[i].item, target)
            && k.page == presets::NEW_FOLDER
        {
            let here = self.cfg.pages[self.page].name.clone();
            let base = t("Carpeta").to_string();
            let name = (1..).map(|n| format!("{base} {n}")).find(|n| self.cfg.page_index(n).is_none()).unwrap_or(base);
            let back = Key { pos: 0, label: t("Volver").into(), icon: "\u{f004d}".into(), page: here, ..Default::default() };
            self.cfg.pages.push(Page { name: name.clone(), keys: vec![back], folder: true, ..Default::default() });
            let page = &mut self.cfg.pages[self.page];
            page.keys.retain(|k| k.pos != pos);
            page.keys.push(Key { pos, label: name.clone(), page: name.clone(), ..k.clone() });
            self.sel = Some(target);
            self.notice = tf("Carpeta «{}» creada: ábrela en su pestaña para llenarla", &[&name]);
            return;
        }
        let page = &mut self.cfg.pages[self.page];
        match (&self.presets[i].item, target) {
            (Item::Key(k), Sel::Key(pos)) => {
                page.keys.retain(|k| k.pos != pos);
                page.keys.push(Key { pos, instance: instance_for(&k.action), ..k.clone() });
            }
            (Item::Dial(d), Sel::Dial(pos)) => {
                page.dials.retain(|d| d.pos != pos);
                page.dials.push(Dial { pos, instance: instance_for(&d.action), ..d.clone() });
            }
            (Item::Key(_), _) => {
                self.notice = t("Esa acción es para una tecla").into();
                return;
            }
            (Item::Dial(_), _) => {
                self.notice = t("Esa acción es para una perilla").into();
                return;
            }
        }
        self.tested.remove(&target);
        self.sel = Some(target);
    }

    /// G keys and other extra keys (F13–F24): set the K100's G keys up in OpenLinkHub, and what each key does.
    fn extra_keys_ui(&mut self, ui: &mut egui::Ui, card: Color32) {
        if self.gkeys.1.elapsed() > Duration::from_secs(10) {
            let keys = self.devices.keyboard().map(|k| fluxdeck_core::corsair::g_keys(&k)).unwrap_or_default();
            self.gkeys = (keys, Instant::now());
        }
        let pages: Vec<String> = self.cfg.pages.iter().map(|p| p.name.clone()).collect();
        let apps = self.installed_apps.get_or_insert_with(fluxdeck_core::apps::installed).clone();
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            egui::Frame::new().fill(card).corner_radius(18).inner_margin(20).show(ui, |ui| {
                ui.set_width((ui.available_width() - 40.0).min(760.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new(t("Teclas G y teclas extra")).size(17.0).strong());
                    ui.label(RichText::new(t("Cada tecla corre una secuencia, como una tecla del deck. Sirve con cualquier teclado que tenga F13–F24.")).weak().size(12.5));
                    let gkeys = self.gkeys.0.clone();
                    if let (Some(kb), false) = (self.devices.keyboard(), gkeys.is_empty()) {
                        ui.add_space(8.0);
                        let ready = gkeys.iter().all(|(_, _, default)| !default);
                        ui.horizontal(|ui| {
                            let names: Vec<&str> = gkeys.iter().map(|(n, _, _)| n.as_str()).collect();
                            if !ready && ui.button(tf("Usar {} con FluxDeck", &[&names.join(", ")])).on_hover_text(t("En OpenLinkHub, cada tecla G pasa a mandar F13, F14…")).clicked() {
                                let failed = gkeys.iter().enumerate().filter(|(i, (_, id, _))| fluxdeck_core::corsair::set_g_key(&kb, *id, Some(*i as u16)).is_err()).count();
                                self.notice = if failed == 0 { t("Teclas G listas").into() } else { t("OpenLinkHub no aceptó el cambio").into() };
                                self.gkeys.1 = Instant::now() - Duration::from_secs(60);
                            }
                            if ready && ui.button(t("Devolverlas a su función original")).clicked() {
                                for (_, id, _) in &gkeys {
                                    let _ = fluxdeck_core::corsair::set_g_key(&kb, *id, None);
                                }
                                self.notice = t("Teclas G restauradas").into();
                                self.gkeys.1 = Instant::now() - Duration::from_secs(60);
                            }
                        });
                    }
                    ui.add_space(8.0);
                    // G keys first, by name; then any other extra key that already has steps.
                    let mut rows: Vec<(String, String)> = self.gkeys.0.iter().enumerate().map(|(i, (n, _, _))| (format!("F{}", 13 + i), n.clone())).collect();
                    for k in self.cfg.extra_keys.keys() {
                        if !rows.iter().any(|(f, _)| f == k) {
                            rows.push((k.clone(), k.clone()));
                        }
                    }
                    if rows.is_empty() {
                        rows.push(("F13".into(), "F13".into()));
                    }
                    for (fkey, name) in rows {
                        ui.add_space(6.0);
                        let label = if name == fkey { name.clone() } else { format!("{name}  ({fkey})") };
                        let what = steps_summary(self.cfg.extra_keys.get(&fkey).map_or(&[][..], |s| s.as_slice()), &apps);
                        let label = RichText::new(format!("{label}   ·   {what}")).strong();
                        egui::CollapsingHeader::new(label).id_salt(("xk", &fkey)).show(ui, |ui| {
                            let steps = self.cfg.extra_keys.entry(fkey.clone()).or_default();
                            steps_editor(ui, steps, &pages, &apps);
                        });
                    }
                    // Keys left without steps are not kept in the config.
                    self.cfg.extra_keys.retain(|_, s| !s.is_empty());
                });
            });
        });
    }

    /// Perfiles: a list on the left, the chosen profile on the right.
    /// With no profile yet: what one does, shown with an example, and the two ways to start.
    fn profiles_empty(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let width = (ui.available_width() - 80.0).clamp(280.0, 560.0);
            egui::Frame::new().fill(card).corner_radius(18).inner_margin(28).show(ui, |ui| ui.vertical(|ui| {
                ui.set_width(width);
                ui.label(RichText::new("\u{f0297}").size(40.0).color(accent));
                ui.add_space(6.0);
                ui.label(RichText::new(t("Que todo cambie según la app que uses")).size(19.0).strong());
                ui.add_space(10.0);
                for (step, text) in [
                    ("1", t("Abres Steam o un juego.")),
                    ("2", t("El Chat sube al 100 % y la Música baja al 40 %; si quieres, también el deck cambia de página.")),
                    ("3", t("Cierras el juego y todo vuelve como estaba.")),
                ] {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(step).size(13.0).strong().color(accent));
                        ui.add(egui::Label::new(RichText::new(text).size(13.5)).wrap());
                    });
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let first = egui::Button::new(RichText::new(t("Crear perfil de juegos")).color(mic::ink(accent))).fill(accent).min_size(vec2(0.0, 32.0));
                    if ui.add(first).clicked() {
                        self.cfg.profiles.push(fluxdeck_core::profiles::starter());
                        self.profile_sel = 0;
                    }
                    if ui.add(egui::Button::new(t("Empezar de cero")).min_size(vec2(0.0, 32.0))).clicked() {
                        self.cfg.profiles.push(fluxdeck_core::profiles::Profile { id: "perfil".into(), name: t("Nuevo perfil").into(), ..Default::default() });
                        self.profile_sel = 0;
                    }
                    self.profile_from_window(ui);
                });
            }));
        });
    }

    /// A menu of the open windows; picking one makes a profile that turns on with that app.
    fn profile_from_window(&mut self, ui: &mut egui::Ui) {
        let used: Vec<String> = self.cfg.profiles.iter().flat_map(|p| p.apps.iter().map(|a| a.to_lowercase())).collect();
        let windows: Vec<(String, String)> = self.windows.0.iter().filter(|(c, _)| c != "fluxdeck-editor" && !used.contains(&c.to_lowercase())).cloned().collect();
        let mut pick = None;
        egui::ComboBox::from_id_salt("profile-from-window").selected_text(t("Desde una app abierta")).show_ui(ui, |ui| {
            if windows.is_empty() {
                ui.label(RichText::new(t("No hay otras ventanas abiertas.")).weak());
            }
            for (class, title) in &windows {
                if ui.selectable_label(false, app_name(class)).on_hover_text(format!("{class}\n{title}")).clicked() {
                    pick = Some(class.clone());
                }
            }
        });
        if let Some(class) = pick {
            let ids: Vec<fluxdeck_core::groups::Group> = self.cfg.profiles.iter().map(|p| fluxdeck_core::groups::Group { id: p.id.clone(), ..Default::default() }).collect();
            let name = app_name(&class);
            let id = fluxdeck_core::groups::new_id(&ids, &name);
            self.cfg.profiles.push(fluxdeck_core::profiles::Profile { id, name, apps: vec![class], ..Default::default() });
            self.profile_sel = self.cfg.profiles.len() - 1;
            self.notice = t("Perfil creado: elige qué cambia cuando usas esa app").into();
        }
    }

    fn profiles_page(&mut self, ui: &mut egui::Ui) {
        let card = ui.visuals().faint_bg_color;
        let accent = self.accent();
        if self.windows.1.elapsed() > Duration::from_secs(3) {
            self.windows = (open_windows(), Instant::now());
            self.active_profile = fluxdeck_core::profiles::active();
        }
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(t("Cambian solos cuando usas ciertas apps: la página del deck, el volumen de tus grupos o un comando. Al salir, todo vuelve como estaba.")).weak().size(13.0));
                // The service says which desktop it follows; some cannot tell which window is active.
                match std::fs::read_to_string(fluxdeck_core::paths::runtime("focus")).unwrap_or_default().as_str() {
                    "gnome-missing" => {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(t("GNOME no dice qué ventana está activa: instala la extensión Focused Window D-Bus para que los perfiles cambien solos.")).color(Color32::from_rgb(229, 160, 72)).size(13.0));
                            ui.hyperlink_to(t("Abrir la extensión"), "https://extensions.gnome.org/extension/5592/focused-window-d-bus/");
                        });
                    }
                    "none" => {
                        ui.label(RichText::new(t("Este escritorio no dice qué ventana está activa, así que los perfiles no cambian solos. Funcionan en Hyprland, Sway, i3, niri, KDE Plasma, GNOME (con una extensión) y escritorios X11.")).color(Color32::from_rgb(229, 160, 72)).size(13.0));
                    }
                    _ => {}
                }
            });
        });
        ui.add_space(14.0);
        if self.cfg.profiles.is_empty() {
            self.profiles_empty(ui, card, accent);
            return;
        }
        ui.horizontal_top(|ui| {
            ui.add_space(24.0);
            // The list.
            ui.vertical(|ui| {
                ui.set_width(250.0);
                for i in 0..self.cfg.profiles.len() {
                    let p = &self.cfg.profiles[i];
                    let on = self.profile_sel == i;
                    let live = self.active_profile.as_deref() == Some(p.name.as_str());
                    let fill = if on { mix(ui.visuals().panel_fill, accent, 0.22) } else { card };
                    let r = egui::Frame::new().fill(fill).corner_radius(12).inner_margin(12).show(ui, |ui| {
                        ui.set_width(226.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.icon).size(20.0).color(accent));
                            ui.vertical(|ui| {
                                ui.add(egui::Label::new(RichText::new(&p.name).strong()).truncate());
                                let sub = if live { t("Activo ahora").to_string() } else { tf("{} apps", &[&p.apps.len().to_string()]) };
                                ui.label(RichText::new(sub).size(12.0).color(if live { accent } else { ui.visuals().weak_text_color() }));
                            });
                        });
                    });
                    if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.profile_sel = i;
                    }
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    if ui.button(t("+ Nuevo perfil")).clicked() {
                        let ids: Vec<fluxdeck_core::groups::Group> = self.cfg.profiles.iter().map(|p| fluxdeck_core::groups::Group { id: p.id.clone(), ..Default::default() }).collect();
                        let id = fluxdeck_core::groups::new_id(&ids, "perfil");
                        self.cfg.profiles.push(fluxdeck_core::profiles::Profile { id, name: t("Nuevo perfil").into(), ..Default::default() });
                        self.profile_sel = self.cfg.profiles.len() - 1;
                    }
                    if !self.cfg.profiles.iter().any(|p| p.id == "juegos") && ui.button(t("Perfil de juegos")).on_hover_text(t("Con Steam y otros juegos: Chat al 100 %, Música al 40 %")).clicked() {
                        self.cfg.profiles.push(fluxdeck_core::profiles::starter());
                        self.profile_sel = self.cfg.profiles.len() - 1;
                    }
                });
                self.profile_from_window(ui);
            });
            ui.add_space(16.0);
            // The chosen profile.
            self.profile_sel = self.profile_sel.min(self.cfg.profiles.len() - 1);
            let i = self.profile_sel;
            egui::ScrollArea::vertical().id_salt("profile").show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(620.0);
                    let mut remove = false;
                    egui::Frame::new().fill(card).corner_radius(14).inner_margin(18).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let p = &mut self.cfg.profiles[i];
                            egui::ComboBox::from_id_salt("picon").selected_text(RichText::new(&p.icon).size(18.0).color(accent)).width(44.0).show_ui(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    for icon in GROUP_ICONS.iter().chain(&["\u{f0169}", "\u{f03d8}", "\u{f00d6}"]) {
                                        if ui.selectable_label(p.icon == *icon, RichText::new(*icon).size(20.0)).clicked() {
                                            p.icon = icon.to_string();
                                        }
                                    }
                                });
                            });
                            ui.add(TextEdit::singleline(&mut p.name).desired_width(240.0).font(egui::TextStyle::Heading));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(t("Quitar perfil")).clicked() {
                                    remove = true;
                                }
                            });
                        });
                    });
                    if remove {
                        self.cfg.profiles.remove(i);
                        return;
                    }
                    ui.add_space(10.0);
                    self.profile_apps(ui, card, i);
                    ui.add_space(10.0);
                    self.profile_actions(ui, card, i);
                });
            });
        });
    }

    /// Which windows turn the profile on.
    fn profile_apps(&mut self, ui: &mut egui::Ui, card: Color32, i: usize) {
        egui::Frame::new().fill(card).corner_radius(14).inner_margin(18).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t("Se activa con")).size(15.0).strong());
            ui.label(RichText::new(t("El nombre de la ventana o parte de él: «steam_app» sirve para cualquier juego de Steam.")).weak().size(12.5));
            ui.add_space(6.0);
            let mut drop = None;
            ui.horizontal_wrapped(|ui| {
                for (pos, app) in self.cfg.profiles[i].apps.iter().enumerate() {
                    if ui.button(format!("{app}  ✕")).clicked() {
                        drop = Some(pos);
                    }
                }
                if self.cfg.profiles[i].apps.is_empty() {
                    ui.label(RichText::new(t("Sin apps todavía.")).weak());
                }
            });
            if let Some(pos) = drop {
                self.cfg.profiles[i].apps.remove(pos);
            }
            let mut add = None;
            ui.horizontal(|ui| {
                let key = format!("profile:{}", self.cfg.profiles[i].id);
                let text = self.new_app.entry(key).or_default();
                let r = ui.add(TextEdit::singleline(text).hint_text(t("Nombre de la ventana")).desired_width(180.0));
                let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button(t("Agregar")).clicked() || entered) && !text.trim().is_empty() {
                    add = Some(std::mem::take(text).trim().to_string());
                }
                egui::ComboBox::from_id_salt("pwin").selected_text(t("Ventanas abiertas")).show_ui(ui, |ui| {
                    for (class, title) in &self.windows.0 {
                        if ui.selectable_label(false, class).on_hover_text(title).clicked() {
                            add = Some(class.clone());
                        }
                    }
                });
            });
            if let Some(app) = add
                && !self.cfg.profiles[i].apps.contains(&app)
            {
                self.cfg.profiles[i].apps.push(app);
            }
        });
    }

    /// What the profile changes.
    fn profile_actions(&mut self, ui: &mut egui::Ui, card: Color32, i: usize) {
        let pages: Vec<String> = self.cfg.pages.iter().map(|p| p.name.clone()).collect();
        let groups = self.cfg.groups.clone();
        egui::Frame::new().fill(card).corner_radius(14).inner_margin(18).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(t("Al activarse")).size(15.0).strong());
            ui.add_space(6.0);
            let p = &mut self.cfg.profiles[i];
            ui.horizontal(|ui| {
                ui.label(t("Página del deck"));
                egui::ComboBox::from_id_salt("ppage").selected_text(if p.page.is_empty() { t("Sin cambiar").to_string() } else { p.page.clone() }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut p.page, String::new(), t("Sin cambiar"));
                    for name in &pages {
                        ui.selectable_value(&mut p.page, name.clone(), name);
                    }
                });
            });
            ui.add_space(8.0);
            ui.label(t("Volumen de grupos"));
            if groups.is_empty() {
                ui.label(RichText::new(t("Todavía no hay grupos. Créalos en Audio → Grupos.")).weak().size(12.5));
            }
            for g in &groups {
                ui.horizontal(|ui| {
                    let mut on = p.levels.contains_key(&g.id);
                    if ui.checkbox(&mut on, format!("{}  {}", g.icon, g.name)).changed() {
                        if on {
                            p.levels.insert(g.id.clone(), Default::default());
                        } else {
                            p.levels.remove(&g.id);
                        }
                    }
                    if let Some(level) = p.levels.get_mut(&g.id) {
                        ui.add(egui::Slider::new(&mut level.volume, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                        ui.checkbox(&mut level.muted, t("Silenciado"));
                    }
                });
            }
            ui.add_space(8.0);
            code(ui, t("Comando al activarse"), &mut p.command, "obs --startreplaybuffer");
            ui.add_space(6.0);
            ui.checkbox(&mut p.restore, t("Al salir de la app, volver a la página y volúmenes de antes"));
        });
    }

    /// Audio → Grupos: make groups and say which apps go in each.
    fn groups_tab(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        if self.apps_now.1.elapsed() > Duration::from_secs(3) {
            let mut names: Vec<String> = fluxdeck_core::mixer::snapshot().of(fluxdeck_core::mixer::Kind::Playback).map(|c| c.label.clone()).collect();
            names.dedup();
            self.apps_now = (names, Instant::now());
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.vertical(|ui| {
                    ui.set_max_width(760.0);
                    ui.label(RichText::new(t("Cada grupo tiene su propio volumen y silencio. Las apps que pongas en un grupo lo siguen solas, también las que abras después.")).weak().size(13.0));
                    ui.add_space(10.0);
                    if self.cfg.groups.is_empty() {
                        ui.horizontal(|ui| {
                            if ui.button(t("Crear Juegos, Música, Navegador y Chat")).clicked() {
                                self.cfg.groups = fluxdeck_core::groups::starters();
                            }
                            if ui.button(t("Grupo vacío")).clicked() {
                                let id = fluxdeck_core::groups::new_id(&self.cfg.groups, "grupo");
                                self.cfg.groups.push(fluxdeck_core::groups::Group { id, name: t("Nuevo grupo").into(), icon: "\u{f075a}".into(), apps: vec![] });
                            }
                        });
                        return;
                    }
                    let mut remove = None;
                    for gi in 0..self.cfg.groups.len() {
                        let id = self.cfg.groups[gi].id.clone();
                        egui::Frame::new().fill(card).corner_radius(14).inner_margin(16).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let g = &mut self.cfg.groups[gi];
                                egui::ComboBox::from_id_salt(("gicon", &id)).selected_text(RichText::new(&g.icon).size(18.0).color(accent)).width(44.0).show_ui(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for icon in GROUP_ICONS {
                                            if ui.selectable_label(g.icon == icon, RichText::new(icon).size(20.0)).clicked() {
                                                g.icon = icon.into();
                                            }
                                        }
                                    });
                                });
                                ui.add(TextEdit::singleline(&mut g.name).desired_width(220.0).font(egui::TextStyle::Heading));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if confirm_button(ui, true, t("Quitar grupo"), t("¿Quitar el grupo?"), ("group", &g.id)) {
                                        remove = Some(gi);
                                    }
                                });
                            });
                            ui.add_space(8.0);
                            let mut drop_app = None;
                            ui.horizontal_wrapped(|ui| {
                                for (pos, app) in self.cfg.groups[gi].apps.iter().enumerate() {
                                    if ui.button(format!("{app}  ✕")).on_hover_text(t("Quitar del grupo")).clicked() {
                                        drop_app = Some(pos);
                                    }
                                }
                                if self.cfg.groups[gi].apps.is_empty() {
                                    ui.label(RichText::new(t("Sin apps todavía.")).weak());
                                }
                            });
                            if let Some(pos) = drop_app {
                                self.cfg.groups[gi].apps.remove(pos);
                            }
                            ui.add_space(6.0);
                            let mut add = None;
                            ui.horizontal(|ui| {
                                let text = self.new_app.entry(id.clone()).or_default();
                                let r = ui.add(TextEdit::singleline(text).hint_text(t("Nombre de la app")).desired_width(170.0));
                                let entered = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if (ui.button(t("Agregar")).clicked() || entered) && !text.trim().is_empty() {
                                    add = Some(std::mem::take(text).trim().to_string());
                                }
                                egui::ComboBox::from_id_salt(("gnow", &id)).selected_text(t("+ App que suena")).show_ui(ui, |ui| {
                                    if self.apps_now.0.is_empty() {
                                        ui.label(RichText::new(t("Ninguna app está sonando.")).weak());
                                    }
                                    for name in &self.apps_now.0 {
                                        if ui.selectable_label(false, name).clicked() {
                                            add = Some(name.clone());
                                        }
                                    }
                                });
                            });
                            if let Some(app) = add {
                                // An app belongs to one group: take it out of the others.
                                let low = app.to_lowercase();
                                for g in &mut self.cfg.groups {
                                    g.apps.retain(|a| a.to_lowercase() != low);
                                }
                                self.cfg.groups[gi].apps.push(app);
                            }
                        });
                        ui.add_space(10.0);
                    }
                    if let Some(gi) = remove {
                        self.cfg.groups.remove(gi);
                    }
                    if ui.button(t("+ Nuevo grupo")).clicked() {
                        let id = fluxdeck_core::groups::new_id(&self.cfg.groups, t("Nuevo grupo"));
                        self.cfg.groups.push(fluxdeck_core::groups::Group { id, name: t("Nuevo grupo").into(), icon: "\u{f075a}".into(), apps: vec![] });
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(t("Basta con parte del nombre: «chrom» sirve para Chrome y Chromium.")).weak().size(12.5));
                });
            });
        });
    }

    /// Which installed app an "Abrir una app" key opens: a searchable list.
    fn launch_picker(&mut self, ui: &mut egui::Ui, current: &str) -> Option<fluxdeck_core::apps::App> {
        let apps = self.installed_apps.get_or_insert_with(fluxdeck_core::apps::installed).clone();
        section(ui, t("App"));
        let shown = apps.iter().find(|a| a.id == current).map_or(t("Elige una app").to_string(), |a| a.name.clone());
        ui.label(RichText::new(shown).strong());
        ui.add(TextEdit::singleline(&mut self.app_search).hint_text(t("Buscar apps…")).desired_width(f32::INFINITY));
        let q = self.app_search.trim().to_lowercase();
        let mut out = None;
        egui::ScrollArea::vertical().id_salt("launch_list").max_height(220.0).show(ui, |ui| {
            for a in apps.iter().filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.id.to_lowercase().contains(&q)) {
                if ui.selectable_label(a.id == current, &a.name).on_hover_text(&a.id).clicked() {
                    out = Some(a.clone());
                }
            }
        });
        out
    }

    /// Which group a "Volumen de un grupo" slot controls.
    fn group_picker(&mut self, ui: &mut egui::Ui, current: &str) -> Option<fluxdeck_core::groups::Group> {
        section(ui, t("Grupo"));
        if self.cfg.groups.is_empty() {
            ui.label(RichText::new(t("Todavía no hay grupos. Créalos en Audio → Grupos.")).weak());
            if ui.button(t("Ir a Grupos")).clicked() {
                self.view = View::Mic;
                self.audio_tab = 2;
            }
            return None;
        }
        let shown = self.cfg.groups.iter().find(|g| g.id == current).map_or(t("Elige un grupo").to_string(), |g| format!("{}  {}", g.icon, g.name));
        let mut out = None;
        egui::ComboBox::from_id_salt("group_pick").selected_text(shown).width(220.0).show_ui(ui, |ui| {
            for g in &self.cfg.groups {
                if ui.selectable_label(g.id == current, format!("{}  {}", g.icon, g.name)).clicked() {
                    out = Some(g.clone());
                }
            }
        });
        out
    }

    /// Which app a "Volumen de una app" slot controls: one of those playing, or a typed name.
    fn app_picker(&mut self, ui: &mut egui::Ui, current: &str) -> Option<String> {
        if self.apps_now.1.elapsed() > Duration::from_secs(3) {
            let mut names: Vec<String> = fluxdeck_core::mixer::snapshot().of(fluxdeck_core::mixer::Kind::Playback).map(|c| c.label.clone()).collect();
            names.dedup();
            self.apps_now = (names, Instant::now());
        }
        section(ui, t("App"));
        ui.label(RichText::new(t("Elige la app que controla")).weak().size(12.5));
        let mut out = None;
        let mut text = current.to_string();
        ui.horizontal(|ui| {
            if ui.add(TextEdit::singleline(&mut text).hint_text("Spotify").desired_width(170.0)).on_hover_text(t("Escribe el nombre si no está sonando")).changed() {
                out = Some(text.clone());
            }
            egui::ComboBox::from_id_salt("apps_now").selected_text(t("+ App que suena")).show_ui(ui, |ui| {
                if self.apps_now.0.is_empty() {
                    ui.label(RichText::new(t("Ninguna app está sonando.")).weak());
                }
                for name in &self.apps_now.0 {
                    if ui.selectable_label(name == current, name).clicked() {
                        out = Some(name.clone());
                    }
                }
            })
            .response
            .on_hover_text(t("Las apps que están sonando ahora"));
        });
        out
    }

    /// Inspector for a slot driven by a plugin. Returns true when the user removes it.
    fn plugin_section(&mut self, ui: &mut egui::Ui, action: &str, instance: &str) -> bool {
        let installed = fluxdeck_core::plugins::installed();
        let found = fluxdeck_core::plugins::find_action(&installed, action);
        section(ui, t("Plugin"));
        match found {
            Some((p, a)) => {
                ui.label(RichText::new(&a.name).strong().size(15.0));
                ui.label(RichText::new(tf("de «{}»", &[&p.manifest.name])).weak());
                if !a.tooltip.is_empty() {
                    ui.label(RichText::new(&a.tooltip).size(12.5));
                }
                ui.add_space(8.0);
                let port = std::fs::read_to_string(fluxdeck_core::paths::runtime("plugins.port"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u16>().ok());
                let has_page = p.inspector(a).is_some();
                let saved = !self.dirty();
                let ready = has_page && saved && port.is_some();
                let hint = if !has_page {
                    t("Esta acción no tiene ajustes.")
                } else if !saved {
                    t("Guardando…")
                } else {
                    t("El servicio de FluxDeck no está corriendo.")
                };
                let resp = ui.add_enabled(ready, egui::Button::new(t("Abrir ajustes del plugin"))).on_hover_text(t("Se abren en el navegador; los cambios se guardan solos")).on_disabled_hover_text(hint);
                if resp.clicked()
                    && let Some(port) = port
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!("http://127.0.0.1:{}/pi/{instance}", port + 1)));
                }
                if !ready {
                    ui.label(RichText::new(hint).weak().size(12.5));
                }
            }
            None => {
                ui.label(RichText::new(t("El plugin de esta acción ya no está instalado.")).weak());
            }
        }
        ui.add_space(12.0);
        ui.button(t("Quitar")).clicked()
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.label(RichText::new(t("Acciones")).size(17.0).strong());
        ui.label(RichText::new(t("Arrástralas al deck o haz clic para ponerlas en lo seleccionado.")).weak().size(12.5));
        ui.add(TextEdit::singleline(&mut self.filter).hint_text(t("Buscar…")).desired_width(f32::INFINITY));
        let q = self.filter.trim().to_lowercase();
        let mut groups: Vec<String> = Vec::new();
        for p in &self.presets {
            if !groups.contains(&p.group) {
                groups.push(p.group.clone());
            }
        }
        let mut clicked = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for g in groups {
                let items: Vec<usize> = (0..self.presets.len())
                    .filter(|i| self.presets[*i].group == g && (q.is_empty() || self.presets[*i].name.to_lowercase().contains(&q) || g.to_lowercase().contains(&q)))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                // While searching every group shows; otherwise a click folds it.
                let folded = q.is_empty() && self.folded.contains(&g);
                let arrow = if folded { "▸" } else { "▾" };
                let head = ui.add(egui::Label::new(RichText::new(format!("{arrow}  {}  ({})", g.to_uppercase(), items.len())).size(11.5).weak().strong()).sense(Sense::click()));
                if head.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && q.is_empty() {
                    if !self.folded.remove(&g) {
                        self.folded.insert(g.clone());
                    }
                    let list: Vec<&str> = self.folded.iter().map(String::as_str).collect();
                    let _ = std::fs::create_dir_all(fluxdeck_core::paths::state());
                    let _ = std::fs::write(fluxdeck_core::paths::state().join("library-folded"), list.join("\n"));
                }
                if folded {
                    continue;
                }
                for i in items {
                    let p = &self.presets[i];
                    let (icon, kind) = match &p.item {
                        Item::Key(k) => (k.icon.clone(), "Para una tecla"),
                        Item::Dial(d) => (d.icon.clone(), "Para una perilla"),
                    };
                    let button = if icon.contains('/') {
                        let tex = self
                            .lib_icons
                            .entry(icon.clone())
                            .or_insert_with(|| {
                                fluxdeck_core::plugins::load_image(std::path::Path::new(&icon), 48)
                                    .map(|img| ui.ctx().load_texture(&icon, to_egui(&img), TextureOptions::LINEAR))
                            })
                            .clone();
                        match tex {
                            Some(tex) => egui::Button::image_and_text(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(18.0, 18.0)), RichText::new(&p.name)),
                            None => egui::Button::new(RichText::new(&p.name)),
                        }
                    } else {
                        egui::Button::new(RichText::new(format!("{icon}   {}", p.name)))
                    };
                    let resp = ui.add_sized([ui.available_width(), 30.0], button.sense(Sense::click_and_drag())).on_hover_text(t(kind));
                    if resp.drag_started() {
                        egui::DragAndDrop::set_payload(ui.ctx(), Drag::Preset(i));
                    }
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                }
            }
        });
        if let Some(i) = clicked {
            match self.sel {
                Some(sel) => self.apply_preset(i, sel),
                None => self.notice = t("Primero selecciona una tecla o perilla").into(),
            }
        }
    }

    fn deck(&mut self, ui: &mut egui::Ui) {
        let accent = self.accent();
        // Live values, for the level rings around the knobs.
        let preview = self.preview();
        let (cols, rows, dials) = self.layout();
        let gap = 14.0;
        let margin = 28.0;
        let k = ((ui.available_width() - 2.0 * margin - 40.0 - (cols as f32 - 1.0) * gap) / cols as f32).clamp(44.0, 132.0);
        let width = cols as f32 * k + (cols as f32 - 1.0) * gap;
        let seg = vec2(width / dials.max(1) as f32, width / dials.max(1) as f32 / 2.0);
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            egui::Frame::new().fill(Color32::from_rgb(8, 8, 10)).corner_radius(28).inner_margin(margin).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::splat(gap);
                for r in 0..rows {
                    ui.horizontal(|ui| {
                        for c in 0..cols {
                            if let Some((rect, radius, stroke, _)) = self.slot(ui, Sel::Key(r * cols + c), Vec2::splat(k), 16, accent) {
                                ui.painter().rect_stroke(rect, radius, stroke, StrokeKind::Outside);
                            }
                        }
                    });
                }
                if dials == 0 {
                    return;
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let mut outlines: Vec<_> = (0..dials).filter_map(|i| self.slot(ui, Sel::Dial(i), seg, 4, accent)).collect();
                    // The chosen one last, so no neighbour covers its border.
                    outlines.sort_by_key(|o| o.3);
                    for (rect, radius, stroke, _) in outlines {
                        ui.painter().rect_stroke(rect, radius, stroke, StrokeKind::Outside);
                    }
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for i in 0..dials {
                        let (rect, resp) = ui.allocate_exact_size(vec2(seg.x, 76.0), Sense::click());
                        let on = self.sel == Some(Sel::Dial(i));
                        let hover = ui.ctx().animate_bool(resp.id, resp.hovered() || on);
                        let r = 26.0 + 2.0 * hover;
                        let p = ui.painter();
                        let c = rect.center();
                        p.circle_filled(c, r, Color32::from_rgb(28, 28, 33));
                        p.circle_stroke(c, r, Stroke::new(1.0 + 1.5 * hover, mix(Color32::from_gray(60), accent, hover)));
                        // A ring with the dial's level, like the arc around a real knob.
                        let level = preview.get(&Sel::Dial(i)).and_then(|s| render::parse_status(s).level);
                        let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
                            let n = 48;
                            (0..=n).map(|k| from + (to - from) * k as f32 / n as f32).map(|a| c + vec2(a.cos(), a.sin()) * (r + 7.0)).collect()
                        };
                        let (start, sweep) = (std::f32::consts::PI * 0.75, std::f32::consts::PI * 1.5);
                        if self.cfg.pages[self.page].dial(i).is_some() {
                            p.add(egui::Shape::line(arc(start, start + sweep), Stroke::new(3.0, Color32::from_gray(48))));
                            if let Some(l) = level.filter(|l| *l > 0.0) {
                                p.add(egui::Shape::line(arc(start, start + sweep * l.clamp(0.0, 1.0)), Stroke::new(3.0, accent)));
                            }
                        }
                        let a = start + sweep * level.unwrap_or(0.0).clamp(0.0, 1.0);
                        p.line_segment([c + vec2(a.cos(), a.sin()) * (r - 3.0), c + vec2(a.cos(), a.sin()) * (r - 12.0)], Stroke::new(2.5, Color32::from_gray(200)));
                        if resp.clicked() {
                            self.sel = Some(Sel::Dial(i));
                        }
                    }
                });
            });
            ui.add_space(10.0);
            let help = format!("{}\n{}", t("Arrastra para mover · suelta en una página para copiar · desliza la tira del deck para cambiar de página"), t("Clic derecho para más · Supr quita · Ctrl+C / Ctrl+V copia y pega · Ctrl+Z deshace"));
            ui.label(RichText::new(format!("\u{f02d7}  {}", t("Ayuda"))).weak().size(13.0)).on_hover_text(help);
        });
        if let Some(Drag::Slot(sel)) = egui::DragAndDrop::payload::<Drag>(ui.ctx()).map(|d| *d)
            && let (Some(tex), Some(pos)) = (self.textures.get(&sel), ui.ctx().pointer_interact_pos())
        {
            let size = match sel {
                Sel::Key(_) => Vec2::splat(k * 0.8),
                Sel::Dial(_) => seg * 0.8,
            };
            let painter = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::unique("drag-ghost")));
            let rect = egui::Rect::from_center_size(pos, size);
            painter.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::from_white_alpha(220));
        }
    }

    /// Draws a key or dial place and returns its outline (rect, corner radius, stroke, selected) for
    /// the caller to paint: dial places touch, so their outlines go on top once all are drawn.
    fn slot(&mut self, ui: &mut egui::Ui, sel: Sel, size: Vec2, radius: u8, accent: Color32) -> Option<(egui::Rect, u8, Stroke, bool)> {
        let id = self.textures.get(&sel).map(|t| t.id())?;
        let resp = ui.add(egui::Image::new(egui::load::SizedTexture::new(id, size)).corner_radius(radius).sense(Sense::click_and_drag()));
        let filled = match sel {
            Sel::Key(i) => self.cfg.pages[self.page].key(i).is_some(),
            Sel::Dial(i) => self.cfg.pages[self.page].dial(i).is_some(),
        };
        if resp.drag_started() && filled {
            egui::DragAndDrop::set_payload(ui.ctx(), Drag::Slot(sel));
        }
        let hovering_drop = resp.dnd_hover_payload::<Drag>().is_some();
        if let Some(d) = resp.dnd_release_payload::<Drag>() {
            self.drop_on(sel, *d);
        }
        let hover = ui.ctx().animate_bool(resp.id, resp.hovered() || hovering_drop);
        // Empty places say they take an action.
        if !filled && (resp.hovered() || hovering_drop) {
            let col = ui.visuals().weak_text_color().gamma_multiply(0.6 + 0.4 * hover);
            ui.painter().text(resp.rect.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(size.y.min(size.x) * 0.35), col);
        }
        let stroke = if self.sel == Some(sel) || hovering_drop {
            Stroke::new(2.5, accent)
        } else {
            Stroke::new(1.5 * hover, mix(Color32::from_gray(60), accent, 0.5))
        };
        let outline = (resp.rect.expand(3.0), radius + 3, stroke, self.sel == Some(sel) || hovering_drop);
        if resp.clicked() || resp.secondary_clicked() {
            self.sel = Some(sel);
        }
        let mut act = None;
        let can_paste = matches!((&self.clip, sel), (Some(Item::Key(_)), Sel::Key(_)) | (Some(Item::Dial(_)), Sel::Dial(_)));
        resp.context_menu(|ui| {
            let mut entry = |ui: &mut egui::Ui, enabled: bool, label: &str, keys: &str, a: SlotAct| {
                if ui.add_enabled(enabled, egui::Button::new(label).shortcut_text(keys)).clicked() {
                    act = Some(a);
                    ui.close();
                }
            };
            entry(ui, filled, t("Copiar"), "Ctrl+C", SlotAct::Copy);
            entry(ui, can_paste, t("Pegar"), "Ctrl+V", SlotAct::Paste);
            entry(ui, filled, t("Duplicar"), "Ctrl+D", SlotAct::Duplicate);
            ui.separator();
            entry(ui, filled, t("Quitar"), "Supr", SlotAct::Remove);
        });
        if let Some(a) = act {
            self.slot_act(sel, a);
        }
        Some(outline)
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        match self.sel {
            None => self.page_panel(ui),
            Some(Sel::Key(i)) => self.key_editor(ui, i),
            Some(Sel::Dial(i)) => self.dial_editor(ui, i),
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, title: &str) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(17.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("✕").on_hover_text(t("Cerrar")).clicked() {
                    self.sel = None;
                }
            });
        });
    }

    /// The deck inspector with nothing selected: the current page.
    fn page_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("Página")).size(17.0).strong());
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.rename).desired_width(170.0));
            let new = self.rename.trim().to_string();
            let old = self.cfg.pages[self.page].name.clone();
            let ok = !new.is_empty() && new != old && self.cfg.page_index(&new).is_none();
            if ui.add_enabled(ok, egui::Button::new(t("Renombrar"))).clicked() {
                self.cfg.pages[self.page].name = new.clone();
                for k in self.cfg.pages.iter_mut().flat_map(|p| p.keys.iter_mut()).filter(|k| k.page == old) {
                    k.page = new.clone();
                }
            }
        });
        let only = self.cfg.pages.len() == 1;
        let name = self.cfg.pages[self.page].name.clone();
        if only {
            ui.add_enabled(false, egui::Button::new(t("Borrar página"))).on_disabled_hover_text(t("Tiene que quedar al menos una"));
        } else if confirm_button(ui, true, t("Borrar página"), t("¿Borrar la página?"), ("page", &name)) {
            let gone = self.cfg.pages.remove(self.page).name;
            for k in self.cfg.pages.iter_mut().flat_map(|p| p.keys.iter_mut()).filter(|k| k.page == gone) {
                k.page.clear();
            }
            self.select_page(self.page.saturating_sub(1));
        }
        ui.horizontal(|ui| {
            if ui.button(t("Duplicar página")).clicked() {
                let mut copy = self.cfg.pages[self.page].clone();
                let base = copy.name.clone();
                copy.name = (2..).map(|n| format!("{base} {n}")).find(|n| self.cfg.page_index(n).is_none()).unwrap_or(base);
                for k in &mut copy.keys {
                    k.instance = instance_for(&k.action);
                }
                for d in &mut copy.dials {
                    d.instance = instance_for(&d.action);
                }
                self.cfg.pages.insert(self.page + 1, copy);
                self.select_page(self.page + 1);
            }
            let n = self.cfg.pages.len();
            if ui.add_enabled(self.page > 0, egui::Button::new("←")).on_hover_text(t("Mover a la izquierda")).clicked() {
                self.cfg.pages.swap(self.page, self.page - 1);
                self.page -= 1;
            }
            if ui.add_enabled(self.page + 1 < n, egui::Button::new("→")).on_hover_text(t("Mover a la derecha")).clicked() {
                self.cfg.pages.swap(self.page, self.page + 1);
                self.page += 1;
            }
        });
        ui.checkbox(&mut self.cfg.pages[self.page].folder, t("Es una carpeta: se abre desde su tecla y deslizar la tira la salta"));
        ui.label(RichText::new(t("La primera página es la que se muestra al conectar el deck.")).weak().size(12.5));

        ui.add_space(16.0);
        ui.label(RichText::new(t("Selecciona una tecla o perilla para editarla, o arrastra una acción desde la izquierda.")).weak().size(12.5));
    }

    /// Ajustes: General, Apariencia and Extensiones.
    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let accent = self.accent();
        let card = ui.visuals().faint_bg_color;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.set_max_width(760.0);
                    ui.horizontal(|ui| {
                        for (i, name) in [t("General"), t("Apariencia"), t("Extensiones")].into_iter().enumerate() {
                            if ui.selectable_label(self.settings_tab == i, RichText::new(name).size(15.0)).clicked() {
                                self.settings_tab = i;
                                self.ryoku_widget = None;
                                self.backups = None;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    match self.settings_tab {
                        0 => self.settings_general(ui, card),
                        1 => self.settings_look(ui, card, accent),
                        _ => self.settings_extensions(ui, card, accent),
                    }
                    ui.add_space(20.0);
                });
            });
        });
    }

    fn settings_general(&mut self, ui: &mut egui::Ui, card: Color32) {
        setting(ui, card, t("Arranque"), t("Al encender, solo arranca el servicio: el ícono queda en la bandeja y ninguna ventana se abre"), |ui| {
            use fluxdeck_core::paths::Startup;
            let options = [(Startup::Systemd, t("Servicio del sistema")), (Startup::Desktop, t("Lo arranca mi escritorio")), (Startup::Manual, t("No arrancar solo"))];
            for (mode, name) in options {
                let installed = mode != Startup::Systemd || fluxdeck_core::paths::service_installed();
                let r = ui.add_enabled(installed, egui::Button::selectable(self.startup == mode, name)).on_disabled_hover_text(t("Instálalo con dist/install.sh"));
                if r.clicked() && self.startup != mode {
                    if fluxdeck_core::paths::set_startup(mode) {
                        self.startup = mode;
                    } else {
                        self.notice = t("No se pudo cambiar el arranque automático").into();
                    }
                }
            }
        });
        if self.startup == fluxdeck_core::paths::Startup::Desktop {
            let listed = fluxdeck_core::paths::in_desktop_startup();
            let ryoku = fluxdeck_core::paths::ryoku_desktop();
            let hint = match (listed, ryoku) {
                (true, true) => t("Ya está en la lista de inicio de tu escritorio.").to_string(),
                (true, false) => t("Listo: tu escritorio lo arrancará al iniciar sesión.").to_string(),
                (false, true) => t("Agrégalo en Ryoku Hub → Sistema → Sesión → Al iniciar: pulsa + y escribe «fluxdeck».").to_string(),
                (false, false) => t("Agrega «fluxdeck» a los programas de inicio de tu escritorio.").to_string(),
            };
            ui.label(RichText::new(hint).weak().size(12.5));
            ui.add_space(8.0);
        }
        setting(ui, card, t("Brillo del deck"), t("Se aplica al soltar la barra"), |ui| {
            ui.add(egui::Slider::new(&mut self.cfg.brightness, 0..=100).suffix("%"));
        });
        setting(ui, card, t("Apagar el deck"), t("Tras un rato sin tocarlo. También se apaga al bloquear la sesión o suspender; el primer toque solo lo despierta"), |ui| {
            let options = [(0, t("Nunca")), (5, "5 min"), (10, "10 min"), (30, "30 min"), (60, "1 h")];
            let shown = options.iter().find(|(m, _)| *m == self.cfg.sleep_after).map_or_else(|| format!("{} min", self.cfg.sleep_after), |(_, n)| n.to_string());
            egui::ComboBox::from_id_salt("sleep").selected_text(shown).show_ui(ui, |ui| {
                for (m, name) in options {
                    ui.selectable_value(&mut self.cfg.sleep_after, m, name);
                }
            });
        });
        setting(ui, card, t("Idioma"), t("Cambia al abrir de nuevo FluxDeck"), |ui| {
            let names = [("", t("Del sistema")), ("es", "Español"), ("en", "English")];
            let shown = names.iter().find(|(c, _)| *c == self.cfg.language).map_or(names[0].1, |(_, n)| n);
            egui::ComboBox::from_id_salt("lang").selected_text(shown).show_ui(ui, |ui| {
                for (code, name) in names {
                    if ui.selectable_label(self.cfg.language == code, name).clicked() {
                        self.cfg.language = code.into();
                    }
                }
            });
        });
        setting(ui, card, t("Fuente de las teclas"), t("Una familia instalada; los íconos salen de cualquier Nerd Font"), |ui| {
            let families = self.font_families.get_or_insert_with(font_families);
            let shown = if self.cfg.font.is_empty() { "sans-serif".to_string() } else { self.cfg.font.clone() };
            egui::ComboBox::from_id_salt("key-font").selected_text(shown).width(220.0).height(360.0).show_ui(ui, |ui| {
                ui.add(TextEdit::singleline(&mut self.font_query).hint_text(t("Buscar fuente…")).desired_width(200.0));
                let q = self.font_query.to_lowercase();
                for f in std::iter::once("sans-serif").chain(families.iter().map(String::as_str)).filter(|f| q.is_empty() || f.to_lowercase().contains(&q)) {
                    if ui.selectable_label(self.cfg.font == f, f).clicked() {
                        self.cfg.font = f.to_string();
                    }
                }
            });
        });
        self.backup_ui(ui, card);
        ui.add_space(10.0);
        ui.label(RichText::new(tf("FluxDeck {}", &[env!("CARGO_PKG_VERSION")])).weak().size(12.5));
    }

    /// "View as": edit the deck page in the shape of a phone that has a deck of its own. Shown only
    /// when there is one.
    fn view_as_menu(&mut self, ui: &mut egui::Ui) {
        use fluxdeck_core::remote::{self, Mode, Remote};
        if self.phone_shapes.1.elapsed() > Duration::from_secs(2) {
            let phones = if remote::enabled() { Remote::load().phones } else { Vec::new() };
            self.phone_shapes = (phones.iter().filter(|p| p.mode != Mode::Mirror).map(|p| (p.id.clone(), p.name.clone(), p.shape())).collect(), Instant::now());
        }
        if self.phone_shapes.0.is_empty() {
            self.view_as = None;
            return;
        }
        let shown = self.view_as.as_ref().and_then(|id| self.phone_shapes.0.iter().find(|p| &p.0 == id)).map_or(t("Stream Deck").to_string(), |p| p.1.clone());
        let mut pick = None;
        egui::ComboBox::from_id_salt("view-as").selected_text(RichText::new(format!("\u{f0570}  {shown}")).size(14.0)).show_ui(ui, |ui| {
            if ui.selectable_label(self.view_as.is_none(), t("Stream Deck")).clicked() {
                pick = Some(None);
            }
            for (id, name, (c, r, d)) in &self.phone_shapes.0 {
                let label = if *d > 0 { format!("{name} · {c}×{r} + {d}") } else { format!("{name} · {c}×{r}") };
                if ui.selectable_label(self.view_as.as_ref() == Some(id), label).clicked() {
                    pick = Some(Some(id.clone()));
                }
            }
        })
        .response
        .on_hover_text(t("Ver como: edita las páginas con la forma de un celular o tablet"));
        if let Some(p) = pick {
            self.view_as = p;
            self.sel = None;
            self.drawn = None;
        }
        ui.separator();
    }

    /// The deck on a phone: the switch, and one card per phone or tablet with its mode, its deck
    /// shape and its QR code.
    fn phone_deck_ui(&mut self, ui: &mut egui::Ui, card: Color32) {
        use fluxdeck_core::remote::{self, MODELS, Mode, Phone, Remote};
        ui.add_space(14.0);
        ui.label(RichText::new(t("Deck en el celular")).size(16.0).strong());
        ui.add_space(6.0);
        let mut on = remote::enabled();
        setting(ui, card, t("Usar el celular como Stream Deck"), t("Celulares o tablets en tu misma red WiFi muestran tus teclas y las usan, con o sin Stream Deck conectado"), |ui| {
            if toggle_switch(ui, &mut on) {
                let mut r = Remote::load();
                r.on = on;
                r.save();
            }
        });
        if !on {
            return;
        }
        let mut r = Remote::load();
        let before = r.clone();
        let modes = [(Mode::Mirror, t("Espejo de mi Stream Deck")), (Mode::Extend, t("Teclas extra")), (Mode::Alone, t("No tengo Stream Deck"))];
        let mut remove = None;
        let many = r.phones.len() > 1;
        for i in 0..r.phones.len() {
            let url = r.url(&r.phones[i]);
            let p = &mut r.phones[i];
            egui::Frame::new().fill(card).corner_radius(14).inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add(TextEdit::singleline(&mut p.name).desired_width(220.0).font(egui::TextStyle::Heading));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if many && confirm_button(ui, true, t("Quitar"), t("¿Quitar este celular?"), ("phone-rm", &p.id)) {
                            remove = Some(i);
                        }
                        let open = self.phone_qr.as_deref() == Some(p.id.as_str());
                        if ui.button(if open { t("Ocultar QR") } else { t("Ver QR") }).clicked() {
                            self.phone_qr = if open { None } else { Some(p.id.clone()) };
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(t("Modo"));
                    egui::ComboBox::from_id_salt(("phone-mode", &p.id)).selected_text(modes.iter().find(|m| m.0 == p.mode).map_or("", |m| m.1)).width(210.0).show_ui(ui, |ui| {
                        for (mode, name) in modes {
                            ui.selectable_value(&mut p.mode, mode, name);
                        }
                    });
                    ui.add_space(12.0);
                    ui.label(t("Forma"));
                    let shape = |m: &(&str, &str, u8, u8, u8)| if m.4 > 0 { tf("{} · {}×{} y {} perillas", &[m.1, &m.2.to_string(), &m.3.to_string(), &m.4.to_string()]) } else { tf("{} · {}×{}", &[m.1, &m.2.to_string(), &m.3.to_string()]) };
                    let shown = MODELS.iter().find(|m| m.0 == p.model).map(shape).unwrap_or_default();
                    egui::ComboBox::from_id_salt(("phone-model", &p.id)).selected_text(shown).width(250.0).show_ui(ui, |ui| {
                        for m in &MODELS {
                            if ui.selectable_label(p.model == m.0, shape(m)).clicked() {
                                p.model = m.0.to_string();
                            }
                        }
                    });
                });
                let about = match p.mode {
                    Mode::Mirror => t("Muestra lo mismo que tu Stream Deck y cambia de página con él. Usa la forma del aparato."),
                    Mode::Extend => t("Sus propias páginas, al lado de tu Stream Deck. Edítalas en el Deck con «Ver como»."),
                    Mode::Alone => t("Es tu Stream Deck. Edita sus páginas en el Deck con «Ver como»."),
                };
                ui.label(RichText::new(about).weak().size(12.5));
                if p.mode != Mode::Mirror {
                    ui.label(RichText::new(t("Las teclas de plugins todavía solo funcionan en el Stream Deck y en los espejos.")).weak().size(12.0));
                }
                if self.phone_qr.as_deref() == Some(p.id.as_str()) {
                    ui.add_space(8.0);
                    match &url {
                        Some(url) => {
                            ui.horizontal_top(|ui| {
                                qr_code(ui, url, 190.0);
                                ui.add_space(12.0);
                                ui.vertical(|ui| {
                                    ui.set_max_width(340.0);
                                    ui.label(t("Escanéalo con la cámara del celular. En el navegador, «Añadir a pantalla de inicio» lo deja como una app."));
                                    ui.add(egui::Label::new(RichText::new(url).monospace().size(11.5)).wrap());
                                    ui.horizontal(|ui| {
                                        if ui.button(t("Copiar enlace")).clicked() {
                                            ui.ctx().copy_text(url.clone());
                                            self.notice = t("Enlace copiado").into();
                                        }
                                        if confirm_button(ui, true, t("Cambiar clave"), t("¿Desconectar este celular?"), ("phone-key", &p.id)) {
                                            p.token = remote::new_token();
                                            self.notice = t("Clave nueva: vuelve a escanear el QR").into();
                                        }
                                    });
                                });
                            });
                        }
                        None => drop(ui.label(RichText::new(t("No encontré tu red local: conéctate a una red WiFi o por cable.")).weak().size(12.5))),
                    }
                }
            });
            ui.add_space(8.0);
        }
        if let Some(i) = remove {
            r.phones.remove(i);
        }
        if ui.button(t("+ Agregar celular o tablet")).clicked() {
            let n = r.phones.len() + 1;
            // With a Stream Deck plugged in, a new phone adds keys; without one, it is the deck.
            let mode = if self.live.as_ref().is_some_and(|l| !l.model.is_empty()) { Mode::Extend } else { Mode::Alone };
            let phone = Phone { id: r.new_id(), name: tf("Celular {}", &[&n.to_string()]), mode, ..Default::default() };
            self.phone_qr = Some(phone.id.clone());
            r.phones.push(phone);
        }
        if r != before {
            r.save();
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t("Quien tenga un QR puede usar esas teclas, y las teclas ejecutan comandos. No lo compartas; si lo hiciste, cambia la clave.")).weak().size(12.0));
        if !self.service_up {
            ui.label(RichText::new(t("Necesita el servicio encendido.")).color(Color32::from_rgb(229, 72, 77)).size(12.5));
        }
    }

    /// Export the whole setup to a file, or bring one back (after saving what there is now).
    fn backup_ui(&mut self, ui: &mut egui::Ui, card: Color32) {
        use fluxdeck_core::backup;
        // A file picked in the system's chooser lands here, from its own thread.
        if let Some(pick) = &self.backup_pick {
            let done = pick.lock().unwrap().take();
            if let Some(file) = done {
                self.backup_pick = None;
                if file.is_some() {
                    self.backup_choice = file;
                }
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(300));
            }
        }
        let backups = self.backups.get_or_insert_with(backup::list).clone();
        setting(ui, card, t("Respaldo"), t("Tus páginas, teclas, perillas, grupos, perfiles, efectos e íconos en un archivo. Los plugins no van: se descargan de nuevo."), |ui| {
            if ui.button(t("Exportar")).clicked() {
                match backup::export(self.backup_plugins) {
                    Ok(file) => {
                        self.notice = tf("Respaldo guardado en {}", &[&file.display().to_string()]);
                        self.backups = None;
                    }
                    Err(e) => self.notice = tf("No se pudo exportar: {}", &[&e]),
                }
            }
            if ui.button(t("Abrir carpeta")).clicked() {
                let _ = std::fs::create_dir_all(backup::folder());
                run(&format!("xdg-open '{}'", backup::folder().display().to_string().replace('\'', "")));
            }
        });
        ui.checkbox(&mut self.backup_plugins, t("Incluir los ajustes de los plugins. Pueden tener claves de acceso: no compartas ese archivo."));
        ui.add_space(8.0);
        let shown = self.backup_choice.as_ref().map_or(t("Elige un respaldo").to_string(), |p| p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
        let mut import = false;
        setting(ui, card, t("Importar un respaldo"), t("Reemplaza tu configuración. Antes se guarda una copia de la actual."), |ui| {
            egui::ComboBox::from_id_salt("backup-pick").selected_text(shown).width(230.0).show_ui(ui, |ui| {
                if backups.is_empty() {
                    ui.label(RichText::new(t("No hay respaldos en la carpeta.")).weak());
                }
                for b in &backups {
                    let name = b.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    if ui.selectable_label(self.backup_choice.as_ref() == Some(b), name).clicked() {
                        self.backup_choice = Some(b.clone());
                    }
                }
            });
            if ui.add_enabled(self.backup_pick.is_none(), egui::Button::new(t("Otro archivo…"))).clicked() {
                let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
                self.backup_pick = Some(slot.clone());
                std::thread::spawn(move || {
                    let out = Command::new("zenity").args(["--file-selection", "--file-filter=*.tar.gz"]).output();
                    let file = out.ok().filter(|o| o.status.success()).map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
                    *slot.lock().unwrap() = Some(file);
                });
            }
            import = confirm_button(ui, self.backup_choice.is_some(), t("Importar"), t("¿Reemplazar tu configuración?"), "backup-import");
        });
        if import && let Some(file) = self.backup_choice.clone() {
            match backup::import(&file) {
                Ok(done) => {
                    self.reload();
                    let mut msg = tf("Respaldo importado. Lo que tenías quedó en {}", &[&done.previous.display().to_string()]);
                    if !done.missing_plugins.is_empty() {
                        msg += &tf(" · Vuelve a instalar: {}", &[&done.missing_plugins.join(", ")]);
                    }
                    self.notice = msg;
                }
                Err(e) => self.notice = tf("No se importó: {}", &[&e]),
            }
        }
    }

    fn settings_look(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        if config::file_theme().is_some() {
            setting(ui, card, t("Colores"), t("Los colores vienen de theme.toml:"), |ui| {
                ui.label(RichText::new(config::theme_path().display().to_string()).weak().monospace().size(12.0));
            });
            return;
        }
        let desktop = config::desktop_palette();
        let accent_now = self.cfg.theme.accent.clone();
        let mode = if !self.cfg.palette.is_empty() {
            0
        } else if self.cfg.theme == Theme::dark(&accent_now) {
            1
        } else if self.cfg.theme == Theme::light(&accent_now) {
            2
        } else {
            3
        };
        let mut picked = None;
        setting(ui, card, t("Tema"), t("También colorea las teclas del deck"), |ui| {
            let options = [(0, t("Escritorio")), (1, t("Oscuro")), (2, t("Claro")), (3, t("Personalizado"))];
            for (i, name) in options {
                let enabled = i != 0 || desktop.is_some() || mode == 0;
                if ui.add_enabled(enabled, egui::Button::selectable(mode == i, name)).on_disabled_hover_text(t("No encontré una paleta del escritorio")).clicked() {
                    picked = Some(i);
                }
            }
        });
        match picked {
            Some(0) => self.cfg.palette = desktop.clone().unwrap_or_default(),
            Some(1) => {
                self.cfg.palette.clear();
                self.cfg.theme = Theme::dark(&accent_now);
            }
            Some(2) => {
                self.cfg.palette.clear();
                self.cfg.theme = Theme::light(&accent_now);
            }
            Some(_) => self.cfg.palette.clear(),
            None => {}
        }
        if picked.is_some() {
            self.theme = self.cfg.effective_theme();
        }
        if mode == 0 && picked.is_none() {
            // A palette from the desktop is named after it; only a file of the user's own shows its path.
            let source = if self.cfg.palette.contains("/ryoku/") { t("Colores del escritorio (Ryoku)").to_string() } else { tf("Paleta: {}", &[&self.cfg.palette]) };
            ui.label(RichText::new(source).weak().size(12.5)).on_hover_text(&self.cfg.palette);
            ui.label(RichText::new(t("Cambian solos cuando cambias el fondo de pantalla.")).weak().size(12.5));
        } else if mode != 0 {
            let mut changed = false;
            setting(ui, card, t("Color de acento"), "", |ui| {
                for c in ACCENTS {
                    let col = render::parse_color(c).map_or(Color32::GRAY, |p| Color32::from_rgb(p[0], p[1], p[2]));
                    let on = self.cfg.theme.accent.eq_ignore_ascii_case(c);
                    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                    ui.painter().circle_filled(rect.center(), 11.0, col);
                    if on {
                        ui.painter().circle_stroke(rect.center(), 13.0, Stroke::new(2.0, ui.visuals().text_color()));
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.cfg.theme.accent = c.to_string();
                        changed = true;
                    }
                }
            });
            if mode == 3 {
                setting(ui, card, t("Colores propios"), "", |ui| {
                    let th = &mut self.cfg.theme;
                    for (name, value) in [(t("Fondo"), &mut th.background), (t("Teclas"), &mut th.key), (t("Texto"), &mut th.text), (t("Acento"), &mut th.accent)] {
                        ui.label(name);
                        changed |= color_field(ui, value);
                    }
                });
            }
            if changed {
                self.theme = self.cfg.effective_theme();
            }
        }
        setting(ui, card, t("Tamaño de la interfaz"), "", |ui| {
            for (v, name) in [(0.9, t("Compacta")), (1.0, t("Normal")), (1.15, t("Cómoda"))] {
                if ui.add(egui::Button::selectable(self.cfg.ui_scale == v, name)).clicked() {
                    self.cfg.ui_scale = v;
                }
            }
        });
        let _ = accent;
    }

    fn settings_extensions(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32) {
        ui.label(RichText::new(t("Módulos")).size(16.0).strong());
        ui.add_space(6.0);
        let mut corsair = self.cfg.module("corsair");
        setting(ui, card, t("Periféricos Corsair"), t("Batería, cancelación de ruido y luces, a través de OpenLinkHub"), |ui| {
            if toggle_switch(ui, &mut corsair) {
                self.cfg.set_module("corsair", corsair);
                self.redetect();
            }
        });
        if corsair && !self.devices.running {
            ui.label(RichText::new(t("OpenLinkHub no está corriendo. Instálalo y arráncalo: github.com/jurkovic-nikola/OpenLinkHub")).weak().size(12.5));
        }
        let mut rgb = self.cfg.module("openrgb");
        setting(ui, card, t("Luces de cualquier marca"), t("Efecto, color y brillo de lo que controla OpenRGB: teclados, ratones, placas, RAM y tarjetas gráficas"), |ui| {
            if toggle_switch(ui, &mut rgb) {
                self.cfg.set_module("openrgb", rgb);
                self.redetect();
            }
        });
        if rgb && !fluxdeck_core::openrgb::running() {
            ui.label(RichText::new(t("El servidor de OpenRGB no está corriendo. Ábrelo y pulsa Start Server en la pestaña SDK Server, o arráncalo con openrgb --server.")).weak().size(12.5));
        }
        let mut headsets = self.cfg.module("headset");
        setting(ui, card, t("Auriculares de otras marcas"), t("Batería, retorno de voz y luces de auriculares Logitech, SteelSeries, HyperX y más, a través de HeadsetControl"), |ui| {
            if toggle_switch(ui, &mut headsets) {
                self.cfg.set_module("headset", headsets);
                self.redetect();
            }
        });
        if headsets && !fluxdeck_core::headset::installed() {
            ui.label(RichText::new(t("HeadsetControl no está instalado: github.com/Sapd/HeadsetControl")).weak().size(12.5));
        }
        // Only on a Ryoku desktop; elsewhere there is nothing to integrate with.
        if fluxdeck_core::paths::ryoku_desktop() {
            ui.add_space(14.0);
            ui.label(RichText::new(t("Integraciones")).size(16.0).strong());
            ui.add_space(6.0);
            use fluxdeck_core::ryoku;
            let w = self.ryoku_widget.get_or_insert_with(ryoku::Widget::read).clone();
            let about = match (&w.installed, &w.shipped) {
                (Some(_), _) if w.outdated() => t("Hay una versión nueva del widget"),
                (Some(_), _) => t("El micrófono, tus grupos y la mezcla para stream en la barra. Apagarlo solo lo oculta"),
                (None, Some(_)) => t("El micrófono, tus grupos y la mezcla para stream en la barra"),
                (None, None) => t("Esta copia de FluxDeck no trae el widget; instálalo con dist/install.sh"),
            };
            let mut action: Option<Result<(), String>> = None;
            setting(ui, card, t("Widget en la barra de Ryoku"), about, |ui| {
                if w.installed.is_none() {
                    if w.shipped.is_some() && ui.button(t("Instalar widget")).clicked() {
                        action = Some(ryoku::install(false));
                    }
                    return;
                }
                let mut shown = w.shown.unwrap_or(false);
                if toggle_switch(ui, &mut shown) {
                    action = Some(if ryoku::set_bar_widget(shown) { Ok(()) } else { Err(t("Ryoku no respondió; prueba en QS Bar Settings → Widgets").into()) });
                }
                if w.outdated() && ui.button(t("Actualizar")).clicked() {
                    action = Some(ryoku::install(true));
                }
                if ui.button(t("Quitar")).on_hover_text(t("Lo desinstala de Ryoku; puedes volver a instalarlo aquí")).clicked() {
                    action = Some(ryoku::remove());
                }
            });
            if let Some(result) = action {
                if let Err(e) = result {
                    self.notice = tf("Ryoku: {}", &[&e]);
                }
                self.ryoku_widget = None;
            }
        }
        self.phone_deck_ui(ui, card);
        // Plugins are installed, switched on and off, and removed in one place: the Plugins page.
        ui.add_space(14.0);
        let installed = fluxdeck_core::plugins::installed();
        let off = installed.iter().filter(|p| self.cfg.disabled_plugins.contains(&p.uuid)).count();
        let about = match (installed.len(), off) {
            (0, _) => t("Ningún plugin instalado.").to_string(),
            (n, 0) => tf("{} instalados, todos activos", &[&n.to_string()]),
            (n, off) => tf("{} instalados, {} apagados", &[&n.to_string(), &off.to_string()]),
        };
        setting(ui, card, t("Plugins"), &about, |ui| {
            if ui.button(t("Administrar")).clicked() {
                self.view = View::Plugins;
            }
        });
        let _ = accent;
    }

    fn key_editor(&mut self, ui: &mut egui::Ui, i: u8) {
        self.header(ui, &tf("Tecla {}", &[&(i + 1).to_string()]));
        let pages: Vec<String> = self.cfg.pages.iter().map(|p| p.name.clone()).collect();
        let apps = self.installed_apps.get_or_insert_with(fluxdeck_core::apps::installed).clone();
        let page = &mut self.cfg.pages[self.page];
        let Some(idx) = page.keys.iter().position(|k| k.pos == i) else {
            ui.add_space(6.0);
            ui.label(RichText::new(t("Vacía. Arrastra una acción desde la izquierda, o:")).weak());
            if ui.button(t("Crear tecla en blanco")).clicked() {
                page.keys.push(Key { pos: i, ..Default::default() });
            }
            return;
        };
        if !page.keys[idx].action.is_empty() {
            let (action, instance) = (page.keys[idx].action.clone(), page.keys[idx].instance.clone());
            if self.plugin_section(ui, &action, &instance) {
                self.cfg.pages[self.page].keys.remove(idx);
            }
            return;
        }
        if let Some(id) = presets::launched_app(&page.keys[idx].command)
            && let Some(app) = self.launch_picker(ui, &id)
        {
            let fresh = presets::launch_key(Some(&app));
            let k = &mut self.cfg.pages[self.page].keys[idx];
            (k.label, k.icon, k.command) = (fresh.label, fresh.icon, fresh.command);
        }
        let page = &mut self.cfg.pages[self.page];
        if let Some(app) = presets::app_of(&page.keys[idx].status)
            && let Some(new) = self.app_picker(ui, &app)
        {
            let fresh = presets::app_key(&new);
            let k = &mut self.cfg.pages[self.page].keys[idx];
            (k.label, k.command, k.status) = (fresh.label, fresh.command, fresh.status);
        }
        if let Some(gid) = presets::group_id_of(&self.cfg.pages[self.page].keys[idx].status)
            && let Some(g) = self.group_picker(ui, &gid)
        {
            let fresh = presets::group_key(Some(&g));
            let k = &mut self.cfg.pages[self.page].keys[idx];
            (k.label, k.icon, k.command, k.status) = (fresh.label, fresh.icon, fresh.command, fresh.status);
        }
        let page = &mut self.cfg.pages[self.page];
        let key = &mut page.keys[idx];
        let (mut test, mut remove) = (false, false);

        section(ui, t("Aspecto"));
        ui.label(t("Texto"));
        ui.add(TextEdit::multiline(&mut key.label).desired_rows(2).desired_width(f32::INFINITY).hint_text(t("Hasta tres líneas")));
        if icon_field(ui, &mut key.icon, &self.glyphs, &mut self.icon_query) {
            self.icon_target = Some(Sel::Key(i));
            self.icon_store.open_for(&key.label);
        }
        let mut own = !key.color.is_empty();
        ui.horizontal(|ui| {
            if ui.checkbox(&mut own, t("Color propio")).changed() {
                key.color = if own { "#3a3a48".into() } else { String::new() };
            }
            if own {
                color_field(ui, &mut key.color);
            }
        });

        section(ui, t("Al presionar"));
        code(ui, t("Comando"), &mut key.command, "kitty, hyprctl dispatch …, wtype -M ctrl c …");
        ui.horizontal(|ui| {
            ui.label(t("Abrir página"));
            egui::ComboBox::from_id_salt("goto").selected_text(if key.page.is_empty() { "—" } else { &key.page }).show_ui(ui, |ui| {
                ui.selectable_value(&mut key.page, String::new(), "—");
                for p in &pages {
                    ui.selectable_value(&mut key.page, p.clone(), p);
                }
            });
        });

        section(ui, t("Secuencia"));
        steps_editor(ui, &mut key.steps, &pages, &apps);

        section(ui, t("Estado en vivo"));
        code(ui, t("Comando"), &mut key.status, "date +%H:%M");
        status_help(ui);
        interval(ui, &mut key.interval);

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            test = ui.button(t("Probar")).on_hover_text(t("Ejecuta el comando y el estado ahora")).clicked();
            remove = ui.button(t("Quitar tecla")).clicked();
        });
        let (cmd, status) = (key.command.clone(), key.status.clone());
        if test {
            run(&cmd);
            self.tested.insert(Sel::Key(i), status_output(&status));
        }
        if remove {
            page.keys.remove(idx);
            self.tested.remove(&Sel::Key(i));
        }
    }

    fn dial_editor(&mut self, ui: &mut egui::Ui, i: u8) {
        self.header(ui, &tf("Perilla {}", &[&(i + 1).to_string()]));
        let page = &mut self.cfg.pages[self.page];
        let Some(idx) = page.dials.iter().position(|d| d.pos == i) else {
            ui.add_space(6.0);
            ui.label(RichText::new(t("Vacía. Arrastra una acción desde la izquierda, o:")).weak());
            if ui.button(t("Crear perilla en blanco")).clicked() {
                page.dials.push(Dial { pos: i, ..Default::default() });
            }
            return;
        };
        if !page.dials[idx].action.is_empty() {
            let (action, instance) = (page.dials[idx].action.clone(), page.dials[idx].instance.clone());
            if self.plugin_section(ui, &action, &instance) {
                self.cfg.pages[self.page].dials.remove(idx);
            }
            return;
        }
        if let Some(app) = presets::app_of(&page.dials[idx].status)
            && let Some(new) = self.app_picker(ui, &app)
        {
            let fresh = presets::app_dial(&new);
            let d = &mut self.cfg.pages[self.page].dials[idx];
            (d.label, d.right, d.left, d.press, d.status) = (fresh.label, fresh.right, fresh.left, fresh.press, fresh.status);
        }
        if let Some(gid) = presets::group_id_of(&self.cfg.pages[self.page].dials[idx].status)
            && let Some(g) = self.group_picker(ui, &gid)
        {
            let fresh = presets::group_dial(Some(&g));
            let d = &mut self.cfg.pages[self.page].dials[idx];
            (d.label, d.icon, d.right, d.left, d.press, d.status) = (fresh.label, fresh.icon, fresh.right, fresh.left, fresh.press, fresh.status);
        }
        let page = &mut self.cfg.pages[self.page];
        let dial = &mut page.dials[idx];
        let (mut test, mut remove) = (false, false);

        section(ui, t("Aspecto"));
        ui.label(t("Texto"));
        ui.add(TextEdit::singleline(&mut dial.label).desired_width(f32::INFINITY));
        if icon_field(ui, &mut dial.icon, &self.glyphs, &mut self.icon_query) {
            self.icon_target = Some(Sel::Dial(i));
            self.icon_store.open_for(&dial.label);
        }

        section(ui, t("Acciones"));
        code(ui, t("Girar a la derecha"), &mut dial.right, "");
        code(ui, t("Girar a la izquierda"), &mut dial.left, "");
        code(ui, t("Presionar (o tocar la tira)"), &mut dial.press, "");
        ui.label(RichText::new("$FLUXDECK_STEPS dice cuántos pasos giró la perilla.").weak().size(12.5));

        section(ui, t("Estado en vivo"));
        code(ui, t("Comando"), &mut dial.status, "");
        status_help(ui);
        interval(ui, &mut dial.interval);
        ui.horizontal(|ui| {
            ui.label(t("Medidor"));
            let shown = self.sources.iter().find(|n| n.name == dial.meter).map_or(
                if dial.meter.is_empty() { "—".to_string() } else { dial.meter.clone() },
                |n| if n.description.is_empty() { n.name.clone() } else { n.description.clone() },
            );
            egui::ComboBox::from_id_salt("meter").selected_text(shown).width(220.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut dial.meter, String::new(), "—");
                for n in &self.sources {
                    let label = if n.description.is_empty() { &n.name } else { &n.description };
                    ui.selectable_value(&mut dial.meter, n.name.clone(), label);
                }
            });
        })
        .response
        .on_hover_text(t("Barra con el nivel de entrada en vivo de esa fuente de audio"));

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            test = ui.button(t("Probar estado")).clicked();
            remove = ui.button(t("Quitar perilla")).clicked();
        });
        let status = dial.status.clone();
        if test {
            self.tested.insert(Sel::Dial(i), status_output(&status));
        }
        if remove {
            page.dials.remove(idx);
            self.tested.remove(&Sel::Dial(i));
        }
    }

    /// The icon store, while open: the icon picked goes on the key or dial it was opened for.
    fn icon_store_ui(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.icon_target else { return };
        if let Some(path) = self.icon_store.ui(ctx) {
            let page = &mut self.cfg.pages[self.page];
            match sel {
                Sel::Key(i) => page.keys.iter_mut().filter(|k| k.pos == i).for_each(|k| k.icon = path.clone()),
                Sel::Dial(i) => page.dials.iter_mut().filter(|d| d.pos == i).for_each(|d| d.icon = path.clone()),
            }
            self.notice = t("Ícono puesto").into();
        }
        if !self.icon_store.open {
            self.icon_target = None;
        }
    }

    fn close_guard(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty() && !self.closing && (self.blocked.is_some() || !self.save()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close {
            return;
        }
        egui::Modal::new(Id::unique("close")).show(ctx, |ui| {
            ui.label(RichText::new(t("No se pudieron guardar los cambios")).size(17.0).strong());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let mut close = false;
                if ui.button(t("Reintentar y salir")).clicked() && self.save() {
                    close = true;
                }
                if ui.button(t("Salir sin guardar")).clicked() {
                    close = true;
                }
                if ui.button(t("Cancelar")).clicked() {
                    self.confirm_close = false;
                }
                if close {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S))) && self.dirty() {
            self.save();
        }
        self.poll(&ctx);
        self.shortcuts(&ctx);
        self.track_history(&ctx);
        self.apply_style(&ctx);
        self.icon_store_ui(&ctx);
        self.refresh(&ctx);
        self.sidebar(ui);
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        self.home.set_active(self.view == View::Home, self.cfg.module("corsair"));
        if self.view == View::Home {
            let card = ui.visuals().faint_bg_color;
            let accent = self.accent();
            let live = self.live.clone();
            let groups = self.cfg.groups.clone();
            let mixer = &mut self.mixer;
            let go = egui::CentralPanel::default().show(ui, |ui| self.home.ui(ui, card, accent, self.service_up, live.as_ref(), self.cfg.module("corsair"), |ui| mixer.mini(ui, accent, &groups))).inner;
            match go {
                Some(home::Go::Deck) => self.view = View::Deck,
                Some(home::Go::Audio) => self.view = View::Mic,
                Some(home::Go::Devices) => self.view = if self.view_available(View::Devices) { View::Devices } else { View::Settings },
                Some(home::Go::Plugins) => self.view = View::Plugins,
                Some(home::Go::StartService) => self.start_service(),
                None => {}
            }
            self.close_guard(&ctx);
            return;
        }
        if self.view == View::Profiles {
            egui::CentralPanel::default().show(ui, |ui| self.profiles_page(ui));
            self.close_guard(&ctx);
            return;
        }
        if self.view == View::Settings {
            egui::CentralPanel::default().show(ui, |ui| self.settings_page(ui));
            self.close_guard(&ctx);
            return;
        }
        self.mic.set_active(self.view == View::Mic && self.audio_tab == 1);
        self.mixer.set_active((self.view == View::Mic && self.audio_tab == 0) || self.view == View::Home);
        self.devices.set_active(self.view == View::Devices);
        if std::mem::take(&mut self.store.changed) {
            self.redetect();
        }
        if self.view == View::Plugins {
            let card = ui.visuals().faint_bg_color;
            let accent = self.accent();
            egui::CentralPanel::default().show(ui, |ui| self.store.ui(ui, card, accent, &mut self.cfg.disabled_plugins));
            self.close_guard(&ctx);
            return;
        }
        if self.view == View::Devices {
            let card = ui.visuals().faint_bg_color;
            let accent = self.accent();
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.devices.ui(ui, card, accent);
                    ui.add_space(14.0);
                    self.extra_keys_ui(ui, card);
                    ui.add_space(20.0);
                });
            });
            self.close_guard(&ctx);
            return;
        }
        if self.view == View::Mic {
            let card = ui.visuals().faint_bg_color;
            let accent = self.accent();
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    for (i, name) in [t("Mezclador"), t("Micrófono"), t("Grupos")].into_iter().enumerate() {
                        if ui.selectable_label(self.audio_tab == i, RichText::new(name).size(15.0)).clicked() {
                            self.audio_tab = i;
                        }
                    }
                });
                if self.audio_tab == 0 {
                    match self.mixer.ui(ui, card, accent, &mut self.cfg.hidden_devices, &mut self.cfg.output_names, &self.cfg.groups.clone()) {
                        Some(mixer::Go::Effects) => self.audio_tab = 1,
                        Some(mixer::Go::Group(name, matcher)) => {
                            if !self.cfg.groups.iter().any(|g| g.apps.contains(&matcher)) {
                                let id = fluxdeck_core::groups::new_id(&self.cfg.groups, &name);
                                self.cfg.groups.push(fluxdeck_core::groups::Group { id, name: name.clone(), icon: GROUP_ICONS[9].into(), apps: vec![matcher] });
                                self.notice = tf("Grupo «{}» creado: ya puedes mandarla al stream", &[&name]);
                            }
                        }
                        None => {}
                    }
                } else if self.audio_tab == 1 {
                    self.mic.ui(ui, card, accent);
                } else {
                    self.groups_tab(ui, card, accent);
                }
            });
            self.close_guard(&ctx);
            return;
        }
        // As in Elgato's app: the actions on the right, the deck in the middle and what is selected under it.
        // Narrow windows hide the actions behind their button so the deck keeps room.
        let w = ctx.content_rect().width();
        if w >= 1100.0 || self.lib_open {
            egui::Panel::right("library").resizable(false).exact_size(if w >= 1500.0 { 290.0 } else { 250.0 }).show(ui, |ui| self.library(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("deck_and_inspector").show(ui, |ui| {
                self.deck(ui);
                ui.add_space(14.0);
                let width = (ui.available_width() - 48.0).clamp(320.0, 760.0);
                let card = ui.visuals().faint_bg_color;
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        egui::Frame::new().fill(card).corner_radius(16).inner_margin(egui::Margin { left: 20, right: 20, top: 8, bottom: 18 }).show(ui, |ui| {
                            ui.set_width(width - 40.0);
                            self.inspector(ui);
                        });
                    });
                });
                ui.add_space(24.0);
            });
        });
        self.close_guard(&ctx);
    }
}

fn model_name(kind: &str) -> String {
    match kind {
        "Plus" => "Stream Deck +".into(),
        "PlusXl" => "Stream Deck + XL".into(),
        other => format!("Stream Deck {other}"),
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

const GROUP_ICONS: [&str; 12] = ["\u{f0297}", "\u{f075a}", "\u{f059f}", "\u{f066f}", "\u{f04d3}", "\u{f02cb}", "\u{f036c}", "\u{f05c3}", "\u{f0379}", "\u{f08c6}", "\u{f0381}", "\u{f04c7}"];

const ACCENTS: [&str; 8] = ["#9b8cff", "#7aa2f7", "#4fb3a9", "#6cc070", "#e5a04a", "#e5534b", "#e06c9f", "#c0a36e"];

/// One settings row: title and explanation on the left, the control on the right.
fn setting(ui: &mut egui::Ui, card: Color32, title: &str, about: &str, control: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new().fill(card).corner_radius(14).inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Controls that would not fit beside the text go under it; their width is known from the last frame.
        let id = Id::unique(("setting", title));
        let wide: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
        let text = |ui: &mut egui::Ui| {
            ui.label(RichText::new(title).size(14.5));
            if !about.is_empty() {
                ui.label(RichText::new(about).weak().size(12.5));
            }
        };
        let width = if wide + 340.0 > ui.available_width() {
            text(ui);
            ui.add_space(4.0);
            ui.horizontal(control).response.rect.width()
        } else {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(320.0);
                    text(ui);
                });
                // Right-aligned, but the controls keep their reading order.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.horizontal(control).response.rect.width()).inner
            })
            .inner
        };
        ui.ctx().data_mut(|d| d.insert_temp(id, width));
    });
    ui.add_space(8.0);
}

/// An on/off switch; true when flipped.
/// Font families fontconfig knows, each once, sorted.
fn font_families() -> Vec<String> {
    let out = std::process::Command::new("fc-list").args([":", "family"]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // A line can list several names for one family ("Noto Sans,Noto Sans Regular"): the first is enough.
    let mut all: Vec<String> = out.lines().filter_map(|l| l.split(',').next()).map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect();
    all.sort_by_key(|f| f.to_lowercase());
    all.dedup();
    all
}

/// A QR code for `text`, black on white with a quiet border, `size` points wide.
fn qr_code(ui: &mut egui::Ui, text: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else { return };
    let (n, quiet) = (code.width(), 3);
    let dark = code.to_colors();
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 10, Color32::WHITE);
    let cell = size / (n + 2 * quiet) as f32;
    for (i, c) in dark.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % n + quiet) as f32, (i / n + quiet) as f32);
            // A hair wider than a cell, so neighbours meet without seams.
            p.rect_filled(egui::Rect::from_min_size(rect.min + vec2(x * cell, y * cell), Vec2::splat(cell + 0.4)), 0, Color32::BLACK);
        }
    }
}

/// A button that acts on a second click within four seconds; the first click only asks.
fn confirm_button(ui: &mut egui::Ui, enabled: bool, label: &str, ask: &str, salt: impl std::hash::Hash + std::fmt::Debug) -> bool {
    let id = Id::unique(("confirm", salt));
    let now = ui.input(|i| i.time);
    let armed = ui.ctx().data(|d| d.get_temp::<f64>(id)).is_some_and(|at| now - at < 4.0);
    if armed {
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
    let text = if armed { RichText::new(ask).color(Color32::from_rgb(229, 72, 77)) } else { RichText::new(label) };
    if !ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
        return false;
    }
    if armed {
        ui.ctx().data_mut(|d| d.remove::<f64>(id));
        return true;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, now));
    false
}

pub fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let v = ui.visuals();
    let track = mix(v.extreme_bg_color, v.selection.bg_fill, t);
    ui.painter().rect_filled(rect, 12, track);
    let x = rect.left() + 12.0 + t * (rect.width() - 24.0);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 9.0, if *on { v.selection.stroke.color } else { v.text_color() });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Open windows on Hyprland, one per class.
/// (class, title) of windows used lately, as the service saw them on any desktop.
fn open_windows() -> Vec<(String, String)> {
    let saved = std::fs::read_to_string(fluxdeck_core::paths::runtime("windows")).unwrap_or_default();
    let mut seen: Vec<(String, String)> = saved.lines().filter_map(|l| l.split_once('\t')).map(|(c, t)| (c.to_string(), t.to_string())).collect();
    seen.sort();
    seen
}

/// A readable name from a window class: "dev.zed.Zed" → "Zed", "steam_app_123" → "Steam app 123".
fn app_name(class: &str) -> String {
    let last = class.rsplit('.').next().unwrap_or(class);
    let words: Vec<String> = last
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .enumerate()
        .map(|(i, w)| {
            let mut c = w.chars();
            match (i, c.next()) {
                (0, Some(f)) => f.to_uppercase().chain(c).collect(),
                _ => w.to_string(),
            }
        })
        .collect();
    if words.is_empty() { class.to_string() } else { words.join(" ") }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(16.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.separator();
}

fn code(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(label);
    ui.add(TextEdit::multiline(value).code_editor().desired_rows(1).desired_width(f32::INFINITY).hint_text(hint));
}

fn status_help(ui: &mut egui::Ui) {
    ui.label(RichText::new(t("Imprime texto, o texto|nivel|#color: «42 dB|0.5» dibuja una barra, «Mute||#b3261e» pinta el fondo.")).weak().size(12.5));
}

fn interval(ui: &mut egui::Ui, value: &mut f32) {
    ui.horizontal(|ui| {
        ui.label(t("Cada"));
        let mut v = if *value > 0.0 { *value } else { 2.0 };
        if ui.add(egui::DragValue::new(&mut v).range(0.2..=3600.0).speed(0.1).suffix(" s")).changed() {
            *value = v;
        }
    });
}

fn color_field(ui: &mut egui::Ui, value: &mut String) -> bool {
    let mut c = render::parse_color(value).map_or([128; 3], |p| [p[0], p[1], p[2]]);
    let changed = ui.color_edit_button_srgb(&mut c).changed();
    if changed {
        *value = format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    }
    changed
}

/// The image the service last sent to the deck for a plugin key.
fn live_plugin_image(instance: &str) -> Option<RgbaImage> {
    let img = fluxdeck_core::image::open(fluxdeck_core::paths::runtime(&format!("plugin-{instance}.png"))).ok()?.to_rgba8();
    Some(fluxdeck_core::image::imageops::resize(&img, KEY_PX, KEY_PX, fluxdeck_core::image::imageops::FilterType::Triangle))
}

/// A key's sequence: one row per step, in order, with buttons to add, move and remove.
/// One line saying what a sequence does: its first step and how many more follow.
fn steps_summary(steps: &[fluxdeck_core::Step], apps: &[fluxdeck_core::apps::App]) -> String {
    use fluxdeck_core::Step;
    let Some(first) = steps.first() else { return t("Sin asignar").into() };
    let short = |s: &str| if s.chars().count() > 40 { format!("{}…", s.chars().take(40).collect::<String>()) } else { s.to_string() };
    let what = match first {
        Step::Command { run } => short(run),
        Step::Page { name } => tf("Página «{}»", &[name]),
        Step::App { id } => tf("Abre {}", &[&apps.iter().find(|a| a.id == *id).map_or(id.clone(), |a| a.name.clone())]),
        Step::Wait { ms } => tf("Espera {} ms", &[&ms.to_string()]),
    };
    if steps.len() > 1 { tf("{} y {} pasos más", &[&what, &(steps.len() - 1).to_string()]) } else { what }
}

fn steps_editor(ui: &mut egui::Ui, steps: &mut Vec<fluxdeck_core::Step>, pages: &[String], apps: &[fluxdeck_core::apps::App]) {
    use fluxdeck_core::Step;
    ui.label(RichText::new(t("Pasos que se hacen en orden al presionar, después del comando.")).weak().size(12.5));
    let mut action: Option<(usize, i32)> = None;
    let n = steps.len();
    for (i, step) in steps.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}.", i + 1)).weak());
            match step {
                Step::Command { run } => {
                    ui.label(t("Comando"));
                    ui.add(TextEdit::singleline(run).code_editor().desired_width(170.0));
                }
                Step::Wait { ms } => {
                    ui.label(t("Esperar"));
                    ui.add(egui::DragValue::new(ms).range(0..=60_000).speed(10.0).suffix(" ms"));
                }
                Step::Page { name } => {
                    ui.label(t("Ir a página"));
                    egui::ComboBox::from_id_salt(("step_page", i)).selected_text(if name.is_empty() { "—" } else { name.as_str() }).show_ui(ui, |ui| {
                        for p in pages {
                            ui.selectable_value(name, p.clone(), p);
                        }
                    });
                }
                Step::App { id } => {
                    ui.label(t("Abrir app"));
                    let shown = apps.iter().find(|a| a.id == *id).map_or("—".to_string(), |a| a.name.clone());
                    egui::ComboBox::from_id_salt(("step_app", i)).selected_text(shown).width(150.0).show_ui(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                            for a in apps {
                                ui.selectable_value(id, a.id.clone(), &a.name);
                            }
                        });
                    });
                }
            }
            if ui.small_button("✕").on_hover_text(t("Quitar paso")).clicked() {
                action = Some((i, 0));
            }
            if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).clicked() {
                action = Some((i, 1));
            }
            if ui.add_enabled(i > 0, egui::Button::new("↑").small()).clicked() {
                action = Some((i, -1));
            }
        });
    }
    match action {
        Some((i, 0)) => drop(steps.remove(i)),
        Some((i, d)) => steps.swap(i, (i as i32 + d) as usize),
        None => {}
    }
    ui.horizontal_wrapped(|ui| {
        if ui.small_button(t("+ Comando")).clicked() {
            steps.push(Step::Command { run: String::new() });
        }
        if ui.small_button(t("+ Esperar")).clicked() {
            steps.push(Step::Wait { ms: 500 });
        }
        if ui.small_button(t("+ Página")).clicked() {
            steps.push(Step::Page { name: pages.first().cloned().unwrap_or_default() });
        }
        if ui.small_button(t("+ App")).clicked() {
            steps.push(Step::App { id: String::new() });
        }
    });
}

/// Plugin actions get their own id per placement, so their settings stay apart.
fn instance_for(action: &str) -> String {
    if action.is_empty() { String::new() } else { fluxdeck_core::plugins::new_instance() }
}

/// The icon of a key or dial: a glyph or an image path, a glyph search, and the icon store button.
/// Returns true when the store was asked for.
fn icon_field(ui: &mut egui::Ui, icon: &mut String, glyphs: &[(char, String)], query: &mut String) -> bool {
    let mut store = false;
    ui.horizontal(|ui| {
        ui.label(t("Icono"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            store = ui.button(t("Tienda de íconos…")).on_hover_text(t("Miles de íconos libres: Material, logos de apps y más")).clicked();
        });
    });
    ui.add(TextEdit::singleline(icon).desired_width(f32::INFINITY).hint_text(t("Un glifo, o la ruta a una imagen (PNG o SVG)")));
    if glyphs.is_empty() {
        ui.label(RichText::new(t("Instala una Nerd Font para buscar iconos por nombre.")).weak());
        return store;
    }
    egui::CollapsingHeader::new(t("Buscar icono")).id_salt("icons").show(ui, |ui| {
        ui.add(TextEdit::singleline(query).hint_text(t("volume, mic, github, play…")).desired_width(f32::INFINITY));
        let q = query.trim().to_lowercase();
        if q.len() < 2 {
            return;
        }
        egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (c, name) in glyphs.iter().filter(|(_, n)| n.contains(&q)).take(160) {
                    let b = ui.add(egui::Button::new(RichText::new(c.to_string()).size(22.0)).min_size(Vec2::splat(38.0)));
                    if b.on_hover_text(name).clicked() {
                        *icon = c.to_string();
                    }
                }
            });
        });
    });
    store
}

fn run(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    if let Ok(mut child) = fluxdeck_core::apps::shell(cmd, 1).stdout(Stdio::null()).spawn() {
        std::thread::spawn(move || child.wait());
    }
}

fn status_output(cmd: &str) -> String {
    if cmd.trim().is_empty() {
        return String::new();
    }
    match fluxdeck_core::apps::shell(cmd, 1).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string(),
        Err(e) => format!("error: {e}"),
    }
}

fn to_egui(img: &RgbaImage) -> ColorImage {
    ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Nerd Font icons with their names (md-volume_high, dev-terminal…), read from the font itself.
fn glyph_names() -> Vec<(char, String)> {
    let Some(bytes) = render::nerd_font_file().and_then(|f| std::fs::read(f).ok()) else { return vec![] };
    let Ok(face) = ttf_parser::Face::parse(&bytes, 0) else { return vec![] };
    let mut out = Vec::new();
    for sub in face.tables().cmap.into_iter().flat_map(|c| c.subtables) {
        if !sub.is_unicode() {
            continue;
        }
        sub.codepoints(|cp| {
            let private = (0xe000..=0xf8ff).contains(&cp) || cp >= 0xf0000;
            if let (true, Some(c), Some(id)) = (private, char::from_u32(cp), sub.glyph_index(cp))
                && let Some(name) = face.glyph_name(id)
            {
                out.push((c, name.to_lowercase()));
            }
        });
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup_by_key(|g| g.0);
    out
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, file, first) in [("system", render::font_file("sans-serif"), true), ("nerd", render::nerd_font_file(), false)] {
        let Some(bytes) = file.and_then(|f| std::fs::read(f).ok()) else { continue };
        fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = fonts.families.entry(family.clone()).or_default();
            if first && family == egui::FontFamily::Proportional {
                list.insert(0, name.into());
            } else {
                list.push(name.into());
            }
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    #[test]
    fn window_classes_read_as_app_names() {
        assert_eq!(super::app_name("dev.zed.Zed"), "Zed");
        assert_eq!(super::app_name("discord"), "Discord");
        assert_eq!(super::app_name("steam_app_1245620"), "Steam app 1245620");
        assert_eq!(super::app_name("com.obsproject.Studio"), "Studio");
    }

    /// Every string marked for translation has an English entry.
    #[test]
    fn all_marked_strings_are_translated() {
        let sources = [include_str!("main.rs"), include_str!("mic.rs"), include_str!("devices.rs"), include_str!("store.rs"), include_str!("effects.rs"), include_str!("home.rs"), include_str!("mixer.rs"), include_str!("icons.rs"), include_str!("../../core/src/presets.rs")];
        let mut missing = Vec::new();
        for src in sources {
            for marker in ["t(\"", "tf(\""] {
                for (i, _) in src.match_indices(marker) {
                    if src[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let rest = &src[i + marker.len()..];
                    let lit = &rest[..rest.find('"').unwrap()];
                    if !fluxdeck_core::i18n::has(lit) {
                        missing.push(lit.to_string());
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing translations: {missing:?}");
    }
}
