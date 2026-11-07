//! `fluxdeck rgb …`: lights of any brand through OpenRGB, printed in the status line format.
use fluxdeck_core::openrgb;

const USAGE: &str = "usage: fluxdeck rgb list | effect ID [next|prev|NAME] | brightness ID [up|down|PERCENT] | color ID [RRGGBB]";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let device = |id: &str| openrgb::device(id).ok_or_else(|| format!("OpenRGB does not know {id}, or its SDK server is not running"));
    match a.as_slice() {
        ["list"] => {
            for d in openrgb::devices() {
                println!("{}", d.id);
            }
        }
        ["effect", id, rest @ ..] => {
            let d = device(id)?;
            let mut effect = d.effect().ok_or("this device has no effects")?.to_string();
            if let [change] = rest {
                let all = d.effects();
                let (i, n) = (d.active, all.len());
                effect = match *change {
                    "next" => all[(i + 1) % n].clone(),
                    "prev" => all[(i + n - 1) % n].clone(),
                    name => all.iter().find(|e| e.eq_ignore_ascii_case(name)).cloned().ok_or_else(|| format!("no effect {name}"))?,
                };
                openrgb::set_effect(id, &effect)?;
            }
            println!("{}", openrgb::effect_name(&effect));
        }
        ["brightness", id, rest @ ..] => {
            let mut pct = device(id)?.brightness().ok_or("this effect has no brightness")?;
            if let [change] = rest {
                pct = match *change {
                    "up" => pct.saturating_add(10).min(100),
                    "down" => pct.saturating_sub(10),
                    n => n.trim_end_matches('%').parse::<u8>().map_err(|_| USAGE.to_string())?.min(100),
                };
                openrgb::set_brightness(id, pct)?;
            }
            println!("{pct}%|{:.2}", pct as f64 / 100.0);
        }
        ["color", id, rest @ ..] => {
            if let [hex] = rest {
                openrgb::set_color(id, hex)?;
            }
            // Read back: some effects keep their own color.
            match device(id)?.color() {
                Some(c) => println!("#{c}||#{c}"),
                None => println!("—"),
            }
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
