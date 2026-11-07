//! `fluxdeck battery …`: wireless peripherals of any brand through UPower, in the status line format.
use fluxdeck_core::upower;

const USAGE: &str = "usage: fluxdeck battery list | battery MODEL";

pub fn command(args: &[String]) -> Result<(), String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["list"] => {
            for b in upower::batteries() {
                println!("{}\t{}\t{}%", b.model, b.kind, b.level);
            }
        }
        [model] => {
            let n = upower::battery(model).ok_or_else(|| format!("UPower has no battery for {model}"))?;
            let color = if n <= 15 { "|#b3261e" } else { "" };
            println!("{n}%|{:.2}{color}", n as f64 / 100.0);
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}
