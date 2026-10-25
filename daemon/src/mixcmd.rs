//! `fluxdeck mix app NAME [+N|-N|=N|mute|unmute|toggle]`: an app's volume, for deck dials and keys.
//! Prints `NAME 45%|0.45`, or a muted line in red, in the status format. `mix mic` does the same for
//! the microphone in use, `mix stream on|off|toggle` switches the stream mix, and `mix status`
//! prints everything a bar widget shows as one JSON line.
use fluxdeck_core::groups::{self, Level};
use fluxdeck_core::streammix::StreamMix;
use serde_json::json;
use fluxdeck_core::mixer::{self, Kind};

const USAGE: &str = "usage: fluxdeck mix app NAME|group ID|mic [+N|-N|=N|mute|unmute|toggle] [short] | stream [on|off|toggle] | status";

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
        ["mic", rest @ ..] => return mic(rest, &snap, short),
        ["stream", rest @ ..] => return stream(rest),
        ["status"] => return status(&snap),
        _ => return Err(USAGE.into()),
    };
    let name = name.as_str();
    let Some(first) = streams.first() else {
        println!("{}|0", if short { "—" } else { name });
        return Ok(());
    };
    let Level { volume, muted } = change(Level { volume: first.volume, muted: first.muted }, rest)?;
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
    let level = change(groups::levels().get(id).copied().unwrap_or_default(), rest)?;
    if !rest.is_empty() {
        groups::set_level(id, level);
        // Right away, instead of waiting for the service's next pass.
        groups::apply(&groups, snap);
    }
    print_level(&g.name, level.volume, level.muted, short);
    Ok(())
}

/// Applies `+N`, `-N`, `=N` (percent), `mute`, `unmute` or `toggle`; no word leaves it as it is.
fn change(mut level: Level, rest: &[&str]) -> Result<Level, String> {
    match rest {
        [] => {}
        ["mute"] => level.muted = true,
        ["unmute"] => level.muted = false,
        ["toggle"] => level.muted = !level.muted,
        [c] => {
            let n: f32 = c.trim_start_matches(['+', '=']).parse().map_err(|_| USAGE.to_string())?;
            level.volume = if c.starts_with('=') { n / 100.0 } else { level.volume + n / 100.0 }.clamp(0.0, 1.0);
        }
        _ => return Err(USAGE.into()),
    }
    Ok(level)
}

/// The microphone in use (the default input).
fn mic(rest: &[&str], snap: &mixer::Snapshot, short: bool) -> Result<(), String> {
    let m = snap.default_of(Kind::Input).ok_or("no microphone")?;
    let level = change(Level { volume: m.volume, muted: m.muted }, rest)?;
    if level.muted != m.muted {
        mixer::set_mute(m.id, level.muted);
    }
    if (level.volume - m.volume).abs() > 0.001 {
        mixer::set_volume(m.id, level.volume);
    }
    print_level(&m.label, level.volume, level.muted, short);
    Ok(())
}

fn stream(rest: &[&str]) -> Result<(), String> {
    let mut mix = StreamMix::load();
    let on = match rest {
        [] => mix.on,
        ["on"] => true,
        ["off"] => false,
        ["toggle"] => !mix.on,
        _ => return Err(USAGE.into()),
    };
    if on != mix.on {
        mix.on = on;
        mix.save();
    }
    println!("{}", if on { "on" } else { "off" });
    Ok(())
}

/// One JSON line: the microphone, every group with what plays in it, the active profile and
/// whether the stream mix is on.
fn status(snap: &mixer::Snapshot) -> Result<(), String> {
    let cfg = fluxdeck_core::Config::load().unwrap_or_default();
    let levels = groups::levels();
    let mic = snap.default_of(Kind::Input).map(|m| json!({ "name": m.label, "volume": m.volume, "muted": m.muted }));
    let all: Vec<_> = cfg
        .groups
        .iter()
        .map(|g| {
            let l = levels.get(&g.id).copied().unwrap_or_default();
            let playing: Vec<&str> = snap.of(Kind::Playback).filter(|a| groups::group_of(&cfg.groups, a).is_some_and(|x| x.id == g.id)).map(|a| a.label.as_str()).collect();
            json!({ "id": g.id, "name": g.name, "icon": g.icon, "volume": l.volume, "muted": l.muted, "playing": playing })
        })
        .collect();
    let profile = fluxdeck_core::profiles::active().and_then(|id| cfg.profiles.iter().find(|p| p.id == id).map(|p| p.name.clone()));
    println!("{}", json!({ "mic": mic, "groups": all, "profile": profile, "stream": StreamMix::load().on }));
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
