//! Lines over a progression: a bass figure or an arpeggio, one note at a
//! time, as a melody track plays them. Every note is a tone of the chord
//! under it (root, third, fifth, an octave of one), stacked within the mode,
//! so a line is always in key.

use super::{rng, Mode, Register};

/// A note a line plays: its step from 0, MIDI note, velocity and hold in
/// sixteenths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub step: usize,
    pub note: i32,
    pub velocity: u8,
    pub hold: u8,
}

/// What a line plays over: the key, its mode, the chords as degrees, how
/// long each lasts, and how long the line is.
#[derive(Debug, Clone)]
pub struct Harmony {
    /// The tonic's pitch class, C = 0.
    pub key: i32,
    pub mode: Mode,
    pub degrees: Vec<i32>,
    /// Steps each chord lasts: 8, 16, 32 or 64.
    pub chord_steps: usize,
    /// Steps the line runs: 16, 32 or 64.
    pub length: usize,
}

impl Harmony {
    /// The degree of the chord at `step`; the progression repeats.
    pub fn degree_at(&self, step: usize) -> i32 {
        self.degrees[(step / self.chord_steps) % self.degrees.len()]
    }

    /// Where the chord at `step` ends.
    fn chord_end(&self, step: usize) -> usize {
        ((step / self.chord_steps + 1) * self.chord_steps).min(self.length)
    }

    /// The chord's root in `register`: the octave nearest the tonic, so a
    /// line moves by the smallest leap, kept within the register.
    fn root(&self, step: usize, register: Register) -> i32 {
        let tonic = register.tonic(self.key);
        let mut off = self.mode.degree(self.degree_at(step)).rem_euclid(12);
        if off > 6 {
            off -= 12;
        }
        fold(tonic + off, register)
    }

    /// The chord's triad over `root`, as notes.
    fn triad(&self, step: usize, root: i32) -> [i32; 3] {
        let t = self.mode.triad(self.degree_at(step));
        [root, root + t[1] - t[0], root + t[2] - t[0]]
    }
}

/// `note` moved by octaves into the register.
fn fold(note: i32, register: Register) -> i32 {
    let (lo, hi) = register.range();
    fold_in(note, lo, hi)
}

fn fold_in(mut note: i32, lo: i32, hi: i32) -> i32 {
    while note > hi {
        note -= 12;
    }
    while note < lo {
        note += 12;
    }
    note
}

const ACCENT: u8 = 120;
const NORMAL: u8 = 96;
const SOFT: u8 = 80;

/// A bass figure: where its notes fall in a bar and which chord tone each
/// plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Figure {
    EbmPulse,
    OctavePulse,
    Offbeat,
    RootFifth,
    Gallop,
    Sparse,
    Rolling,
    Acid,
}

impl Figure {
    pub const ALL: [Figure; 8] = [
        Figure::EbmPulse,
        Figure::Rolling,
        Figure::OctavePulse,
        Figure::Acid,
        Figure::Offbeat,
        Figure::RootFifth,
        Figure::Gallop,
        Figure::Sparse,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Figure::EbmPulse => "ebm_pulse",
            Figure::OctavePulse => "octave_pulse",
            Figure::Offbeat => "offbeat",
            Figure::RootFifth => "root_fifth",
            Figure::Gallop => "gallop",
            Figure::Sparse => "sparse",
            Figure::Rolling => "rolling",
            Figure::Acid => "acid",
        }
    }

    pub const fn describe(self) -> &'static str {
        match self {
            Figure::EbmPulse => {
                "sixteenths on the root with a few gaps, accented on the beat (DAF, Nitzer Ebb)"
            }
            Figure::OctavePulse => "sixteenths alternating root and octave",
            Figure::Offbeat => "eighths on the offbeats, between the kicks",
            Figure::RootFifth => "root on beats 1 and 3, fifth on 2 and 4",
            Figure::Gallop => "an eighth and two sixteenths on every beat",
            Figure::Sparse => "long roots with space between, dub-like",
            Figure::Rolling => {
                "three sixteenths after every beat, rests where the kick lands (Front 242)"
            }
            Figure::Acid => {
                "a different sixteenth line every bar: rests, accents, octave jumps and ties (303)"
            }
        }
    }

    pub fn from_name(text: &str) -> Option<Figure> {
        let t = text.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        Figure::ALL.into_iter().find(|f| f.name() == t)
    }
}

/// Which tone of the chord a figure's note plays.
#[derive(Debug, Clone, Copy)]
enum Tone {
    Root,
    Fifth,
    /// The root's octave: above when it fits the register, else the root
    /// is taken an octave down and this is the root.
    Octave,
}

