//! Backups of the whole setup in one file: the config (pages, keys, dials, groups, profiles, theme),
//! the microphone effects, group volumes and outputs, the stream mix, and the icons keys use.
//! Plugin settings go in only when asked, since they can hold sign-in tokens. Installed plugins are
//! listed, not copied: they can be downloaded again.
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const FORMAT: u64 = 1;
const CONFIG_FILES: [&str; 3] = ["config.toml", "effects.toml", "theme.toml"];
const PLUGIN_SETTINGS: &str = "plugin-settings.json";
const STATE_FILES: [&str; 3] = ["groups.json", "group-outputs.json", "stream-mix.json"];

/// Where exports go: `~/Documentos/FluxDeck` (the documents folder, in any language).
pub fn folder() -> PathBuf {
    let docs = Command::new("xdg-user-dir").arg("DOCUMENTS").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|d| !d.is_empty());
    docs.map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()).join("FluxDeck")
}

/// Copies of what was there before each import.
fn previous_folder() -> PathBuf {
    crate::paths::data().join("respaldos")
}

fn icons() -> PathBuf {
    crate::paths::data().join("icons")
}

/// Backups in the export folder, newest first.
pub fn list() -> Vec<PathBuf> {
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(folder())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".tar.gz"))
        .filter_map(|p| Some((std::fs::metadata(&p).and_then(|m| m.modified()).ok()?, p)))
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0));
    all.into_iter().map(|(_, p)| p).collect()
}

/// The error naming a program that is not installed, if that is what failed.
fn missing(e: &std::io::Error, program: &str) -> Option<String> {
    (e.kind() == std::io::ErrorKind::NotFound).then(|| crate::i18n::tf("Falta el programa {}.", &[program]))
}

fn stamp() -> Result<String, String> {
    match Command::new("date").arg("+%Y-%m-%d-%H%M%S").output() {
        Ok(o) => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Err(e) => missing(&e, "date").map_or(Ok("copia".into()), Err),
    }
}

/// Writes a backup into the export folder; returns the file.
pub fn export(plugin_settings: bool) -> Result<PathBuf, String> {
    write(&folder(), "fluxdeck", plugin_settings)
}

