//! What the daemon shows right now, published for the editor's live preview.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct State {
    /// Empty when no deck is connected.
    pub model: String,
    pub cols: u8,
    pub rows: u8,
    pub dials: u8,
    pub page: String,
    /// Status lines by slot: "k3" for key 3, "d1" for dial 1.
    pub status: BTreeMap<String, String>,
}

pub fn path() -> PathBuf {
    crate::paths::runtime("state.toml")
}

impl State {
    pub fn read() -> Option<State> {
        toml::from_str(&fs::read_to_string(path()).ok()?).ok()
    }

    pub fn write(&self) {
        let tmp = path().with_extension("tmp");
        if let Ok(text) = toml::to_string(self)
            && fs::write(&tmp, text).is_ok()
        {
            let _ = fs::rename(tmp, path());
        }
    }

    pub fn clear() {
        let _ = fs::remove_file(path());
    }
}
