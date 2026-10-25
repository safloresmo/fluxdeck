//! Plugin store view over fluxdeck_core::catalogue.
use crate::mic::card_frame;
use fluxdeck_core::catalogue::{Entry, Source, cache, fetch, install, load};
use fluxdeck_core::i18n::{t, tf};
use fluxdeck_core::plugins::{self, Plugin};
use eframe::egui::{self, Color32, RichText, TextureHandle, TextureOptions, vec2};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
enum Job {
    Working,
    Failed(String),
}

pub struct Store {
    source: Source,
    entries: Arc<Mutex<HashMap<u8, Result<Vec<Entry>, String>>>>,
    asked: Vec<Source>,
    query: String,
    installed: Vec<Plugin>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    icons: Arc<Mutex<HashMap<String, Option<fluxdeck_core::image::RgbaImage>>>>,
    textures: HashMap<String, TextureHandle>,
    icon_queue: Sender<(String, String)>,
    requested: std::collections::HashSet<String>,
    confirm_remove: Option<String>,
    linux_only: bool,
    pub changed: bool,
    pub notice: String,
}

impl Store {
    pub fn new() -> Store {
        let icons: Arc<Mutex<HashMap<String, Option<fluxdeck_core::image::RgbaImage>>>> = Arc::default();
        let (icon_queue, rx) = channel::<(String, String)>();
        {
            let icons = icons.clone();
            std::thread::spawn(move || {
                let dir = cache().join("icons");
                let _ = std::fs::create_dir_all(&dir);
                while let Ok((key, url)) = rx.recv() {
                    let file = dir.join(format!("{}.img", key.replace(['/', ':'], "_")));
                    if !file.is_file() && !url.is_empty() {
                        fetch(&url, &file);
                    }
                    let img = plugins::load_image(&file, 96);
                    icons.lock().unwrap().insert(key, img);
                }
            });
        }
        Store {
            source: Source::OpenAction,
            entries: Arc::default(),
            asked: Vec::new(),
            query: String::new(),
            installed: plugins::installed(),
            jobs: Arc::default(),
            icons,
            textures: HashMap::new(),
            icon_queue,
            requested: Default::default(),
            confirm_remove: None,
            linux_only: false,
            changed: false,
            notice: String::new(),
        }
    }

    fn catalogue(&mut self) -> Option<Result<Vec<Entry>, String>> {
        if !self.asked.contains(&self.source) {
            self.asked.push(self.source);
            let (entries, source) = (self.entries.clone(), self.source);
            std::thread::spawn(move || {
                let r = load(source);
                entries.lock().unwrap().insert(source as u8, r);
            });
        }
        self.entries.lock().unwrap().get(&(self.source as u8)).cloned()
    }

