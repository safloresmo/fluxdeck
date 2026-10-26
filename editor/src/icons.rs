//! The icon store window: search Iconify's open icon packs, browse one pack, preview, and pick an
//! icon for the key or dial being edited. Searches run on a thread; the window never waits.
use eframe::egui::{self, Color32, RichText, TextEdit, TextureHandle, TextureOptions, vec2};
use fluxdeck_core::image::RgbaImage;
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::iconify::{self, Pack};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Found {
    /// The (search, pack) these results are for.
    asked: (String, String),
    /// Name, SVG, and its preview already drawn in the window's text colour.
    icons: Vec<(String, String, Option<RgbaImage>)>,
    loading: bool,
}

pub struct IconStore {
    pub open: bool,
    query: String,
    prefix: String,
    pack_query: String,
    packs: Arc<Mutex<Option<Vec<Pack>>>>,
    /// Sample icons of the front page's packs, fetched with the pack list.
    samples: Arc<Mutex<Vec<(String, String, Option<RgbaImage>)>>>,
    /// The window's text colour, which single-colour previews are drawn in.
    ink: Color32,
    found: Arc<Mutex<Found>>,
    textures: HashMap<String, TextureHandle>,
    edited: Instant,
}

impl IconStore {
    pub fn new() -> IconStore {
        IconStore {
            open: false,
            query: String::new(),
            prefix: String::new(),
            pack_query: String::new(),
            packs: Arc::new(Mutex::new(None)),
            samples: Arc::new(Mutex::new(Vec::new())),
            ink: Color32::WHITE,
            found: Arc::new(Mutex::new(Found::default())),
            textures: HashMap::new(),
            edited: Instant::now(),
        }
    }

    /// Opens the store already searching for icons that fit a key or dial named `label`.
    pub fn open_for(&mut self, label: &str) {
        self.open = true;
        let q = iconify::query_for(label);
        if !q.is_empty() {
            self.query = q;
            self.prefix.clear();
            self.edited = Instant::now() - Duration::from_secs(1);
        }
    }

    /// Starts a search once typing pauses; results for an older search are dropped when they land.
    fn request(&mut self) {
        let want = (self.query.trim().to_lowercase(), self.prefix.clone());
        if self.found.lock().unwrap().asked == want || self.edited.elapsed() < Duration::from_millis(250) {
            return;
        }
        let mut f = self.found.lock().unwrap();
        f.asked = want.clone();
        f.icons.clear();
        f.loading = want.0.chars().count() >= 2 || !want.1.is_empty();
        if !f.loading {
            return;
        }
        let (found, ink) = (self.found.clone(), self.ink);
        std::thread::spawn(move || {
            let names = if want.0.chars().count() >= 2 { iconify::search(&want.0, &want.1) } else { iconify::pack_icons(&want.1) };
            let icons = previews(iconify::svgs(&names), ink);
            let mut f = found.lock().unwrap();
            if f.asked == want {
                f.icons = icons;
                f.loading = false;
            }
        });
    }

