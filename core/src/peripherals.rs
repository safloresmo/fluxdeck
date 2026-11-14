//! Every peripheral as one entry, with what each source (OpenLinkHub, OpenRGB, HeadsetControl, ratbagd, UPower)
//! knows about it.
use crate::{corsair, headset, openrgb, ratbag, upower};

#[derive(Debug, Clone, Default)]
pub struct Peripheral {
    /// The name its first source gives it.
    pub name: String,
    /// 0 keyboard, 1 mouse, 2 headset, 3 screen (see corsair::icon).
    pub kind: Option<u64>,
    pub corsair: Option<corsair::Device>,
    pub rgb: Option<openrgb::Device>,
    pub headset: Option<headset::Headset>,
    pub mouse: Option<ratbag::Mouse>,
    pub battery: Option<upower::Battery>,
}

impl Peripheral {
    /// A device without a battery never gets the battery glyph.
    pub fn icon(&self) -> char {
        match (self.kind, self.battery.is_some() || self.headset.as_ref().is_some_and(|h| h.battery.is_some())) {
            (Some(k), _) => corsair::icon(Some(k)),
            (None, true) => corsair::icon(None),
            (None, false) => '\u{f0335}',
        }
    }
}

const BRANDS: [&str; 5] = ["corsair", "logitech", "razer", "steelseries", "hyperx"];

/// Generic words that differ between sources for the same device.
const GENERIC: [&str; 7] = ["wireless", "gaming", "receiver", "usb", "mouse", "keyboard", "headset"];

/// The model words of a name, in order-free form, without brand or generic words.
fn key(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !BRANDS.contains(w) && !GENERIC.contains(w)).collect();
    words.sort_unstable();
    words.join(" ")
}

/// The brand a name states, if any.
fn brand(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    BRANDS.into_iter().find(|b| lower.split(|c: char| !c.is_alphanumeric()).any(|w| w == *b))
}

/// Same model words, and no two different brands: "Razer Viper" and "Logitech Viper" are two devices.
fn same(a: &str, b: &str) -> bool {
    key(a) == key(b) && !matches!((brand(a), brand(b)), (Some(x), Some(y)) if x != y)
}

/// Puts `item` on the one peripheral with its name, or adds a new one. Only a match on a name
/// that is unique on both sides merges: with any doubt there are two entries.
fn attach<T: Clone>(out: &mut Vec<Peripheral>, items: &[T], name: impl Fn(&T) -> &str, set: impl Fn(&mut Peripheral, T)) {
    for item in items {
        let n = name(item);
        let twins = items.iter().filter(|i| same(name(i), n)).count();
        let hits: Vec<usize> = out.iter().enumerate().filter(|(_, p)| same(&p.name, n)).map(|(i, _)| i).collect();
        match hits[..] {
            [i] if twins == 1 && !key(n).is_empty() => set(&mut out[i], item.clone()),
            _ => {
                out.push(Peripheral { name: n.to_string(), ..Default::default() });
                set(out.last_mut().expect("just pushed"), item.clone());
            }
        }
    }
}

/// Joins what the five sources report; OpenLinkHub's devices come first.
pub fn merge(corsair: &[corsair::Device], rgb: &[openrgb::Device], headsets: &[headset::Headset], mice: &[ratbag::Mouse], batteries: &[upower::Battery]) -> Vec<Peripheral> {
    let mut out: Vec<Peripheral> = corsair.iter().map(|d| Peripheral { name: d.product.clone(), kind: d.kind, corsair: Some(d.clone()), ..Default::default() }).collect();
    attach(&mut out, rgb, |d| &d.name, |p, d| p.rgb = Some(d));
    attach(&mut out, headsets, |h| &h.product, |p, h| {
        p.kind = p.kind.or(Some(2));
        p.headset = Some(h);
    });
    attach(&mut out, mice, |m| &m.name, |p, m| {
        p.kind = p.kind.or(Some(1));
        p.mouse = Some(m);
    });
    attach(&mut out, batteries, |b| &b.model, |p, b| {
        p.kind = p.kind.or(b.kind_code());
        p.battery = Some(b);
    });
    out
}

