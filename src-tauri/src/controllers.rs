//! Controllers: MIDI devices and the controls learned from them.
//!
//! Story 1 of `controller-mapping.md`. Nothing here reaches the audio thread.
//! A device is a port name. A control is one knob or pad on it, learned the
//! first time it is touched and named by you. Roles are inferred from what
//! arrives: a note is a pad, a CC is a knob unless it only ever sends 0 and
//! 127, which is how a pad in CC mode looks.
//!
//! **The registry lives with the app, not the patch.** Devices, controls and
//! (later) maps belong to this machine and this person, so they share one
//! versioned file, `controllers.json`, in the app config directory.
//!
//! The LPD8 on Georg's desk (2026-09-14) sends its four programs on channels
//! 1, 2, 1 and 4, so programs 1 and 3 share every knob. Learning on touch is
//! indifferent to that; whether it matters is story 5's problem.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::mapping::{valid_map_name, Map, Target};

/// Bumped when the file's shape changes. There are no old files to migrate.
pub const VERSION: u32 = 1;

/// A control that moved within this long is drawn lit.
pub const ACTIVE_FOR: Duration = Duration::from_millis(250);

const MAX_NAME: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Cc,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Knob,
    Pad,
}

impl Role {
    pub const NAMES: [&'static str; 2] = ["knob", "pad"];

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "knob" => Some(Role::Knob),
            "pad" => Some(Role::Pad),
            _ => None,
        }
    }
}

/// Whether a control sends a position or a change.
///
/// A pot or a fader has somewhere it physically is, and says so: that is
/// `Absolute`, and pickup exists to stop it jumping. An endless encoder has
/// no position at all and sends how far it just moved, in one of three
/// encodings that look alike on a slow turn. **Never inferred.** A right turn
/// under `BinaryOffset` sends 65, which is a left turn under `SignedBit`, so
/// guessing from traffic is circular — control-plane.md §5 warns about this
/// and it is right. Set it by hand, then turn the encoder one click left: the
/// panel shows the decoded change, and the correct encoding reads −1.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Absolute,
    /// 1..63 up, 65..127 down, the sign in bit 6. Behringer "rel-1".
    SignedBit,
    /// 1..63 up, 127..65 down. Two's complement.
    TwosComplement,
    /// 64 is no change, above is up, below is down. Behringer "rel-3".
    BinaryOffset,
}

impl Mode {
    pub const NAMES: [&'static str; 4] = [
        "absolute",
        "signed bit",
        "two's complement",
        "binary offset",
    ];

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "absolute" => Some(Mode::Absolute),
            "signed bit" => Some(Mode::SignedBit),
            "two's complement" => Some(Mode::TwosComplement),
            "binary offset" => Some(Mode::BinaryOffset),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        Mode::NAMES[match self {
            Mode::Absolute => 0,
            Mode::SignedBit => 1,
            Mode::TwosComplement => 2,
            Mode::BinaryOffset => 3,
        }]
    }

    /// True for an endless encoder, which never arms pickup.
    pub fn is_relative(self) -> bool {
        self != Mode::Absolute
    }

    /// How far this message says the encoder moved, or `None` when the
    /// control sends a position instead.
    pub fn delta(self, value: u8) -> Option<i8> {
        let v = i16::from(value & 0x7F);
        let d = match self {
            Mode::Absolute => return None,
            Mode::SignedBit if v >= 64 => 64 - v,
            Mode::SignedBit => v,
            Mode::TwosComplement if v >= 64 => v - 128,
            Mode::TwosComplement => v,
            Mode::BinaryOffset => v - 64,
        };
        Some(d as i8)
    }
}

/// How many messages one direction needs before pot and encoder are told
/// apart. Three, not two, so a pot would have to repeat itself three times
/// running to be mistaken for one.
pub const SAMPLES: usize = 3;

/// Every encoding a knob might be speaking, in the order they are tried.
const RELATIVE: [Mode; 3] = [Mode::SignedBit, Mode::TwosComplement, Mode::BinaryOffset];

/// Whether a run of messages from turning one way came from an encoder.
///
/// **A pot's number moves and an encoder's does not.** Turn a pot three
/// clicks and it says 66, 67, 68, because it is reporting where it now is.
/// Turn an encoder three clicks and it says 1, 1, 1, because it is reporting
/// that it moved, and it moved the same amount each time. That is structural,
/// not a threshold to tune.
pub fn looks_relative(run: &[u8]) -> bool {
    run.len() >= SAMPLES && run.windows(2).all(|w| w[0] == w[1])
}

