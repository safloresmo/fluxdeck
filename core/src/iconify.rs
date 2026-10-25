//! The icon store: Iconify's open icon sets (Material, Lucide, brand logos…), searched and fetched
//! from api.iconify.design. A chosen icon is saved as an SVG under the data folder, so keys keep it
//! offline. Single-colour icons are drawn in `currentColor`, which the deck paints in the theme's
//! text colour.
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const API: &str = "https://api.iconify.design";
/// Results shown for one search or pack.
pub const LIMIT: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub prefix: String,
    pub name: String,
    pub total: u64,
    pub license: String,
    pub category: String,
    /// Icons keep their own colours instead of following the theme.
    pub coloured: bool,
}

fn get(url: &str) -> Option<Value> {
    let out = Command::new("curl").args(["-sfL", "--max-time", "20", "-H", "User-Agent: fluxdeck"]).arg(url).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| serde_json::from_slice(&out.stdout).ok()).flatten()
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Every pack still maintained, the biggest first within each category.
pub fn packs() -> Vec<Pack> {
    let Some(v) = crate::catalogue::fetch_json(&format!("{API}/collections"), "iconify-packs.json", Duration::from_secs(7 * 86400)) else { return Vec::new() };
    packs_from(&v)
}

fn packs_from(v: &Value) -> Vec<Pack> {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let mut out: Vec<Pack> = v
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, p)| p["hidden"] != true && p["category"] != "Archive / Unmaintained")
        .map(|(prefix, p)| Pack {
            prefix: prefix.clone(),
            name: s(&p["name"]),
            total: p["total"].as_u64().unwrap_or(0),
            license: s(&p["license"]["title"]),
            category: s(&p["category"]),
            coloured: p["palette"] == true,
        })
        .collect();
    out.sort_by(|a, b| a.category.cmp(&b.category).then(b.total.cmp(&a.total)));
    out
}

/// Icons ("prefix:name") matching an English search, optionally within one pack.
pub fn search(query: &str, prefix: &str) -> Vec<String> {
    let mut url = format!("{API}/search?query={}&limit={LIMIT}", encode(query.trim()));
    if !prefix.is_empty() {
        url += &format!("&prefix={}", encode(prefix));
    }
    get(&url).map(|v| strings(&v["icons"])).unwrap_or_default()
}

/// The first icons of a pack, for browsing it without a search.
pub fn pack_icons(prefix: &str) -> Vec<String> {
    let Some(v) = get(&format!("{API}/collection?prefix={}", encode(prefix))) else { return Vec::new() };
    let mut names = strings(&v["uncategorized"]);
    for list in v["categories"].as_object().into_iter().flatten().map(|(_, l)| strings(l)) {
        names.extend(list);
    }
    names.sort();
    names.dedup();
    names.into_iter().take(LIMIT).map(|n| format!("{prefix}:{n}")).collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
}

/// The SVG of each icon ("prefix:name"), fetched a pack at a time; icons that fail are left out.
pub fn svgs(icons: &[String]) -> Vec<(String, String)> {
    let mut by_pack: Vec<(String, Vec<String>)> = Vec::new();
    for full in icons {
        let Some((prefix, name)) = full.split_once(':') else { continue };
        match by_pack.iter_mut().find(|(p, _)| p == prefix) {
            Some((_, names)) => names.push(name.to_string()),
            None => by_pack.push((prefix.to_string(), vec![name.to_string()])),
        }
    }
    let mut found: Vec<(String, String)> = Vec::new();
    for (prefix, names) in by_pack {
        // Short URLs: a few dozen names per request.
        for chunk in names.chunks(40) {
            let Some(v) = get(&format!("{API}/{}.json?icons={}", encode(&prefix), chunk.iter().map(|n| encode(n)).collect::<Vec<_>>().join(","))) else { continue };
            for name in chunk {
                if let Some(svg) = svg_of(&v, name) {
                    found.push((format!("{prefix}:{name}"), svg));
                }
            }
        }
    }
    // Back in the order the search ranked them.
    found.sort_by_key(|(full, _)| icons.iter().position(|i| i == full));
    found
}

/// A whole SVG from an icon set's JSON: the icon's body in a view box of its own size (or the
/// set's), following an alias to the icon it names.
fn svg_of(set: &Value, name: &str) -> Option<String> {
    let icon = match &set["icons"][name] {
        Value::Null => {
            let parent = set["aliases"][name]["parent"].as_str()?;
            &set["icons"][parent]
        }
        icon => icon,
    };
    let body = icon["body"].as_str()?;
    let num = |k: &str, default: f64| icon[k].as_f64().or_else(|| set[k].as_f64()).unwrap_or(default);
    let (left, top, w, h) = (num("left", 0.0), num("top", 0.0), num("width", 16.0), num("height", 16.0));
    Some(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{left} {top} {w} {h}" width="{w}" height="{h}">{body}</svg>"#))
}

/// Saves an icon where keys can use it; returns the file.
pub fn save(full: &str, svg: &str) -> std::io::Result<PathBuf> {
    let (prefix, name) = full.split_once(':').unwrap_or(("icons", full));
    let clean = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>();
    let dir = crate::paths::data().join("icons").join(clean(prefix));
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}.svg", clean(name)));
    std::fs::write(&file, svg)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_svgs_and_follows_aliases() {
        let set = json!({ "width": 24, "height": 24, "icons": { "mic": { "body": "<path d=\"M1 1\"/>" }, "wide": { "body": "<g/>", "width": 32 } },
                          "aliases": { "microphone": { "parent": "mic" } } });
        assert_eq!(svg_of(&set, "mic").unwrap(), r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24"><path d="M1 1"/></svg>"#);
        assert!(svg_of(&set, "microphone").unwrap().contains("M1 1"));
        assert!(svg_of(&set, "wide").unwrap().contains("viewBox=\"0 0 32 24\""));
        assert!(svg_of(&set, "missing").is_none());
    }

    #[test]
    fn keeps_live_packs_and_encodes_queries() {
        let v = json!({ "mdi": { "name": "Material", "total": 7000, "license": { "title": "Apache 2.0" }, "category": "Material" },
                        "old": { "name": "Old", "category": "Archive / Unmaintained" },
                        "logos": { "name": "Logos", "total": 1800, "palette": true, "category": "Logos" } });
        let p = packs_from(&v);
        assert_eq!(p.iter().map(|p| p.prefix.as_str()).collect::<Vec<_>>(), ["logos", "mdi"]);
        assert!(p[0].coloured && p[1].license == "Apache 2.0");
        assert_eq!(encode("play & pause"), "play%20%26%20pause");
    }
}
