//! Runs the microphone effect chain from effects.toml as a child `pipewire -c` process, and keeps it
//! in step with the file: a new effect or order restarts it, a changed value is applied live.
use fluxdeck_core::effects::{self, CAPTURE, Chain};
use fluxdeck_core::pw;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

pub fn start() {
    std::thread::spawn(|| {
        let mut running: Option<(Chain, Child)> = None;
        let mut seen: Option<SystemTime> = None;
        loop {
            let mtime = std::fs::metadata(effects::path()).and_then(|m| m.modified()).ok();
            let died = running.as_mut().is_some_and(|(_, c)| matches!(c.try_wait(), Ok(Some(_))));
            if mtime != seen || died {
                seen = mtime;
                let chain = Chain::load();
                running = match (running.take(), chain) {
                    (Some((old, child)), Some(new)) if !died && old.same_shape(&new) => {
                        apply_values(&old, &new);
                        Some((new, child))
                    }
                    (old, Some(new)) => {
                        if let Some((_, mut c)) = old {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                        if died {
                            eprintln!("fluxdeck: effect chain stopped; starting it again");
                            std::thread::sleep(Duration::from_secs(2));
                        }
                        spawn(&new).map(|c| (new, c))
                    }
                    (Some((_, mut c)), None) => {
                        let _ = c.kill();
                        let _ = c.wait();
                        None
                    }
                    (None, None) => None,
                };
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    });
}

fn spawn(chain: &Chain) -> Option<Child> {
    let conf = effects::conf_path();
    if let Err(e) = std::fs::write(&conf, chain.to_conf()) {
        eprintln!("fluxdeck: cannot write {}: {e}", conf.display());
        return None;
    }
    let mut cmd = Command::new("pipewire");
    cmd.arg("-c").arg(&conf).stdin(Stdio::null()).stdout(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(c) => {
            eprintln!("fluxdeck: effect chain running ({} effects)", chain.effects.iter().filter(|e| e.enabled).count());
            Some(c)
        }
        Err(e) => {
            eprintln!("fluxdeck: cannot start the effect chain: {e}");
            None
        }
    }
}

/// Sends only the values that changed.
fn apply_values(old: &Chain, new: &Chain) {
    for e in new.effects.iter().filter(|e| e.enabled) {
        let before = old.effects.iter().find(|o| o.id == e.id);
        for (k, v) in &e.controls {
            if before.and_then(|b| b.controls.get(k)) != Some(v) {
                pw::set_param(CAPTURE, &format!("{}:{k}", e.id), *v);
            }
        }
    }
}
