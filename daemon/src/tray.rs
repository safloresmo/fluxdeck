//! The tray icon: open the editor, switch the deck's page, mute the microphone, stop the service.
//! It lives in the service, so it is there even with the editor closed.
use crate::Msg;
use fluxdeck_core::i18n::t;
use fluxdeck_core::{Config, State};
use ksni::blocking::TrayMethods;
use ksni::menu::{RadioGroup, RadioItem, StandardItem};
use ksni::{Icon, MenuItem};
use std::sync::mpsc::Sender;
use std::time::Duration;

const LOGO: &str = include_str!("../../assets/fluxdeck.svg");

struct Tray {
    tx: Sender<Msg>,
    pages: Vec<String>,
    page: String,
    icon: Vec<Icon>,
}

fn open_editor() {
    use std::os::unix::process::CommandExt;
    let bin = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(format!("{}-editor", fluxdeck_core::paths::NAME)))).filter(|p| p.is_file());
    let bin = bin.unwrap_or_else(|| format!("{}-editor", fluxdeck_core::paths::NAME).into());
    let _ = std::process::Command::new(bin).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0).spawn();
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        fluxdeck_core::paths::NAME.into()
    }

    fn title(&self) -> String {
        "FluxDeck".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icon.clone()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "FluxDeck".into(), description: self.page.clone(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_editor();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem { label: t("Abrir FluxDeck").into(), activate: Box::new(|_| open_editor()), ..Default::default() }.into(),
            MenuItem::Separator,
        ];
        if !self.pages.is_empty() {
            items.push(
                ksni::menu::SubMenu {
                    label: t("Página del deck").into(),
                    submenu: vec![
                        RadioGroup {
                            selected: self.pages.iter().position(|p| *p == self.page).unwrap_or(0),
                            select: Box::new(|this: &mut Self, i| {
                                if let Some(name) = this.pages.get(i) {
                                    this.page = name.clone();
                                    let _ = this.tx.send(Msg::GoPage(name.clone()));
                                }
                            }),
                            options: self.pages.iter().map(|p| RadioItem { label: p.clone(), ..Default::default() }).collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(
            StandardItem {
                label: t("Silenciar o activar el micrófono").into(),
                activate: Box::new(|_| drop(std::process::Command::new("wpctl").args(["set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"]).status())),
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        items.push(StandardItem { label: t("Detener el servicio").into(), activate: Box::new(|_| std::process::exit(0)), ..Default::default() }.into());
        items
    }
}

/// The logo as ARGB32 (network byte order), the format tray hosts read.
fn logo(size: u32) -> Vec<Icon> {
    let Some(img) = fluxdeck_core::plugins::decode(LOGO.as_bytes(), size) else { return Vec::new() };
    let data = img.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
    vec![Icon { width: size as i32, height: size as i32, data }]
}

pub fn start(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let pages = |c: &Config| c.pages.iter().filter(|p| !p.folder).map(|p| p.name.clone()).collect::<Vec<_>>();
        let tray = Tray { tx, pages: Config::load().map(|c| pages(&c)).unwrap_or_default(), page: String::new(), icon: logo(64) };
        let handle = match tray.spawn() {
            Ok(h) => h,
            Err(e) => {
                eprintln!("fluxdeck: no tray icon: {e}");
                return;
            }
        };
        // Keep the page list and the current page in step with the deck.
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let now = (Config::load().map(|c| pages(&c)).unwrap_or_default(), State::read().map(|s| s.page).unwrap_or_default());
            handle.update(|t| {
                if t.pages != now.0 || t.page != now.1 {
                    (t.pages, t.page) = now.clone();
                }
            });
        }
    });
}
