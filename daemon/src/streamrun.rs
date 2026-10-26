//! Runs the stream mix while it is on: the "FluxDeck Stream" output (held by a pw-cli child), a
//! `pipewire -c` child with each group's own output and one loopback per send, and the send
//! volumes; `groups::apply` moves each group's apps into its output. Both children die with the
//! service, so nothing is left behind.
use fluxdeck_core::groups::{self, Group};
use fluxdeck_core::streammix::{self, GroupSink, MIC, SINK, StreamMix};
use fluxdeck_core::{Config, config, mixer};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime};

/// Sends, the microphone they follow, and the groups' outputs.
type Shape = (Vec<String>, String, Vec<GroupSink>);

#[derive(Default)]
struct Running {
    sink: Option<Child>,
    /// The sends and group outputs running, and what they were made from.
    sends: Option<(Shape, Child)>,
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
                // Apps go back to their own volumes; their group outputs are gone.
                std::thread::sleep(Duration::from_millis(500));
                groups::apply(&groups, &mixer::snapshot());
            }
            std::thread::sleep(Duration::from_millis(if mix.on { 1000 } else { 1500 }));
        }
    });
}

fn stop(run: &mut Running) {
    for mut c in run.sink.take().into_iter().chain(run.sends.take().map(|(_, c)| c)) {
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

    // The microphone only matters when it is sent.
    let mic = if keys.iter().any(|k| k == MIC) { snap.default_input.clone() } else { String::new() };
    let routes = groups::routes();
    let sinks: Vec<GroupSink> = groups.iter().map(|g| GroupSink { id: g.id.clone(), name: g.name.clone(), output: routes.get(&g.id).cloned().unwrap_or_default() }).collect();
    let shape = (keys, mic, sinks);

    let died = run.sends.as_mut().is_some_and(|(_, c)| !matches!(c.try_wait(), Ok(None)));
    let same = run.sends.as_ref().map_or(shape.0.is_empty() && shape.2.is_empty(), |(s, _)| *s == shape);
    if died || !same {
        if let Some((_, mut c)) = run.sends.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        run.volumes.clear();
        if !shape.0.is_empty() || !shape.2.is_empty() {
            run.sends = spawn_sends(&shape).map(|c| (shape, c));
        }
        // Nodes appear a moment later; volumes and apps are set on the next pass.
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

    // Apps into their group's output, and each group's volume onto it.
    groups::apply(groups, &snap);
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

fn spawn_sends((keys, mic, sinks): &Shape) -> Option<Child> {
    let conf = fluxdeck_core::paths::runtime("stream-mix.conf");
    std::fs::write(&conf, streammix::to_conf(keys, mic, sinks)).map_err(|e| eprintln!("fluxdeck: cannot write {}: {e}", conf.display())).ok()?;
    let mut cmd = Command::new("pipewire");
    cmd.arg("-c").arg(&conf).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    dies_with_us(&mut cmd).spawn().map_err(|e| eprintln!("fluxdeck: cannot start the stream sends: {e}")).ok()
}
