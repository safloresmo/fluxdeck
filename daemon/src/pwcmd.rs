//! `fluxdeck pw …`: short PipeWire actions for keys and dials.
use fluxdeck_core::pw;

fn number(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.2}") }
}

pub fn command(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        // param NODE NAME [+N|-N|=N MIN MAX]
        ["param", node, name, rest @ ..] => {
            let cur = pw::param(node, name).ok_or_else(|| format!("{node} has no control {name}"))?;
            let value = match rest {
                [] => cur,
                [change, min, max] => {
                    let (min, max): (f64, f64) = (min.parse().map_err(|_| "bad min")?, max.parse().map_err(|_| "bad max")?);
                    let n: f64 = change.trim_start_matches(['+', '=']).parse().map_err(|_| format!("bad change {change}"))?;
                    let v = if change.starts_with('=') { n } else { cur + n }.clamp(min, max);
                    if !pw::set_param(node, name, v) {
                        return Err(format!("could not set {name} on {node}"));
                    }
                    v
                }
                _ => return Err("usage: fluxdeck pw param NODE NAME [+N|-N|=N MIN MAX]".into()),
            };
            println!("{}", number(value));
        }
        ["default"] => println!("{}", pw::default_source().unwrap_or_default()),
        ["toggle-default", a, b] => {
            let next = if pw::default_source().as_deref() == Some(a) { b } else { a };
            if !pw::set_default(next) {
                return Err(format!("no source named {next}"));
            }
            println!("{next}");
        }
        _ => return Err("usage: fluxdeck pw param|default|toggle-default …".into()),
    }
    Ok(())
}
