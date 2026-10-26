mod backupcmd;
mod corsaircmd;
mod effectscmd;
mod effectsrun;
mod extrakeys;
mod groupsrun;
mod mixcmd;
mod plugincmd;
mod plugins;
mod profilesrun;
mod pwcmd;
mod remote;
mod streamrun;
mod tray;
mod xlr;

use fluxdeck_core::image::{DynamicImage, RgbaImage};
use fluxdeck_core::{Config, Dial, Painter, State, Theme, config};
use plugins::Host;
use serde_json::{Value, json};
use elgato_streamdeck::images::ImageRect;
use elgato_streamdeck::info::Kind;
use elgato_streamdeck::{DeviceStateUpdate, StreamDeck, StreamDeckError, list_devices, new_hidapi, refresh_device_list};
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Slot {
    Key(u8),
    Dial(u8),
}

impl Slot {
    fn id(self) -> String {
        match self {
            Slot::Key(i) => format!("k{i}"),
            Slot::Dial(i) => format!("d{i}"),
        }
    }
}

pub(crate) enum Msg {
    Reload,
    /// Something a plugin wants shown: setTitle, setImage, setState, setFeedback…
    Plugin(Value),
    /// A plugin connected and is ready for events.
    PluginReady(String),
    /// A plugin was installed or removed.
    Plugins,
    /// An action's settings page opened (true) or closed (false).
    Inspector(String, bool),
    /// A profile wants this page shown, or (None) the page from before it.
    ProfilePage(Option<String>),
    /// A key's sequence reached a "go to page" step.
    GoPage(String),
    Status { generation: u64, slot: Slot, text: String },
    Meter { generation: u64, slot: Slot, level: f32 },
    /// A tap on the deck shown on a phone.
    Remote(remote::Input),
}

fn main() {
    if let Ok(c) = Config::load() {
        fluxdeck_core::i18n::init(&c.language);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sub = match args.first().map(String::as_str) {
        Some("xlr") => Some(xlr::command(&args[1..])),
        Some("pw") => Some(pwcmd::command(&args[1..])),
        Some("corsair") => Some(corsaircmd::command(&args[1..])),
        Some("plugin") => Some(plugincmd::command(&args[1..])),
        Some("effects") => Some(effectscmd::command(&args[1..])),
        Some("mix") => Some(mixcmd::command(&args[1..])),
        Some("backup") => Some(backupcmd::command(&args[1..])),
        Some("launch") => Some(match args.get(1).and_then(|id| fluxdeck_core::apps::find(id)) {
            Some(app) => fluxdeck_core::apps::launch(&app).map_err(|e| e.to_string()),
            None => Err("usage: fluxdeck launch APP_ID (a .desktop file name)".into()),
        }),
        Some(other) => Some(Err(format!("unknown command {other}; run without arguments to start the daemon"))),
        None => None,
    };
    if let Some(result) = sub {
        if let Err(e) = result {
            eprintln!("fluxdeck: {e}");
            std::process::exit(1);
        }
        return;
    }
    daemon();
}

fn daemon() {
    // One copy at a time: the desktop's startup list and systemd may both try.
    if fluxdeck_core::paths::service_running() {
        eprintln!("fluxdeck: already running");
        return;
    }
    let (tx, rx) = channel();
    let _watcher = watch(tx.clone());
    effectsrun::start();
    remote::start(tx.clone());
    groupsrun::start();
    streamrun::start();
    profilesrun::start(tx.clone());
    extrakeys::start(tx.clone());
    tray::start(tx.clone());
    let host = Host::start(tx.clone());
    if host.is_none() {
        eprintln!("fluxdeck: no free port for plugins; they are off");
    }
    if let (Some(h), Ok(cfg)) = (&host, Config::load()) {
        h.set_layout(&cfg);
    }
    let mut hid = match new_hidapi() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("fluxdeck: cannot start hidapi: {e}");
            std::process::exit(1);
        }
    };
    State::clear();
    let mut waiting = false;
    loop {
        let _ = refresh_device_list(&mut hid);
        // shortcut: drives only the first deck found; loop over all of them when someone has two.
        match list_devices(&hid).into_iter().next() {
            Some((kind, serial)) => {
                waiting = false;
                match StreamDeck::connect(&hid, kind, &serial) {
                    Ok(deck) => {
                        eprintln!("fluxdeck: connected to {kind:?} ({serial})");
                        if let Err(e) = Session::new(Some(deck), kind, tx.clone(), host.clone(), &serial).run(&rx, || false) {
                            eprintln!("fluxdeck: lost the deck: {e}");
                        }
                        State::clear();
                    }
                    Err(e) => eprintln!("fluxdeck: cannot open {kind:?}: {e} (check the udev rule)"),
                }
            }
            // No hardware, but the deck on the phone is on: run the deck for it alone, shaped like a
            // Stream Deck +, until hardware shows up or the phone deck is switched off.
            None if fluxdeck_core::remote::enabled() => {
                waiting = false;
                eprintln!("fluxdeck: no Stream Deck; the deck runs for the phone");
                let stop = || {
                    let _ = refresh_device_list(&mut hid);
                    !list_devices(&hid).is_empty() || !fluxdeck_core::remote::enabled()
                };
                if let Err(e) = Session::new(None, Kind::Plus, tx.clone(), host.clone(), "fluxdeck-phone").run(&rx, stop) {
                    eprintln!("fluxdeck: the phone deck stopped: {e}");
                }
                State::clear();
            }
            None if !waiting => {
                eprintln!("fluxdeck: waiting for a Stream Deck");
                waiting = true;
            }
            None => {}
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Sends Reload whenever config.toml or theme.toml change. Kept alive by the caller.
fn watch(tx: Sender<Msg>) -> Option<notify::RecommendedWatcher> {
    let dir = config::dir();
    let _ = std::fs::create_dir_all(&dir);
    let plugins_dir = fluxdeck_core::plugins::dir();
    let _ = std::fs::create_dir_all(&plugins_dir);
    let pd = plugins_dir.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        if ev.paths.iter().any(|p| p.parent() == Some(pd.as_path())) && matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_)) {
            let _ = tx.send(Msg::Plugins);
        }
        let ours = ev.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "config.toml" || n == "theme.toml"));
        if ours {
            let _ = tx.send(Msg::Reload);
        }
    })
    .map_err(|e| eprintln!("fluxdeck: config changes will not be noticed: {e}"))
    .ok()?;
    w.watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| eprintln!("fluxdeck: cannot watch {}: {e}", dir.display()))
        .ok()?;
    let _ = w.watch(&plugins_dir, RecursiveMode::NonRecursive);
    Some(w)
}

