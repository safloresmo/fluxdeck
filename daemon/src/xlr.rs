//! Phantom power and low-impedance switches of the Elgato XLR Dock (0fd9:00a6).
//!
//! Layout from the OpenWave and OpenXLR reverse engineering: a 34-byte config block read and
//! written with class requests to interface 3. Only the two bytes below are ever changed.
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::PathBuf;

const PID: &str = "00a6";
const BLOCK: usize = 34;
const INDEX: u16 = 0x3303;
const PHANTOM: usize = 6;
const LOWZ: usize = 33;

#[repr(C)]
struct CtrlTransfer {
    request_type: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    timeout: u32,
    data: *mut u8,
}

// _IOWR('U', 0, struct usbdevfs_ctrltransfer)
const USBDEVFS_CONTROL: u64 = (3 << 30) | ((size_of::<CtrlTransfer>() as u64) << 16) | ((b'U' as u64) << 8);

fn device() -> Result<PathBuf, String> {
    for entry in std::fs::read_dir("/sys/bus/usb/devices").map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        let read = |f: &str| std::fs::read_to_string(p.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
        if read("idVendor") == "0fd9" && read("idProduct") == PID {
            let (bus, dev): (u32, u32) = (read("busnum").parse().unwrap_or(0), read("devnum").parse().unwrap_or(0));
            return Ok(PathBuf::from(format!("/dev/bus/usb/{bus:03}/{dev:03}")));
        }
    }
    Err("no XLR Dock (0fd9:00a6) connected".into())
}

fn transfer(f: &File, request_type: u8, request: u8, buf: &mut [u8; BLOCK]) -> Result<(), String> {
    let mut t = CtrlTransfer { request_type, request, value: 0, index: INDEX, length: BLOCK as u16, timeout: 1000, data: buf.as_mut_ptr() };
    // SAFETY: `t` points at a live buffer of `length` bytes for the duration of the call.
    let n = unsafe { libc::ioctl(f.as_raw_fd(), USBDEVFS_CONTROL as _, &mut t) };
    if n == BLOCK as i32 {
        Ok(())
    } else if n < 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Err(format!("short transfer: {n} of {BLOCK} bytes"))
    }
}

fn read(f: &File) -> Result<[u8; BLOCK], String> {
    let mut buf = [0u8; BLOCK];
    transfer(f, 0xA1, 0x85, &mut buf)?;
    Ok(buf)
}

/// `fluxdeck xlr phantom|lowz [on|off|toggle]`: prints the state after any change.
pub fn command(args: &[String]) -> Result<(), String> {
    let offset = match args.first().map(String::as_str) {
        Some("phantom") => PHANTOM,
        Some("lowz") => LOWZ,
        _ => return Err("usage: fluxdeck xlr phantom|lowz [on|off|toggle]".into()),
    };
    let path = device()?;
    // usbfs refuses control transfers on a read-only handle, even for reads.
    let f = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| {
        format!("{}: {e} (install dist/70-fluxdeck.rules so your user can reach the Dock)", path.display())
    })?;
    let mut cfg = read(&f)?;
    if let Some(action) = args.get(1) {
        let on = match action.as_str() {
            "on" => true,
            "off" => false,
            "toggle" => cfg[offset] == 0,
            _ => return Err(format!("unknown action {action}")),
        };
        cfg[offset] = on as u8;
        transfer(&f, 0x21, 0x05, &mut cfg.clone())?;
        if read(&f)?[offset] != on as u8 {
            return Err("the Dock did not keep the change".into());
        }
    }
    println!("{}", if cfg[offset] != 0 { "on" } else { "off" });
    Ok(())
}
