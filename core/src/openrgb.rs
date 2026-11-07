//! Lights of any brand through OpenRGB's SDK server (https://openrgb.org), on its default port.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const ADDR: &str = "127.0.0.1:6742";
/// Version 4 has every field used here; newer servers answer in it when asked.
const PROTOCOL: u32 = 4;

const REQUEST_CONTROLLER_COUNT: u32 = 0;
const REQUEST_CONTROLLER_DATA: u32 = 1;
const REQUEST_PROTOCOL_VERSION: u32 = 40;
const SET_CLIENT_NAME: u32 = 50;
const UPDATE_LEDS: u32 = 1050;
const UPDATE_MODE: u32 = 1101;

const HAS_BRIGHTNESS: u32 = 1 << 4;
const HAS_PER_LED_COLOR: u32 = 1 << 5;
const HAS_MODE_SPECIFIC_COLOR: u32 = 1 << 6;
const COLORS_PER_LED: u32 = 1;
const COLORS_MODE_SPECIFIC: u32 = 2;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mode {
    pub name: String,
    value: i32,
    flags: u32,
    speed_min: u32,
    speed_max: u32,
    brightness_min: u32,
    brightness_max: u32,
    colors_min: u32,
    colors_max: u32,
    speed: u32,
    brightness: u32,
    direction: u32,
    color_mode: u32,
    colors: Vec<u32>,
}


#[derive(Debug, Clone, PartialEq, Default)]
pub struct Device {
    /// The name, with "#2", "#3"… on repeated names (two identical RAM sticks).
    pub id: String,
    pub name: String,
    pub modes: Vec<Mode>,
    pub active: usize,
    index: u32,
    leds: usize,
    /// The first LED's color.
    color: Option<u32>,
}

impl Device {
    pub fn effect(&self) -> Option<&str> {
        self.modes.get(self.active).map(|m| m.name.as_str())
    }

    pub fn effects(&self) -> Vec<String> {
        self.modes.iter().map(|m| m.name.clone()).collect()
    }

    /// Brightness of the current effect in percent, when it has one.
    pub fn brightness(&self) -> Option<u8> {
        let m = self.modes.get(self.active).filter(|m| m.flags & HAS_BRIGHTNESS != 0 && m.brightness_max != m.brightness_min)?;
        let (lo, hi) = (m.brightness_min.min(m.brightness_max), m.brightness_min.max(m.brightness_max));
        Some(((m.brightness.clamp(lo, hi) - lo) * 100 / (hi - lo)) as u8)
    }

    /// Some boards report no LEDs, and then only effects with their own color can show one.
    fn colors(&self, m: &Mode) -> bool {
        m.flags & HAS_MODE_SPECIFIC_COLOR != 0 || m.flags & HAS_PER_LED_COLOR != 0 && self.leds > 0
    }

    /// The color shown now, as RRGGBB, when the effect uses one.
    pub fn color(&self) -> Option<String> {
        let m = self.modes.get(self.active).filter(|m| self.colors(m))?;
        let c = if m.color_mode == COLORS_MODE_SPECIFIC { m.colors.first().copied() } else { self.color }?;
        Some(format!("{:02x}{:02x}{:02x}", c & 0xff, c >> 8 & 0xff, c >> 16 & 0xff))
    }

    pub fn takes_color(&self) -> bool {
        self.modes.iter().any(|m| self.colors(m))
    }
}

struct Buf<'a> {
    d: &'a [u8],
    o: usize,
}

impl Buf<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.o..self.o.checked_add(n)?)?;
        self.o += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    /// Length-prefixed, with a trailing NUL.
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        let s = self.take(n)?;
        Some(String::from_utf8_lossy(s.strip_suffix(&[0]).unwrap_or(s)).into_owned())
    }
}

fn parse_mode(b: &mut Buf) -> Option<Mode> {
    let mut m = Mode { name: b.str()?, value: b.u32()? as i32, flags: b.u32()?, ..Default::default() };
    [m.speed_min, m.speed_max, m.brightness_min, m.brightness_max, m.colors_min, m.colors_max, m.speed, m.brightness, m.direction, m.color_mode] =
        [b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?, b.u32()?];
    m.colors = (0..b.u16()?).map(|_| b.u32()).collect::<Option<_>>()?;
    Some(m)
}

/// A REQUEST_CONTROLLER_DATA reply in protocol 4.
fn parse_device(index: u32, d: &[u8]) -> Option<Device> {
    let b = &mut Buf { d, o: 0 };
    b.u32()?; // size
    b.u32()?; // type
    let name = b.str()?;
    for _ in 0..5 {
        b.str()?; // vendor, description, version, serial, location
    }
    let count = b.u16()?;
    let active = b.u32()? as usize;
    let modes = (0..count).map(|_| parse_mode(b)).collect::<Option<Vec<_>>>()?;
    for _ in 0..b.u16()? {
        b.str()?;
        b.take(16)?; // type, LED min, max and count
        let matrix = b.u16()? as usize;
        b.take(matrix)?;
        for _ in 0..b.u16()? {
            b.str()?;
            b.take(12)?; // segment type, start and count
        }
    }
    for _ in 0..b.u16()? {
        b.str()?;
        b.u32()?;
    }
    let colors: Vec<u32> = (0..b.u16()?).map(|_| b.u32()).collect::<Option<_>>()?;
    Some(Device { id: name.clone(), name, modes, active, index, leds: colors.len(), color: colors.first().copied() })
}

