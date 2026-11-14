//! Mice of many brands (Logitech, SteelSeries, Roccat…) through ratbagd, the libratbag service
//! (https://libratbag.github.io/dbus.html). It is read and written with `busctl`, which every
//! systemd machine has, so no D-Bus library is needed. Every write ends with the device's `Commit`.
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEST: &str = "org.freedesktop.ratbag1";
/// The only D-Bus API version this code understands: ratbagd promises nothing across versions.
const API_VERSION: i64 = 2;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Resolution {
    pub path: String,
    pub index: u32,
    pub active: bool,
    pub disabled: bool,
    /// DPI of the x axis; y is the same unless `split` (the device keeps two values).
    pub dpi: u32,
    pub split: bool,
    /// The DPI values the device accepts, ascending.
    pub dpis: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Profile {
    pub path: String,
    pub index: u32,
    pub name: String,
    pub active: bool,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mouse {
    /// ratbagd's model id ("usb:046d:c077:0"): the same after a reboot, unlike the object path.
    pub id: String,
    pub name: String,
    pub path: String,
    pub profiles: Vec<Profile>,
    /// The resolutions (DPI stages) of the active profile.
    pub resolutions: Vec<Resolution>,
}

impl Mouse {
    pub fn active_profile(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.active)
    }

    pub fn active_resolution(&self) -> Option<&Resolution> {
        self.resolutions.iter().find(|r| r.active)
    }

    pub fn dpi(&self) -> Option<u32> {
        self.active_resolution().map(|r| r.dpi)
    }

    /// The DPI values of the active resolution.
    pub fn dpis(&self) -> &[u32] {
        self.active_resolution().map_or(&[], |r| &r.dpis)
    }
}

/// Killed after 5 s: a mouse that never answers must not hold everything else.
fn run(args: &[&str]) -> Result<String, String> {
    let mut child = Command::new("busctl").arg("--system").arg("--json=short").args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| "busctl is not installed".to_string())?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let (out, err) = (read(Box::new(child.stdout.take().ok_or("no output")?)), read(Box::new(child.stderr.take().ok_or("no output")?)));
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break Some(s);
        }
        if start.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    match status {
        Some(s) if s.success() => Ok(out),
        Some(_) => Err(err.trim().to_string()),
        None => Err("ratbagd did not answer".into()),
    }
}

/// The values of a `busctl get-property` output: one JSON line per property, each as {"type","data"}.
/// A variant property wraps its value once more, which is unwrapped here.
fn values(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| if v["type"] == "v" { v["data"]["data"].clone() } else { v["data"].clone() })
        .collect()
}

