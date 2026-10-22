//! Plugin catalogues: OpenAction (free plugins, many made for Linux) and the archive of Elgato's
//! store (mostly Windows plugins that need Wine). Downloads go through curl and unpack with bsdtar.
use crate::i18n::{t, tf};
use crate::plugins::{self, Plugin};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

const OPENACTION: &str = "https://openactionapi.github.io/plugins/catalogue.json";
const OPENACTION_ICONS: &str = "https://openactionapi.github.io/plugins/icons/";
const ELGATO: &str = "https://plugins.amankhanna.me/catalogue.json";
const ELGATO_ZIPS: &str = "https://plugins.amankhanna.me/rezipped/";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    OpenAction,
    Elgato,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub author: String,
    pub description: String,
    /// GitHub repository whose latest release holds the plugin, or a direct download.
    pub repository: String,
    pub download: String,
    pub icon: String,
}

impl Entry {
    /// Says it targets Linux or OpenDeck; others may still run, but these are the safe bets.
    pub fn made_for_linux(&self) -> bool {
        let text = format!("{} {}", self.name, self.description).to_lowercase();
        ["linux", "opendeck", "openaction", "pipewire", "mpris", "wayland"].iter().any(|w| text.contains(w))
    }
}

pub fn cache() -> PathBuf {
    crate::paths::cache()
}

/// curl into a file; true on HTTP success.
pub fn fetch(url: &str, to: &Path) -> bool {
    Command::new("curl").args(["-sfL", "--max-time", "120", "-H", "User-Agent: fluxdeck", "-o"]).arg(to).arg(url).status().is_ok_and(|s| s.success())
}

fn fetch_json(url: &str, file: &str, max_age: Duration) -> Option<Value> {
    let path = cache().join(file);
    let fresh = std::fs::metadata(&path).and_then(|m| m.modified()).ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|age| age < max_age);
    if !fresh {
        let _ = std::fs::create_dir_all(cache());
        let tmp = path.with_extension("part");
        if fetch(url, &tmp) {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()
}

pub fn load(source: Source) -> Result<Vec<Entry>, String> {
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let day = Duration::from_secs(86400);
    let mut out: Vec<Entry> = match source {
        Source::OpenAction => {
            let v = fetch_json(OPENACTION, "openaction.json", day).ok_or_else(|| t("No se pudo descargar el catálogo.").to_string())?;
            v.as_object()
                .into_iter()
                .flatten()
                .map(|(id, p)| Entry {
                    id: id.clone(),
                    name: s(p, "name"),
                    author: s(p, "author"),
                    description: s(p, "description"),
                    repository: s(p, "repository"),
                    download: s(p, "download_url"),
                    icon: format!("{OPENACTION_ICONS}{id}.png"),
                })
                .collect()
        }
        Source::Elgato => {
            let v = fetch_json(ELGATO, "elgato.json", day).ok_or_else(|| t("No se pudo descargar el catálogo.").to_string())?;
            v.as_array()
                .into_iter()
                .flatten()
                .map(|p| Entry {
                    id: s(p, "id"),
                    name: s(p, "name"),
                    author: s(p, "author"),
                    description: String::new(),
                    repository: String::new(),
                    download: format!("{ELGATO_ZIPS}{}.zip", s(p, "id")),
                    icon: s(p, "icon"),
                })
                .collect()
        }
    };
    out.sort_by_key(|e| (!e.made_for_linux(), e.name.to_lowercase()));
    Ok(out)
}

/// The release file to download: a direct link, or the best asset of the repository's latest GitHub release.
fn asset_url(e: &Entry) -> Result<String, String> {
    if !e.download.is_empty() {
        return Ok(e.download.clone());
    }
    let repo = e.repository.trim_end_matches('/').strip_prefix("https://github.com/").ok_or_else(|| tf("No sé descargar desde {}", &[&e.repository]))?;
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let tmp = cache().join("release.json");
    if !fetch(&api, &tmp) {
        return Err(t("Ese plugin no tiene versiones publicadas.").into());
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&tmp).unwrap_or_default()).unwrap_or_default();
    let assets: Vec<(String, String)> = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| (a["name"].as_str().unwrap_or_default().to_lowercase(), a["browser_download_url"].as_str().unwrap_or_default().to_string()))
        .filter(|(n, _)| n.ends_with(".streamdeckplugin") || n.ends_with(".zip"))
        .collect();
    // Prefer a Linux build when the release has one per system.
    let rank = |n: &str| if n.contains("linux") { 0 } else if n.contains("win") || n.contains("mac") || n.contains("darwin") { 2 } else { 1 };
    assets.iter().min_by_key(|(n, _)| rank(n)).map(|(_, u)| u.clone()).ok_or_else(|| t("La versión publicada no trae un plugin para instalar.").into())
}

/// Downloads and unpacks a plugin into plugins::dir(); returns the installed plugin.
pub fn install(e: &Entry) -> Result<Plugin, String> {
    let url = asset_url(e)?;
    // Unpacked next to the plugins folder so the final move never crosses filesystems.
    let base = plugins::dir();
    let work = base.parent().unwrap_or(&base).join(format!(".install-{}", plugins::new_instance()));
    let _ = std::fs::create_dir_all(&work);
    let result = (|| {
        let archive = work.join("plugin.zip");
        if !fetch(&url, &archive) {
            return Err(t("No se pudo descargar el plugin.").to_string());
        }
        let out = work.join("x");
        let _ = std::fs::create_dir_all(&out);
        let ok = Command::new("bsdtar").arg("-xf").arg(&archive).arg("-C").arg(&out).status().is_ok_and(|s| s.success());
        if !ok {
            return Err(t("No se pudo abrir el archivo del plugin.").into());
        }
        let folder = find_plugin(&out).ok_or_else(|| t("El archivo no contiene un plugin.").to_string())?;
        let name = folder.file_name().and_then(|n| n.to_str()).map(String::from).filter(|n| n.ends_with(".sdPlugin")).unwrap_or(format!("{}.sdPlugin", e.id));
        let dest = plugins::dir().join(name);
        let _ = std::fs::create_dir_all(plugins::dir());
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        std::fs::rename(&folder, &dest).map_err(|err| err.to_string())?;
        plugins::read(&dest)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// The folder holding manifest.json, at most a few levels down.
fn find_plugin(dir: &Path) -> Option<PathBuf> {
    if dir.join("manifest.json").is_file() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_dir()).find_map(|e| find_plugin(&e.path()))
}

