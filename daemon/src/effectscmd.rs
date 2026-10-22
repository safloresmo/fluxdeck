//! `fluxdeck effects scan`: lists the LADSPA and LV2 effects installed here, as JSON.
//! It loads plugin libraries, so it runs as its own process: a broken plugin cannot take the editor down.
use fluxdeck_core::effects;
use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_ulong, c_void};
use std::path::PathBuf;

pub fn command(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("scan") => {
            let mut all = ladspa();
            all.extend(lv2());
            println!("{}", Value::Array(all));
            Ok(())
        }
        // Starts the chain as a copy of the system's microphone filter, or empty on the first microphone.
        Some("import") => {
            if effects::path().exists() {
                return Err(format!("{} already exists", effects::path().display()));
            }
            let chain = fluxdeck_core::pw::rnnoise_filters()
                .into_iter()
                .filter(|(f, _)| f.name != effects::CAPTURE)
                .find_map(|(f, _)| effects::import_system(&f.name))
                .ok_or("no microphone filter-chain config found")?;
            let chain = effects::Chain { description: fluxdeck_core::i18n::t("Micrófono con efectos").into(), ..chain };
            chain.save().map_err(|e| e.to_string())?;
            println!("{} effects copied to {}", chain.effects.len(), effects::path().display());
            Ok(())
        }
        _ => Err("usage: fluxdeck effects scan | import".into()),
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

#[repr(C)]
struct RangeHint {
    hint: c_int,
    lower: c_float,
    upper: c_float,
}

#[repr(C)]
struct LadspaDescriptor {
    unique_id: c_ulong,
    label: *const c_char,
    properties: c_int,
    name: *const c_char,
    maker: *const c_char,
    copyright: *const c_char,
    port_count: c_ulong,
    port_descriptors: *const c_int,
    port_names: *const *const c_char,
    port_range_hints: *const RangeHint,
}

const INPUT: c_int = 1;
const OUTPUT: c_int = 2;
const CONTROL: c_int = 4;
const AUDIO: c_int = 8;

/// A control's range and default from LADSPA hints (rate-relative bounds use 48 kHz).
fn ladspa_control(h: &RangeHint) -> (f64, f64, f64, bool, bool, bool) {
    let rate = if h.hint & 0x8 != 0 { 48000.0 } else { 1.0 };
    let (lo, hi) = (h.lower as f64 * rate, h.upper as f64 * rate);
    let (lo, hi) = (if h.hint & 0x1 != 0 { lo } else { 0.0 }, if h.hint & 0x2 != 0 { hi } else { lo.max(0.0) + 1.0 });
    let log = h.hint & 0x10 != 0 && lo > 0.0;
    let mix = |w: f64| if log { (lo.ln() * (1.0 - w) + hi.ln() * w).exp() } else { lo * (1.0 - w) + hi * w };
    let def = match h.hint & 0x3C0 {
        0x40 => lo,
        0x80 => mix(0.25),
        0xC0 => mix(0.5),
        0x100 => mix(0.75),
        0x140 => hi,
        0x200 => 0.0,
        0x240 => 1.0,
        0x280 => 100.0,
        0x2C0 => 440.0,
        _ => lo.max(0.0).min(hi),
    };
    (lo, hi, def, log, h.hint & 0x20 != 0, h.hint & 0x4 != 0)
}

fn ladspa_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("LADSPA_PATH").map(|p| p.split(':').map(PathBuf::from).collect()).unwrap_or_default();
    dirs.extend(["/usr/lib/ladspa", "/usr/local/lib/ladspa", "/usr/lib64/ladspa"].map(PathBuf::from));
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".ladspa"));
    }
    dirs
}

fn ladspa() -> Vec<Value> {
    let mut out = Vec::new();
    let files: Vec<PathBuf> = ladspa_dirs().iter().flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "so")).collect();
    // /usr/lib64 is often a link to /usr/lib: compare real paths.
    let mut files: Vec<PathBuf> = files.into_iter().map(|f| f.canonicalize().unwrap_or(f)).collect();
    files.sort();
    files.dedup();
    for file in files {
        let Ok(c) = CString::new(file.to_string_lossy().as_bytes()) else { continue };
        let lib = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            continue;
        }
        let sym = unsafe { libc::dlsym(lib, c"ladspa_descriptor".as_ptr()) };
        if sym.is_null() {
            continue;
        }
        let get: extern "C" fn(c_ulong) -> *const LadspaDescriptor = unsafe { std::mem::transmute(sym) };
        for i in 0.. {
            let d = get(i);
            if d.is_null() {
                break;
            }
            let d = unsafe { &*d };
            let n = d.port_count as usize;
            let (mut input, mut output, mut controls) = (None, None, Vec::new());
            for p in 0..n {
                let kind = unsafe { *d.port_descriptors.add(p) };
                let name = cstr(unsafe { *d.port_names.add(p) });
                if kind & AUDIO != 0 && kind & INPUT != 0 && input.is_none() {
                    input = Some(name);
                } else if kind & AUDIO != 0 && kind & OUTPUT != 0 && output.is_none() {
                    output = Some(name);
                } else if kind & CONTROL != 0 && kind & INPUT != 0 {
                    let (min, max, default, log, int, toggle) = ladspa_control(unsafe { &*d.port_range_hints.add(p) });
                    controls.push(json!({ "name": name, "min": min, "max": max, "default": default, "log": log, "int": int, "toggle": toggle }));
                }
            }
            if let (Some(input), Some(output)) = (input, output) {
                out.push(json!({
                    "kind": "ladspa", "plugin": file, "label": cstr(d.label), "name": cstr(d.name), "maker": cstr(d.maker),
                    "input": input, "output": output, "controls": controls,
                }));
            }
        }
        // Libraries stay loaded: some plugins misbehave when unloaded, and this process ends right away.
    }
    out
}

