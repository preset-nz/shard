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
    /// The hand's values. A caught knob writes here, like the UI does.
    pub bank: Arc<ParamBank>,
}

impl Controllers {
    /// Load the registry from `path`, or start empty. An unreadable file is
    /// reported and left alone rather than overwritten.
    pub fn load(path: PathBuf, bank: Arc<ParamBank>) -> Controllers {
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
            pickup: Mutex::new(Pickup::default()),
            learn: Mutex::new(Learn::default()),
            activity: Mutex::new(Activity::default()),
            connected: Mutex::new(Vec::new()),
            path,
            bank,
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