    fn icon(&mut self, ctx: &egui::Context, key: &str, url: &str) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(key) {
            return Some(t.clone());
        }
        if self.requested.insert(key.to_string()) {
            let _ = self.icon_queue.send((key.to_string(), url.to_string()));
            return None;
        }
        let img = self.icons.lock().unwrap().get(key).cloned().flatten()?;
        let tex = ctx.load_texture(key, egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()), TextureOptions::LINEAR);
        self.textures.insert(key.to_string(), tex.clone());
        Some(tex)
    }

    fn start_install(&mut self, e: Entry) {
        self.jobs.lock().unwrap().insert(e.id.clone(), Job::Working);
        let jobs = self.jobs.clone();
        std::thread::spawn(move || {
            let r = install(&e);
            let mut jobs = jobs.lock().unwrap();
            match r {
                Ok(_) => jobs.remove(&e.id),
                Err(err) => jobs.insert(e.id.clone(), Job::Failed(err)),
            };
        });
    }

    fn is_installed(&self, id: &str) -> Option<&Plugin> {
        self.installed.iter().find(|p| p.uuid == id || p.uuid == id.trim_end_matches(".sdPlugin"))
    }

    /// `disabled` is the config's list of plugins switched off, by uuid.
    pub fn ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32, disabled: &mut Vec<String>) {
        // Pick up finished installs.
        let working = self.jobs.lock().unwrap().values().any(|j| matches!(j, Job::Working));
        if working {
            ui.ctx().request_repaint_after(Duration::from_millis(300));
        }
        let now = plugins::installed();
        if now.len() != self.installed.len() {
            self.changed = true;
            self.installed = now;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(700));

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.selectable_value(&mut self.source, Source::OpenAction, RichText::new(t("Plugins libres")).size(15.0)).on_hover_text(t("Catálogo de OpenAction: código abierto, muchos hechos para Linux"));
            ui.selectable_value(&mut self.source, Source::Elgato, RichText::new(t("Tienda de Elgato")).size(15.0)).on_hover_text(t("Archivo de la tienda de Elgato: casi todos son para Windows y necesitan Wine"));
            ui.add_space(12.0);
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text(t("Buscar plugins…")).desired_width(280.0));
            ui.checkbox(&mut self.linux_only, t("Solo hechos para Linux"));
        });
        ui.add_space(10.0);

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            if !self.installed.is_empty() {
                self.installed_ui(ui, card, accent, disabled);
                ui.add_space(14.0);
            }
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new(t("Catálogo")).size(17.0).strong());
            });
            if self.source == Source::Elgato {
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.label(RichText::new(t("Casi todos estos plugins son para Windows: necesitan Wine y algunos no funcionan.")).weak().size(12.5));
                });
            }
            ui.add_space(6.0);
            match self.catalogue() {
                None => {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.spinner();
                        ui.label(t("Cargando catálogo…"));
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
                Some(Err(e)) => {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        ui.label(RichText::new(e).weak());
                    });
                }
                Some(Ok(list)) => self.grid(ui, &list, card, accent),
            }
        });
    }

    fn installed_ui(&mut self, ui: &mut egui::Ui, card: Color32, accent: Color32, disabled: &mut Vec<String>) {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.label(RichText::new(t("Instalados")).size(17.0).strong());
        });
        ui.add_space(6.0);
        let installed = self.installed.clone();
        rows(ui, installed.len(), vec2(328.0, 72.0), |ui, i| {
            let p = &installed[i];
            card_frame(card).inner_margin(14).show(ui, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    let icon = plugins::image_file(&p.path, &p.manifest.icon).map(|f| f.display().to_string()).unwrap_or_default();
                    if let Some(tex) = self.local_icon(ui.ctx(), &icon) {
                        ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(36.0, 36.0)).corner_radius(8));
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(&p.manifest.name).strong()).truncate());
                        let (text, color) = match p.missing() {
                            None => (tf("{} acciones · listo", &[&p.manifest.actions.len().to_string()]), accent),
                            Some("node") => (t("Necesita Node.js").to_string(), Color32::from_rgb(229, 160, 72)),
                            Some("wine") => (t("Necesita Wine").to_string(), Color32::from_rgb(229, 160, 72)),
                            _ => (t("No funciona en este sistema").to_string(), Color32::from_rgb(229, 72, 77)),
                        };
                        ui.label(RichText::new(text).color(color).size(12.5));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut on = !disabled.contains(&p.uuid);
                        if crate::toggle_switch(ui, &mut on) {
                            disabled.retain(|u| *u != p.uuid);
                            if !on {
                                disabled.push(p.uuid.clone());
                            }
                        }
                        let confirming = self.confirm_remove.as_deref() == Some(&p.uuid);
                        let label = if confirming { t("¿Quitar?") } else { t("Quitar") };
                        if ui.button(label).clicked() {
                            if confirming {
                                let _ = std::fs::remove_dir_all(&p.path);
                                self.confirm_remove = None;
                                self.notice = tf("«{}» quitado", &[&p.manifest.name]);
                            } else {
                                self.confirm_remove = Some(p.uuid.clone());
                            }
                        }
                    });
                });
            });
        });
    }

    fn local_icon(&mut self, ctx: &egui::Context, path: &str) -> Option<TextureHandle> {
        if path.is_empty() {
            return None;
        }
        if let Some(t) = self.textures.get(path) {
            return Some(t.clone());
        }
        let img = plugins::load_image(Path::new(path), 96)?;
        let tex = ctx.load_texture(path, egui::ColorImage::from_rgba_unmultiplied([96, 96], img.as_raw()), TextureOptions::LINEAR);
        self.textures.insert(path.to_string(), tex.clone());
        Some(tex)
    }

    fn grid(&mut self, ui: &mut egui::Ui, list: &[Entry], card: Color32, accent: Color32) {
        let q = self.query.trim().to_lowercase();
        let shown: Vec<&Entry> = list
            .iter()
            .filter(|e| !self.linux_only || e.made_for_linux())
            .filter(|e| q.is_empty() || e.name.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q) || e.author.to_lowercase().contains(&q))
            .collect();
        if shown.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(20.0);
                ui.label(RichText::new(t("Nada coincide con la búsqueda.")).weak());
            });
            return;
        }
        let jobs = self.jobs.lock().unwrap().clone();
        let mut install = None;
        rows(ui, shown.len(), vec2(328.0, 146.0), |ui, i| {
            let e = shown[i];
            card_frame(card).inner_margin(14).show(ui, |ui| {
                ui.set_width(300.0);
                ui.set_height(118.0);
                ui.horizontal(|ui| {
                    // Icons are fetched only for cards that get drawn.
                    let key = format!("{}:{}", self.source as u8, e.id);
                    match self.icon(ui.ctx(), &key, &e.icon) {
                        Some(tex) => drop(ui.add(egui::Image::new(&tex).fit_to_exact_size(vec2(44.0, 44.0)).corner_radius(10))),
                        None => drop(ui.add_sized([44.0, 44.0], egui::Label::new(RichText::new("\u{f0431}").size(28.0).weak()))),
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(RichText::new(&e.name).strong().size(14.5)).truncate());
                        ui.add(egui::Label::new(RichText::new(&e.author).weak().size(12.0)).truncate());
                    });
                });
                ui.add_space(4.0);
                if e.made_for_linux() {
                    ui.label(RichText::new(t("Hecho para Linux")).color(accent).size(11.5));
                }
                let desc = if e.description.is_empty() { "—" } else { &e.description };
                ui.add(egui::Label::new(RichText::new(desc).size(12.5)).truncate()).on_hover_text(desc);
                ui.add_space(6.0);
                ui.horizontal(|ui| match (self.is_installed(&e.id).is_some(), jobs.get(&e.id)) {
                    (_, Some(Job::Working)) => {
                        ui.spinner();
                        ui.label(t("Instalando…"));
                    }
                    (true, _) => drop(ui.label(RichText::new(t("Instalado")).color(accent))),
                    (false, failed) => {
                        if ui.button(t("Instalar")).clicked() {
                            install = Some(e.clone());
                        }
                        if !e.repository.is_empty() && ui.small_button("↗").on_hover_text(&e.repository).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(&e.repository));
                        }
                        if let Some(Job::Failed(err)) = failed {
                            ui.add(egui::Label::new(RichText::new(err).color(Color32::from_rgb(229, 72, 77)).size(12.0)).truncate()).on_hover_text(err);
                        }
                    }
                });
            });
        });
        if let Some(e) = install {
            self.start_install(e);
        }
    }
}

/// Lays out `n` cells of a fixed size in rows that fill the width.
fn rows(ui: &mut egui::Ui, n: usize, size: egui::Vec2, mut cell: impl FnMut(&mut egui::Ui, usize)) {
    let per_row = ((ui.available_width() - 40.0) / (size.x + 10.0)).floor().max(1.0) as usize;
    for start in (0..n).step_by(per_row) {
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            for i in start..(start + per_row).min(n) {
                ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    cell(ui, i);
                });
            }
        });
    }
}