fn write(dir: &Path, name: &str, plugin_settings: bool) -> Result<PathBuf, String> {
    let staging = crate::paths::runtime(&format!("backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let result = (|| {
        let copy = |from: PathBuf, to: PathBuf| -> Result<(), String> {
            if from.is_file() {
                std::fs::create_dir_all(to.parent().unwrap_or(&staging)).map_err(|e| e.to_string())?;
                std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
            }
            Ok(())
        };
        for f in CONFIG_FILES.iter().chain(plugin_settings.then_some(&PLUGIN_SETTINGS)) {
            copy(crate::paths::config().join(f), staging.join("config").join(f))?;
        }
        for f in STATE_FILES {
            copy(crate::paths::state().join(f), staging.join("state").join(f))?;
        }
        copy_tree(&icons(), &staging.join("icons"))?;
        let plugins: Vec<Value> = crate::plugins::installed().iter().map(|p| json!({ "uuid": p.uuid, "name": p.manifest.name })).collect();
        let manifest = json!({ "format": FORMAT, "version": env!("CARGO_PKG_VERSION"), "data": crate::paths::data(), "plugins": plugins });
        std::fs::write(staging.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap_or_default()).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{name}-{}.tar.gz", stamp()?));
        let ok = match Command::new("tar").arg("czf").arg(&file).arg("-C").arg(&staging).arg(".").status() {
            Ok(s) => s.success(),
            Err(e) => return Err(missing(&e, "tar").unwrap_or_else(|| e.to_string())),
        };
        // The backup can hold keys: only its owner reads it.
        let _ = std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600));
        if ok { Ok(file) } else { Err("tar could not write the backup".into()) }
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        // A link in a backup could point anywhere on this computer: links are never followed.
        if entry.file_type().is_ok_and(|t| t.is_symlink()) {
            continue;
        }
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else if src.is_file() {
            std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
            std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub struct Imported {
    /// A backup of what there was before, to go back to.
    pub previous: PathBuf,
    /// Plugins the backup's keys used that are not installed here.
    pub missing_plugins: Vec<String>,
}

/// Replaces the setup with a backup's, after saving the current one. Nothing is touched unless the
/// file is a FluxDeck backup whose config reads.
pub fn import(file: &Path) -> Result<Imported, String> {
    let tmp = crate::paths::runtime(&format!("restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = restore(file, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn restore(file: &Path, tmp: &Path) -> Result<Imported, String> {
    // GNU tar refuses member names with "..", and only the known files below are read anyway.
    let ok = match Command::new("tar").arg("xzf").arg(file).arg("-C").arg(tmp).arg("--no-same-owner").stderr(std::process::Stdio::null()).status() {
        Ok(s) => s.success(),
        Err(e) => return Err(missing(&e, "tar").unwrap_or_else(|| e.to_string())),
    };
    if !ok {
        return Err(crate::i18n::t("No se pudo abrir el archivo.").into());
    }
    let manifest: Value = std::fs::read_to_string(tmp.join("manifest.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or(crate::i18n::t("No es un respaldo de FluxDeck."))?;
    if manifest["format"].as_u64() != Some(FORMAT) {
        return Err(crate::i18n::t("Este respaldo es de otra versión de FluxDeck.").into());
    }
    // Icons were saved under the other machine's data folder: point keys at this one's.
    let config = std::fs::read_to_string(tmp.join("config/config.toml")).map_err(|_| crate::i18n::t("El respaldo no trae configuración."))?;
    let config = match manifest["data"].as_str() {
        Some(old) if !old.is_empty() => config.replace(old, &crate::paths::data().to_string_lossy()),
        _ => config,
    };
    toml::from_str::<crate::Config>(&config).map_err(|e| crate::i18n::tf("La configuración del respaldo tiene un error: {}", &[&e.to_string()]))?;

    let previous = write(&previous_folder(), "antes-de-importar", true)?;
    let put = |from: PathBuf, to: PathBuf| -> Result<(), String> {
        if from.is_file() {
            std::fs::create_dir_all(to.parent().unwrap_or(Path::new("/"))).map_err(|e| e.to_string())?;
            let text = std::fs::read_to_string(&from).map_err(|e| e.to_string())?;
            crate::paths::write_private(&to, &text).map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    for f in ["effects.toml", "theme.toml", PLUGIN_SETTINGS] {
        put(tmp.join("config").join(f), crate::paths::config().join(f))?;
    }
    for f in STATE_FILES {
        put(tmp.join("state").join(f), crate::paths::state().join(f))?;
    }
    copy_tree(&tmp.join("icons"), &icons())?;
    // The config goes last: the service reloads when it changes, and finds everything else in place.
    std::fs::create_dir_all(crate::paths::config()).map_err(|e| e.to_string())?;
    // Through a temp file, at the link's target when the config is symlinked (as Config::save does).
    let target = std::fs::canonicalize(crate::config::path()).unwrap_or_else(|_| crate::config::path());
    crate::paths::write_private(&target, &config).map_err(|e| e.to_string())?;

    let here: Vec<String> = crate::plugins::installed().into_iter().map(|p| p.uuid).collect();
    let missing_plugins = manifest["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !here.iter().any(|h| p["uuid"].as_str() == Some(h)))
        .map(|p| p["name"].as_str().or(p["uuid"].as_str()).unwrap_or_default().to_string())
        .collect();
    Ok(Imported { previous, missing_plugins })
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test sets the XDG folders for the whole process, so nothing here touches the real ones.
    #[test]
    fn round_trip_and_bad_files() {
        let _env = crate::paths::tests::ENV.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("fluxdeck-backup-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (var, dir) in [("XDG_CONFIG_HOME", "c"), ("XDG_DATA_HOME", "d"), ("XDG_STATE_HOME", "s"), ("XDG_RUNTIME_DIR", "r")] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            unsafe { std::env::set_var(var, root.join(dir)) };
        }
        let cfg = crate::config::path();
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();

        // A broken config is an error, not a default.
        std::fs::write(&cfg, "pages = [[[").unwrap();
        assert!(crate::Config::load().is_err());

        let text = "brightness = 42\n";
        std::fs::write(&cfg, text).unwrap();
        let file = write(&root.join("out"), "t", true).unwrap();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777, 0o600);
        std::fs::write(&cfg, "brightness = 7\n").unwrap();
        import(&file).unwrap();
        assert_eq!(crate::Config::load().unwrap().brightness, 42);

        let bad = root.join("bad.tar.gz");
        std::fs::write(&bad, "not a tar").unwrap();
        let before = std::fs::read_to_string(&cfg).unwrap();
        assert!(import(&bad).is_err());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before);
        let _ = std::fs::remove_dir_all(&root);
    }
}