/// Which encoding fits a known right turn and a known left turn.
///
/// Knowing the direction is the whole trick. Passively, a 65 could be a
/// right turn under binary offset or a left turn under signed bit, which is
/// why control-plane.md §5 forbids inferring from traffic. Once you have
/// *asked* for a right turn and then a left turn, only one encoding reads
/// the first as positive and the second as negative. Where more than one
/// does, the smallest movement wins, because one click should read as one.
///
/// `None` means this pair settles nothing — a message that trailed in from
/// the right turn, most likely — so keep waiting rather than guessing.
pub fn encoding_for(right: u8, left: u8) -> Option<Mode> {
    let mut best: Option<(Mode, i16)> = None;
    for m in RELATIVE {
        let (Some(r), Some(l)) = (m.delta(right), m.delta(left)) else {
            continue;
        };
        if r > 0 && l < 0 {
            let cost = i16::from(r) - i16::from(l);
            if best.is_none_or(|(_, c)| cost < c) {
                best = Some((m, cost));
            }
        }
    }
    best.map(|(m, _)| m)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Right,
    Left,
}

/// Working out what a knob is by asking it, rather than asking you.
///
/// Nobody should have to know what "two's complement" means to use a knob.
/// Turn it right a few clicks, then left, and the answer falls out — this is
/// control-plane.md §5's "turn-left test", with the app reading the result
/// instead of the person.
#[derive(Debug, Clone)]
pub struct Calibration {
    pub control: u64,
    pub step: Step,
    right: Vec<u8>,
}

impl Calibration {
    pub fn new(control: u64) -> Calibration {
        Calibration {
            control,
            step: Step::Right,
            right: Vec::new(),
        }
    }

    /// What to say to the person right now.
    pub fn instruction(&self) -> &'static str {
        match self.step {
            Step::Right => "Turn it a few clicks to the right.",
            Step::Left => "Now turn it back to the left.",
        }
    }

    /// One message from the control being calibrated. `Some` once it knows,
    /// `None` while it still wants turning.
    pub fn feed(&mut self, value: u8) -> Option<Mode> {
        match self.step {
            Step::Right => {
                self.right.push(value);
                if self.right.len() < SAMPLES {
                    return None;
                }
                if !looks_relative(&self.right) {
                    return Some(Mode::Absolute);
                }
                self.step = Step::Left;
                None
            }
            // A message that settles nothing is one that trailed in from the
            // right turn. Ignore it and wait for a real left turn.
            Step::Left => encoding_for(self.right[0], value),
        }
    }
}

