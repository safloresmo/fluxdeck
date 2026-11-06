//! The deck on a phone: a page on the local network that shows the deck and takes taps. Off until
//! switched on; every request must carry the secret key the QR code holds, since keys run commands.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

pub const PORT: u16 = 27080;

/// The deck shapes a phone can take, by id: (id, name, columns, rows, dials).
pub const MODELS: [(&str, &str, u8, u8, u8); 5] = [
    ("plus", "Stream Deck +", 4, 2, 4),
    ("mini", "Stream Deck Mini", 3, 2, 0),
    ("mk2", "Stream Deck", 5, 3, 0),
    ("xl", "Stream Deck XL", 8, 4, 0),
    ("neo", "Stream Deck Neo", 4, 2, 0),
];

/// What a phone shows.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The same as the Stream Deck plugged in.
    #[default]
    Mirror,
    /// Pages of its own, beside the Stream Deck.
    Extend,
    /// The only deck there is.
    Alone,
}

/// One phone or tablet, with its own QR code.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Phone {
    pub id: String,
    pub name: String,
    pub token: String,
    /// One of `MODELS`.
    pub model: String,
    pub mode: Mode,
}

impl Default for Phone {
    fn default() -> Self {
        Phone { id: String::new(), name: String::new(), token: new_token(), model: "plus".into(), mode: Mode::Mirror }
    }
}

impl Phone {
    /// The deck this phone shows: the hardware's for a mirror, its own otherwise.
    pub fn view(&self) -> &str {
        if self.mode == Mode::Mirror { MAIN } else { &self.id }
    }

    /// (columns, rows, dials) of its model.
    pub fn shape(&self) -> (u8, u8, u8) {
        MODELS.iter().find(|m| m.0 == self.model).map_or((4, 2, 4), |m| (m.2, m.3, m.4))
    }
}

/// The view of the Stream Deck plugged in, which mirrors show.
pub const MAIN: &str = "main";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Remote {
    pub on: bool,
    /// The port in use; another is taken when this one is busy.
    pub port: u16,
    pub phones: Vec<Phone>,
    /// The single key of the first version, moved into a phone on load.
    #[serde(skip_serializing)]
    token: String,
}

impl Default for Remote {
    fn default() -> Self {
        Remote { on: false, port: PORT, phones: Vec::new(), token: String::new() }
    }
}

fn path() -> PathBuf {
    crate::paths::state().join("remote.json")
}

/// Whether the deck on the phone is switched on; reads only, so nothing is made while it is off.
pub fn enabled() -> bool {
    std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str::<Remote>(&t).ok()).is_some_and(|r| r.on)
}

impl Remote {
    /// The settings, with a first phone (keeping an older single key, so its QR still works).
    pub fn load() -> Remote {
        crate::paths::keep_private(&path());
        let mut r: Remote = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if r.settle() {
            r.save();
        }
        r
    }

    /// Fills in what is missing; true when anything changed.
    fn settle(&mut self) -> bool {
        let mut changed = false;
        if self.phones.is_empty() {
            let mut first = Phone { id: "celular".into(), name: crate::i18n::t("Mi celular").into(), ..Default::default() };
            if self.token.len() >= 32 {
                first.token = std::mem::take(&mut self.token);
            }
            self.phones.push(first);
            changed = true;
        }
        for p in &mut self.phones {
            if p.token.len() < 32 {
                p.token = new_token();
                changed = true;
            }
        }
        changed
    }

    /// Saved readable only by its owner: it holds the phones' keys.
    pub fn save(&self) {
        let _ = crate::paths::write_private(&path(), &serde_json::to_string_pretty(self).unwrap_or_default());
    }

    /// The phone a key belongs to.
    pub fn phone(&self, token: &str) -> Option<&Phone> {
        self.phones.iter().find(|p| same_key(&p.token, token))
    }

    /// The address a phone's QR code opens: this computer on the local network, with its key.
    pub fn url(&self, phone: &Phone) -> Option<String> {
        Some(format!("http://{}:{}/?k={}", lan_ip()?, self.port, phone.token))
    }

    /// A fresh id for a new phone.
    pub fn new_id(&self) -> String {
        (1..).map(|n| format!("tel{n}")).find(|id| !self.phones.iter().any(|p| &p.id == id)).unwrap_or_default()
    }
}

/// 32 hex digits from the kernel's random source.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    // A key of zeros would let anyone in; better to stop than hand one out.
    std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes)).expect("cannot read /dev/urandom");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// This computer's address on the local network: the one it uses to reach the internet.
pub fn lan_ip() -> Option<String> {
    let out = Command::new("ip").args(["-j", "-4", "route", "get", "1.1.1.1"]).output().ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v[0]["prefsrc"].as_str().map(String::from)
}

/// Whether a peer may use the deck: only addresses of local and private networks (home, VPN),
/// never the open internet.
pub fn local_peer(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            let [a, b, ..] = v.octets();
            v.is_loopback() || v.is_private() || v.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        std::net::IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Compares keys without stopping at the first difference.
pub fn same_key(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_long_and_compared_whole() {
        let t = new_token();
        assert_eq!(t.len(), 32);
        assert_ne!(t, new_token());
        assert!(same_key(&t, &t.clone()));
        assert!(!same_key(&t, &t[..31]));
        assert!(!same_key("abc", "abd"));
    }

    #[test]
    fn an_old_single_key_becomes_the_first_phone() {
        let mut r: Remote = serde_json::from_str(r#"{"on": true, "token": "0123456789abcdef0123456789abcdef", "port": 27081}"#).unwrap();
        assert!(r.settle());
        assert_eq!(r.phones.len(), 1);
        assert_eq!(r.phones[0].token, "0123456789abcdef0123456789abcdef");
        assert_eq!((r.phones[0].view(), r.phones[0].shape()), (MAIN, (4, 2, 4)));
        assert!(r.phone("0123456789abcdef0123456789abcdef").is_some() && r.phone("x").is_none());
        assert!(!serde_json::to_string(&r).unwrap().contains("\"token\": \"0123"), "the old field is not written back");
        let tablet = Phone { id: "tel1".into(), mode: Mode::Extend, model: "xl".into(), ..Default::default() };
        assert_eq!((tablet.view(), tablet.shape()), ("tel1", (8, 4, 0)));
    }

    #[test]
    fn only_local_peers() {
        for ip in ["192.168.1.30", "10.0.0.5", "172.16.4.1", "127.0.0.1", "100.101.2.3", "fd12::1"] {
            assert!(local_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2001:4860::1"] {
            assert!(!local_peer(ip.parse().unwrap()), "{ip}");
        }
    }
}
