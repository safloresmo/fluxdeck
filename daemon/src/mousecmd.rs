//! `fluxdeck mouse …`: mice of many brands through ratbagd, in the status line format.
use fluxdeck_core::i18n::t;
use fluxdeck_core::ratbag::{self, Mouse};

const USAGE: &str = "usage: fluxdeck mouse list | dpi ID [up|down|DPI] | resolution ID [next|prev] | profile ID [next|prev|N]";

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let find = |id: &str| ratbag::mouse(id).ok_or_else(|| format!("ratbagd does not see {id}, or is not installed"));
    // FLUXDECK_STEPS: how many detents a dial turned.
    let steps: u32 = std::env::var("FLUXDECK_STEPS").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    match a.as_slice() {
        ["list"] => {
            for m in ratbag::mice() {
                println!("{}\t{}", m.id, m.name);
            }
        }
        ["dpi", id, rest @ ..] => {
            let mut m = find(id)?;
            if let [change] = rest {
                let dpi = match *change {
                    "up" => ratbag::dpi_step(&m, true, steps),
                    "down" => ratbag::dpi_step(&m, false, steps),
                    n => Some(n.parse().map_err(|_| USAGE.to_string())?),
                };
                ratbag::set_dpi(&m, dpi.ok_or("this mouse has no DPI to change")?)?;
                m = find(id)?;
            }
            let dpi = m.dpi().ok_or("this mouse has no active resolution")?;
            let max = m.dpis().last().copied().unwrap_or(dpi).max(1);
            println!("{dpi} dpi|{:.2}", (dpi as f64 / max as f64).min(1.0));
        }
        ["resolution", id, change] => {
            let m = find(id)?;
            let forward = match *change {
                "next" => true,
                "prev" => false,
                _ => return Err(USAGE.into()),
            };
            ratbag::cycle_resolution(&m, forward)?;
            println!("{} dpi", find(id)?.dpi().unwrap_or(0));
        }
        ["profile", id, rest @ ..] => {
            let m = find(id)?;
            match rest {
                [] => {}
                ["next"] => {
                    ratbag::cycle_profile(&m, true)?;
                }
                ["prev"] => {
                    ratbag::cycle_profile(&m, false)?;
                }
                [n] => {
                    ratbag::set_profile(&m, n.parse::<u32>().ok().and_then(|n| n.checked_sub(1)).ok_or_else(|| USAGE.to_string())?)?;
                }
                _ => return Err(USAGE.into()),
            }
            println!("{}", profile_label(&find(id)?));
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn profile_label(m: &Mouse) -> String {
    match m.active_profile() {
        Some(p) if !p.name.is_empty() => p.name.clone(),
        Some(p) => format!("{} {}", t("Perfil"), p.index + 1),
        None => "—".into(),
    }
}
