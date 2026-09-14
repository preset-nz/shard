//! MIDI input. One thread, no audio.
//!
//! Every input port is opened and its callback does nothing but push the
//! bytes onto a channel. The control thread drains that channel, feeds the
//! registry, and writes `controllers.json` when something was learned. Ports
//! are re-scanned once a second, which is how plugging in and pulling out are
//! noticed; CoreMIDI has notifications, midir does not expose them.
//!
//! Story 2 puts the map and pickup on this same thread: an event resolved
//! here becomes the same `set_param` the UI makes.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use midir::{Ignore, MidiInput, MidiInputConnection};

use crate::controllers::{Activity, Registry};

const RESCAN_EVERY: Duration = Duration::from_secs(1);

/// Shared with the Tauri commands. Lock order: `registry`, then `activity`.
pub struct Controllers {
    pub registry: Mutex<Registry>,
    pub activity: Mutex<Activity>,
    /// Ports open right now, by name. Written by the MIDI thread.
    pub connected: Mutex<Vec<String>>,
    pub path: PathBuf,
}

impl Controllers {
    /// Load the registry from `path`, or start empty. An unreadable file is
    /// reported and left alone rather than overwritten.
    pub fn load(path: PathBuf) -> Controllers {
        let registry = match std::fs::read_to_string(&path) {
            Ok(text) => match Registry::parse(&text) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "shard: {}: {e}; starting with no controllers",
                        path.display()
                    );
                    Registry::default()
                }
            },
            Err(_) => Registry::default(),
        };
        Controllers {
            registry: Mutex::new(registry),
            activity: Mutex::new(Activity::default()),
            connected: Mutex::new(Vec::new()),
            path,
        }
    }

    /// Write the registry out. Called after every change, on whichever thread
    /// made it; the file is small and changes are rare.
    pub fn save(&self, registry: &Registry) -> Result<(), String> {
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
    if let Some(ev) = event {
        shared
            .activity
            .lock()
            .expect("activity poisoned")
            .note(&ev, Instant::now());
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
