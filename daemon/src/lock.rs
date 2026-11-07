//! Whether the desktop session is locked or the computer is about to sleep, from logind, so the deck
//! can go dark. Works on any desktop that tells logind (GNOME, KDE, and lockers that set LockedHint
//! or answer `loginctl lock-session`).
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

static LOCKED: AtomicBool = AtomicBool::new(false);
static SLEEPING: AtomicBool = AtomicBool::new(false);

const LOGIND: &str = "org.freedesktop.login1";
const SESSION: &str = "org.freedesktop.login1.Session";

/// True while the session is locked or the computer is going to sleep.
pub fn dark() -> bool {
    LOCKED.load(Ordering::Relaxed) || SLEEPING.load(Ordering::Relaxed)
}

pub fn start() {
    std::thread::spawn(|| {
        loop {
            if let Err(e) = watch() {
                eprintln!("fluxdeck: cannot follow the session lock: {e}");
            }
            LOCKED.store(false, Ordering::Relaxed);
            SLEEPING.store(false, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

fn watch() -> zbus::Result<()> {
    let conn = Connection::system()?;
    // The user's graphical session; the service itself runs outside it.
    let user = Proxy::new(&conn, LOGIND, "/org/freedesktop/login1/user/self", "org.freedesktop.login1.User")?;
    let (_, session): (String, OwnedObjectPath) = user.get_property("Display")?;
    let props = Proxy::new(&conn, LOGIND, session.as_str(), SESSION)?;
    LOCKED.store(props.get_property::<bool>("LockedHint").unwrap_or(false), Ordering::Relaxed);
    let rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(LOGIND)?.build();
    for msg in MessageIterator::for_match_rule(rule, &conn, None)? {
        let msg = msg?;
        let h = msg.header();
        let (Some(member), path) = (h.member(), h.path()) else { continue };
        let on_session = path.is_some_and(|p| p.as_str() == session.as_str());
        match member.as_str() {
            "PrepareForSleep" => SLEEPING.store(msg.body().deserialize::<bool>()?, Ordering::Relaxed),
            "Lock" if on_session => LOCKED.store(true, Ordering::Relaxed),
            "Unlock" if on_session => LOCKED.store(false, Ordering::Relaxed),
            "PropertiesChanged" if on_session => {
                let (iface, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) = msg.body().deserialize()?;
                if let Some(v) = changed.get("LockedHint").filter(|_| iface == SESSION) {
                    LOCKED.store(bool::try_from(v).unwrap_or(false), Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
    Ok(())
}
