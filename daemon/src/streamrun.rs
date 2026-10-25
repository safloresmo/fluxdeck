//! Runs the stream mix while it is on: the "FluxDeck Stream" output (held by a pw-cli child), a
//! `pipewire -c` child with one loopback per send, send volumes, and links from each group's apps.
//! Both children die with the service, so nothing is left behind.
use fluxdeck_core::groups::{self, Group};
use fluxdeck_core::streammix::{self, MIC, SINK, StreamMix};
use fluxdeck_core::{Config, config, mixer};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

#[derive(Default)]
struct Running {
    sink: Option<Child>,
    sends: Option<(Vec<String>, String, Child)>,
    /// Volumes already set on each send, so wpctl runs only on a change.
    volumes: HashMap<String, f32>,
}

pub fn start() {
    std::thread::spawn(|| {
        let mut run = Running::default();
        let mut groups: Vec<Group> = Vec::new();
        let mut seen: Option<SystemTime> = None;
        loop {
            let mix = StreamMix::load();
            if mix.on {
                let mtime = std::fs::metadata(config::path()).and_then(|m| m.modified()).ok();
                if mtime != seen {
                    seen = mtime;
                    groups = Config::load().map(|c| c.groups).unwrap_or_default();
                }
                step(&mut run, &mix, &groups);
            } else if run.sink.is_some() || run.sends.is_some() {
                stop(&mut run);
            }
            std::thread::sleep(Duration::from_millis(if mix.on { 1000 } else { 1500 }));
        }
    });
}

fn stop(run: &mut Running) {
    for mut c in run.sink.take().into_iter().chain(run.sends.take().map(|(_, _, c)| c)) {
        let _ = c.kill();
        let _ = c.wait();
    }
    run.volumes.clear();
}

fn dies_with_us(cmd: &mut Command) -> &mut Command {
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        })
    }
}

fn alive(c: &mut Option<Child>) -> bool {
    c.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
}

fn step(run: &mut Running, mix: &StreamMix, groups: &[Group]) {
    if !alive(&mut run.sink) {
        run.sink = spawn_sink();
    }
    let Ok(out) = Command::new("pw-dump").stderr(Stdio::null()).output() else { return };
    let Ok(all) = serde_json::from_slice::<Vec<Value>>(&out.stdout) else { return };
    let snap = mixer::parse(&all);
    // Groups that no longer exist send nothing.
    let active: Vec<(String, groups::Level)> = mix.active().into_iter().filter(|(k, _)| k == MIC || groups.iter().any(|g| &g.id == k)).collect();
    let keys: Vec<String> = active.iter().map(|(k, _)| k.clone()).collect();
    let mic = snap.default_input.clone();

    let died = run.sends.as_mut().is_some_and(|(_, _, c)| !matches!(c.try_wait(), Ok(None)));
    let same = run.sends.as_ref().map_or(keys.is_empty(), |(k, m, _)| *k == keys && (*m == mic || !keys.iter().any(|k| k == MIC)));
    if died || !same {
        if let Some((_, _, mut c)) = run.sends.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        run.volumes.clear();
        if !keys.is_empty() {
            run.sends = spawn_sends(&keys, &mic).map(|c| (keys.clone(), mic.clone(), c));
        }
        // Nodes appear a moment later; volumes and links are set on the next pass.
        return;
    }

    for (key, level) in &active {
        if run.volumes.get(key).is_some_and(|v| (v - level.volume).abs() < 0.005) {
            continue;
        }
        if let Some(id) = streammix::node_id(&all, &streammix::send_out(key))
            && mixer::set_volume(id, level.volume)
        {
            run.volumes.insert(key.clone(), level.volume);
        }
    }

    let ports = streammix::ports(&all);
    let existing = streammix::links(&all);
    for (key, _) in active.iter().filter(|(k, _)| k != MIC) {
        let Some(send) = streammix::node_id(&all, &streammix::send_in(key)) else { continue };
        for app in snap.of(mixer::Kind::Playback).filter(|a| groups::group_of(groups, a).is_some_and(|g| &g.id == key)) {
            for (o, i) in streammix::missing(app.id, send, &ports, &existing) {
                let _ = Command::new("pw-link").args([o.to_string(), i.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
        }
    }
}

fn spawn_sink() -> Option<Child> {
    let mut cmd = Command::new("pw-cli");
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = dies_with_us(&mut cmd).spawn().map_err(|e| eprintln!("fluxdeck: cannot start pw-cli: {e}")).ok()?;
    // stdin stays open: pw-cli, and with it the output, lasts until the child is killed.
    child.stdin.as_mut()?.write_all(streammix::sink_command().as_bytes()).ok()?;
    eprintln!("fluxdeck: stream output {SINK} on");
    Some(child)
}

fn spawn_sends(keys: &[String], mic: &str) -> Option<Child> {
    let conf = fluxdeck_core::paths::runtime("stream-mix.conf");
    std::fs::write(&conf, streammix::to_conf(keys, mic)).map_err(|e| eprintln!("fluxdeck: cannot write {}: {e}", conf.display())).ok()?;
    let mut cmd = Command::new("pipewire");
    cmd.arg("-c").arg(&conf).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    dies_with_us(&mut cmd).spawn().map_err(|e| eprintln!("fluxdeck: cannot start the stream sends: {e}")).ok()
}