fn first_line(out: &[u8]) -> String {
    String::from_utf8_lossy(out).lines().next().unwrap_or("").trim().to_string()
}

/// Runs `cmd` with `sh`, with this binary's folder first in PATH so commands can call `fluxdeck …`.
pub(crate) fn shell(cmd: &str, steps: u32) -> Command {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).env("FLUXDECK_STEPS", steps.to_string()).stdin(Stdio::null());
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let joined = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path)));
        if let Ok(p) = joined {
            c.env("PATH", p);
        }
    }
    c
}

struct Session {
    /// The hardware; None while the deck exists only on a phone.
    deck: Option<Arc<StreamDeck>>,
    kind: Kind,
    tx: Sender<Msg>,
    cfg: Config,
    theme: Theme,
    painter: Painter,
    page: usize,
    /// Bumped on every page change or reload, so late status results are dropped.
    generation: u64,
    status: HashMap<Slot, String>,
    due: HashMap<Slot, Instant>,
    busy: HashSet<Slot>,
    sent: HashMap<Slot, u64>,
    meters: Vec<Child>,
    levels: HashMap<Slot, f32>,
    published: Option<State>,
    /// Modification time of the palette file, checked once a second.
    palette_seen: (Instant, Option<std::time::SystemTime>),
    dirty: bool,
    host: Option<Host>,
    /// What plugins asked to show, by instance.
    looks: HashMap<String, Look>,
    /// Default images from plugin manifests, by "action/state".
    images: HashMap<String, Option<RgbaImage>>,
    /// Off-screen actions shown to their plugin while their settings page is open.
    ghosts: HashSet<String>,
    /// (page before a profile, page the profile showed).
    return_to: Option<(usize, usize)>,
}

/// A plugin-drawn slot.
#[derive(Default, Clone)]
struct Look {
    title: Option<String>,
    image: Option<RgbaImage>,
    state: usize,
    feedback: serde_json::Map<String, Value>,
    /// Until when a showOk / showAlert mark stays, and which one.
    flash: Option<(Instant, bool)>,
}

fn sdk_kind(kind: Kind) -> u8 {
    match kind {
        Kind::Mini | Kind::MiniMk2 | Kind::MiniDiscord | Kind::MiniMk2Module => 1,
        Kind::Xl | Kind::XlV2 => 2,
        Kind::Pedal => 5,
        Kind::Plus => 7,
        Kind::Neo => 9,
        Kind::PlusXl => 11,
        _ => 0,
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop_meters();
    }
}

