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

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use shard_dsp::arrangement;
use shard_dsp::ParamBank;

use crate::controllers::{describe_mode, Activity, Calibration, Registry, Role};
use crate::mapping::{Pickup, Target, Turn};
use crate::profile::{self, Does, Profile};

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

/// Calibration: working out whether a knob is a pot or an endless encoder by
/// turning it, rather than by asking the person which encoding it speaks.
#[derive(Debug, Default)]
pub struct Calibrate {
    pub active: Option<Calibration>,
    /// The last thing it said, numbered so the UI can tell a new one.
    pub report: Option<(u64, String)>,
    pub reports: u64,
}

impl Calibrate {
    pub fn say(&mut self, text: String) {
        self.reports += 1;
        self.report = Some((self.reports, text));
    }
}

/// Shared with the Tauri commands. Lock order: `registry`, `pickup`,
/// `learn`, `calibrate`, `activity`.
pub struct Controllers {
    pub registry: Mutex<Registry>,
    pub pickup: Mutex<Pickup>,
    pub learn: Mutex<Learn>,
    pub calibrate: Mutex<Calibrate>,
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
    /// The arrangement's values, for a knob mapped to an `arrangement.` id.
    pub arrangement: Arc<ParamBank>,
    /// The transport, shared with the audio thread. `request` is what was
    /// asked for; `playing` is what the engine actually did, and a light is
    /// drawn from the second one.
    pub play_request: Arc<AtomicBool>,
    pub playing: Arc<AtomicBool>,
}

impl Controllers {
    /// Load the registry from `path`, or start empty.
    ///
    /// **An unreadable file is moved aside, never overwritten.** Starting
    /// empty is fine; saving that empty registry is not, and the first port
    /// to appear saves. Without this, one unparseable byte — or a future
    /// `VERSION` bump, which `parse` rejects outright — would silently take
    /// every device, control and map with it a second after launch.
    pub fn load(
        path: PathBuf,
        bank: Arc<ParamBank>,
        arrangement: Arc<ParamBank>,
        play_request: Arc<AtomicBool>,
        playing: Arc<AtomicBool>,
    ) -> Controllers {
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
        // Controls belonging to a device shard now drives are not yours to
        // map, so they do not belong in the file. Self-healing rather than a
        // migration: a device that gains a profile sheds its learned controls
        // the next time shard starts.
        let mut registry = registry;
        let shed: Vec<u64> = registry
            .controls
            .iter()
            .filter(|c| profile::for_input(&c.address.device).is_some())
            .map(|c| c.id)
            .collect();
        for id in shed {
            let _ = registry.forget_control(id);
        }
        Controllers {
            registry: Mutex::new(registry),
            pickup: Mutex::new(Pickup::default()),
            learn: Mutex::new(Learn::default()),
            calibrate: Mutex::new(Calibrate::default()),
            activity: Mutex::new(Activity::default()),
            connected: Mutex::new(Vec::new()),
            path,
            trouble: Mutex::new(trouble),
            keep_file: AtomicBool::new(keep_file),
            bank,
            arrangement,
            play_request,
            playing,
        }
    }