/// What calibration found, in words that do not assume you read the manual.
pub fn describe_mode(name: &str, mode: Mode) -> String {
    match mode {
        Mode::Absolute => {
            format!("{name} is a pot: it sends where it is, so it takes over by pickup.")
        }
        m => format!("{name} is an endless encoder, reading {}.", m.name()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// The port name CoreMIDI gives it. Identity, never edited.
    pub port: String,
    pub name: String,
}

/// A named channel on a device.
///
/// **A channel is how these boxes spell a bank.** The LPD8's four programs
/// come out on channels 1, 2, 1 and 4; the Launch Control's templates do the
/// same. Nothing announces the switch, so the channel is the only evidence,
/// and "ch 5" tells you nothing a month later. Naming it turns the long flat
/// list of controls into something you can read: "LC3 · Mixer · Send A 1".
///
/// This is not a page in the [`Map`] sense. A map is a set of bindings you
/// switch between in the app; a bank is a face of the hardware.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bank {
    pub device: String,
    pub channel: u8,
    pub name: String,
}

/// What a channel is called when you have not named it. One-based, the way
/// every piece of hardware and every manual counts channels.
pub fn default_bank_name(channel: u8) -> String {
    format!("Ch {}", channel + 1)
}

/// Where a message came from, ignoring its value. Two messages with the same
/// address are the same physical control.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Address {
    pub device: String,
    pub channel: u8,
    pub kind: Kind,
    pub number: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Control {
    pub id: u64,
    #[serde(flatten)]
    pub address: Address,
    pub role: Role,
    /// Absolute unless you say otherwise. A new field on an existing file
    /// reads as `absolute`, which is what every control was before this.
    #[serde(default)]
    pub mode: Mode,
    pub name: String,
    /// True once you or a value other than 0 and 127 has settled the role.
    /// Until then a CC that has only sent 0 and 127 may still be flipped to
    /// a pad. Not saved: a restart trusts the saved role.
    #[serde(skip)]
    pub role_settled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registry {
    pub version: u32,
    pub devices: Vec<Device>,
    pub controls: Vec<Control>,
    /// Ids are never reused, like LFO ids. Renaming and forgetting must not
    /// make a later map point at the wrong knob.
    pub next_control_id: u64,
    /// Named maps, app-wide. A patch names the one it wants.
    #[serde(default)]
    pub maps: Vec<Map>,
    #[serde(default = "one")]
    pub next_map_id: u64,
    #[serde(default)]
    pub active: Option<u64>,
    /// Names for the channels you have bothered to name. A channel with no
    /// entry draws as `Ch N`; there is no point storing a row per channel
    /// that is only ever called what it was already called.
    #[serde(default)]
    pub banks: Vec<Bank>,
}

fn one() -> u64 {
    1
}

impl Default for Registry {
    fn default() -> Self {
        Registry {
            version: VERSION,
            devices: Vec::new(),
            controls: Vec::new(),
            next_control_id: 1,
            maps: Vec::new(),
            next_map_id: 1,
            active: None,
            banks: Vec::new(),
        }
    }
}

/// What one incoming message meant, once the registry has seen it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub control: u64,
    /// CC value, or note velocity (0 for a release).
    pub value: u8,
    /// Note off, or note on with velocity 0.
    pub released: bool,
}

impl Registry {
    pub fn parse(text: &str) -> Result<Registry, String> {
        let r: Registry = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if r.version != VERSION {
            return Err(format!(
                "controllers.json is version {}, this build reads {VERSION}",
                r.version
            ));
        }
        let max = r.controls.iter().map(|c| c.id).max().unwrap_or(0);
        if r.next_control_id <= max {
            return Err("controllers.json reuses a control id".into());
        }
        let max = r.maps.iter().map(|m| m.id).max().unwrap_or(0);
        if r.next_map_id <= max {
            return Err("controllers.json reuses a map id".into());
        }
        Ok(r)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("registry serialises")
    }

    /// A port has appeared. Registers it under its own name if it is new.
    /// Returns true when the file changed.
    pub fn see_device(&mut self, port: &str) -> bool {
        if self.devices.iter().any(|d| d.port == port) {
            return false;
        }
        self.devices.push(Device {
            port: port.to_string(),
            name: port.to_string(),
        });
        true
    }

    /// One raw MIDI message from `port`. Learns the control if it is new and
    /// refines a CC's role while it is unsettled. Returns the event and
    /// whether the registry changed and wants saving.
    pub fn observe(&mut self, port: &str, msg: &[u8]) -> (Option<Event>, bool) {
        let Some((address, value, released)) = decode(port, msg) else {
            return (None, false);
        };
        let mut changed = self.see_device(port);
        let idx = match self.controls.iter().position(|c| c.address == address) {
            Some(i) => i,
            None => {
                let id = self.next_control_id;
                self.next_control_id += 1;
                let role = infer_role(address.kind, value);
                let name = default_name(&address);
                self.controls.push(Control {
                    id,
                    address,
                    role,
                    mode: Mode::Absolute,
                    name,
                    role_settled: role == Role::Knob,
                });
                changed = true;
                self.controls.len() - 1
            }
        };
        let control = &mut self.controls[idx];
        if !control.role_settled && control.role == Role::Pad && control.address.kind == Kind::Cc {
            // A "pad" that sends a value between the ends is a knob at one end.
            if value != 0 && value != 127 {
                control.role = Role::Knob;
                control.role_settled = true;
                changed = true;
            }
        }
        (
            Some(Event {
                control: control.id,
                value,
                released,
            }),
            changed,
        )
    }

    pub fn rename_control(&mut self, id: u64, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a control needs a name".into());
        }
        if name.chars().count() > MAX_NAME {
            return Err(format!("a name is at most {MAX_NAME} characters"));
        }
        let c = self.control_mut(id)?;
        c.name = name.to_string();
        Ok(())
    }

    pub fn set_role(&mut self, id: u64, role: Role) -> Result<(), String> {
        let c = self.control_mut(id)?;
        c.role = role;
        c.role_settled = true;
        // A pad is always absolute. Without this the invariant only holds
        // one way: `set_mode` refuses a relative mode on a pad, but calling
        // a relative knob a pad would leave the mode behind, where `play`
        // ignores it and the panel does not draw the select to fix it.
        if role == Role::Pad {
            c.mode = Mode::Absolute;
        }
        Ok(())
    }

    /// Say whether a control is a pot or an endless encoder. A pad has no
    /// position to send either way, so the question only applies to knobs.
    pub fn set_mode(&mut self, id: u64, mode: Mode) -> Result<(), String> {
        let c = self.control_mut(id)?;
        if c.role != Role::Knob && mode.is_relative() {
            return Err(format!("{} is a pad, so it sends no position", c.name));
        }
        c.mode = mode;
        Ok(())
    }

    pub fn forget_control(&mut self, id: u64) -> Result<(), String> {
        let n = self.controls.len();
        self.controls.retain(|c| c.id != id);
        if self.controls.len() == n {
            return Err(format!("no control {id}"));
        }
        for m in &mut self.maps {
            m.mappings.remove(&id);
        }
        Ok(())
    }

    pub fn rename_device(&mut self, port: &str, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a device needs a name".into());
        }
        let d = self
            .devices
            .iter_mut()
            .find(|d| d.port == port)
            .ok_or_else(|| format!("no device {port:?}"))?;
        d.name = name.to_string();
        Ok(())
    }

    /// Forget a device and every control learned from it.
    pub fn forget_device(&mut self, port: &str) -> Result<(), String> {
        let n = self.devices.len();
        self.devices.retain(|d| d.port != port);
        if self.devices.len() == n {
            return Err(format!("no device {port:?}"));
        }
        let gone: Vec<u64> = self
            .controls
            .iter()
            .filter(|c| c.address.device == port)
            .map(|c| c.id)
            .collect();
        self.controls.retain(|c| c.address.device != port);
        self.banks.retain(|b| b.device != port);
        for m in &mut self.maps {
            m.mappings.retain(|c, _| !gone.contains(c));
        }
        Ok(())
    }

    pub fn control(&self, id: u64) -> Option<&Control> {
        self.controls.iter().find(|c| c.id == id)
    }

    /// What this device's channel is called.
    pub fn bank_name(&self, device: &str, channel: u8) -> String {
        self.banks
            .iter()
            .find(|b| b.device == device && b.channel == channel)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| default_bank_name(channel))
    }

    /// Name a channel, or hand it back its default name by clearing it.
    pub fn rename_bank(&mut self, device: &str, channel: u8, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.chars().count() > MAX_NAME {
            return Err(format!("a name is at most {MAX_NAME} characters"));
        }
        self.banks
            .retain(|b| !(b.device == device && b.channel == channel));
        // Blank, or the name it would have anyway, stores nothing.
        if !name.is_empty() && name != default_bank_name(channel) {
            self.banks.push(Bank {
                device: device.to_string(),
                channel,
                name: name.to_string(),
            });
        }
        Ok(())
    }

    pub fn active_map(&self) -> Option<&Map> {
        let id = self.active?;
        self.maps.iter().find(|m| m.id == id)
    }

    /// The active map, made if there is none yet. The first map is "live".
    pub fn active_map_mut(&mut self) -> &mut Map {
        if self.active_map().is_none() {
            let id = match self.maps.first() {
                Some(m) => m.id,
                None => self.add_map("live").expect("a valid name"),
            };
            self.active = Some(id);
        }
        let id = self.active.expect("just set");
        self.maps
            .iter_mut()
            .find(|m| m.id == id)
            .expect("active map exists")
    }

    pub fn add_map(&mut self, name: &str) -> Result<u64, String> {
        let name = valid_map_name(name)?;
        let id = self.next_map_id;
        self.next_map_id += 1;
        self.maps.push(Map {
            id,
            name,
            mappings: BTreeMap::new(),
        });
        Ok(id)
    }

    pub fn rename_map(&mut self, id: u64, name: &str) -> Result<(), String> {
        let name = valid_map_name(name)?;
        let m = self
            .maps
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| format!("no map {id}"))?;
        m.name = name;
        Ok(())
    }

    /// Switch maps. True if the map exists; false leaves the active one.
    pub fn set_active(&mut self, id: u64) -> bool {
        if self.maps.iter().any(|m| m.id == id) {
            self.active = Some(id);
            true
        } else {
            false
        }
    }

    /// Point `control` at `target` in the active map. Refuses a role that
    /// does not fit: only knobs reach parameters until story 3. Answers with
    /// a sentence for the UI.
    pub fn learn(&mut self, control: u64, target: Target) -> Result<String, String> {
        let c = self
            .control(control)
            .ok_or_else(|| format!("no control {control}"))?;
        if c.role != Role::Knob {
            return Err(format!(
                "{} is a pad; pads reach switches and gates in a later story",
                c.name
            ));
        }
        let name = c.name.clone();
        let what = target.describe();
        let replaced = self.active_map_mut().bind(control, target);
        Ok(match replaced.and_then(|r| self.control(r)) {
            Some(prev) => format!("{name} now sets {what}, taking it from {}", prev.name),
            None => format!("{name} now sets {what}"),
        })
    }

    /// Take `target` off its control in the active map.
    pub fn unlearn(&mut self, target: &Target) -> Result<(), String> {
        if self.active_map_mut().unbind(target) {
            Ok(())
        } else {
            Err(format!("{} has no control", target.describe()))
        }
    }

    fn control_mut(&mut self, id: u64) -> Result<&mut Control, String> {
        self.controls
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("no control {id}"))
    }
}