    fn texture(&mut self, ctx: &egui::Context, full: &str, img: Option<&RgbaImage>) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(full) {
            return Some(t.clone());
        }
        let img = img?;
        let tex = ctx.load_texture(full, egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()), TextureOptions::LINEAR);
        if self.textures.len() > 800 {
            self.textures.clear();
        }
        self.textures.insert(full.to_string(), tex.clone());
        Some(tex)
    }

    /// With nothing searched: the featured packs as cards with a few of their icons; a click opens one.
    fn front_page(&mut self, ui: &mut egui::Ui, packs: &[Pack]) {
        let featured = iconify::featured(packs);
        if featured.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t("Cargando packs…"));
            });
            return;
        }
        ui.label(RichText::new(t("Packs destacados")).strong());
        ui.label(RichText::new(t("Elige uno para verlo, o busca arriba en todos.")).weak().size(12.5));
        ui.add_space(6.0);
        let samples = self.samples.lock().unwrap().clone();
        let card = ui.visuals().faint_bg_color;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for p in &featured {
                let r = ui.allocate_ui(vec2(196.0, 92.0), |ui| {
                    egui::Frame::new().fill(card).corner_radius(12).inner_margin(10).show(ui, |ui| {
                        ui.set_width(176.0);
                        ui.set_height(72.0);
                        ui.add(egui::Label::new(RichText::new(&p.name).strong()).truncate());
                        ui.add(egui::Label::new(RichText::new(tf("{} íconos · {}", &[&p.total.to_string(), &p.license])).weak().size(11.5)).truncate());
                        ui.horizontal(|ui| {
                            for (full, _, img) in samples.iter().filter(|(f, _, _)| f.split(':').next() == Some(p.prefix.as_str())) {
                                if let Some(tex) = self.texture(ui.ctx(), full, img.as_ref()) {
                                    ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(24.0, 24.0)));
                                }
                            }
                        });
                    })
                });
                let r = r.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(&p.category);
                if r.clicked() {
                    self.prefix = p.prefix.clone();
                    self.edited = Instant::now() - Duration::from_secs(1);
                }
            }
        });
    }

    /// Draws the window while it is open; returns the saved file of the icon picked, if one was.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<String> {
        if !self.open {
            return None;
        }
        if self.packs.lock().unwrap().is_none() {
            *self.packs.lock().unwrap() = Some(Vec::new());
            let (packs, samples, ink) = (self.packs.clone(), self.samples.clone(), self.ink);
            std::thread::spawn(move || {
                let all = iconify::packs();
                let names: Vec<String> = iconify::featured(&all).iter().flat_map(|p| p.samples.iter().take(5).map(|s| format!("{}:{s}", p.prefix))).collect();
                *packs.lock().unwrap() = Some(all);
                *samples.lock().unwrap() = previews(iconify::svgs(&names), ink);
            });
        }
        self.request();
        let packs = self.packs.lock().unwrap().clone().unwrap_or_default();
        let (icons, loading, asked) = {
            let f = self.found.lock().unwrap();
            (f.icons.clone(), f.loading, f.asked.clone())
        };
        if loading || self.edited.elapsed() < Duration::from_millis(500) {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        self.ink = ctx.global_style().visuals.text_color();
        let mut picked = None;
        let mut open = self.open;
        egui::Window::new(t("Tienda de íconos")).open(&mut open).default_size([660.0, 540.0]).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.add(TextEdit::singleline(&mut self.query).hint_text(t("Buscar en inglés: microphone, discord, play…")).desired_width(300.0)).changed() {
                    self.edited = Instant::now();
                }
                let shown = packs.iter().find(|p| p.prefix == self.prefix).map_or(t("Todos los packs").to_string(), |p| p.name.clone());
                egui::ComboBox::from_id_salt("icon-pack").selected_text(shown).width(230.0).height(360.0).show_ui(ui, |ui| {
                    ui.add(TextEdit::singleline(&mut self.pack_query).hint_text(t("Buscar pack…")).desired_width(210.0));
                    if ui.selectable_label(self.prefix.is_empty(), t("Todos los packs")).clicked() {
                        self.prefix.clear();
                        self.edited = Instant::now();
                    }
                    let q = self.pack_query.to_lowercase();
                    for p in packs.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.category.to_lowercase().contains(&q)) {
                        let label = format!("{} · {}", p.name, p.total);
                        if ui.selectable_label(self.prefix == p.prefix, label).on_hover_text(format!("{} · {}", p.category, p.license)).clicked() {
                            self.prefix = p.prefix.clone();
                            self.edited = Instant::now();
                        }
                    }
                });
            });
            if let Some(p) = packs.iter().find(|p| p.prefix == self.prefix) {
                let colours = if p.coloured { t("a color") } else { t("toman el color del tema") };
                ui.label(RichText::new(tf("{} íconos · licencia {} · {}", &[&p.total.to_string(), &p.license, colours])).weak().size(12.5));
            }
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, false]).show(ui, |ui| {
                if loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(t("Buscando…"));
                    });
                } else if asked.0.chars().count() < 2 && asked.1.is_empty() {
                    self.front_page(ui, &packs);
                } else if icons.is_empty() {
                    ui.label(RichText::new(t("Nada encontrado. Prueba otra palabra en inglés, o revisa la conexión.")).weak());
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for (full, svg, img) in &icons {
                        let Some(tex) = self.texture(ui.ctx(), full, img.as_ref()) else { continue };
                        let pack = full.split(':').next().and_then(|pre| packs.iter().find(|p| p.prefix == pre));
                        let tip = pack.map_or(full.to_string(), |p| format!("{full}\n{} · {}", p.name, p.license));
                        let b = ui.add(egui::Button::image(egui::Image::new(&tex).fit_to_exact_size(vec2(34.0, 34.0))).min_size(vec2(46.0, 46.0)));
                        if b.on_hover_text(tip).clicked() {
                            picked = iconify::save(full, svg).ok().map(|p| p.display().to_string());
                        }
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(RichText::new(t("Íconos de Iconify, de código abierto. Cada pack tiene su licencia: la ves al pasar el ratón. El ícono elegido se guarda en tu equipo.")).weak().size(12.0));
        });
        self.open = open && picked.is_none();
        picked
    }
}

/// Draws each SVG small, single-colour ones in `ink`, so the window only uploads finished images.
fn previews(svgs: Vec<(String, String)>, ink: Color32) -> Vec<(String, String, Option<RgbaImage>)> {
    let hex = format!("#{:02x}{:02x}{:02x}", ink.r(), ink.g(), ink.b());
    svgs.into_iter()
        .map(|(full, svg)| {
            let img = fluxdeck_core::plugins::data_url(&svg.replace("currentColor", &hex), 64);
            (full, svg, img)
        })
        .collect()
}