fn mode_bytes(index: usize, m: &Mode) -> Vec<u8> {
    let mut out = vec![0; 4];
    out.extend((index as u32).to_le_bytes());
    out.extend((m.name.len() as u16 + 1).to_le_bytes());
    out.extend(m.name.as_bytes());
    out.push(0);
    out.extend(m.value.to_le_bytes());
    for v in [m.flags, m.speed_min, m.speed_max, m.brightness_min, m.brightness_max, m.colors_min, m.colors_max, m.speed, m.brightness, m.direction, m.color_mode] {
        out.extend(v.to_le_bytes());
    }
    out.extend((m.colors.len() as u16).to_le_bytes());
    for c in &m.colors {
        out.extend(c.to_le_bytes());
    }
    let size = out.len() as u32;
    out[..4].copy_from_slice(&size.to_le_bytes());
    out
}

struct Conn(TcpStream);

impl Conn {
    fn open() -> Option<Conn> {
        let s = TcpStream::connect_timeout(&ADDR.parse().ok()?, Duration::from_millis(300)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        let mut c = Conn(s);
        c.ask(0, REQUEST_PROTOCOL_VERSION, &PROTOCOL.to_le_bytes())?;
        c.send(0, SET_CLIENT_NAME, b"FluxDeck\0")?;
        Some(c)
    }

    fn send(&mut self, dev: u32, id: u32, data: &[u8]) -> Option<()> {
        let mut p = b"ORGB".to_vec();
        for v in [dev, id, data.len() as u32] {
            p.extend(v.to_le_bytes());
        }
        p.extend(data);
        self.0.write_all(&p).ok()
    }

    fn ask(&mut self, dev: u32, id: u32, data: &[u8]) -> Option<Vec<u8>> {
        self.send(dev, id, data)?;
        let mut head = [0u8; 16];
        self.0.read_exact(&mut head).ok()?;
        let mut body = vec![0; u32::from_le_bytes(head[12..16].try_into().ok()?) as usize];
        self.0.read_exact(&mut body).ok()?;
        Some(body)
    }

    fn devices(&mut self) -> Option<Vec<Device>> {
        let n = u32::from_le_bytes(self.ask(0, REQUEST_CONTROLLER_COUNT, &[])?.get(..4)?.try_into().ok()?);
        let mut out: Vec<Device> = (0..n).filter_map(|i| parse_device(i, &self.ask(i, REQUEST_CONTROLLER_DATA, &PROTOCOL.to_le_bytes())?)).collect();
        for i in 0..out.len() {
            let same = out[..i].iter().filter(|d| d.name == out[i].name).count();
            if same > 0 {
                out[i].id = format!("{}#{}", out[i].name, same + 1);
            }
        }
        Some(out)
    }
}

/// Devices OpenRGB controls; empty when its server is not running.
pub fn devices() -> Vec<Device> {
    Conn::open().and_then(|mut c| c.devices()).unwrap_or_default()
}

pub fn running() -> bool {
    Conn::open().is_some()
}

pub fn device(id: &str) -> Option<Device> {
    devices().into_iter().find(|d| d.id == id)
}

fn with(id: &str, change: impl FnOnce(&mut Conn, &Device) -> Option<()>) -> Result<(), String> {
    let mut c = Conn::open().ok_or("OpenRGB's SDK server is not running")?;
    let all = c.devices().ok_or("OpenRGB did not answer")?;
    let d = all.iter().find(|d| d.id == id).ok_or_else(|| format!("OpenRGB does not know {id}"))?;
    change(&mut c, d).ok_or_else(|| "OpenRGB closed the connection".into())
}

pub fn set_effect(id: &str, name: &str) -> Result<(), String> {
    let d = device(id).ok_or_else(|| format!("OpenRGB does not know {id}"))?;
    let i = d.modes.iter().position(|m| m.name == name).ok_or_else(|| format!("{id} has no effect {name}"))?;
    with(id, |c, d| c.send(d.index, UPDATE_MODE, &mode_bytes(i, &d.modes[i])))
}

/// Brightness of the current effect, in percent.
pub fn set_brightness(id: &str, percent: u8) -> Result<(), String> {
    device(id).and_then(|d| d.brightness()).ok_or("this effect has no brightness")?;
    with(id, |c, d| {
        let mut m = d.modes[d.active].clone();
        let (lo, hi) = (m.brightness_min.min(m.brightness_max), m.brightness_min.max(m.brightness_max));
        m.brightness = lo + (hi - lo) * percent.min(100) as u32 / 100;
        c.send(d.index, UPDATE_MODE, &mode_bytes(d.active, &m))
    })
}

/// One color on every LED. An effect without a color gives way to "Static", or else "Direct".
pub fn set_color(id: &str, rrggbb: &str) -> Result<(), String> {
    let hex = rrggbb.trim_start_matches('#');
    let rgb = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6).ok_or("the color goes as RRGGBB")?;
    let color = (rgb >> 16) | (rgb & 0xff00) | (rgb & 0xff) << 16;
    let d = device(id).ok_or_else(|| format!("OpenRGB does not know {id}"))?;
    let named = |n: &str| d.modes.iter().position(|m| m.name.eq_ignore_ascii_case(n) && d.colors(m));
    let i = Some(d.active)
        .filter(|&i| d.modes.get(i).is_some_and(|m| d.colors(m)))
        .or_else(|| named("static"))
        .or_else(|| named("direct"))
        .or_else(|| d.modes.iter().position(|m| d.colors(m)))
        .ok_or("this device cannot show a color")?;
    with(id, |c, d| {
        let mut m = d.modes[i].clone();
        if m.flags & HAS_MODE_SPECIFIC_COLOR != 0 && m.color_mode != COLORS_PER_LED {
            m.color_mode = COLORS_MODE_SPECIFIC;
            m.colors = vec![color; m.colors.len().max(m.colors_min as usize).max(1)];
            return c.send(d.index, UPDATE_MODE, &mode_bytes(i, &m));
        }
        if i != d.active {
            c.send(d.index, UPDATE_MODE, &mode_bytes(i, &m))?;
        }
        let mut leds = (d.leds as u32 * 4 + 6).to_le_bytes().to_vec();
        leds.extend((d.leds as u16).to_le_bytes());
        for _ in 0..d.leds {
            leds.extend(color.to_le_bytes());
        }
        c.send(d.index, UPDATE_LEDS, &leds)
    })
}