/// Note the trap for endless encoders: one in two's complement, turned
/// slowly anticlockwise, only ever sends 1 and 127, and 127 is in the pad
/// set. Nothing later rescues it, because a middle value never arrives, so
/// it stays a pad until you say otherwise in the panel. Inferring harder
/// does not help — the encoding is exactly what cannot be inferred — so the
/// panel's mode tooltip explains it instead.
fn infer_role(kind: Kind, first_value: u8) -> Role {
    match kind {
        Kind::Note => Role::Pad,
        Kind::Cc if first_value == 0 || first_value == 127 => Role::Pad,
        Kind::Cc => Role::Knob,
    }
}

fn default_name(a: &Address) -> String {
    match a.kind {
        Kind::Cc => format!("CC {}", a.number),
        Kind::Note => format!("Note {}", a.number),
    }
}

/// Channel voice messages only. Everything else (clock, sysex, aftertouch,
/// program change, bends) is ignored for now.
fn decode(port: &str, msg: &[u8]) -> Option<(Address, u8, bool)> {
    if msg.len() < 3 {
        return None;
    }
    let status = msg[0] >> 4;
    let channel = msg[0] & 0x0F;
    let number = msg[1] & 0x7F;
    let value = msg[2] & 0x7F;
    let (kind, value, released) = match status {
        0xB => (Kind::Cc, value, false),
        0x9 if value > 0 => (Kind::Note, value, false),
        0x9 | 0x8 => (Kind::Note, 0, true),
        _ => return None,
    };
    Some((
        Address {
            device: port.to_string(),
            channel,
            kind,
            number,
        },
        value,
        released,
    ))
}

