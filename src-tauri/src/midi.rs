//! MIDI input. One thread, no audio.
//!
//! Every input port is opened and its callback does nothing but push the
//! bytes onto a channel. The control thread drains that channel, feeds the
//! registry, and writes `controllers.json` when something was learned. Ports
//! are re-scanned once a second, which is how plugging in and pulling out are
//! noticed; CoreMIDI has notifications, midir does not expose them.
//!
//! The map and pickup live on this thread too: a knob that has caught its
//! value makes the same bank write the UI's `set_param` makes, and the audio
//! thread learns nothing new. Learning is a slot the UI arms; the next control
//! to move is bound and the slot answers with a sentence.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use midir::{Ignore, MidiInput, MidiInputConnection};
use shard_dsp::params::{index_of, PARAMS};
use shard_dsp::ParamBank;

use crate::controllers::{Activity, Registry, Role};
use crate::mapping::{Pickup, Target, Turn};

const RESCAN_EVERY: Duration = Duration::from_secs(1);

/// Learning: what the UI is waiting to bind, and what the last bind said.
#[derive(Debug, Default)]
pub struct Learn {
    /// The target waiting for a control to move.
    pub waiting: Option<Target>,
    /// The last thing learning said, numbered so the UI can tell a new one.
    pub report: Option<(u64, Result<String, String>)>,
    pub reports: u64,
}

impl Learn {
    pub fn say(&mut self, r: Result<String, String>) {
        self.reports += 1;
        self.report = Some((self.reports, r));
    }
}

/// Shared with the Tauri commands. Lock order: `registry`, `pickup`,
/// `learn`, `activity`.
pub struct Controllers {
    pub registry: Mutex<Registry>,
    pub pickup: Mutex<Pickup>,
    pub learn: Mutex<Learn>,
    pub activity: Mutex<Activity>,
    /// Ports open right now, by name. Written by the MIDI thread.
    pub connected: Mutex<Vec<String>>,
    pub path: PathBuf,
    /// What went wrong with `controllers.json` at startup, for the panel to
    /// say. `None` in the ordinary case, including a first run with no file.
    pub trouble: Mutex<Option<String>>,
    /// Set when the file could not be read *and* could not be moved aside.
    /// Saving would write an empty registry over devices, controls and maps
    /// that are probably all still there, so saving is refused instead.
    keep_file: AtomicBool,
    /// The hand's values. A caught knob writes here, like the UI does.
    pub bank: Arc<ParamBank>,
}

impl Controllers {
    /// Load the registry from `path`, or start empty.
    ///
    /// **An unreadable file is moved aside, never overwritten.** Starting
    /// empty is fine; saving that empty registry is not, and the first port
    /// to appear saves. Without this, one unparseable byte — or a future
    /// `VERSION` bump, which `parse` rejects outright — would silently take
    /// every device, control and map with it a second after launch.
    pub fn load(path: PathBuf, bank: Arc<ParamBank>) -> Controllers {
        let (registry, trouble, keep_file) = match std::fs::read_to_string(&path) {
            // No file is the first run, and there is nothing to lose.
            Err(_) => (Registry::default(), None, false),
            Ok(text) => match Registry::parse(&text) {
                Ok(r) => (r, None, false),
                Err(e) => {
                    let aside = path.with_extension("json.broken");
                    match std::fs::rename(&path, &aside) {
                        Ok(()) => (
                            Registry::default(),
                            Some(format!(
                                "{e}. Starting with no controllers; the old file is kept as {}.",
                                aside.display()
                            )),
                            false,
                        ),
                        // Cannot read it, cannot move it: leave it alone.
                        Err(m) => (
                            Registry::default(),
                            Some(format!(
                                "{e}. It could not be moved aside ({m}), so nothing is being \
                                 saved until that file is dealt with by hand."
                            )),
                            true,
                        ),
                    }
                }
            },
        };
        if let Some(t) = &trouble {
            eprintln!("shard: {}: {t}", path.display());
        }
        Controllers {
            registry: Mutex::new(registry),
            pickup: Mutex::new(Pickup::default()),
            learn: Mutex::new(Learn::default()),
            activity: Mutex::new(Activity::default()),
            connected: Mutex::new(Vec::new()),
            path,
            trouble: Mutex::new(trouble),
            keep_file: AtomicBool::new(keep_file),
            bank,
        }
    }

