//! The deck on a phone: a page on the local network that shows the deck and takes taps. Off until
//! switched on; every request must carry the secret key the QR code holds, since keys run commands.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

pub const PORT: u16 = 27080;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Remote {
    pub on: bool,
    pub token: String,
    /// The port in use; another is taken when this one is busy.
    pub port: u16,
}

impl Default for Remote {
    fn default() -> Self {
        Remote { on: false, token: String::new(), port: PORT }
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
    /// The settings, with a key made on first use.
    pub fn load() -> Remote {
        let mut r: Remote = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if r.token.len() < 32 {
            r.token = new_token();
            r.save();
        }
        r
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::paths::state());
        let tmp = path().with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(tmp, path());
        }
    }

    /// The address the QR code opens: this computer on the local network, with the key.
    pub fn url(&self) -> Option<String> {
        Some(format!("http://{}:{}/?k={}", lan_ip()?, self.port, self.token))
    }
}

/// 32 hex digits from the kernel's random source.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = std::io::Read::read_exact(&mut f, &mut bytes);
    }
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
    fn only_local_peers() {
        for ip in ["192.168.1.30", "10.0.0.5", "172.16.4.1", "127.0.0.1", "100.101.2.3", "fd12::1"] {
            assert!(local_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2001:4860::1"] {
            assert!(!local_peer(ip.parse().unwrap()), "{ip}");
        }
    }
}