/// Recent activity per control, for the indicator in Settings. Kept beside
/// the registry, not in it, because it is never saved.
#[derive(Debug, Default)]
pub struct Activity {
    last: BTreeMap<u64, (Instant, u8)>,
}

impl Activity {
    pub fn note(&mut self, ev: &Event, at: Instant) {
        self.last.insert(ev.control, (at, ev.value));
    }

    pub fn forget(&mut self, control: u64) {
        self.last.remove(&control);
    }

    /// Whether the control moved within `ACTIVE_FOR` of `now`, and its last value.
    pub fn of(&self, control: u64, now: Instant) -> (bool, Option<u8>) {
        match self.last.get(&control) {
            Some(&(at, v)) => (now.duration_since(at) < ACTIVE_FOR, Some(v)),
            None => (false, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(ch: u8, n: u8, v: u8) -> [u8; 3] {
        [0xB0 | ch, n, v]
    }
    fn on(ch: u8, n: u8, v: u8) -> [u8; 3] {
        [0x90 | ch, n, v]
    }
    fn off(ch: u8, n: u8) -> [u8; 3] {
        [0x80 | ch, n, 0]
    }

    #[test]
    fn a_touched_control_is_learned_once_and_keeps_its_id() {
        let mut r = Registry::default();
        let (e1, changed) = r.observe("LPD8", &cc(0, 1, 10));
        assert!(changed);
        let (e2, changed) = r.observe("LPD8", &cc(0, 1, 11));
        assert!(!changed);
        assert_eq!(e1.unwrap().control, e2.unwrap().control);
        assert_eq!(r.controls.len(), 1);
        assert_eq!(r.devices.len(), 1);
        assert_eq!(r.devices[0].port, "LPD8");
    }

    #[test]
    fn same_number_on_another_channel_or_device_is_another_control() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 10));
        r.observe("LPD8", &cc(3, 1, 10));
        r.observe("nanoKONTROL", &cc(0, 1, 10));
        assert_eq!(r.controls.len(), 3);
        assert_eq!(r.devices.len(), 2);
    }

