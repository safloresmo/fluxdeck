//! The program each connected peripheral needs (OpenLinkHub, OpenRGB, HeadsetControl), whether it
//! is ready, and how to get it ready.
use crate::effects::{on_path, package_manager};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Helper {
    OpenLinkHub,
    OpenRgb,
    HeadsetControl,
}

impl Helper {
    pub fn name(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "OpenLinkHub",
            Helper::OpenRgb => "OpenRGB",
            Helper::HeadsetControl => "HeadsetControl",
        }
    }

    /// The FluxDeck module that uses it.
    pub fn module(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "corsair",
            Helper::OpenRgb => "openrgb",
            Helper::HeadsetControl => "headset",
        }
    }

    fn page(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "https://github.com/jurkovic-nikola/OpenLinkHub",
            Helper::OpenRgb => "https://openrgb.org",
            Helper::HeadsetControl => "https://github.com/Sapd/HeadsetControl",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Missing,
    /// Installed but not answering.
    Stopped,
    Ready,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fix {
    /// A command to run in a terminal, where the user can read it and type their password.
    Terminal(String),
    /// A program to open.
    Launch(Vec<String>),
    /// No command fits this system: the project's page.
    Page(String),
}

/// One helper, the connected devices that need it, and what to do.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    pub helper: Helper,
    pub devices: Vec<String>,
    pub state: State,
    pub fix: Option<Fix>,
}

/// Helpers for a USB device by its vendor and product name. Only devices with a HID interface are
/// asked about, so webcams and drives of the same brands are left out.
fn helpers_for(vendor: &str, product: &str) -> Vec<Helper> {
    // shortcut: headsets are told apart by name; HeadsetControl's own list would be exact once installed.
    let p = product.to_lowercase();
    let headset = ["headset", "arctis", "cloud", "astro", "virtuoso"].iter().any(|w| p.contains(w));
    match vendor {
        "1b1c" => vec![Helper::OpenLinkHub],
        "046d" | "1038" | "0951" | "03f0" if headset => vec![Helper::HeadsetControl],
        // Logitech, SteelSeries, HyperX, Razer, ASUS, MSI, ITE (Gigabyte boards), NZXT, Cooler Master, Wooting.
        "046d" | "1038" | "0951" | "03f0" | "1532" | "0b05" | "0db0" | "048d" | "1e71" | "2516" | "31e3" => vec![Helper::OpenRgb],
        _ => Vec::new(),
    }
}

/// (vendor, product name) of every connected USB device with a HID interface.
fn usb_hid(root: &Path) -> Vec<(String, String)> {
    let read = |p: &Path| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.join("idVendor").exists())
        .filter(|d| std::fs::read_dir(d).into_iter().flatten().flatten().any(|i| read(&i.path().join("bInterfaceClass")) == "03"))
        .map(|d| (read(&d.join("idVendor")), read(&d.join("product"))))
        .collect()
}

fn state(h: Helper) -> State {
    let installed = match h {
        Helper::OpenLinkHub => ["/usr/lib/systemd/system", "/etc/systemd/system"].iter().any(|d| Path::new(d).join("openlinkhub.service").exists()) || on_path("OpenLinkHub"),
        Helper::OpenRgb => on_path("openrgb"),
        Helper::HeadsetControl => on_path("headsetcontrol"),
    };
    let running = match h {
        Helper::OpenLinkHub => crate::corsair::running(),
        Helper::OpenRgb => crate::openrgb::running(),
        Helper::HeadsetControl => installed,
    };
    if running {
        State::Ready
    } else if installed {
        State::Stopped
    } else {
        State::Missing
    }
}

fn fix(h: Helper, state: &State) -> Option<Fix> {
    // Only where the distro has the package (checked by the release workflow); elsewhere, the page.
    let install = |pkg: &str, managers: &[&str]| match package_manager().filter(|pm| managers.contains(pm)) {
        Some("pacman") => Some(format!("sudo pacman -S --needed {pkg}")),
        Some("apt") => Some(format!("sudo apt install {pkg}")),
        Some("dnf") => Some(format!("sudo dnf install {pkg}")),
        Some(_) => Some(format!("sudo zypper install {pkg}")),
        None => None,
    };
    let page = || Fix::Page(h.page().to_string());
    Some(match (h, state) {
        (_, State::Ready) => return None,
        (Helper::OpenLinkHub, State::Stopped) => Fix::Terminal("sudo systemctl enable --now openlinkhub".into()),
        // Only in the AUR; elsewhere it installs from its releases.
        (Helper::OpenLinkHub, State::Missing) => match ["yay", "paru"].into_iter().find(|a| on_path(a)) {
            Some(aur) => Fix::Terminal(format!("{aur} -S --needed openlinkhub-bin && sudo systemctl enable --now openlinkhub")),
            None => page(),
        },
        (Helper::OpenRgb, State::Stopped) => Fix::Launch(vec!["openrgb".into(), "--server".into(), "--startminimized".into()]),
        (Helper::OpenRgb, State::Missing) => install("openrgb", &["pacman", "dnf", "zypper"]).map(Fix::Terminal).unwrap_or_else(page),
        (Helper::HeadsetControl, _) => install("headsetcontrol", &["pacman", "dnf"]).map(Fix::Terminal).unwrap_or_else(page),
    })
}

fn group(found: &[(String, String)]) -> Vec<(Helper, Vec<String>)> {
    let mut out: Vec<(Helper, Vec<String>)> = Vec::new();
    for (vendor, product) in found {
        for h in helpers_for(vendor, product) {
            match out.iter_mut().find(|(x, _)| *x == h) {
                Some((_, names)) if !names.contains(product) => names.push(product.clone()),
                Some(_) => {}
                None => out.push((h, vec![product.clone()])),
            }
        }
    }
    out
}

/// What the connected peripherals need, one entry per helper.
pub fn needs() -> Vec<Need> {
    group(&usb_hid(Path::new("/sys/bus/usb/devices")))
        .into_iter()
        .map(|(helper, devices)| {
            let state = state(helper);
            Need { fix: fix(helper, &state), helper, devices, state }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_devices_by_helper() {
        let found: Vec<(String, String)> = [
            ("1b1c", "CORSAIR SLIPSTREAM WIRELESS USB Receiver"),
            ("1b1c", "CORSAIR SLIPSTREAM WIRELESS USB Receiver"),
            ("1b1c", "CORSAIR VIRTUOSO MAX WIRELESS Gaming Receiver"),
            ("046d", "G502 LIGHTSPEED"),
            ("046d", "PRO X Wireless Gaming Headset"),
            ("0db0", "MYSTIC LIGHT"),
            ("0fd9", "Stream Deck Plus"),
        ]
        .iter()
        .map(|(v, p)| (v.to_string(), p.to_string()))
        .collect();
        let g = group(&found);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0], (Helper::OpenLinkHub, vec!["CORSAIR SLIPSTREAM WIRELESS USB Receiver".into(), "CORSAIR VIRTUOSO MAX WIRELESS Gaming Receiver".into()]));
        assert_eq!(g[1], (Helper::OpenRgb, vec!["G502 LIGHTSPEED".into(), "MYSTIC LIGHT".into()]));
        assert_eq!(g[2], (Helper::HeadsetControl, vec!["PRO X Wireless Gaming Headset".into()]));
    }

    #[test]
    fn ready_needs_no_fix() {
        assert_eq!(fix(Helper::OpenRgb, &State::Ready), None);
        assert_eq!(fix(Helper::OpenLinkHub, &State::Stopped), Some(Fix::Terminal("sudo systemctl enable --now openlinkhub".into())));
    }
}