    /// The bank that holds `id`, and where in it: the arrangement's for an
    /// `arrangement.` id, the patch's otherwise.
    pub fn locate(&self, id: &str) -> Option<(&ParamBank, usize)> {
        let bank = if id.starts_with(arrangement::PREFIX) {
            &self.arrangement
        } else {
            &self.bank
        };
        bank.index(id).map(|i| (bank.as_ref(), i))
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

/// The channel and number of a control change, or `None` for anything else.
fn decode_cc(bytes: &[u8]) -> Option<(u8, u8)> {
    match bytes {
        [status, number, _] if status >> 4 == 0xB => Some((status & 0x0F, number & 0x7F)),
        _ => None,
    }
}

/// Spawn the MIDI thread. Returns at once; the thread lives as long as the app.
pub fn start(shared: Arc<Controllers>) {
    std::thread::Builder::new()
        .name("shard-midi".into())
        .spawn(move || run(shared))
        .expect("midi thread spawns");
}

/// How often the surface is redrawn from state. A light must follow the
/// transport, not the press, so it is compared rather than pushed.
const FEEDBACK_EVERY: Duration = Duration::from_millis(40);

/// Everything the MIDI thread owns that is not shared.
#[derive(Default)]
struct Surfaces {
    /// Open output ports, by the *output* port name.
    out: BTreeMap<String, MidiOutputConnection>,
    /// The last transport state drawn on each profiled device, so nothing is
    /// sent while nothing has changed.
    drawn: BTreeMap<String, bool>,
}

fn run(shared: Arc<Controllers>) {
    let (tx, rx): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    let mut open: BTreeMap<String, MidiInputConnection<()>> = BTreeMap::new();
    let mut surfaces = Surfaces::default();
    let mut next_scan = Instant::now();
    let mut next_feedback = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_scan {
            rescan(&shared, &tx, &mut open, &mut surfaces);
            next_scan = now + RESCAN_EVERY;
        }
        if now >= next_feedback {
            draw(&shared, &mut surfaces);
            next_feedback = now + FEEDBACK_EVERY;
        }
        let wait = next_scan
            .min(next_feedback)
            .saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(m) => handle(&shared, &m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Redraw every profiled surface from the transport.
///
/// **Compared, not pushed.** The spacebar, the UI and a patch load all move
/// the transport without touching a pad, so a light driven by the press lies
/// the first time one of them does. This reads `playing` — what the engine
/// actually did — rather than `play_request`, which is only what was asked.
fn draw(shared: &Controllers, surfaces: &mut Surfaces) {
    let playing = shared.playing.load(Ordering::Relaxed);
    for p in profile::PROFILES {
        let Some(conn) = surfaces.out.get_mut(p.output) else {
            continue;
        };
        if surfaces.drawn.get(p.output) == Some(&playing) {
            continue;
        }
        let colour = if playing {
            profile::PLAYING
        } else {
            profile::STOPPED
        };
        let mut ok = true;
        for c in p.claims {
            if matches!(c.does, Does::TransportToggle) {
                ok &= conn.send(&p.light(c.led, colour)).is_ok();
            }
        }
        if ok {
            surfaces.drawn.insert(p.output.to_string(), playing);
        }
    }
}

/// Open a profiled device's output and put it into the mode that makes it
/// answer. Without this the Launch Control 3 has no LEDs, no screen and a
/// silent DAW port.
fn wake(p: &'static Profile, surfaces: &mut Surfaces) {
    if surfaces.out.contains_key(p.output) {
        return;
    }
    let Ok(out) = MidiOutput::new("shard") else {
        return;
    };
    let found = out
        .ports()
        .iter()
        .find(|port| out.port_name(port).is_ok_and(|n| n == p.output))
        .cloned();
    let Some(port) = found else {
        return;
    };
    match out.connect(&port, "shard out") {
        Ok(mut conn) => {
            for m in p.on_connect {
                let _ = conn.send(m);
            }
            eprintln!("shard: {} woke on {:?}", p.name, p.output);
            surfaces.out.insert(p.output.to_string(), conn);
            // Nothing has been drawn yet, so the next tick draws.
            surfaces.drawn.remove(p.output);
        }
        Err(e) => eprintln!("shard: could not open MIDI out {:?}: {e}", p.output),
    }
}

/// Hand a profiled device back to whatever else wants it.
pub fn release_surfaces() {
    for p in profile::PROFILES {
        let Ok(out) = MidiOutput::new("shard") else {
            continue;
        };
        let found = out
            .ports()
            .iter()
            .find(|port| out.port_name(port).is_ok_and(|n| n == p.output))
            .cloned();
        let Some(port) = found else { continue };
        if let Ok(mut conn) = out.connect(&port, "shard out") {
            for m in p.on_disconnect {
                let _ = conn.send(m);
            }
        }
    }
}

fn handle(shared: &Controllers, m: &Message) {
    // A profiled surface is not a set of controls to map, so its controls are
    // never learned: shard already knows what they are and answers for them.
    // The device itself is registered, so Settings can say it is there.
    if let Some(p) = profile::for_input(&m.port) {
        let mut registry = shared.registry.lock().expect("registry poisoned");
        if registry.see_device(&m.port) {
            if let Err(e) = shared.save(&registry) {
                eprintln!("shard: could not save controllers: {e}");
            }
        }
        drop(registry);
        let claim = decode_cc(&m.bytes).and_then(|(ch, cc)| p.claim(ch, cc));
        // A press, not a release: a toggle that fired on both would undo
        // itself. The target decides what a press means (decision 5).
        if let Some(claim) = claim {
            if m.bytes.get(2).is_some_and(|v| *v > 0) {
                match claim.does {
                    Does::TransportToggle => {
                        let now = shared.playing.load(Ordering::Relaxed);
                        shared.play_request.store(!now, Ordering::Relaxed);
                    }
                }
            }
        }
        return;
    }

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

    // Calibration takes the message rather than playing it, so working out
    // what a knob is does not drag whatever it is mapped to along with it.
    let mut cal = shared.calibrate.lock().expect("calibrate poisoned");
    if cal.active.as_ref().is_some_and(|c| c.control == ev.control) {
        let found = cal.active.as_mut().expect("just checked").feed(ev.value);
        if let Some(mode) = found {
            cal.active = None;
            let said = match registry.set_mode(ev.control, mode) {
                Ok(()) => {
                    let name = registry
                        .control(ev.control)
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    if let Err(e) = shared.save(&registry) {
                        eprintln!("shard: could not save controllers: {e}");
                    }
                    describe_mode(&name, mode)
                }
                Err(e) => e,
            };
            cal.say(said);
            // It was a stranger before and it is a stranger now.
            shared
                .pickup
                .lock()
                .expect("pickup poisoned")
                .forget(ev.control);
        }
        drop(cal);
        shared
            .activity
            .lock()
            .expect("activity poisoned")
            .note(&ev, Instant::now());
        return;
    }
    drop(cal);

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
    let Some((role, mode)) = registry.control(control).map(|c| (c.role, c.mode)) else {
        return;
    };
    match (target, role) {
        (Target::Param(id), Role::Knob) => {
            let Some((bank, index)) = shared.locate(id) else {
                return;
            };
            let def = &bank.defs()[index];
            let base = bank.get(index);
            // `None` for a pot, which arms; `Some(delta)` for an endless
            // encoder, which has no position and never does.
            let delta = mode.delta(value);
            let turn = shared
                .pickup
                .lock()
                .expect("pickup poisoned")
                .turn(control, value, delta, def, base);
            if let Turn::Write(v) = turn {
                bank.set(index, v);
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
    surfaces: &mut Surfaces,
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
    // A device that has gone takes its output with it.
    surfaces.out.retain(|out, _| {
        profile::PROFILES
            .iter()
            .any(|p| p.output == out && names.contains(&p.input.to_string()))
    });
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
                // A profiled device is put into the mode that makes it answer.
                if let Some(p) = profile::for_input(name) {
                    wake(p, surfaces);
                }
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
        Controllers::load(
            path.to_path_buf(),
            Arc::new(ParamBank::new()),
            Arc::new(ParamBank::for_table(arrangement::params())),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    /// A claimed control toggles the transport off `playing`, not off the
    /// press, and only on the press.
    #[test]
    fn button_eight_toggles_the_transport_and_ignores_the_release() {
        let path = scratch("claim").join("controllers.json");
        let c = load(&path);
        let press = Message {
            port: "LC3 1 DAW Out".into(),
            bytes: vec![0xB0, 44, 127],
        };
        let release = Message {
            port: "LC3 1 DAW Out".into(),
            bytes: vec![0xB0, 44, 0],
        };

        handle(&c, &press);
        assert!(c.play_request.load(Ordering::Relaxed), "a press starts it");
        // The release must not undo the press.
        handle(&c, &release);
        assert!(c.play_request.load(Ordering::Relaxed));

        // The engine caught up; the next press stops it.
        c.playing.store(true, Ordering::Relaxed);
        handle(&c, &press);
        assert!(
            !c.play_request.load(Ordering::Relaxed),
            "the next press stops it"
        );
    }

    #[test]
    fn a_surface_registers_itself_but_none_of_its_controls() {
        let path = scratch("surface").join("controllers.json");
        let c = load(&path);
        for bytes in [vec![0xB0, 44, 127], vec![0xBF, 13, 65], vec![0xB0, 39, 127]] {
            handle(
                &c,
                &Message {
                    port: "LC3 1 DAW Out".into(),
                    bytes,
                },
            );
        }
        let r = c.registry.lock().unwrap();
        // Visible in Settings...
        assert_eq!(r.devices.len(), 1);
        assert_eq!(r.devices[0].port, "LC3 1 DAW Out");
        // ...but its controls are shard's to drive, not yours to map.
        assert!(r.controls.is_empty(), "a surface learns no controls");
    }

    /// A device that gains a profile sheds the controls learned before it had
    /// one, rather than leaving them to clutter Settings for ever.
    #[test]
    fn a_file_holding_a_surface_s_old_controls_sheds_them_on_load() {
        let path = scratch("shed").join("controllers.json");
        let mut r = Registry::default();
        r.observe("LC3 1 DAW Out", &[0xB0, 44, 127]);
        r.observe("LPD8", &[0xB0, 1, 40]);
        std::fs::write(&path, r.to_json()).unwrap();

        let c = load(&path);
        let r = c.registry.lock().unwrap();
        assert_eq!(r.controls.len(), 1, "only the LPD8's control survives");
        assert_eq!(r.controls[0].address.device, "LPD8");
    }

    /// The same bytes from a device with no profile are ordinary traffic and
    /// reach the map instead.
    #[test]
    fn an_unprofiled_device_sending_the_same_cc_touches_no_transport() {
        let path = scratch("unclaimed").join("controllers.json");
        let c = load(&path);
        handle(
            &c,
            &Message {
                port: "LPD8".into(),
                bytes: vec![0xB0, 44, 127],
            },
        );
        assert!(!c.play_request.load(Ordering::Relaxed));
        // It was still learned, so it shows up in Settings.
        assert_eq!(c.registry.lock().unwrap().controls.len(), 1);
    }

    #[test]
    fn a_control_change_is_decoded_and_nothing_else_is() {
        assert_eq!(decode_cc(&[0xB0, 44, 127]), Some((0, 44)));
        assert_eq!(decode_cc(&[0xBF, 13, 64]), Some((15, 13)));
        assert_eq!(decode_cc(&[0x90, 44, 127]), None);
        assert_eq!(decode_cc(&[0xB0, 44]), None);
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

    #[test]
    fn a_knob_finds_the_arrangements_rows_in_the_arrangements_bank() {
        let dir = scratch("locate");
        let c = load(&dir.join("controllers.json"));
        let (bank, i) = c.locate("arrangement.crush.mix").expect("arrangement row");
        assert_eq!(bank.defs()[i].id, "arrangement.crush.mix");
        let (bank, i) = c.locate("crush.mix").expect("patch row");
        assert_eq!(bank.defs()[i].id, "crush.mix");
        assert!(c.locate("arrangement.crush.env.amount").is_none());
    }
}
