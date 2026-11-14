//! The program each connected peripheral needs (OpenLinkHub, OpenRGB, HeadsetControl, ratbagd), whether it
//! is ready, and how to get it ready.
use crate::effects::{on_path, package_manager};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Helper {
    OpenLinkHub,
    OpenRgb,
    HeadsetControl,
    Ratbag,
}

impl Helper {
    pub fn name(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "OpenLinkHub",
            Helper::OpenRgb => "OpenRGB",
            Helper::HeadsetControl => "HeadsetControl",
            Helper::Ratbag => "ratbagd",
        }
    }

    /// The FluxDeck module that uses it.
    pub fn module(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "corsair",
            Helper::OpenRgb => "openrgb",
            Helper::HeadsetControl => "headset",
            Helper::Ratbag => "mouse",
        }
    }

    fn page(self) -> &'static str {
        match self {
            Helper::OpenLinkHub => "https://github.com/jurkovic-nikola/OpenLinkHub",
            Helper::OpenRgb => "https://openrgb.org",
            Helper::HeadsetControl => "https://github.com/Sapd/HeadsetControl",
            Helper::Ratbag => "https://github.com/libratbag/libratbag",
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

/// A connected USB device with a HID interface.
#[derive(Debug, Clone, PartialEq)]
struct Usb {
    vendor: String,
    id: String,
    name: String,
    /// Has a HID interface that speaks the mouse protocol.
    mouse: bool,
}

/// Helpers for a USB device. Only devices with a HID interface are asked about, so webcams and
/// drives of the same brands are left out. `supported` is what HeadsetControl itself lists, when it is installed.
fn helpers_for(u: &Usb, supported: Option<&[(String, String)]>) -> Vec<Helper> {
    let p = u.name.to_lowercase();
    let headset = match supported {
        Some(list) => list.iter().any(|(v, i)| *v == u.vendor && *i == u.id),
        // shortcut: without HeadsetControl, headsets are told apart by name; its own list is used once installed.
        None => ["046d", "1038", "0951", "03f0"].contains(&u.vendor.as_str()) && ["headset", "arctis", "cloud", "astro", "virtuoso"].iter().any(|w| p.contains(w)),
    };
    let mut out = Vec::new();
    if u.vendor == "1b1c" {
        out.push(Helper::OpenLinkHub);
    }
    // Corsair headsets go through OpenLinkHub.
    if headset && u.vendor != "1b1c" {
        out.push(Helper::HeadsetControl);
    // Logitech, SteelSeries, HyperX, Razer, ASUS, MSI, ITE (Gigabyte boards), NZXT, Cooler Master, Wooting.
    } else if ["046d", "1038", "0951", "03f0", "1532", "0b05", "0db0", "048d", "1e71", "2516", "31e3"].contains(&u.vendor.as_str()) {
        out.push(Helper::OpenRgb);
    }
    // libratbag's mice are Logitech, SteelSeries and Roccat; Corsair is not among them.
    if u.mouse && ["046d", "1038", "1e7d"].contains(&u.vendor.as_str()) {
        out.push(Helper::Ratbag);
    }
    out
}

/// Every connected USB device with a HID interface.
fn usb_hid(root: &Path) -> Vec<Usb> {
    let read = |p: &Path| std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default();
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.join("idVendor").exists())
        .filter(|d| std::fs::read_dir(d).into_iter().flatten().flatten().any(|i| read(&i.path().join("bInterfaceClass")) == "03"))
        // Protocol 02 of a HID interface is the mouse.
        .map(|d| {
            let mouse = std::fs::read_dir(&d).into_iter().flatten().flatten().any(|i| read(&i.path().join("bInterfaceClass")) == "03" && read(&i.path().join("bInterfaceProtocol")) == "02");
            Usb { vendor: read(&d.join("idVendor")), id: read(&d.join("idProduct")), name: read(&d.join("product")), mouse }
        })
        .collect()
}

/// Whether a Corsair device (USB vendor 1b1c) is plugged in.
pub fn corsair_connected() -> bool {
    usb_hid(Path::new("/sys/bus/usb/devices")).iter().any(|u| u.vendor == "1b1c")
}

fn state(h: Helper) -> State {
    let installed = match h {
        Helper::OpenLinkHub => ["/usr/lib/systemd/system", "/etc/systemd/system"].iter().any(|d| Path::new(d).join("openlinkhub.service").exists()) || on_path("OpenLinkHub"),
        Helper::OpenRgb => on_path("openrgb"),
        Helper::HeadsetControl => on_path("headsetcontrol"),
        Helper::Ratbag => crate::ratbag::installed(),
    };
    let running = match h {
        Helper::OpenLinkHub => crate::corsair::running(),
        Helper::OpenRgb => crate::openrgb::running(),
        // ratbagd starts through D-Bus when asked.
        Helper::HeadsetControl | Helper::Ratbag => installed,
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
        // The package has a different name in each distro.
        (Helper::Ratbag, _) => install("libratbag", &["pacman"]).or_else(|| install("ratbagd", &["apt", "zypper"])).or_else(|| install("libratbag-ratbagd", &["dnf"])).map(Fix::Terminal).unwrap_or_else(page),
    })
}

fn group(found: &[Usb], supported: Option<&[(String, String)]>) -> Vec<(Helper, Vec<String>)> {
    let mut out: Vec<(Helper, Vec<String>)> = Vec::new();
    for u in found {
        for h in helpers_for(u, supported) {
            match out.iter_mut().find(|(x, _)| *x == h) {
                Some((_, names)) if !names.contains(&u.name) => names.push(u.name.clone()),
                Some(_) => {}
                None => out.push((h, vec![u.name.clone()])),
            }
        }
    }
    out
}

