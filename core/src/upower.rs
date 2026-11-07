//! Batteries of wireless mice, keyboards, controllers and headsets of any brand, as UPower sees them.
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    pub model: String,
    /// UPower's kind: mouse, keyboard, headset, gaming-input…
    pub kind: String,
    pub level: u64,
}

impl Battery {
    /// The same kinds as corsair::icon: 0 keyboard, 1 mouse, 2 headset.
    pub fn kind_code(&self) -> Option<u64> {
        match self.kind.as_str() {
            "keyboard" => Some(0),
            "mouse" | "touchpad" => Some(1),
            "headset" | "headphones" => Some(2),
            _ => None,
        }
    }
}

/// Peripherals with a battery; the computer's own battery and power supplies are left out.
fn parse(dump: &str) -> Vec<Battery> {
    let mut out: Vec<Battery> = dump
        .split("\nDevice: ")
        .filter_map(|block| {
            let field = |name: &str| block.lines().find_map(|l| l.trim().strip_prefix(name)?.strip_prefix(':').map(|v| v.trim().to_string()));
            if block.contains("/DisplayDevice") || field("power supply")? != "no" {
                return None;
            }
            // The line after the header fields that names the kind, indented by two spaces.
            let kind = block.lines().skip(1).find(|l| l.starts_with("  ") && !l.starts_with("   ") && !l.contains(':'))?.trim().to_string();
            let level = field("percentage")?.split('%').next()?.trim().parse::<f64>().ok()? as u64;
            let model = field("model").filter(|m| !m.is_empty()).unwrap_or_else(|| kind.clone());
            Some(Battery { model, kind, level })
        })
        .collect();
    out.sort_by(|a, b| a.model.cmp(&b.model));
    out
}

pub fn batteries() -> Vec<Battery> {
    let Ok(o) = Command::new("upower").arg("--dump").env("LC_ALL", "C").output() else { return Vec::new() };
    parse(&format!("\n{}", String::from_utf8_lossy(&o.stdout)))
}

/// A peripheral's battery by its model name.
pub fn battery(model: &str) -> Option<u64> {
    batteries().into_iter().find(|b| b.model == model).map(|b| b.level)
}

#[cfg(test)]
mod tests {
    #[test]
    fn keeps_peripherals_only() {
        let dump = "
Device: /org/freedesktop/UPower/devices/battery_BAT0
  native-path:          BAT0
  model:                5B10W
  power supply:         yes
  battery
    percentage:          80%
Device: /org/freedesktop/UPower/devices/mouse_hidpp_battery_0
  native-path:          hidpp_battery_0
  model:                G502 LIGHTSPEED Wireless Gaming Mouse
  power supply:         no
  mouse
    present:             yes
    battery-level:       normal
    percentage:          55% (should be ignored)
Device: /org/freedesktop/UPower/devices/gaming_input_ps_controller_battery_0
  model:                Wireless Controller
  power supply:         no
  gaming-input
    percentage:          70.5%
Device: /org/freedesktop/UPower/devices/DisplayDevice
  power supply:         no
  unknown
    percentage:          0%
";
        let b = super::parse(dump);
        assert_eq!(b.len(), 2);
        assert_eq!((b[0].model.as_str(), b[0].kind.as_str(), b[0].level, b[0].kind_code()), ("G502 LIGHTSPEED Wireless Gaming Mouse", "mouse", 55, Some(1)));
        assert_eq!((b[1].kind.as_str(), b[1].level, b[1].kind_code()), ("gaming-input", 70, None));
    }
}
