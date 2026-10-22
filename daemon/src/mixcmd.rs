//! `fluxdeck mix app NAME [+N|-N|=N|mute|unmute|toggle]`: an app's volume, for deck dials and keys.
//! Prints `NAME 45%|0.45`, or a muted line in red, in the status format.
use fluxdeck_core::groups;
use fluxdeck_core::mixer::{self, Kind};

const USAGE: &str = "usage: fluxdeck mix app NAME|group ID [+N|-N|=N|mute|unmute|toggle]";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let snap = mixer::snapshot();
    let (name, rest, streams): (String, &[&str], Vec<_>) = match a.as_slice() {
        // Every stream of the app: a browser plays one per tab.
        ["app", name, rest @ ..] => {
            let want = name.to_lowercase();
            (name.to_string(), rest, snap.channels.iter().filter(|c| c.kind == Kind::Playback && c.label.to_lowercase().contains(&want)).collect())
        }
        ["group", id, rest @ ..] => return group(id, rest, &snap),
        _ => return Err(USAGE.into()),
    };
    let name = name.as_str();
    let Some(first) = streams.first() else {
        println!("{name}|0");
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
    if muted {
        println!("{name}: mute||#b3261e");
    } else {
        let label = if first.kind == Kind::Playback { first.label.as_str() } else { name };
        println!("{label} {:.0}%|{volume:.2}", volume * 100.0);
    }
    Ok(())
}

/// A group's volume lives in FluxDeck's state, so it works even with none of its apps open.
fn group(id: &str, rest: &[&str], snap: &mixer::Snapshot) -> Result<(), String> {
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
    if level.muted {
        println!("{}: mute||#b3261e", g.name);
    } else {
        println!("{} {:.0}%|{:.2}", g.name, level.volume * 100.0, level.volume);
    }
    Ok(())
}
