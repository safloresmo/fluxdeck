//! `fluxdeck backup export [--plugin-settings] | import FILE | list`: the whole setup in one file.
use fluxdeck_core::backup;

const USAGE: &str = "usage: fluxdeck backup export [--plugin-settings] | import FILE | list";

pub fn command(args: &[String]) -> Result<(), String> {
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["export", rest @ ..] => {
            let plugin_settings = rest.contains(&"--plugin-settings");
            println!("{}", backup::export(plugin_settings)?.display());
            Ok(())
        }
        ["import", file] => {
            let done = backup::import(std::path::Path::new(file))?;
            println!("imported; the setup before is in {}", done.previous.display());
            if !done.missing_plugins.is_empty() {
                println!("plugins to install again: {}", done.missing_plugins.join(", "));
            }
            Ok(())
        }
        ["list"] => {
            for f in backup::list() {
                println!("{}", f.display());
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}