/// What each enabled source sees now, as one list.
pub fn peripherals(modules: &[String]) -> Vec<Peripheral> {
    let on = |m: &str| modules.iter().any(|x| x == m);
    let corsair = if on("corsair") { corsair::devices() } else { Vec::new() };
    let rgb = if on("openrgb") { openrgb::devices() } else { Vec::new() };
    let headsets = if on("headset") { headset::headsets() } else { Vec::new() };
    let mice = if on("mouse") { ratbag::mice() } else { Vec::new() };
    merge(&corsair, &rgb, &headsets, &mice, &upower::batteries())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(name: &str) -> openrgb::Device {
        let mut d = openrgb::Device::default();
        d.id = name.into();
        d.name = name.into();
        d
    }

    fn bat(model: &str, kind: &str) -> upower::Battery {
        upower::Battery { model: model.into(), kind: kind.into(), level: 60 }
    }

    #[test]
    fn same_mouse_from_two_sources_is_one_entry() {
        let p = merge(&[], &[rgb("Logitech G502 LIGHTSPEED Wireless Gaming Mouse")], &[], &[], &[bat("G502 LIGHTSPEED Wireless Gaming Mouse", "mouse")]);
        assert_eq!(p.len(), 1);
        assert!(p[0].rgb.is_some() && p[0].battery.is_some());
        assert_eq!(p[0].kind, Some(1));
    }

    #[test]
    fn similar_names_stay_apart() {
        let p = merge(&[], &[rgb("Logitech G502 HERO")], &[], &[], &[bat("G502 LIGHTSPEED Wireless Gaming Mouse", "mouse")]);
        assert_eq!(p.len(), 2);
        // Two identical devices on one side are never matched to the other side.
        let p = merge(&[], &[rgb("Razer Viper"), rgb("Razer Viper")], &[], &[], &[bat("Razer Viper", "mouse")]);
        assert_eq!(p.len(), 3);
        let p = merge(&[], &[rgb("Razer Viper")], &[], &[], &[bat("Razer Viper Ultimate", "mouse")]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn corsair_mouse_names_from_two_sources_merge() {
        let d = corsair::Device { serial: "S".into(), product: "DARK CORE RGB PRO".into(), battery: false, kind: Some(1), anc: false, lights: true, lcd: false };
        let p = merge(&[d], &[rgb("Corsair Dark Core RGB PRO Gaming Mouse")], &[], &[], &[bat("Corsair Dark Core RGB PRO Gaming Mouse", "mouse")]);
        assert_eq!(p.len(), 1);
        assert!(p[0].rgb.is_some() && p[0].battery.is_some());
    }

    #[test]
    fn different_brands_never_merge() {
        let p = merge(&[], &[rgb("Razer Viper")], &[], &[], &[bat("Logitech Viper", "mouse")]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn corsair_headset_and_its_headsetcontrol_entry_merge() {
        let d = corsair::Device { serial: "S".into(), product: "VIRTUOSO MAX".into(), battery: true, kind: Some(2), anc: true, lights: true, lcd: false };
        let h = headset::Headset { product: "CORSAIR VIRTUOSO MAX WIRELESS Gaming Receiver".into(), has_sidetone: true, ..Default::default() };
        let p = merge(&[d], &[], &[h], &[], &[]);
        assert_eq!(p.len(), 1);
        assert!(p[0].corsair.is_some() && p[0].headset.is_some());
    }

    #[test]
    fn ratbagd_mouse_joins_the_same_mouse_from_other_sources() {
        let m = ratbag::Mouse { name: "Logitech G502 HERO".into(), ..Default::default() };
        let p = merge(&[], &[rgb("Logitech G502 HERO Gaming Mouse")], &[], std::slice::from_ref(&m), &[bat("G502 HERO", "mouse")]);
        assert_eq!(p.len(), 1);
        assert!(p[0].rgb.is_some() && p[0].mouse.is_some() && p[0].battery.is_some());
        assert_eq!(p[0].kind, Some(1));
        // A mouse ratbagd lists alone is its own card.
        assert_eq!(merge(&[], &[], &[], &[m], &[]).len(), 1);
    }

    #[test]
    fn no_battery_no_battery_glyph() {
        let screen = Peripheral { kind: Some(3), ..Default::default() };
        assert_ne!(screen.icon(), corsair::icon(None));
        assert_ne!(Peripheral::default().icon(), corsair::icon(None));
        assert_eq!(Peripheral { battery: Some(bat("X", "gaming-input")), ..Default::default() }.icon(), corsair::icon(None));
    }
}