/// One bar of a figure: (step in the bar, tone, hold, velocity).
fn bar_of(figure: Figure) -> Vec<(usize, Tone, u8, u8)> {
    let beat = |s: usize| if s.is_multiple_of(4) { ACCENT } else { SOFT };
    match figure {
        Figure::EbmPulse => (0..16).map(|s| (s, Tone::Root, 1, beat(s))).collect(),
        Figure::OctavePulse => (0..16)
            .map(|s| {
                let t = if s % 2 == 0 { Tone::Root } else { Tone::Octave };
                (s, t, 1, beat(s))
            })
            .collect(),
        Figure::Offbeat => [2, 6, 10, 14]
            .into_iter()
            .map(|s| (s, Tone::Root, 2, NORMAL))
            .collect(),
        Figure::RootFifth => vec![
            (0, Tone::Root, 3, ACCENT),
            (4, Tone::Fifth, 2, NORMAL),
            (8, Tone::Root, 3, ACCENT),
            (12, Tone::Fifth, 2, NORMAL),
        ],
        Figure::Gallop => (0..4)
            .flat_map(|b| {
                [
                    (4 * b, Tone::Root, 2, ACCENT),
                    (4 * b + 2, Tone::Root, 1, SOFT),
                    (4 * b + 3, Tone::Root, 1, SOFT),
                ]
            })
            .collect(),
        Figure::Sparse => vec![(0, Tone::Root, 6, ACCENT)],
        Figure::Rolling => (0..16)
            .filter(|s| s % 4 != 0)
            .map(|s| (s, Tone::Root, 1, if s % 4 == 1 { NORMAL } else { SOFT }))
            .collect(),
        // Drawn per bar in `bass`.
        Figure::Acid => Vec::new(),
    }
}

/// The draws a bar makes, all of them every bar, whatever the figure uses.
struct BarDraws {
    turn: f32,
    pickup: f32,
    late: f32,
    which: f32,
    /// Per step: whether a pulse leaves it out, or acid plays it.
    gap: [f32; 16],
    /// Per step: acid's tone.
    tone: [f32; 16],
    /// Per step: acid's accent and tie.
    accent: [f32; 16],
}

impl BarDraws {
    fn draw(r: &mut shard_dsp::rng::Rng) -> Self {
        let (turn, pickup, late, which) = (r.next_f32(), r.next_f32(), r.next_f32(), r.next_f32());
        let mut b = BarDraws {
            turn,
            pickup,
            late,
            which,
            gap: [0.0; 16],
            tone: [0.0; 16],
            accent: [0.0; 16],
        };
        for s in 0..16 {
            b.gap[s] = r.next_f32();
            b.tone[s] = r.next_f32();
            b.accent[s] = r.next_f32();
        }
        b
    }
}

/// How often a pulse figure leaves out a sixteenth off the beat.
const PULSE_GAPS: f32 = 0.3;

/// A bar of acid: the downbeat always, two in three of the other steps, an
/// accent on about one in four, octave jumps and fifths, and some notes
/// tied into the next step.
fn acid_bar(d: &BarDraws) -> Vec<(usize, Tone, u8, u8)> {
    let mut out = Vec::new();
    let mut s = 0;
    while s < 16 {
        if s == 0 || d.gap[s] < 0.66 {
            let tone = match d.tone[s] {
                t if t < 0.22 => Tone::Octave,
                t if t < 0.32 => Tone::Fifth,
                _ => Tone::Root,
            };
            let vel = if s == 0 || d.accent[s] < 0.25 {
                ACCENT
            } else {
                SOFT
            };
            // A tie: held into the next step, which is then not struck.
            let hold = if d.accent[s] > 0.82 && s < 15 { 2 } else { 1 };
            out.push((s, tone, hold, vel));
            s += usize::from(hold);
        } else {
            s += 1;
        }
    }
    out
}

/// A bass line: `figure` over `harmony` in `register`.
///
/// What the seed varies, always within the chord: velocities by a few
/// steps; the pulses leave out some sixteenths off the beat, and they and
/// the gallop answer the last beat of a bar with the fifth or octave;
/// root–fifth picks up the next beat with a sixteenth; sparse adds a
/// second, shorter note late in some bars; acid is drawn afresh every bar.
/// Every draw is made for every bar whether used or not, so changing one
/// thing moves nothing else.
pub fn bass(h: &Harmony, figure: Figure, register: Register, seed: u32) -> Vec<Note> {
    let mut r = rng(seed, 1);
    let mut out: Vec<Note> = Vec::new();
    let bars = h.length.div_ceil(16);
    for bar in 0..bars {
        let draws = BarDraws::draw(&mut r);
        let (turn, pickup, late, which) = (draws.turn, draws.pickup, draws.late, draws.which);
        let mut events = if figure == Figure::Acid {
            acid_bar(&draws)
        } else {
            bar_of(figure)
        };
        if matches!(figure, Figure::EbmPulse | Figure::OctavePulse) {
            events.retain(|e| e.0 % 4 == 0 || draws.gap[e.0] >= PULSE_GAPS);
        }
        match figure {
            Figure::OctavePulse | Figure::Gallop | Figure::EbmPulse | Figure::Rolling
                if turn < 0.35 =>
            {
                // The last beat answers with another tone.
                let tone = if which < 0.5 {
                    Tone::Fifth
                } else {
                    Tone::Octave
                };
                for e in events.iter_mut().filter(|e| e.0 >= 12 && e.0 % 2 == 0) {
                    e.1 = tone;
                }
            }
            Figure::RootFifth if pickup < 0.5 => {
                let at = if which < 0.5 { 7 } else { 15 };
                events.push((at, Tone::Octave, 1, SOFT));
            }
            Figure::Sparse if late < 0.6 => {
                let at = if which < 0.5 { 10 } else { 11 };
                let tone = if turn < 0.5 { Tone::Fifth } else { Tone::Root };
                events.push((at, tone, 2, NORMAL));
            }
            _ => {}
        }
        events.sort_by_key(|e| e.0);
        for (s, tone, hold, vel) in events {
            let step = bar * 16 + s;
            if step >= h.length {
                continue;
            }
            let jitter = (r.next_bipolar() * 6.0) as i32;
            let root = h.root(step, register);
            let (lo, hi) = register.range();
            let note = match tone {
                Tone::Root => root,
                Tone::Fifth => fold(h.triad(step, root)[2], register),
                Tone::Octave if root + 12 <= hi => root + 12,
                Tone::Octave if root - 12 >= lo => root - 12,
                Tone::Octave => root,
            };
            // A note never rings into the next chord.
            let hold = (hold as usize).min(h.chord_end(step) - step) as u8;
            out.push(Note {
                step,
                note,
                velocity: (i32::from(vel) + jitter).clamp(1, 127) as u8,
                hold: hold.max(1),
            });
        }
    }
    out
}

