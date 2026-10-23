//! F13–F24 from any keyboard run the sequence set for them in `extra_keys`. Keyboards are read, not
//! grabbed, so the key still reaches the desktop (where nothing uses it by default).
use crate::Msg;
use fluxdeck_core::Config;
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KEY_F13: u16 = 183;
const KEY_F24: u16 = 194;

/// True when a device's key bitmap (hex words, most significant first) has any of F13–F24.
fn has_extra_keys(bitmap: &str) -> bool {
    let words: Vec<u64> = bitmap.split_whitespace().rev().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect();
    (KEY_F13..=KEY_F24).any(|code| words.get(code as usize / 64).is_some_and(|w| w >> (code % 64) & 1 == 1))
}

pub fn start(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let reading: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
        loop {
            // New keyboards (or OpenLinkHub's virtual one) can show up at any time.
            for entry in std::fs::read_dir("/sys/class/input").into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with("event") {
                    continue;
                }
                let bitmap = std::fs::read_to_string(entry.path().join("device/capabilities/key")).unwrap_or_default();
                let dev = PathBuf::from("/dev/input").join(&name);
                if !has_extra_keys(&bitmap) || reading.lock().unwrap().contains(&dev) {
                    continue;
                }
                let Ok(mut file) = std::fs::File::open(&dev) else { continue };
                reading.lock().unwrap().insert(dev.clone());
                let (tx, reading) = (tx.clone(), reading.clone());
                std::thread::spawn(move || {
                    // struct input_event: 16 bytes of time, then type, code and value.
                    let mut ev = [0u8; 24];
                    while file.read_exact(&mut ev).is_ok() {
                        let (kind, code) = (u16::from_ne_bytes([ev[16], ev[17]]), u16::from_ne_bytes([ev[18], ev[19]]));
                        let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
                        if kind == 1 && value == 1 && (KEY_F13..=KEY_F24).contains(&code) {
                            let name = format!("F{}", code - KEY_F13 + 13);
                            if let Some(steps) = Config::load().ok().and_then(|c| c.extra_keys.get(&name).cloned()).filter(|s| !s.is_empty()) {
                                crate::run_steps(steps, tx.clone());
                            }
                        }
                    }
                    reading.lock().unwrap().remove(&dev);
                });
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_f13_in_a_key_bitmap() {
        // Bit 183 is in the third 64-bit word: 183 - 128 = 55.
        let with = format!("{:x} 0 0", 1u64 << 55);
        assert!(super::has_extra_keys(&with));
        assert!(!super::has_extra_keys("ffffffff 0 0"));
    }
}