    #[test]
    fn notes_are_pads_and_moving_ccs_are_knobs() {
        let mut r = Registry::default();
        r.observe("LPD8", &on(9, 36, 100));
        r.observe("LPD8", &cc(0, 1, 64));
        assert_eq!(r.controls[0].role, Role::Pad);
        assert_eq!(r.controls[1].role, Role::Knob);
    }

    #[test]
    fn a_cc_that_only_sends_the_ends_is_a_pad_until_it_moves() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 20, 127));
        r.observe("LPD8", &cc(0, 20, 0));
        assert_eq!(r.controls[0].role, Role::Pad);
        let (_, changed) = r.observe("LPD8", &cc(0, 20, 60));
        assert!(changed);
        assert_eq!(r.controls[0].role, Role::Knob);
    }

    #[test]
    fn a_role_set_by_hand_is_not_overruled() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 20, 127));
        let id = r.controls[0].id;
        r.set_role(id, Role::Pad).unwrap();
        r.observe("LPD8", &cc(0, 20, 60));
        assert_eq!(r.controls[0].role, Role::Pad);
    }

    #[test]
    fn note_off_and_zero_velocity_both_release() {
        let mut r = Registry::default();
        let (e, _) = r.observe("LPD8", &on(9, 36, 100));
        assert!(!e.unwrap().released);
        let (e, _) = r.observe("LPD8", &off(9, 36));
        assert!(e.unwrap().released);
        let (e, _) = r.observe("LPD8", &on(9, 36, 0));
        assert!(e.unwrap().released);
        assert_eq!(r.controls.len(), 1);
    }

    #[test]
    fn other_messages_are_ignored() {
        let mut r = Registry::default();
        assert_eq!(r.observe("LPD8", &[0xF8]), (None, false));
        assert_eq!(r.observe("LPD8", &[0xC3, 0x02]), (None, false));
        assert_eq!(r.observe("LPD8", &[0xD9, 0x4E]), (None, false));
        assert!(r.controls.is_empty());
    }

    #[test]
    fn ids_are_never_reused() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 10));
        r.forget_control(1).unwrap();
        r.observe("LPD8", &cc(0, 1, 10));
        assert_eq!(r.controls[0].id, 2);
    }

    #[test]
    fn forgetting_a_device_takes_its_controls() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 10));
        r.observe("other", &cc(0, 1, 10));
        r.forget_device("LPD8").unwrap();
        assert_eq!(r.controls.len(), 1);
        assert_eq!(r.controls[0].address.device, "other");
        assert!(r.forget_device("LPD8").is_err());
    }

    #[test]
    fn the_file_round_trips_and_rejects_the_wrong_version() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 10));
        r.rename_control(1, "K1").unwrap();
        r.rename_device("LPD8", "Pads").unwrap();
        let back = Registry::parse(&r.to_json()).unwrap();
        assert_eq!(back.controls[0].name, "K1");
        assert_eq!(back.devices[0].name, "Pads");
        assert_eq!(back.next_control_id, 2);
        let json = r.to_json().replace("\"version\": 1", "\"version\": 9");
        assert!(Registry::parse(&json).is_err());
        let json = r
            .to_json()
            .replace("\"next_control_id\": 2", "\"next_control_id\": 1");
        assert!(Registry::parse(&json).is_err());
    }

    #[test]
    fn names_are_bounded() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 10));
        assert!(r.rename_control(1, "  ").is_err());
        assert!(r.rename_control(1, &"x".repeat(41)).is_err());
        assert!(r.rename_control(1, "K1").is_ok());
        assert!(r.rename_control(7, "K1").is_err());
    }

    #[test]
    fn learning_makes_the_first_map_and_pads_are_refused() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 40));
        r.observe("LPD8", &on(9, 36, 100));
        let t = Target::Param("grain.size".into());
        assert!(r.learn(2, t.clone()).is_err());
        assert!(r.active_map().is_none());
        let msg = r.learn(1, t.clone()).unwrap();
        assert!(msg.contains("grain.size"), "{msg}");
        assert_eq!(r.active_map().unwrap().name, "live");
        assert_eq!(r.active_map().unwrap().control_for(&t), Some(1));
        r.unlearn(&t).unwrap();
        assert!(r.unlearn(&t).is_err());
    }

    #[test]
    fn forgetting_a_control_or_device_takes_its_mappings() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 40));
        r.observe("LPD8", &cc(0, 2, 40));
        r.learn(1, Target::Param("grain.size".into())).unwrap();
        r.learn(2, Target::Param("grain.density".into())).unwrap();
        r.forget_control(1).unwrap();
        assert_eq!(r.active_map().unwrap().mappings.len(), 1);
        r.forget_device("LPD8").unwrap();
        assert!(r.active_map().unwrap().mappings.is_empty());
    }

    #[test]
    fn maps_round_trip_and_ids_hold() {
        let mut r = Registry::default();
        r.observe("LPD8", &cc(0, 1, 40));
        r.learn(1, Target::Param("grain.size".into())).unwrap();
        let second = r.add_map("sound design").unwrap();
        assert!(r.set_active(second));
        assert!(!r.set_active(99));
        r.rename_map(second, "design").unwrap();
        let back = Registry::parse(&r.to_json()).unwrap();
        assert_eq!(back.active, Some(second));
        assert_eq!(back.maps[1].name, "design");
        assert_eq!(
            back.maps[0].mappings[&1],
            Target::Param("grain.size".into())
        );
        assert_eq!(back.next_map_id, 3);
    }

    #[test]
    fn each_encoding_decodes_one_click_each_way() {
        // The turn-left test: one click anticlockwise is −1 under the right
        // encoding, and nothing like it under the other two.
        assert_eq!(Mode::SignedBit.delta(1), Some(1));
        assert_eq!(Mode::SignedBit.delta(65), Some(-1));
        assert_eq!(Mode::TwosComplement.delta(1), Some(1));
        assert_eq!(Mode::TwosComplement.delta(127), Some(-1));
        assert_eq!(Mode::BinaryOffset.delta(65), Some(1));
        assert_eq!(Mode::BinaryOffset.delta(63), Some(-1));
        assert_eq!(Mode::BinaryOffset.delta(64), Some(0));
        // A pot has no change to report, only a position.
        assert_eq!(Mode::Absolute.delta(64), None);
        // Faster turns carry further, and the two 7-bit schemes disagree
        // about which way a big value leans. That is the whole reason this
        // is set by hand.
        assert_eq!(Mode::SignedBit.delta(70), Some(-6));
        assert_eq!(Mode::TwosComplement.delta(70), Some(-58));
    }

    #[test]
    fn every_mode_name_round_trips() {
        for name in Mode::NAMES {
            let m = Mode::parse(name).expect(name);
            assert_eq!(m.name(), name);
        }
        assert!(Mode::parse("endless").is_none());
        assert!(!Mode::Absolute.is_relative());
        assert!(Mode::TwosComplement.is_relative());
    }

    #[test]
    fn a_pad_cannot_be_an_endless_encoder() {
        let mut r = Registry::default();
        r.observe("LPD8", &on(9, 36, 100));
        assert!(r.set_mode(1, Mode::TwosComplement).is_err());
        assert!(r.set_mode(1, Mode::Absolute).is_ok());
        assert_eq!(r.controls[0].mode, Mode::Absolute);
    }

    #[test]
    fn calling_an_encoder_a_pad_takes_its_mode_with_it() {
        let mut r = Registry::default();
        r.observe("LC3", &cc(0, 13, 65));
        r.set_mode(1, Mode::TwosComplement).unwrap();
        // Otherwise the mode is stranded: play ignores a pad's parameter
        // mapping, and the panel only draws the mode select for a knob.
        r.set_role(1, Role::Pad).unwrap();
        assert_eq!(r.controls[0].mode, Mode::Absolute);
    }

    #[test]
    fn a_control_keeps_its_mode_across_a_save() {
        let mut r = Registry::default();
        r.observe("LC3", &cc(0, 13, 65));
        r.set_mode(1, Mode::TwosComplement).unwrap();
        let back = Registry::parse(&r.to_json()).unwrap();
        assert_eq!(back.controls[0].mode, Mode::TwosComplement);
    }

    #[test]
    fn a_channel_is_named_once_and_only_when_it_differs() {
        let mut r = Registry::default();
        r.observe("LC3", &cc(4, 13, 40));
        assert_eq!(r.bank_name("LC3", 4), "Ch 5");
        assert!(r.banks.is_empty());

        r.rename_bank("LC3", 4, "Mixer").unwrap();
        assert_eq!(r.bank_name("LC3", 4), "Mixer");
        assert_eq!(r.banks.len(), 1);
        // Another device's channel 5 is a different channel.
        assert_eq!(r.bank_name("LPD8", 4), "Ch 5");

        // Blank hands back the default, and stores nothing.
        r.rename_bank("LC3", 4, "  ").unwrap();
        assert_eq!(r.bank_name("LC3", 4), "Ch 5");
        assert!(r.banks.is_empty());
        // So does naming it what it was already called.
        r.rename_bank("LC3", 4, "Ch 5").unwrap();
        assert!(r.banks.is_empty());
        assert!(r.rename_bank("LC3", 4, &"x".repeat(41)).is_err());
    }

    #[test]
    fn channel_names_round_trip_and_a_forgotten_device_takes_them() {
        let mut r = Registry::default();
        r.observe("LC3", &cc(4, 13, 40));
        r.observe("LPD8", &cc(0, 1, 40));
        r.rename_bank("LC3", 4, "Mixer").unwrap();
        r.rename_bank("LPD8", 0, "Program 1").unwrap();
        let mut back = Registry::parse(&r.to_json()).unwrap();
        assert_eq!(back.bank_name("LC3", 4), "Mixer");
        back.forget_device("LC3").unwrap();
        assert_eq!(back.bank_name("LC3", 4), "Ch 5");
        assert_eq!(back.bank_name("LPD8", 0), "Program 1");
    }

    #[test]
    fn a_pot_and_an_encoder_are_told_apart_by_whether_the_number_moves() {
        // A pot reports where it is, so three clicks are three numbers.
        assert!(!looks_relative(&[66, 67, 68]));
        // An encoder reports that it moved, so three clicks are one number.
        assert!(looks_relative(&[1, 1, 1]));
        // Not enough to say yet.
        assert!(!looks_relative(&[1, 1]));
        // A pot that happens to repeat twice is still not an encoder.
        assert!(!looks_relative(&[40, 40, 41]));
    }

    #[test]
    fn a_right_turn_then_a_left_turn_names_the_encoding() {
        assert_eq!(encoding_for(1, 65), Some(Mode::SignedBit));
        assert_eq!(encoding_for(1, 127), Some(Mode::TwosComplement));
        assert_eq!(encoding_for(65, 63), Some(Mode::BinaryOffset));
        // Three clicks each way lands on the same answer.
        assert_eq!(encoding_for(3, 125), Some(Mode::TwosComplement));
        assert_eq!(encoding_for(3, 67), Some(Mode::SignedBit));
        assert_eq!(encoding_for(67, 61), Some(Mode::BinaryOffset));
    }

    #[test]
    fn a_message_that_settles_nothing_is_ignored_rather_than_guessed() {
        // The same value twice means the second one trailed in from the
        // right turn; no encoding reads one number as both directions.
        for v in [1u8, 65, 127, 3] {
            assert_eq!(encoding_for(v, v), None, "{v} decided something");
        }
    }

    #[test]
    fn calibration_walks_right_then_left_and_answers() {
        let mut c = Calibration::new(7);
        assert_eq!(c.step, Step::Right);
        assert_eq!(c.feed(1), None);
        assert_eq!(c.feed(1), None);
        // Three the same: an encoder. Now it wants the other direction.
        assert_eq!(c.feed(1), None);
        assert_eq!(c.step, Step::Left);
        // A trailing right click teaches it nothing and is not an answer.
        assert_eq!(c.feed(1), None);
        assert_eq!(c.feed(127), Some(Mode::TwosComplement));
    }

    #[test]
    fn calibration_calls_a_pot_a_pot_without_asking_for_a_left_turn() {
        let mut c = Calibration::new(7);
        assert_eq!(c.feed(40), None);
        assert_eq!(c.feed(41), None);
        assert_eq!(c.feed(42), Some(Mode::Absolute));
        // It never needed the second step.
        assert_eq!(c.step, Step::Right);
    }

    #[test]
    fn what_calibration_says_never_assumes_you_read_the_manual() {
        let pot = describe_mode("K1", Mode::Absolute);
        assert!(pot.contains("pot") && pot.contains("pickup"), "{pot}");
        let enc = describe_mode("Send A 1", Mode::TwosComplement);
        assert!(enc.contains("endless encoder"), "{enc}");
    }

    #[test]
    fn activity_fades() {
        let mut a = Activity::default();
        let t = Instant::now();
        let ev = Event {
            control: 1,
            value: 5,
            released: false,
        };
        a.note(&ev, t);
        assert_eq!(a.of(1, t), (true, Some(5)));
        assert_eq!(a.of(1, t + ACTIVE_FOR * 2), (false, Some(5)));
        assert_eq!(a.of(2, t), (false, None));
    }
}
