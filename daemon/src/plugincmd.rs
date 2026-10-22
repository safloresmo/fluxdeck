//! `fluxdeck plugin …`: search, install and remove plugins from the terminal.
use fluxdeck_core::catalogue::{self, Source};
use fluxdeck_core::plugins;

const USAGE: &str = "usage: fluxdeck plugin list | search [--elgato] TEXT | install [--elgato] ID | remove ID";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let (source, rest) = match a.as_slice() {
        [cmd, "--elgato", rest @ ..] => (Source::Elgato, [&[*cmd][..], rest].concat()),
        _ => (Source::OpenAction, a.clone()),
    };
    match rest.as_slice() {
        ["list"] => {
            for p in plugins::installed() {
                let state = match p.missing() {
                    None => "ready",
                    Some(m) => m,
                };
                println!("{}\t{}\t{}\t{state}", p.uuid, p.manifest.name, p.manifest.version);
            }
        }
        ["search", words @ ..] => {
            let q = words.join(" ").to_lowercase();
            for e in catalogue::load(source)?.iter().filter(|e| (e.name.clone() + " " + &e.description).to_lowercase().contains(&q)) {
                println!("{}\t{}\t{}", e.id, e.name, e.description);
            }
        }
        ["install", id] => {
            let entry = catalogue::load(source)?.into_iter().find(|e| e.id == *id).ok_or_else(|| format!("{id} is not in the catalogue"))?;
            let p = catalogue::install(&entry)?;
            match p.missing() {
                None => println!("installed {} ({} actions)", p.manifest.name, p.manifest.actions.len()),
                Some(m) => println!("installed {}, but it cannot run yet: needs {m}", p.manifest.name),
            }
        }
        ["remove", id] => {
            let p = plugins::installed().into_iter().find(|p| p.uuid == *id).ok_or_else(|| format!("{id} is not installed"))?;
            std::fs::remove_dir_all(&p.path).map_err(|e| e.to_string())?;
            println!("removed {}", p.manifest.name);
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