/// An arp's order through the chord's tones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    Up,
    Down,
    UpDown,
    Random,
}

impl Pattern {
    pub const ALL: [Pattern; 4] = [Pattern::Up, Pattern::Down, Pattern::UpDown, Pattern::Random];

    pub const fn name(self) -> &'static str {
        match self {
            Pattern::Up => "up",
            Pattern::Down => "down",
            Pattern::UpDown => "up_down",
            Pattern::Random => "random",
        }
    }

    pub fn from_name(text: &str) -> Option<Pattern> {
        let t = text.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        let t = if t == "updown" {
            "up_down".to_string()
        } else {
            t
        };
        Pattern::ALL.into_iter().find(|p| p.name() == t)
    }
}

/// An arp's note length in sixteenths: 2 for eighths, 1 for sixteenths.
pub const RATES: [u8; 2] = [8, 16];
/// The octaves an arp may span.
pub const OCTAVES_MAX: u8 = 2;

/// The notes an arp of `octaves` keeps to: its register, widened upwards by
/// each octave past the first, so two octaves of a triad never fold back.
pub fn arp_range(register: Register, octaves: u8) -> (i32, i32) {
    let (lo, hi) = register.range();
    (lo, hi + 12 * (i32::from(octaves.clamp(1, OCTAVES_MAX)) - 1))
}

/// An arpeggio: the chord's triad over `octaves`, walked in `pattern`, one
/// note every eighth (`rate` 8) or sixteenth (16), restarting on every
/// chord. Seeded: the order of `random`, and velocities by a few steps.
pub fn arp(
    h: &Harmony,
    pattern: Pattern,
    rate: u8,
    octaves: u8,
    register: Register,
    seed: u32,
) -> Vec<Note> {
    let mut r = rng(seed, 2);
    let every = if rate == 8 { 2 } else { 1 };
    let (lo, hi) = arp_range(register, octaves);
    let octaves = i32::from(octaves.clamp(1, OCTAVES_MAX));
    let mut out = Vec::new();
    let mut tones: Vec<i32> = Vec::new();
    let mut walk: Vec<usize> = Vec::new();
    let mut i = 0usize;
    let mut last = usize::MAX;
    for step in (0..h.length).step_by(every) {
        // Drawn on every step, used or not.
        let (pick, jitter) = (r.next_u32(), r.next_bipolar());
        if step % h.chord_steps == 0 || tones.is_empty() {
            let mut root = h.root(step, register);
            // The whole span inside the register: down an octave if it
            // would leave the top.
            let span = 12 * (octaves - 1) + h.triad(step, 0)[2];
            while root + span > hi && root - 12 >= lo {
                root -= 12;
            }
            tones = (0..octaves)
                .flat_map(|o| h.triad(step, root).map(|n| n + 12 * o))
                .map(|n| fold_in(n, lo, hi))
                .collect();
            tones.sort_unstable();
            tones.dedup();
            let n = tones.len();
            walk = match pattern {
                Pattern::Up | Pattern::Random => (0..n).collect(),
                Pattern::Down => (0..n).rev().collect(),
                Pattern::UpDown => (0..n).chain((1..n.saturating_sub(1)).rev()).collect(),
            };
            i = 0;
        }
        let k = match pattern {
            Pattern::Random => {
                let n = walk.len();
                let mut k = pick as usize % n;
                if k == last && n > 1 {
                    k = (k + 1) % n;
                }
                k
            }
            _ => walk[i % walk.len()],
        };
        last = k;
        i += 1;
        let base = if step % 4 == 0 { 110 } else { 88 };
        out.push(Note {
            step,
            note: tones[k],
            velocity: (base + (jitter * 8.0) as i32).clamp(1, 127) as u8,
            hold: 1,
        });
    }
    out
}