impl Session {
    /// `deck` is None for the deck on a phone with no hardware; `kind` then gives its shape.
    fn new(deck: Option<StreamDeck>, kind: Kind, tx: Sender<Msg>, host: Option<Host>, serial: &str) -> Session {
        let cfg = Config::load().unwrap_or_else(|e| {
            eprintln!("fluxdeck: {e}; using an empty layout");
            Config::default()
        });
        if let Some(h) = &host {
            let name = if deck.is_some() { model_name(kind) } else { "FluxDeck".into() };
            h.set_device(plugins::Device { id: serial.to_string(), name, kind: sdk_kind(kind), cols: kind.column_count(), rows: kind.row_count() });
            h.set_layout(&cfg);
        }
        Session {
            host,
            looks: HashMap::new(),
            images: HashMap::new(),
            ghosts: HashSet::new(),
            return_to: None,
            kind,
            deck: deck.map(Arc::new),
            tx,
            theme: cfg.effective_theme(),
            painter: Painter::new(&cfg.font),
            cfg,
            page: 0,
            generation: 0,
            status: HashMap::new(),
            due: HashMap::new(),
            busy: HashSet::new(),
            sent: HashMap::new(),
            meters: Vec::new(),
            levels: HashMap::new(),
            published: None,
            palette_seen: (Instant::now(), None),
            dirty: true,
        }
    }