    /// Write the registry out. Called after every change, on whichever thread
    /// made it; the file is small and changes are rare. Refused outright when
    /// the file on disk holds controllers this build could not read.
    pub fn save(&self, registry: &Registry) -> Result<(), String> {
        if self.keep_file.load(Ordering::Relaxed) {
            return Err(format!(
                "{} holds controllers this build cannot read; move it aside to start fresh",
                self.path.display()
            ));
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, registry.to_json()).map_err(|e| e.to_string())
    }
}

struct Message {
    port: String,
    bytes: Vec<u8>,
}

/// Spawn the MIDI thread. Returns at once; the thread lives as long as the app.
pub fn start(shared: Arc<Controllers>) {
    std::thread::Builder::new()
        .name("shard-midi".into())
        .spawn(move || run(shared))
        .expect("midi thread spawns");
}

fn run(shared: Arc<Controllers>) {
    let (tx, rx): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    let mut open: BTreeMap<String, MidiInputConnection<()>> = BTreeMap::new();
    let mut next_scan = Instant::now();
    loop {
        if Instant::now() >= next_scan {
            rescan(&shared, &tx, &mut open);
            next_scan = Instant::now() + RESCAN_EVERY;
        }
        let wait = next_scan.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(m) => handle(&shared, &m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn handle(shared: &Controllers, m: &Message) {
    let mut registry = shared.registry.lock().expect("registry poisoned");
    let (event, changed) = registry.observe(&m.port, &m.bytes);
    if changed {
        if let Err(e) = shared.save(&registry) {
            eprintln!("shard: could not save controllers: {e}");
        }
    }
    let Some(ev) = event else {
        return;
    };

    // Learning takes the message rather than playing it.
    let mut learn = shared.learn.lock().expect("learn poisoned");
    if let Some(target) = learn.waiting.take() {
        let result = registry.learn(ev.control, target);
        if result.is_ok() {
            shared
                .pickup
                .lock()
                .expect("pickup poisoned")
                .forget(ev.control);
            if let Err(e) = shared.save(&registry) {
                eprintln!("shard: could not save controllers: {e}");
            }
        }
        learn.say(result);
    } else {
        drop(learn);
        play(shared, &registry, ev.control, ev.value);
    }

    shared
        .activity
        .lock()
        .expect("activity poisoned")
        .note(&ev, Instant::now());
}

/// Resolve one control movement through the active map and apply it.
fn play(shared: &Controllers, registry: &Registry, control: u64, value: u8) {
    let Some(map) = registry.active_map() else {
        return;
    };
    let Some(target) = map.mappings.get(&control) else {
        return;
    };
    let Some(role) = registry.control(control).map(|c| c.role) else {
        return;
    };
    match (target, role) {
        (Target::Param(id), Role::Knob) => {
            let Some(index) = index_of(id) else {
                return;
            };
            let def = &PARAMS[index];
            let base = shared.bank.get(index);
            let turn = shared
                .pickup
                .lock()
                .expect("pickup poisoned")
                .turn(control, value, def, base);
            if let Turn::Write(v) = turn {
                shared.bank.set(index, v);
            }
        }
        // Pads reach nothing until story 3.
        (Target::Param(_), Role::Pad) => {}
    }
}

/// Open every port that is not open, drop every connection whose port has
/// gone. A port that reappears is reopened on the next scan.
fn rescan(
    shared: &Controllers,
    tx: &Sender<Message>,
    open: &mut BTreeMap<String, MidiInputConnection<()>>,
) {
    let Ok(mut probe) = MidiInput::new("shard") else {
        return;
    };
    probe.ignore(Ignore::All);
    let ports = probe.ports();
    let names: Vec<String> = ports
        .iter()
        .filter_map(|p| probe.port_name(p).ok())
        .collect();
    open.retain(|name, _| names.contains(name));
    for (port, name) in ports.iter().zip(names.iter()) {
        if open.contains_key(name) {
            continue;
        }
        let Ok(mut input) = MidiInput::new("shard") else {
            continue;
        };
        input.ignore(Ignore::All);
        let tx = tx.clone();
        let port_name = name.clone();
        let conn = input.connect(
            port,
            "shard in",
            move |_, bytes, _| {
                // The callback owns nothing and blocks on nothing. A full
                // channel is impossible (it is unbounded) and a closed one
                // means the app is quitting.
                let _ = tx.send(Message {
                    port: port_name.clone(),
                    bytes: bytes.to_vec(),
                });
            },
            (),
        );
        match conn {
            Ok(c) => {
                open.insert(name.clone(), c);
                let mut registry = shared.registry.lock().expect("registry poisoned");
                if registry.see_device(name) {
                    if let Err(e) = shared.save(&registry) {
                        eprintln!("shard: could not save controllers: {e}");
                    }
                }
            }
            Err(e) => eprintln!("shard: could not open MIDI port {name:?}: {e}"),
        }
    }
    *shared.connected.lock().expect("connected poisoned") = open.keys().cloned().collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of our own, so two tests never share a file.
    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shard-midi-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn load(path: &std::path::Path) -> Controllers {
        Controllers::load(path.to_path_buf(), Arc::new(ParamBank::new()))
    }

    #[test]
    fn a_good_file_loads_with_nothing_to_report() {
        let path = scratch("good").join("controllers.json");
        let mut r = Registry::default();
        r.observe("LPD8", &[0xB0, 1, 10]);
        std::fs::write(&path, r.to_json()).unwrap();
        let c = load(&path);
        assert_eq!(c.registry.lock().unwrap().controls.len(), 1);
        assert!(c.trouble.lock().unwrap().is_none());
    }

    #[test]
    fn a_missing_file_is_a_first_run_and_saves_normally() {
        let path = scratch("missing").join("controllers.json");
        let c = load(&path);
        assert!(c.trouble.lock().unwrap().is_none());
        c.save(&c.registry.lock().unwrap()).unwrap();
        assert!(path.exists());
    }

    /// The bug this guards: starting empty is fine, but the first port to
    /// appear saves, and that save used to land on top of the devices,
    /// controls and maps we had merely failed to parse.
    #[test]
    fn an_unreadable_file_is_moved_aside_rather_than_overwritten() {
        for (what, text) in [
            ("garbage", "{ not json".to_string()),
            ("version", {
                let mut r = Registry::default();
                r.observe("LPD8", &[0xB0, 1, 10]);
                r.to_json().replace("\"version\": 1", "\"version\": 9")
            }),
        ] {
            let path = scratch(what).join("controllers.json");
            std::fs::write(&path, &text).unwrap();
            let c = load(&path);
            assert!(c.registry.lock().unwrap().controls.is_empty());
            assert!(c.trouble.lock().unwrap().is_some(), "{what} should report");

            // The old file is still readable, byte for byte.
            let aside = path.with_extension("json.broken");
            assert_eq!(std::fs::read_to_string(&aside).unwrap(), text, "{what}");

            // And the app is usable: a fresh file writes, the old one stays.
            c.save(&c.registry.lock().unwrap()).unwrap();
            assert!(Registry::parse(&std::fs::read_to_string(&path).unwrap()).is_ok());
            assert_eq!(std::fs::read_to_string(&aside).unwrap(), text, "{what}");
        }
    }

    /// Unreadable *and* immovable: refuse to save at all, rather than take
    /// the one copy of the controllers with us.
    #[test]
    fn a_file_that_cannot_be_moved_aside_blocks_saving() {
        let dir = scratch("stuck");
        let path = dir.join("controllers.json");
        std::fs::write(&path, "{ not json").unwrap();
        // A directory sits where the rename target must go, so it fails.
        std::fs::create_dir(path.with_extension("json.broken")).unwrap();
        let c = load(&path);
        assert!(c.trouble.lock().unwrap().is_some());
        assert!(c.save(&c.registry.lock().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }
}
