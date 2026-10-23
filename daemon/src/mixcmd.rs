//! `fluxdeck mix app NAME [+N|-N|=N|mute|unmute|toggle]`: an app's volume, for deck dials and keys.
//! Prints `NAME 45%|0.45`, or a muted line in red, in the status format.
use fluxdeck_core::groups;
use fluxdeck_core::mixer::{self, Kind};

const USAGE: &str = "usage: fluxdeck mix app NAME|group ID [+N|-N|=N|mute|unmute|toggle] [short]";

pub fn command(args: &[String]) -> Result<(), String> {
    // `short` prints only the value: a dial already shows the name above it.
    let short = args.last().is_some_and(|a| a == "short");
    let a: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "short").collect();
    let snap = mixer::snapshot();
    let (name, rest, streams): (String, &[&str], Vec<_>) = match a.as_slice() {
        // Every stream of the app: a browser plays one per tab.
        ["app", name, rest @ ..] => {
            let want = name.to_lowercase();
            (name.to_string(), rest, snap.channels.iter().filter(|c| c.kind == Kind::Playback && c.label.to_lowercase().contains(&want)).collect())
        }
        ["group", id, rest @ ..] => return group(id, rest, &snap, short),
        _ => return Err(USAGE.into()),
    };
    let name = name.as_str();
    let Some(first) = streams.first() else {
        println!("{}|0", if short { "—" } else { name });
        return Ok(());
    };
    let (mut volume, mut muted) = (first.volume, first.muted);
    match rest {
        [] => {}
        ["mute"] => muted = true,
        ["unmute"] => muted = false,
        ["toggle"] => muted = !muted,
        [change] => {
            let n: f32 = change.trim_start_matches(['+', '=']).parse().map_err(|_| USAGE.to_string())?;
            volume = if change.starts_with('=') { n / 100.0 } else { volume + n / 100.0 }.clamp(0.0, 1.0);
        }
        _ => return Err(USAGE.into()),
    }
    if !rest.is_empty() {
        for s in &streams {
            if muted != s.muted {
                mixer::set_mute(s.id, muted);
            }
            if (volume - s.volume).abs() > 0.001 {
                mixer::set_volume(s.id, volume);
            }
        }
    }
    let label = if first.kind == Kind::Playback { first.label.as_str() } else { name };
    print_level(label, volume, muted, short);
    Ok(())
}

/// A group's volume lives in FluxDeck's state, so it works even with none of its apps open.
fn group(id: &str, rest: &[&str], snap: &mixer::Snapshot, short: bool) -> Result<(), String> {
    let groups = fluxdeck_core::Config::load().map(|c| c.groups).unwrap_or_default();
    let g = groups.iter().find(|g| g.id == id).ok_or_else(|| format!("no group {id}"))?;
    let mut level = groups::levels().get(id).copied().unwrap_or_default();
    match rest {
        [] => {}
        ["mute"] => level.muted = true,
        ["unmute"] => level.muted = false,
        ["toggle"] => level.muted = !level.muted,
        [change] => {
            let n: f32 = change.trim_start_matches(['+', '=']).parse().map_err(|_| USAGE.to_string())?;
            level.volume = if change.starts_with('=') { n / 100.0 } else { level.volume + n / 100.0 }.clamp(0.0, 1.0);
        }
        _ => return Err(USAGE.into()),
    }
    if !rest.is_empty() {
        groups::set_level(id, level);
        // Right away, instead of waiting for the service's next pass.
        groups::apply(&groups, snap);
    }
    print_level(&g.name, level.volume, level.muted, short);
    Ok(())
}

/// One status line: `Música 45%|0.45`, `45%|0.45` when short, or a red mute.
fn print_level(name: &str, volume: f32, muted: bool, short: bool) {
    match (muted, short) {
        (true, true) => println!("mute||#b3261e"),
        (true, false) => println!("{name}: mute||#b3261e"),
        (false, true) => println!("{:.0}%|{volume:.2}", volume * 100.0),
        (false, false) => println!("{name} {:.0}%|{volume:.2}", volume * 100.0),
    }
}
