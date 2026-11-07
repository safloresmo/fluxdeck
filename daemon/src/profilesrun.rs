//! Switches profiles with the focused window, on any desktop `focus` can follow.
//! Entering a profile shows its deck page, sets its group volumes and runs its command; leaving puts
//! the page and volumes back when the profile says so.
use crate::Msg;
use fluxdeck_core::groups::{self, Level};
use fluxdeck_core::profiles::{self, Profile};
use fluxdeck_core::{Config, mixer};
use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

pub fn start(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let mut state = Switcher { tx, active: None, saved: BTreeMap::new() };
        loop {
            // No source here yet (or the desktop restarted): look again later.
            let followed = crate::focus::follow(&mut |class, title| state.focus(class, title));
            std::thread::sleep(Duration::from_secs(if followed { 2 } else { 10 }));
        }
    });
}

struct Switcher {
    tx: Sender<Msg>,
    active: Option<Profile>,
    /// Group levels from before the active profile changed them.
    saved: BTreeMap<String, Level>,
}

impl Switcher {
    fn focus(&mut self, class: &str, title: &str) {
        let cfg = Config::load().unwrap_or_default();
        let next = profiles::for_window(&cfg.profiles, class, title).cloned();
        if next.as_ref().map(|p| &p.id) == self.active.as_ref().map(|p| &p.id) {
            return;
        }
        let mut touched_groups = false;
        if let Some(old) = self.active.take()
            && old.restore
        {
            for (id, level) in std::mem::take(&mut self.saved) {
                groups::set_level(&id, level);
                touched_groups = true;
            }
            // The next profile's page replaces this one; otherwise go back.
            if !old.page.is_empty() && next.as_ref().is_none_or(|n| n.page.is_empty()) {
                let _ = self.tx.send(Msg::ProfilePage(None));
            }
        }
        if let Some(p) = &next {
            let now = groups::levels();
            for (id, level) in &p.levels {
                self.saved.insert(id.clone(), now.get(id).copied().unwrap_or_default());
                groups::set_level(id, *level);
                touched_groups = true;
            }
            if !p.page.is_empty() {
                let _ = self.tx.send(Msg::ProfilePage(Some(p.page.clone())));
            }
            if !p.command.trim().is_empty() {
                let _ = crate::shell(&p.command, 0).stdout(std::process::Stdio::null()).spawn();
            }
            eprintln!("fluxdeck: profile «{}» on", p.name);
        }
        if touched_groups {
            groups::apply(&cfg.groups, &mixer::snapshot());
        }
        let _ = std::fs::write(profiles::active_path(), next.as_ref().map(|p| p.name.as_str()).unwrap_or(""));
        self.active = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn entering_and_leaving_a_profile() {
        let dir = std::env::temp_dir().join(format!("fluxdeck-profile-test-{}", std::process::id()));
        // Config, state and runtime files all go to a throwaway folder.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
            std::env::set_var("XDG_STATE_HOME", dir.join("state"));
            std::env::set_var("XDG_RUNTIME_DIR", dir.join("run"));
        }
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let mut p = profiles::starter();
        p.page = "juegos".into();
        let cfg = Config { profiles: vec![p], ..Default::default() };
        cfg.save().unwrap();
        groups::set_level("musica", Level { volume: 0.9, muted: false });

        let (tx, rx) = channel();
        let mut s = Switcher { tx, active: None, saved: BTreeMap::new() };
        s.focus("steam_app_570", "Dota 2");
        assert!(matches!(rx.try_recv(), Ok(Msg::ProfilePage(Some(ref page))) if page == "juegos"));
        assert_eq!(groups::levels()["musica"].volume, 0.4);
        assert_eq!(profiles::active().as_deref(), Some(s.active.as_ref().unwrap().name.as_str()));

        // Same profile again: nothing happens.
        s.focus("steam_app_570", "Dota 2 - chat");
        assert!(rx.try_recv().is_err());

        s.focus("kitty", "zsh");
        assert!(matches!(rx.try_recv(), Ok(Msg::ProfilePage(None))));
        assert_eq!(groups::levels()["musica"].volume, 0.9);
        assert_eq!(profiles::active(), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