fn need(helper: Helper, devices: Vec<String>) -> Need {
    let state = state(helper);
    Need { fix: fix(helper, &state), helper, devices, state }
}

/// HeadsetControl's own list, asked only when its module is on: it can take seconds to answer.
fn supported_if(on: bool) -> Option<Vec<(String, String)>> {
    on.then(crate::headset::supported).flatten()
}

/// What the connected peripherals need, one entry per helper.
pub fn needs(headset_on: bool) -> Vec<Need> {
    let supported = supported_if(headset_on);
    group(&usb_hid(Path::new("/sys/bus/usb/devices")), supported.as_deref()).into_iter().map(|(helper, devices)| need(helper, devices)).collect()
}

/// Every helper, with the connected devices that need it (none, for many).
pub fn all(headset_on: bool) -> Vec<Need> {
    let supported = supported_if(headset_on);
    let mut found = group(&usb_hid(Path::new("/sys/bus/usb/devices")), supported.as_deref());
    [Helper::OpenLinkHub, Helper::OpenRgb, Helper::HeadsetControl, Helper::Ratbag]
        .into_iter()
        .map(|h| need(h, found.iter_mut().find(|(x, _)| *x == h).map(|(_, d)| std::mem::take(d)).unwrap_or_default()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(vendor: &str, id: &str, name: &str) -> Usb {
        Usb { vendor: vendor.into(), id: id.into(), name: name.into(), mouse: false }
    }

    fn mouse(vendor: &str, id: &str, name: &str) -> Usb {
        Usb { mouse: true, ..usb(vendor, id, name) }
    }

    fn sample() -> Vec<Usb> {
        vec![
            usb("1b1c", "1b8c", "CORSAIR SLIPSTREAM WIRELESS USB Receiver"),
            usb("1b1c", "1b8c", "CORSAIR SLIPSTREAM WIRELESS USB Receiver"),
            usb("1b1c", "2a02", "CORSAIR VIRTUOSO MAX WIRELESS Gaming Receiver"),
            usb("046d", "c539", "G502 LIGHTSPEED"),
            usb("046d", "0aba", "PRO X Wireless Gaming Headset"),
            usb("0db0", "0076", "MYSTIC LIGHT"),
            usb("0fd9", "0084", "Stream Deck Plus"),
        ]
    }

    #[test]
    fn groups_devices_by_helper() {
        let g = group(&sample(), None);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0], (Helper::OpenLinkHub, vec!["CORSAIR SLIPSTREAM WIRELESS USB Receiver".into(), "CORSAIR VIRTUOSO MAX WIRELESS Gaming Receiver".into()]));
        assert_eq!(g[1], (Helper::OpenRgb, vec!["G502 LIGHTSPEED".into(), "MYSTIC LIGHT".into()]));
        assert_eq!(g[2], (Helper::HeadsetControl, vec!["PRO X Wireless Gaming Headset".into()]));
    }

    #[test]
    fn installed_headsetcontrol_decides_what_is_a_headset() {
        // A headset whose name has no known word, and a mouse whose name has one.
        let found = [usb("1038", "12ad", "Odd Buds"), usb("046d", "c08b", "Cloud Mouse")];
        let listed = vec![("1038".to_string(), "12ad".to_string())];
        let g = group(&found, Some(&listed));
        assert_eq!(g, vec![(Helper::HeadsetControl, vec!["Odd Buds".into()]), (Helper::OpenRgb, vec!["Cloud Mouse".into()])]);
        let g = group(&found, None);
        assert_eq!(g, vec![(Helper::OpenRgb, vec!["Odd Buds".into()]), (Helper::HeadsetControl, vec!["Cloud Mouse".into()])]);
    }

    #[test]
    fn no_corsair_vendor_no_openlinkhub() {
        let found: Vec<Usb> = sample().into_iter().filter(|u| u.vendor != "1b1c").collect();
        assert!(group(&found, None).iter().all(|(h, _)| *h != Helper::OpenLinkHub));
    }

    #[test]
    fn ratbagd_is_suggested_only_for_mice_of_its_brands() {
        let found = [mouse("046d", "c08b", "G502 HERO"), mouse("1038", "1824", "Rival 600"), mouse("1e7d", "2e4a", "Kone Pure"), mouse("1b1c", "1b34", "DARK CORE RGB PRO"), mouse("1532", "0043", "Viper"), usb("046d", "c33a", "G413 Keyboard")];
        let g = group(&found, None);
        let ratbag = g.iter().find(|(h, _)| *h == Helper::Ratbag).map(|(_, d)| d.clone()).unwrap_or_default();
        assert_eq!(ratbag, vec!["G502 HERO".to_string(), "Rival 600".into(), "Kone Pure".into()]);
        // Corsair stays with OpenLinkHub alone.
        assert!(g.iter().any(|(h, d)| *h == Helper::OpenLinkHub && d == &vec!["DARK CORE RGB PRO".to_string()]));
    }

    #[test]
    fn ready_needs_no_fix() {
        assert_eq!(fix(Helper::OpenRgb, &State::Ready), None);
        assert_eq!(fix(Helper::OpenLinkHub, &State::Stopped), Some(Fix::Terminal("sudo systemctl enable --now openlinkhub".into())));
    }
}