    /// Runs until the hardware is lost, or, with no hardware, until `stop` (asked once a second) says so.
    fn run(mut self, rx: &Receiver<Msg>, mut stop: impl FnMut() -> bool) -> Result<(), StreamDeckError> {
        if let Some(d) = &self.deck {
            d.set_brightness(self.cfg.brightness)?;
        }
        self.start_meters();
        self.appear(true, None);
        let reader = self.deck.as_ref().map(|d| d.get_reader());
        let mut asked = Instant::now();
        loop {
            match &reader {
                Some(r) => {
                    for ev in r.read(Some(Duration::from_millis(25)))? {
                        self.input(ev);
                    }
                }
                None => {
                    thread::sleep(Duration::from_millis(25));
                    if asked.elapsed() > Duration::from_secs(1) {
                        asked = Instant::now();
                        if stop() {
                            return Ok(());
                        }
                    }
                }
            }
            // A phone started watching: send it every image once.
            if remote::take_refresh() {
                self.sent.clear();
                self.dirty = true;
            }
            let mut reload = false;
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::Reload => reload = true,
                    Msg::Status { generation, slot, text } if generation == self.generation => {
                        self.busy.remove(&slot);
                        if self.status.get(&slot) != Some(&text) {
                            self.status.insert(slot, text);
                            self.dirty = true;
                        }
                    }
                    Msg::Meter { generation, slot, level } if generation == self.generation => {
                        // Fast attack, slow fall, and coarse steps so the strip is not redrawn for noise.
                        let old = self.levels.get(&slot).copied().unwrap_or(0.0);
                        let shown = (level.max(old - 0.05) * 40.0).round() / 40.0;
                        if shown != old {
                            self.levels.insert(slot, shown);
                            self.dirty = true;
                        }
                    }
                    Msg::Plugin(v) => self.plugin_says(v),
                    Msg::PluginReady(uuid) => {
                        if let Some(h) = &self.host {
                            h.hello(&uuid);
                        }
                        self.appear(true, Some(&uuid));
                    }
                    Msg::Inspector(instance, open) => self.inspector(&instance, open),
                    Msg::ProfilePage(Some(name)) => {
                        if let Some(i) = self.cfg.page_index(&name) {
                            // Remember where the deck was, once, so leaving the profile goes back there.
                            if self.return_to.is_none() {
                                self.return_to = Some((self.page, i));
                            }
                            self.open_page(i);
                        }
                    }
                    Msg::GoPage(name) => {
                        if let Some(i) = self.cfg.page_index(&name) {
                            self.open_page(i);
                        }
                    }
                    Msg::ProfilePage(None) => {
                        // Only if the user has not moved to another page in the meantime.
                        if let Some((back, shown)) = self.return_to.take()
                            && self.page == shown
                        {
                            self.open_page(back);
                        }
                    }
                    Msg::Plugins => {
                        if let Some(h) = &self.host {
                            h.launch();
                            h.set_layout(&self.cfg);
                        }
                        self.images.clear();
                        self.dirty = true;
                    }
                    Msg::Remote(input) => self.remote(input),
                    Msg::Status { .. } | Msg::Meter { .. } => {}
                }
            }
            if reload {
                self.reload()?;
            }
            self.watch_palette();
            self.poll_status();
            self.expire_flashes();
            if self.dirty {
                self.draw()?;
                self.publish();
                self.dirty = false;
            }
        }
    }

    fn reload(&mut self) -> Result<(), StreamDeckError> {
        let cfg = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("fluxdeck: keeping the previous layout: {e}");
                return Ok(());
            }
        };
        if cfg.font != self.cfg.font {
            self.painter = Painter::new(&cfg.font);
        }
        if cfg.disabled_plugins != self.cfg.disabled_plugins
            && let Some(h) = &self.host
        {
            h.launch();
        }
        if cfg.brightness != self.cfg.brightness
            && let Some(d) = &self.deck
        {
            d.set_brightness(cfg.brightness)?;
        }
        let page_name = self.cfg.pages[self.page].name.clone();
        self.appear(false, None);
        self.theme = cfg.effective_theme();
        self.page = cfg.page_index(&page_name).unwrap_or(0);
        self.cfg = cfg;
        if let Some(h) = &self.host {
            h.set_layout(&self.cfg);
        }
        self.reset_page();
        self.appear(true, None);
        eprintln!("fluxdeck: layout reloaded");
        Ok(())
    }

    fn reset_page(&mut self) {
        self.generation += 1;
        self.status.clear();
        self.due.clear();
        self.busy.clear();
        self.stop_meters();
        self.start_meters();
        self.dirty = true;
    }

    fn watch_palette(&mut self) {
        if self.cfg.palette.is_empty() || self.palette_seen.0.elapsed() < Duration::from_secs(1) {
            return;
        }
        let mtime = std::fs::metadata(config::expand(&self.cfg.palette)).and_then(|m| m.modified()).ok();
        if mtime != self.palette_seen.1 {
            self.theme = self.cfg.effective_theme();
            self.dirty = true;
        }
        self.palette_seen = (Instant::now(), mtime);
    }

    fn start_meters(&mut self) {
        let page = &self.cfg.pages[self.page];
        let wanted: Vec<(String, Slot)> = page.dials.iter().filter(|d| !d.meter.is_empty()).map(|d| (d.meter.clone(), Slot::Dial(d.pos))).collect();
        for (node, slot) in wanted {
            let (tx, generation) = (self.tx.clone(), self.generation);
            if let Some(child) = fluxdeck_core::pw::meter(&node, move |level| tx.send(Msg::Meter { generation, slot, level }).is_ok()) {
                self.meters.push(child);
            }
        }
    }

    fn stop_meters(&mut self) {
        for mut c in self.meters.drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.levels.clear();
    }

    fn open_page(&mut self, index: usize) {
        if index != self.page {
            self.appear(false, None);
            self.page = index;
            self.reset_page();
            self.appear(true, None);
        }
    }

    /// Plugin instances on the current page, optionally only those of one plugin.
    fn instances(&self, plugin: Option<&str>) -> Vec<String> {
        let page = &self.cfg.pages[self.page];
        let all = page.keys.iter().map(|k| &k.instance).chain(page.dials.iter().map(|d| &d.instance));
        all.filter(|i| !i.is_empty())
            .filter(|i| plugin.is_none_or(|p| self.host.as_ref().and_then(|h| h.placed(i)).is_some_and(|x| x.plugin == p)))
            .cloned()
            .collect()
    }

    /// Tells plugins their actions became visible (willAppear) or hidden (willDisappear).
    fn appear(&self, visible: bool, plugin: Option<&str>) {
        let Some(h) = &self.host else { return };
        for i in self.instances(plugin) {
            let state = self.looks.get(&i).map_or(0, |l| l.state);
            h.event(if visible { "willAppear" } else { "willDisappear" }, &i, json!({ "state": state }));
        }
    }

    /// Plugins only talk to actions they were told are on screen, so an action edited from another
    /// page appears to its plugin while its settings page is open.
    fn inspector(&mut self, instance: &str, open: bool) {
        let Some(h) = self.host.clone() else { return };
        let visible = self.instances(None).iter().any(|i| i == instance);
        if open {
            if !visible && self.ghosts.insert(instance.to_string()) {
                h.event("willAppear", instance, json!({ "state": 0 }));
            }
            h.event("propertyInspectorDidAppear", instance, json!({}));
        } else {
            h.event("propertyInspectorDidDisappear", instance, json!({}));
            if self.ghosts.remove(instance) && !visible {
                h.event("willDisappear", instance, json!({ "state": 0 }));
            }
        }
    }

    fn plugin_event(&self, slot: Slot, event: &str, extra: Value) -> bool {
        let page = &self.cfg.pages[self.page];
        let instance = match slot {
            Slot::Key(i) => page.key(i).map(|k| &k.instance),
            Slot::Dial(i) => page.dial(i).map(|d| &d.instance),
        };
        match (instance.filter(|i| !i.is_empty()), &self.host) {
            (Some(i), Some(h)) => {
                let mut extra = extra;
                if let Value::Object(m) = &mut extra {
                    m.insert("state".into(), json!(self.looks.get(i).map_or(0, |l| l.state)));
                }
                h.event(event, i, extra);
                true
            }
            _ => false,
        }
    }

    /// Multi-state keys step to their next state after a press, as Elgato's software does.
    fn auto_state(&mut self, i: u8) {
        let Some(k) = self.cfg.pages[self.page].key(i).cloned() else { return };
        let Some((_, a)) = self.host.as_ref().and_then(|h| h.plugin_of(&k.action)) else { return };
        if a.states.len() > 1 && !a.disable_automatic_states {
            let look = self.looks.entry(k.instance).or_default();
            look.state = (look.state + 1) % a.states.len();
            self.dirty = true;
        }
    }

    fn plugin_says(&mut self, v: Value) {
        let Some(ctx) = v["context"].as_str() else { return };
        let look = self.looks.entry(ctx.to_string()).or_default();
        let p = &v["payload"];
        let size = self.kind.key_image_format().size.0 as u32;
        match v["event"].as_str().unwrap_or_default() {
            "setTitle" => look.title = p["title"].as_str().map(String::from),
            "setImage" => look.image = p["image"].as_str().filter(|s| !s.is_empty()).and_then(|s| fluxdeck_core::plugins::data_url(s, size)),
            "setState" => look.state = p["state"].as_u64().unwrap_or(0) as usize,
            "setFeedback" => {
                if let Value::Object(m) = p {
                    look.feedback.extend(m.clone());
                }
            }
            "setFeedbackLayout" => look.feedback.clear(),
            "showOk" => look.flash = Some((Instant::now() + Duration::from_millis(1200), true)),
            "showAlert" => look.flash = Some((Instant::now() + Duration::from_millis(1200), false)),
            _ => return,
        }
        self.dirty = true;
    }

    fn expire_flashes(&mut self) {
        let now = Instant::now();
        for l in self.looks.values_mut() {
            if l.flash.is_some_and(|(t, _)| t <= now) {
                l.flash = None;
                self.dirty = true;
            }
        }
    }

    /// Manifest image for an action's state, loaded once.
    fn default_image(&mut self, action: &str, state: usize, size: u32) -> Option<RgbaImage> {
        let key = format!("{action}/{state}/{size}");
        if let Some(img) = self.images.get(&key) {
            return img.clone();
        }
        let img = self.host.as_ref().and_then(|h| h.plugin_of(action)).and_then(|(p, a)| {
            let rel = a.states.get(state).or(a.states.first()).map(|s| s.image.clone()).filter(|i| !i.is_empty() && i != "actionDefaultImage").unwrap_or(a.icon.clone());
            let rel = if a.on_dials() && !a.on_keys() { a.encoder.as_ref().map(|e| e.icon.clone()).filter(|i| !i.is_empty()).unwrap_or(rel) } else { rel };
            fluxdeck_core::plugins::image_file(&p.path, &rel).and_then(|f| fluxdeck_core::plugins::load_image(&f, size))
        });
        self.images.insert(key, img.clone());
        img
    }

    fn manifest_title(&self, action: &str, state: usize) -> String {
        self.host.as_ref().and_then(|h| h.plugin_of(action)).and_then(|(_, a)| a.states.get(state).map(|s| s.title.clone())).unwrap_or_default()
    }

    fn plugin_key_image(&mut self, key: &fluxdeck_core::Key, size: u32) -> RgbaImage {
        let look = self.looks.get(&key.instance).cloned().unwrap_or_default();
        let image = look.image.clone().or_else(|| self.default_image(&key.action, look.state, size));
        let title = look.title.clone().unwrap_or_else(|| self.manifest_title(&key.action, look.state));
        let mut img = self.painter.plugin_key(image.as_ref(), &title, &self.theme, size);
        if let Some((_, ok)) = look.flash {
            mark(&mut img, ok);
        }
        img
    }

    /// The strip segment of a plugin dial, from its setFeedback values (title, value, indicator).
    fn plugin_dial_image(&mut self, d: &Dial, w: u32, h: u32) -> RgbaImage {
        let look = self.looks.get(&d.instance).cloned().unwrap_or_default();
        let text = |k: &str| match look.feedback.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Object(o)) => o.get("value").map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let indicator = match look.feedback.get("indicator") {
            Some(Value::Object(o)) => o.get("value").and_then(Value::as_f64),
            Some(v) => v.as_f64(),
            None => None,
        };
        let label = text("title").or(look.title.clone()).unwrap_or_else(|| d.label.clone());
        let value = text("value").unwrap_or_default();
        let status = match indicator {
            Some(i) => format!("{value}|{}", (i / 100.0).clamp(0.0, 1.0)),
            None => value,
        };
        let shown = Dial { label, icon: String::new(), ..Default::default() };
        let mut img = self.painter.dial(Some(&shown), Some(&status), None, &self.theme, w, h);
        let icon = look.image.clone().or_else(|| self.default_image(&d.action, look.state, h / 2));
        if let Some(pic) = icon {
            let s = h / 2;
            let pic = fluxdeck_core::image::imageops::resize(&pic, s, s, fluxdeck_core::image::imageops::FilterType::Triangle);
            fluxdeck_core::image::imageops::overlay(&mut img, &pic, (h / 10) as i64, ((h - s) / 2) as i64);
        }
        img
    }

    fn input(&mut self, ev: DeviceStateUpdate) {
        let page = &self.cfg.pages[self.page];
        match ev {
            DeviceStateUpdate::ButtonDown(i) => {
                let Some(key) = page.key(i).cloned() else { return };
                self.plugin_event(Slot::Key(i), "keyDown", json!({}));
                if let Some(target) = self.cfg.page_index(&key.page) {
                    self.open_page(target);
                }
                self.act(Slot::Key(i), &key.command, 1);
                if !key.steps.is_empty() {
                    run_steps(key.steps.clone(), self.tx.clone());
                }
            }
            DeviceStateUpdate::ButtonUp(i) => {
                if self.plugin_event(Slot::Key(i), "keyUp", json!({})) {
                    self.auto_state(i);
                }
            }
            DeviceStateUpdate::EncoderUp(i) => {
                self.plugin_event(Slot::Dial(i), "dialUp", json!({}));
            }
            DeviceStateUpdate::EncoderTwist(i, delta) => {
                self.plugin_event(Slot::Dial(i), "dialRotate", json!({ "ticks": delta, "pressed": false }));
                let Some(d) = self.cfg.pages[self.page].dial(i) else { return };
                let cmd = if delta > 0 { d.right.clone() } else { d.left.clone() };
                self.act(Slot::Dial(i), &cmd, delta.unsigned_abs() as u32);
            }
            DeviceStateUpdate::EncoderDown(i) => {
                self.plugin_event(Slot::Dial(i), "dialDown", json!({}));
                self.press_dial(i);
            }
            DeviceStateUpdate::TouchScreenPress(x, y) | DeviceStateUpdate::TouchScreenLongPress(x, y) => {
                let hold = matches!(ev, DeviceStateUpdate::TouchScreenLongPress(..));
                if let Some((w, _)) = self.kind.lcd_strip_size() {
                    let segment = w / self.kind.encoder_count().max(1) as usize;
                    let i = (x as usize / segment) as u8;
                    let tap = json!({ "tapPos": [x as usize % segment, y], "hold": hold });
                    if !self.plugin_event(Slot::Dial(i), "touchTap", tap) {
                        self.press_dial(i);
                    }
                }
            }
            DeviceStateUpdate::TouchScreenSwipe((x0, _), (x1, _)) => {
                // Folders are reached from their key, so swiping walks the other pages.
                let n = self.cfg.pages.len();
                let step = |p: usize| if x1 < x0 { (p + 1) % n } else { (p + n - 1) % n };
                let mut next = step(self.page);
                while self.cfg.pages[next].folder && next != self.page {
                    next = step(next);
                }
                self.open_page(next);
            }
            _ => {}
        }
    }

    /// A tap on the phone goes the same way as the hardware's own press, twist or page swipe.
    fn remote(&mut self, input: remote::Input) {
        match input {
            remote::Input::Key(i) => {
                self.input(DeviceStateUpdate::ButtonDown(i));
                self.input(DeviceStateUpdate::ButtonUp(i));
            }
            remote::Input::Turn(i, d) => self.input(DeviceStateUpdate::EncoderTwist(i, d)),
            remote::Input::Press(i) => {
                self.input(DeviceStateUpdate::EncoderDown(i));
                self.input(DeviceStateUpdate::EncoderUp(i));
            }
            remote::Input::Page(i) if i < self.cfg.pages.len() => self.open_page(i),
            remote::Input::Page(_) => {}
        }
    }

    fn press_dial(&mut self, i: u8) {
        if let Some(cmd) = self.cfg.pages[self.page].dial(i).map(|d| d.press.clone()) {
            self.act(Slot::Dial(i), &cmd, 1);
        }
    }

    fn status_cmd(&self, slot: Slot) -> Option<(String, f32)> {
        let page = &self.cfg.pages[self.page];
        let (cmd, every) = match slot {
            Slot::Key(i) => page.key(i).map(|k| (k.status.clone(), k.interval()))?,
            Slot::Dial(i) => page.dial(i).map(|d| (d.status.clone(), d.interval()))?,
        };
        (!cmd.trim().is_empty()).then_some((cmd, every))
    }

    /// Runs an action; when the slot shows a status, refreshes it right after the action ends.
    fn act(&mut self, slot: Slot, cmd: &str, steps: u32) {
        if cmd.trim().is_empty() {
            return;
        }
        let status = self.status_cmd(slot).map(|(s, _)| s);
        if status.is_some() {
            // Hold the periodic poll so it does not race the refresh below.
            self.due.insert(slot, Instant::now() + Duration::from_secs(1));
        }
        let (cmd, tx, generation) = (cmd.to_string(), self.tx.clone(), self.generation);
        thread::spawn(move || {
            match shell(&cmd, steps).stdout(Stdio::null()).spawn() {
                Ok(mut child) if status.is_none() => {
                    let _ = child.wait();
                    return;
                }
                Ok(mut child) => {
                    // Wait briefly so the status reflects the action, without blocking on long-lived apps.
                    let deadline = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                        thread::sleep(Duration::from_millis(10));
                    }
                    thread::spawn(move || child.wait());
                }
                Err(e) => eprintln!("fluxdeck: cannot run `{cmd}`: {e}"),
            }
            if let Some(s) = status {
                let text = shell(&s, 0).output().map(|o| first_line(&o.stdout)).unwrap_or_default();
                let _ = tx.send(Msg::Status { generation, slot, text });
            }
        });
    }

    fn poll_status(&mut self) {
        let now = Instant::now();
        let page = &self.cfg.pages[self.page];
        let slots: Vec<Slot> = page.keys.iter().map(|k| Slot::Key(k.pos)).chain(page.dials.iter().map(|d| Slot::Dial(d.pos))).collect();
        for slot in slots {
            let Some((cmd, every)) = self.status_cmd(slot) else { continue };
            if self.busy.contains(&slot) || self.due.get(&slot).is_some_and(|d| *d > now) {
                continue;
            }
            self.busy.insert(slot);
            self.due.insert(slot, now + Duration::from_secs_f32(every.max(0.2)));
            let (tx, generation) = (self.tx.clone(), self.generation);
            // shortcut: a status command that never exits keeps its slot frozen; add a timeout if that bites.
            thread::spawn(move || {
                let text = match shell(&cmd, 0).output() {
                    Ok(o) => first_line(&o.stdout),
                    Err(e) => format!("error: {e}"),
                };
                let _ = tx.send(Msg::Status { generation, slot, text });
            });
        }
    }

    /// True when `img` differs from what the deck already shows in `slot`.
    fn changed(&mut self, slot: Slot, img: &RgbaImage) -> bool {
        let mut h = DefaultHasher::new();
        img.as_raw().hash(&mut h);
        let hash = h.finish();
        self.sent.insert(slot, hash) != Some(hash)
    }

    fn draw(&mut self) -> Result<(), StreamDeckError> {
        let page = self.cfg.pages[self.page].clone();
        let size = self.kind.key_image_format().size.0 as u32;
        for i in 0..self.kind.key_count() {
            let st = self.status.get(&Slot::Key(i)).cloned();
            let plugin = page.key(i).filter(|k| !k.instance.is_empty()).map(|k| k.instance.clone());
            let img = match page.key(i) {
                Some(k) if !k.instance.is_empty() => self.plugin_key_image(k, size),
                k => self.painter.key(k, st.as_deref(), &self.theme, size),
            };
            if self.changed(Slot::Key(i), &img) {
                // The editor shows what the plugin draws, not just its manifest icon.
                if let Some(instance) = plugin {
                    let _ = img.save(fluxdeck_core::paths::runtime(&format!("plugin-{instance}.png")));
                }
                if remote::serving() {
                    remote::set_frame(&Slot::Key(i).id(), &img);
                }
                if let Some(d) = &self.deck {
                    d.set_button_image(i, DynamicImage::ImageRgba8(img))?;
                }
            }
        }
        if let Some(d) = &self.deck {
            d.flush()?;
        }
        // shortcut: assumes a horizontal strip (Stream Deck +); the Plus XL strip is vertical.
        if let Some((w, h)) = self.kind.lcd_strip_size() {
            let n = self.kind.encoder_count().max(1);
            let seg = (w / n as usize) as u32;
            for i in 0..n {
                let slot = Slot::Dial(i);
                let st = self.status.get(&slot).cloned();
                let img = match page.dial(i) {
                    Some(d) if !d.instance.is_empty() => self.plugin_dial_image(d, seg, h as u32),
                    d => self.painter.dial(d, st.as_deref(), self.levels.get(&slot).copied(), &self.theme, seg, h as u32),
                };
                if self.changed(slot, &img) {
                    if remote::serving() {
                        remote::set_frame(&slot.id(), &img);
                    }
                    if let Some(d) = &self.deck {
                        let rect = ImageRect::from_image(DynamicImage::ImageRgba8(img))?;
                        d.write_lcd((i as u32 * seg) as u16, 0, &rect)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Shares the current page and statuses with the editor; written only when they change.
    fn publish(&mut self) {
        if remote::serving() {
            let pages = self.cfg.pages.iter().map(|p| p.name.clone()).collect();
            remote::set_layout(self.kind.column_count(), self.kind.row_count(), self.kind.encoder_count(), self.page, pages, &self.theme);
        }
        let state = State {
            model: if self.deck.is_some() { format!("{:?}", self.kind) } else { "Phone".into() },
            cols: self.kind.column_count(),
            rows: self.kind.row_count(),
            dials: self.kind.encoder_count(),
            page: self.cfg.pages[self.page].name.clone(),
            status: self.status.iter().map(|(s, t)| (s.id(), t.clone())).collect(),
        };
        if self.published.as_ref() != Some(&state) {
            state.write();
            self.published = Some(state);
        }
    }
}

/// Runs a key's sequence in its own thread: each command ends (or ten seconds pass) before the next step.
pub(crate) fn run_steps(steps: Vec<fluxdeck_core::Step>, tx: Sender<Msg>) {
    use fluxdeck_core::Step;
    thread::spawn(move || {
        for step in steps {
            match step {
                Step::Command { run } => match shell(&run, 1).stdout(Stdio::null()).spawn() {
                    Ok(mut child) => {
                        let deadline = Instant::now() + Duration::from_secs(10);
                        while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                            thread::sleep(Duration::from_millis(20));
                        }
                        // Long-lived programs keep running on their own.
                        thread::spawn(move || child.wait());
                    }
                    Err(e) => eprintln!("fluxdeck: cannot run `{run}`: {e}"),
                },
                Step::Wait { ms } => thread::sleep(Duration::from_millis(ms.min(60_000) as u64)),
                Step::Page { name } => drop(tx.send(Msg::GoPage(name))),
                Step::App { id } => {
                    if let Some(app) = fluxdeck_core::apps::find(&id) {
                        let _ = fluxdeck_core::apps::launch(&app);
                    }
                }
            }
        }
    });
}

/// A small tick or cross in the corner, for showOk / showAlert.
fn mark(img: &mut RgbaImage, ok: bool) {
    let c = if ok { fluxdeck_core::image::Rgba([76, 175, 80, 255]) } else { fluxdeck_core::image::Rgba([229, 72, 77, 255]) };
    let s = img.width();
    let r = s / 6;
    let (cx, cy) = (s - r - 4, r + 4);
    for y in cy - r..cy + r {
        for x in cx - r..cx + r {
            let (dx, dy) = (x as i32 - cx as i32, y as i32 - cy as i32);
            if dx * dx + dy * dy <= (r * r) as i32 {
                img.put_pixel(x, y, c);
            }
        }
    }
}

fn model_name(kind: Kind) -> String {
    match kind {
        Kind::Plus => "Stream Deck +".into(),
        Kind::PlusXl => "Stream Deck + XL".into(),
        Kind::Neo => "Stream Deck Neo".into(),
        Kind::Pedal => "Stream Deck Pedal".into(),
        Kind::Mini | Kind::MiniMk2 | Kind::MiniDiscord | Kind::MiniMk2Module => "Stream Deck Mini".into(),
        Kind::Xl | Kind::XlV2 => "Stream Deck XL".into(),
        _ => "Stream Deck".into(),
    }
}