/// A resolution as ratbagd gives it: one number for both axes, or a pair (x, y).
fn dpi_of(v: &Value) -> Option<(u32, bool)> {
    match v {
        Value::Number(n) => Some((n.as_u64()? as u32, false)),
        Value::Array(a) => Some((a.first()?.as_u64()? as u32, true)),
        _ => None,
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect()
}

/// Reads `props` of one object; None when the reply does not have them all.
fn get(path: &str, iface: &str, props: &[&str]) -> Option<Vec<Value>> {
    let iface = format!("{DEST}.{iface}");
    let mut args = vec!["get-property", DEST, path, &iface];
    args.extend(props);
    let v = values(&run(&args).ok()?);
    (v.len() == props.len()).then_some(v)
}

fn resolution(path: &str, v: &[Value]) -> Option<Resolution> {
    let (dpi, split) = dpi_of(&v[3])?;
    Some(Resolution {
        path: path.into(),
        index: v[0].as_u64()? as u32,
        active: v[1].as_bool()?,
        disabled: v[2].as_bool()?,
        dpi,
        split,
        dpis: v[4].as_array().into_iter().flatten().filter_map(|d| Some(d.as_u64()? as u32)).collect(),
    })
}

fn profile(path: &str, v: &[Value]) -> Option<Profile> {
    Some(Profile { path: path.into(), index: v[0].as_u64()? as u32, name: v[1].as_str()?.into(), active: v[2].as_bool()?, disabled: v[3].as_bool()? })
}

fn read_mouse(path: &str) -> Option<Mouse> {
    let d = get(path, "Device", &["Name", "Model", "Profiles"])?;
    let mut mouse = Mouse { id: d[1].as_str()?.into(), name: d[0].as_str()?.into(), path: path.into(), ..Default::default() };
    let mut active_resolutions = Vec::new();
    for p in strings(&d[2]) {
        let v = get(&p, "Profile", &["Index", "Name", "IsActive", "Disabled", "Resolutions"])?;
        let prof = profile(&p, &v)?;
        if prof.active {
            active_resolutions = strings(&v[4]);
        }
        mouse.profiles.push(prof);
    }
    for r in active_resolutions {
        let v = get(&r, "Resolution", &["Index", "IsActive", "IsDisabled", "Resolution", "Resolutions"])?;
        mouse.resolutions.push(resolution(&r, &v)?);
    }
    Some(mouse)
}

/// Mice ratbagd supports; empty when it is not installed or sees none.
pub fn mice() -> Vec<Mouse> {
    let Some(m) = get("/org/freedesktop/ratbag1", "Manager", &["APIVersion", "Devices"]) else { return Vec::new() };
    if m[0].as_i64() != Some(API_VERSION) {
        return Vec::new();
    }
    strings(&m[1]).iter().filter_map(|p| read_mouse(p)).collect()
}

pub fn mouse(id: &str) -> Option<Mouse> {
    mice().into_iter().find(|m| m.id == id)
}

/// Whether ratbagd is installed. It starts on demand through D-Bus, so installed is ready.
pub fn installed() -> bool {
    crate::effects::on_path("ratbagd") || std::path::Path::new("/usr/share/dbus-1/system-services/org.freedesktop.ratbag1.service").exists()
}

fn call(path: &str, iface: &str, method: &str) -> Result<(), String> {
    run(&["call", DEST, path, &format!("{DEST}.{iface}"), method]).map(|_| ())
}

fn commit(m: &Mouse) -> Result<(), String> {
    call(&m.path, "Device", "Commit")
}

/// Sets the DPI of the active resolution to one the device accepts.
pub fn set_dpi(m: &Mouse, dpi: u32) -> Result<(), String> {
    let r = m.active_resolution().ok_or("this mouse has no active resolution")?;
    if !r.dpis.contains(&dpi) {
        return Err(format!("{} does not take {dpi} dpi", m.name));
    }
    let (kind, value) = if r.split { ("(uu)", format!("{dpi} {dpi}")) } else { ("u", dpi.to_string()) };
    // The property is itself a variant, so Set's variant holds a variant.
    let mut args = vec!["call", DEST, &r.path, "org.freedesktop.DBus.Properties", "Set", "ssv", "org.freedesktop.ratbag1.Resolution", "Resolution", "v", kind];
    args.extend(value.split(' '));
    run(&args)?;
    commit(m)
}

/// The DPI `steps` hundred above or below the current one, or the nearest the device takes.
pub fn dpi_step(m: &Mouse, up: bool, steps: u32) -> Option<u32> {
    let (cur, dpis) = (m.dpi()?, m.dpis());
    let delta = 100 * steps.max(1);
    if up { dpis.iter().copied().find(|d| *d >= cur.saturating_add(delta)).or(dpis.last().copied()) } else { dpis.iter().rev().copied().find(|d| *d <= cur.saturating_sub(delta)).or(dpis.first().copied()) }
}

/// Makes the next (or previous) resolution of the active profile the active one, like the mouse's own DPI button.
pub fn cycle_resolution(m: &Mouse, forward: bool) -> Result<u32, String> {
    let on: Vec<&Resolution> = m.resolutions.iter().filter(|r| !r.disabled).collect();
    let at = on.iter().position(|r| r.active).ok_or("this mouse has no active resolution")?;
    let next = on[(at + if forward { 1 } else { on.len() - 1 }) % on.len()];
    call(&next.path, "Resolution", "SetActive")?;
    commit(m)?;
    Ok(next.index)
}

/// Makes the next (or previous) enabled profile the active one.
pub fn cycle_profile(m: &Mouse, forward: bool) -> Result<u32, String> {
    let on: Vec<&Profile> = m.profiles.iter().filter(|p| !p.disabled).collect();
    let at = on.iter().position(|p| p.active).ok_or("this mouse has no active profile")?;
    set_profile(m, on[(at + if forward { 1 } else { on.len() - 1 }) % on.len()].index)
}

pub fn set_profile(m: &Mouse, index: u32) -> Result<u32, String> {
    let p = m.profiles.iter().find(|p| p.index == index && !p.disabled).ok_or_else(|| format!("{} has no profile {index}", m.name))?;
    call(&p.path, "Profile", "SetActive")?;
    commit(m)?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Output of `busctl --system --json=short get-property` as it prints it. The manager lines come
    // from the ratbagd 0.18 on this machine (no mouse plugged in); the others from a stand-in that
    // serves the same interfaces and types as libratbag's dbus.rst, read with the same busctl.
    const MANAGER: &str = "{\"type\":\"i\",\"data\":2}\n{\"type\":\"ao\",\"data\":[]}\n";
    const MANAGER_ONE: &str = "{\"type\":\"i\",\"data\":2}\n{\"type\":\"ao\",\"data\":[\"/org/freedesktop/ratbag1/device/event5\"]}\n";
    const DEVICE: &str = "{\"type\":\"s\",\"data\":\"Logitech G502 HERO\"}\n{\"type\":\"s\",\"data\":\"usb:046d:c077:0\"}\n{\"type\":\"ao\",\"data\":[\"/org/freedesktop/ratbag1/device/event5/p0\",\"/org/freedesktop/ratbag1/device/event5/p1\"]}\n";
    const PROFILE: &str = "{\"type\":\"u\",\"data\":0}\n{\"type\":\"s\",\"data\":\"Perfil 0\"}\n{\"type\":\"b\",\"data\":true}\n{\"type\":\"b\",\"data\":false}\n{\"type\":\"ao\",\"data\":[\"/org/freedesktop/ratbag1/device/event5/p0/r0\"]}\n";
    const RES_SINGLE: &str = "{\"type\":\"u\",\"data\":1}\n{\"type\":\"b\",\"data\":true}\n{\"type\":\"b\",\"data\":false}\n{\"type\":\"v\",\"data\":{\"type\":\"u\",\"data\":1600}}\n{\"type\":\"au\",\"data\":[100,200,1600,3200]}\n";
    const RES_SPLIT: &str = "{\"type\":\"u\",\"data\":0}\n{\"type\":\"b\",\"data\":false}\n{\"type\":\"b\",\"data\":false}\n{\"type\":\"v\",\"data\":{\"type\":\"(uu)\",\"data\":[400,400]}}\n{\"type\":\"au\",\"data\":[100,200,400]}\n";

    #[test]
    fn reads_the_manager() {
        let v = values(MANAGER);
        assert_eq!((v[0].as_i64(), strings(&v[1])), (Some(2), vec![]));
        assert_eq!(strings(&values(MANAGER_ONE)[1]), vec!["/org/freedesktop/ratbag1/device/event5"]);
    }

    #[test]
    fn reads_a_device_and_its_profile() {
        let d = values(DEVICE);
        assert_eq!((d[0].as_str(), d[1].as_str(), strings(&d[2]).len()), (Some("Logitech G502 HERO"), Some("usb:046d:c077:0"), 2));
        let p = profile("/p0", &values(PROFILE)).unwrap();
        assert_eq!(p, Profile { path: "/p0".into(), index: 0, name: "Perfil 0".into(), active: true, disabled: false });
        // Index, Name, IsActive, Disabled: a reply with another type in a slot is refused.
        assert!(profile("/p0", &values(RES_SINGLE)).is_none());
    }

    #[test]
    fn reads_both_resolution_types() {
        let one = resolution("/r1", &values(RES_SINGLE)).unwrap();
        assert_eq!((one.index, one.active, one.dpi, one.split, one.dpis.len()), (1, true, 1600, false, 4));
        let two = resolution("/r0", &values(RES_SPLIT)).unwrap();
        assert_eq!((two.dpi, two.split, two.active), (400, true, false));
    }

    #[test]
    fn steps_through_the_dpi_the_device_takes() {
        let r = |dpi: u32, active: bool, dpis: &[u32]| Resolution { dpi, active, dpis: dpis.to_vec(), ..Default::default() };
        let m = Mouse { resolutions: vec![r(400, false, &[]), r(1600, true, &[100, 400, 1600, 1800, 3200])], ..Default::default() };
        assert_eq!(dpi_step(&m, true, 1), Some(1800));
        assert_eq!(dpi_step(&m, true, 5), Some(3200));
        assert_eq!(dpi_step(&m, false, 1), Some(400));
        assert_eq!(dpi_step(&m, false, 30), Some(100));
        assert_eq!(dpi_step(&Mouse::default(), true, 1), None);
    }

    #[test]
    fn refuses_a_dpi_the_device_does_not_take() {
        let m = Mouse { name: "G".into(), resolutions: vec![Resolution { dpi: 800, active: true, dpis: vec![800, 1600], ..Default::default() }], ..Default::default() };
        assert!(set_dpi(&m, 900).is_err());
    }
}
