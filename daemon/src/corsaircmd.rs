//! `fluxdeck corsair …`: Corsair devices through OpenLinkHub, printed in the status line format.
use fluxdeck_core::corsair::{self, LEVELS};
use fluxdeck_core::i18n::t;

const RED: &str = "#b3261e";
const AMBER: &str = "#8a5a00";

const USAGE: &str = "usage: fluxdeck corsair list | battery ID | anc ID [cycle|off|on|transparency] | brightness ID [up|down] | effect ID [next|prev] | lcd ID [next|prev|MODE]";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let profile = |id: &str| corsair::profile(id).ok_or_else(|| format!("OpenLinkHub does not know {id}, or is not running"));
    match a.as_slice() {
        ["list"] => {
            for d in corsair::devices() {
                println!("{}\t{}", d.serial, d.product);
            }
        }
        ["battery", id] => {
            let n = corsair::battery(id).ok_or_else(|| format!("no battery reading for {id}"))?;
            let color = if n <= 15 { format!("|{RED}") } else { String::new() };
            println!("{n}%|{:.2}{color}", n as f64 / 100.0);
        }
        ["anc", id, rest @ ..] => {
            let mut mode = profile(id)?["NoiseCancellation"].as_i64().ok_or("this device has no noise cancellation")?;
            if let [change] = rest {
                mode = match *change {
                    "cycle" => (mode + 1) % 3,
                    "off" => 0,
                    "on" => 1,
                    "transparency" => 2,
                    _ => return Err(USAGE.into()),
                };
                // OpenLinkHub refuses this while sidetone is on.
                corsair::set_anc(id, mode)?;
            }
            match mode {
                1 => println!("ANC"),
                2 => println!("{}||{AMBER}", t("Transparencia")),
                _ => println!("{}", t("Sin ANC")),
            }
        }
        ["brightness", id, rest @ ..] => {
            let mut i = corsair::level_index(profile(id)?["Brightness"].as_u64().ok_or("this device has no brightness")?);
            if let [change] = rest {
                i = match *change {
                    "up" => (i + 1).min(LEVELS.len() - 1),
                    "down" => i.saturating_sub(1),
                    _ => return Err(USAGE.into()),
                };
                corsair::set_brightness(id, LEVELS[i].0)?;
            }
            let pct = LEVELS[i].1;
            println!("{pct}%|{:.2}", pct as f64 / 100.0);
        }
        ["effect", id, rest @ ..] => {
            let cur = profile(id)?["RGBProfile"].as_str().ok_or("this device has no lights")?.to_string();
            let mut effect = cur.clone();
            if let [change] = rest {
                let all = corsair::effects(id);
                let i = all.iter().position(|e| *e == cur).unwrap_or(0);
                let n = all.len().max(1);
                effect = match *change {
                    "next" => all.get((i + 1) % n),
                    "prev" => all.get((i + n - 1) % n),
                    name => all.iter().find(|e| *e == name),
                }
                .cloned()
                .ok_or_else(|| format!("no effect {change}"))?;
                corsair::set_effect(id, &effect)?;
            }
            println!("{effect}");
        }
        ["lcd", id, rest @ ..] => {
            let (modes, mut mode, _) = corsair::lcd(id).ok_or("this device has no screen")?;
            if let [change] = rest {
                let i = modes.iter().position(|(m, _)| *m == mode).unwrap_or(0);
                mode = match *change {
                    "next" => modes[(i + 1) % modes.len()].0,
                    "prev" => modes[(i + modes.len() - 1) % modes.len()].0,
                    n => n.parse().map_err(|_| USAGE.to_string())?,
                };
                corsair::set_lcd(id, mode)?;
            }
            let name = modes.iter().find(|(m, _)| *m == mode).map(|(_, n)| corsair::lcd_mode_name(n)).unwrap_or_default();
            println!("{name}");
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
