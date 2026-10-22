//! Keeps every app at its group's volume and mute, including apps opened later.
//! It listens to PipeWire's change feed and looks again only when an app stream shows up or changes,
//! so it costs nothing while the audio is still.
use fluxdeck_core::{Config, config, groups, mixer};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

pub fn start() {
    std::thread::spawn(|| {
        let mut groups_now = Vec::new();
        let mut seen: Option<SystemTime> = None;
        let mut feed: Option<(Child, Arc<AtomicBool>)> = None;
        let mut last_full = Instant::now();
        loop {
            let mtime = std::fs::metadata(config::path()).and_then(|m| m.modified()).ok();
            if mtime != seen {
                seen = mtime;
                groups_now = Config::load().map(|c| c.groups).unwrap_or_default();
                if let Some((_, dirty)) = &feed {
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            match (&mut feed, groups_now.is_empty()) {
                (None, false) => {
                    feed = listen();
                    last_full = Instant::now() - Duration::from_secs(60);
                }
                (Some((c, _)), true) => {
                    let _ = c.kill();
                    let _ = c.wait();
                    feed = None;
                }
                _ => {}
            }
            if let Some((child, dirty)) = &mut feed {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    feed = None;
                    continue;
                }
                // Group levels change from dials and the editor without touching config.toml: a slow pass covers them.
                if dirty.swap(false, Ordering::Relaxed) || last_full.elapsed() > Duration::from_secs(30) {
                    groups::apply(&groups_now, &mixer::snapshot());
                    last_full = Instant::now();
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

/// `pw-dump --monitor`, flagging any update that mentions an app stream.
fn listen() -> Option<(Child, Arc<AtomicBool>)> {
    let mut cmd = Command::new("pw-dump");
    cmd.args(["--monitor", "--no-colors"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;
    let out = child.stdout.take()?;
    let dirty = Arc::new(AtomicBool::new(true));
    let flag = dirty.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if line.contains("Stream/Output/Audio") {
                flag.store(true, Ordering::Relaxed);
            }
        }
    });
    Some((child, dirty))
}