/// Spanish names for OpenRGB's usual effects; others keep theirs.
pub fn effect_name(name: &str) -> String {
    let t = |s: &'static str| crate::i18n::t(s).to_string();
    match name.to_ascii_lowercase().as_str() {
        "off" => t("Apagado"),
        "static" => t("Color fijo"),
        "direct" => t("Directo"),
        "breathing" => t("Respiración"),
        "flashing" => t("Destellos"),
        "rainbow" => t("Arcoíris"),
        "rainbow wave" => t("Ola arcoíris"),
        "spectrum cycle" | "color cycle" => t("Ciclo de colores"),
        "wave" => t("Ola"),
        "color wave" => t("Ola de color"),
        "reactive" => t("Reactivo"),
        "random" => t("Aleatorio"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(out: &mut Vec<u8>, v: &str) {
        out.extend((v.len() as u16 + 1).to_le_bytes());
        out.extend(v.as_bytes());
        out.push(0);
    }

    #[test]
    fn reads_a_device_and_writes_its_modes_back() {
        let modes = [
            Mode { name: "Static".into(), flags: HAS_MODE_SPECIFIC_COLOR | HAS_BRIGHTNESS, brightness_max: 255, brightness: 51, colors_min: 1, colors_max: 1, color_mode: 2, colors: vec![0x0000ff], ..Default::default() },
            Mode { name: "Rainbow".into(), value: 3, flags: 1, speed_max: 4, ..Default::default() },
        ];
        let mut d = vec![0; 8];
        for v in ["MSI", "MSI", "desc", "1.0", "", "HID: /dev/hidraw3"] {
            s(&mut d, v);
        }
        d.extend(2u16.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        for (i, m) in modes.iter().enumerate() {
            d.extend(&mode_bytes(i, m)[8..]);
        }
        // One zone with a 1×1 matrix and one segment.
        d.extend(1u16.to_le_bytes());
        s(&mut d, "Zone");
        d.extend([0; 16]);
        d.extend(12u16.to_le_bytes());
        d.extend([1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]);
        d.extend(1u16.to_le_bytes());
        s(&mut d, "Seg");
        d.extend([0; 12]);
        d.extend(2u16.to_le_bytes());
        for n in ["A", "B"] {
            s(&mut d, n);
            d.extend(0u32.to_le_bytes());
        }
        d.extend(2u16.to_le_bytes());
        d.extend([0x00ff00u32, 0].iter().flat_map(|c| c.to_le_bytes()));
        let dev = parse_device(7, &d).expect("parses");
        assert_eq!((dev.name.as_str(), dev.index, dev.leds, dev.active), ("MSI", 7, 2, 0));
        assert_eq!(dev.modes, modes);
        assert_eq!(dev.brightness(), Some(20));
        assert_eq!(dev.color().as_deref(), Some("ff0000"));
        assert!(parse_device(0, &d[..d.len() - 3]).is_none());
        let bytes = mode_bytes(1, &modes[1]);
        assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize, bytes.len());
    }
}