type P = *mut c_void;

struct Lilv {
    lib: P,
}

impl Lilv {
    fn open() -> Option<Lilv> {
        let lib = unsafe { libc::dlopen(c"liblilv-0.so.0".as_ptr(), libc::RTLD_NOW) };
        (!lib.is_null()).then_some(Lilv { lib })
    }

    fn f<T>(&self, name: &CStr) -> Option<T> {
        let s = unsafe { libc::dlsym(self.lib, name.as_ptr()) };
        (!s.is_null()).then(|| unsafe { std::mem::transmute_copy(&s) })
    }
}

fn lv2() -> Vec<Value> {
    lv2_scan().unwrap_or_default()
}

fn lv2_scan() -> Option<Vec<Value>> {
    let l = Lilv::open()?;
    let world_new: extern "C" fn() -> P = l.f(c"lilv_world_new")?;
    let load_all: extern "C" fn(P) = l.f(c"lilv_world_load_all")?;
    let all_plugins: extern "C" fn(P) -> P = l.f(c"lilv_world_get_all_plugins")?;
    let begin: extern "C" fn(P) -> P = l.f(c"lilv_plugins_begin")?;
    let is_end: extern "C" fn(P, P) -> bool = l.f(c"lilv_plugins_is_end")?;
    let get: extern "C" fn(P, P) -> P = l.f(c"lilv_plugins_get")?;
    let next: extern "C" fn(P, P) -> P = l.f(c"lilv_plugins_next")?;
    let get_uri: extern "C" fn(P) -> P = l.f(c"lilv_plugin_get_uri")?;
    let get_name: extern "C" fn(P) -> P = l.f(c"lilv_plugin_get_name")?;
    let get_author: extern "C" fn(P) -> P = l.f(c"lilv_plugin_get_author_name")?;
    let as_string: extern "C" fn(P) -> *const c_char = l.f(c"lilv_node_as_string")?;
    let num_ports: extern "C" fn(P) -> u32 = l.f(c"lilv_plugin_get_num_ports")?;
    let port_at: extern "C" fn(P, u32) -> P = l.f(c"lilv_plugin_get_port_by_index")?;
    let is_a: extern "C" fn(P, P, P) -> bool = l.f(c"lilv_port_is_a")?;
    let has_property: extern "C" fn(P, P, P) -> bool = l.f(c"lilv_port_has_property")?;
    let symbol: extern "C" fn(P, P) -> P = l.f(c"lilv_port_get_symbol")?;
    let port_name: extern "C" fn(P, P) -> P = l.f(c"lilv_port_get_name")?;
    let ranges: extern "C" fn(P, *mut f32, *mut f32, *mut f32) = l.f(c"lilv_plugin_get_port_ranges_float")?;
    let new_uri: extern "C" fn(P, *const c_char) -> P = l.f(c"lilv_new_uri")?;

    let world = world_new();
    load_all(world);
    let uri = |s: &CStr| new_uri(world, s.as_ptr());
    let (audio, control, input, output) = (
        uri(c"http://lv2plug.in/ns/lv2core#AudioPort"),
        uri(c"http://lv2plug.in/ns/lv2core#ControlPort"),
        uri(c"http://lv2plug.in/ns/lv2core#InputPort"),
        uri(c"http://lv2plug.in/ns/lv2core#OutputPort"),
    );
    let (log_prop, int_prop, toggle_prop) = (
        uri(c"http://lv2plug.in/ns/ext/port-props#logarithmic"),
        uri(c"http://lv2plug.in/ns/lv2core#integer"),
        uri(c"http://lv2plug.in/ns/lv2core#toggled"),
    );
    let text = |n: P| if n.is_null() { String::new() } else { cstr(as_string(n)) };
    let plugins = all_plugins(world);
    let mut out = Vec::new();
    let mut it = begin(plugins);
    while !is_end(plugins, it) {
        let p = get(plugins, it);
        it = next(plugins, it);
        let n = num_ports(p) as usize;
        let (mut mins, mut maxs, mut defs) = (vec![f32::NAN; n], vec![f32::NAN; n], vec![f32::NAN; n]);
        ranges(p, mins.as_mut_ptr(), maxs.as_mut_ptr(), defs.as_mut_ptr());
        let (mut ain, mut aout, mut controls) = (None, None, Vec::new());
        for i in 0..n {
            let port = port_at(p, i as u32);
            let sym = text(symbol(p, port));
            if is_a(p, port, audio) && is_a(p, port, input) && ain.is_none() {
                ain = Some(sym);
            } else if is_a(p, port, audio) && is_a(p, port, output) && aout.is_none() {
                aout = Some(sym);
            } else if is_a(p, port, control) && is_a(p, port, input) {
                let f = |v: f32, d: f64| if v.is_nan() { d } else { v as f64 };
                let (min, max) = (f(mins[i], 0.0), f(maxs[i], 1.0));
                controls.push(json!({
                    "name": sym, "title": text(port_name(p, port)), "min": min, "max": max, "default": f(defs[i], min),
                    "log": has_property(p, port, log_prop), "int": has_property(p, port, int_prop), "toggle": has_property(p, port, toggle_prop),
                }));
            }
        }
        if let (Some(input), Some(output)) = (ain, aout) {
            out.push(json!({
                "kind": "lv2", "plugin": text(get_uri(p)), "label": "", "name": text(get_name(p)), "maker": text(get_author(p)),
                "input": input, "output": output, "controls": controls,
            }));
        }
    }
    Some(out)
}
