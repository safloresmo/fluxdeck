//! `fluxdeck headset …`: headsets of many brands through HeadsetControl, in the status line format.
use fluxdeck_core::headset;
use fluxdeck_core::i18n::t;

const USAGE: &str = "usage: fluxdeck headset list | battery ID | sidetone ID [up|down|0-128] | lights ID [toggle|on|off]";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let find = |id: &str| headset::headset(id).ok_or_else(|| format!("HeadsetControl does not see {id}, or is not installed"));
    match a.as_slice() {
        ["list"] => {
            for h in headset::headsets() {
                println!("{}\t{}", h.id, h.product);
            }
        }
        ["battery", id] => {
            let h = find(id)?;
            let n = h.battery.ok_or_else(|| format!("no battery reading for {id}"))?;
            let color = if n <= 15 && !h.charging { "|#b3261e" } else { "" };
            println!("{n}%|{:.2}{color}", n as f64 / 100.0);
        }
        ["sidetone", id, rest @ ..] => {
            let h = find(id)?;
            if !h.has_sidetone {
                return Err("this headset has no sidetone".into());
            }
            let mut level = h.sidetone.unwrap_or(0);
            if let [change] = rest {
                // FLUXDECK_STEPS: how many detents a dial turned.
                let steps: u8 = std::env::var("FLUXDECK_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
                level = match *change {
                    "up" => level.saturating_add(16u8.saturating_mul(steps)).min(128),
                    "down" => level.saturating_sub(16u8.saturating_mul(steps)),
                    n => n.parse::<u8>().map_err(|_| USAGE.to_string())?.min(128),
                };
                headset::set_sidetone(id, level)?;
            }
            let pct = level as u64 * 100 / 128;
            println!("{pct}%|{:.2}", pct as f64 / 100.0);
        }
        ["lights", id, rest @ ..] => {
            let h = find(id)?;
            if !h.has_lights {
                return Err("this headset has no lights".into());
            }
            let mut on = h.lights;
            if let [change] = rest {
                let want = match *change {
                    "toggle" => !on.unwrap_or(true),
                    "on" => true,
                    "off" => false,
                    _ => return Err(USAGE.into()),
                };
                headset::set_lights(id, want)?;
                on = Some(want);
            }
            println!(
                "{}",
                match on {
                    Some(true) => t("Encendidas"),
                    Some(false) => t("Apagadas"),
                    None => "—",
                }
            );
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
