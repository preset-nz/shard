//! Shard's musical vocabulary: scales and modes, chord progressions, bass
//! figures, arp patterns and drum styles, and the seeded generators that
//! turn a brief ("ebm_pulse over i-bVI-bVII in A minor") into notes
//! (`projects/shard/design/agent-formats.md`, "First slice: EBM and glitch").
//!
//! **One vocabulary, several readers.** The agent's `make_*` tools read it
//! today; the chord stamp and a piano roll will read the same tables, so a
//! chord picked by hand and one the agent writes come from one definition.
//!
//! **Nothing here knows the app.** No Tauri, no session, no document, no
//! tracker types: the generators answer plain notes and velocity lanes, and
//! `object_model::generate` turns them into tracks. So the module moves with
//! the model when it leaves the app (epic 53).
//!
//! **Deterministic.** Everything that varies is drawn from a seed, through
//! `shard_dsp::rng::Rng`, in a fixed order: the same request and seed always
//! write the same notes.

pub mod drums;
pub mod line;
mod progressions;
#[cfg(test)]
mod tests;

pub use progressions::PROGRESSIONS;

use shard_dsp::rng::Rng;

/// A scale: seven degrees, as semitones above the tonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Major,
    Minor,
    Dorian,
    Phrygian,
    HarmonicMinor,
    PhrygianDominant,
    Locrian,
}

impl Mode {
    pub const ALL: [Mode; 7] = [
        Mode::Minor,
        Mode::Phrygian,
        Mode::Dorian,
        Mode::HarmonicMinor,
        Mode::PhrygianDominant,
        Mode::Locrian,
        Mode::Major,
    ];

    /// Its name in payloads.
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Major => "major",
            Mode::Minor => "minor",
            Mode::Dorian => "dorian",
            Mode::Phrygian => "phrygian",
            Mode::HarmonicMinor => "harmonic_minor",
            Mode::PhrygianDominant => "phrygian_dominant",
            Mode::Locrian => "locrian",
        }
    }

    /// The mode a name means, generously: "Aeolian", "natural minor",
    /// "harmonic minor", "phrygian-dominant", "ionian".
    pub fn from_name(text: &str) -> Option<Mode> {
        let t: String = text
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphabetic() { c } else { ' ' })
            .collect();
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(match t.as_str() {
            "major" | "ionian" | "maj" => Mode::Major,
            "minor" | "natural minor" | "aeolian" | "min" | "m" => Mode::Minor,
            "dorian" => Mode::Dorian,
            "phrygian" => Mode::Phrygian,
            "harmonic minor" | "harmonic" => Mode::HarmonicMinor,
            "phrygian dominant" | "spanish phrygian" | "freygish" => Mode::PhrygianDominant,
            "locrian" => Mode::Locrian,
            _ => return None,
        })
    }

    pub const fn intervals(self) -> [i32; 7] {
        match self {
            Mode::Major => [0, 2, 4, 5, 7, 9, 11],
            Mode::Minor => [0, 2, 3, 5, 7, 8, 10],
            Mode::Dorian => [0, 2, 3, 5, 7, 9, 10],
            Mode::Phrygian => [0, 1, 3, 5, 7, 8, 10],
            Mode::HarmonicMinor => [0, 2, 3, 5, 7, 8, 11],
            Mode::PhrygianDominant => [0, 1, 4, 5, 7, 8, 10],
            Mode::Locrian => [0, 1, 3, 5, 6, 8, 10],
        }
    }

    /// Semitones above the tonic of `degree` (1 to 7, wrapping), octaves
    /// included: degree 8 is 12.
    pub fn degree(self, degree: i32) -> i32 {
        let d = degree - 1;
        let iv = self.intervals();
        iv[d.rem_euclid(7) as usize] + 12 * d.div_euclid(7)
    }

    /// The triad on `degree`, stacked in thirds within the mode: root, third
    /// and fifth as semitones above the tonic.
    pub fn triad(self, degree: i32) -> [i32; 3] {
        [
            self.degree(degree),
            self.degree(degree + 2),
            self.degree(degree + 4),
        ]
    }

    /// Whether a pitch class (semitones above the tonic) is in the mode.
    pub fn holds(self, semis: i32) -> bool {
        self.intervals().contains(&semis.rem_euclid(12))
    }
}

/// A key's tonic as a pitch class (C = 0), and the mode it names, if any:
/// "A", "f#", "Bb", "A minor", "Am", "E phrygian".
pub fn parse_key(text: &str) -> Result<(i32, Option<Mode>), String> {
    let t = text.trim();
    let mut chars = t.chars();
    let refuse = || {
        format!("\"{text}\" is not a key; send a note name such as A, F# or Bb, with a mode if you like (\"E phrygian\")")
    };
    let letter = chars.next().ok_or_else(refuse)?;
    let mut pc: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(refuse()),
    };
    let rest: String = chars.collect();
    let mut rest = rest.as_str();
    for (sign, by) in [
        ("#", 1),
        ("♯", 1),
        ("b", -1),
        ("♭", -1),
        ("-flat", -1),
        ("-sharp", 1),
    ] {
        if let Some(r) = rest.strip_prefix(sign) {
            // "Bb" and "Ebm" have a flat, but "B blues" does not: a "b" is
            // a flat when what follows is nothing or a mode.
            let r_mode = r.trim();
            if sign == "b" && !r_mode.is_empty() && Mode::from_name(r_mode).is_none() {
                continue;
            }
            pc += by;
            rest = r;
            break;
        }
    }
    let rest = rest.trim();
    let mode = if rest.is_empty() {
        None
    } else {
        Some(Mode::from_name(rest).ok_or_else(refuse)?)
    };
    Ok((pc.rem_euclid(12), mode))
}

/// The name of a pitch class, sharps for black keys.
pub fn pitch_name(pc: i32) -> &'static str {
    [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ][pc.rem_euclid(12) as usize]
}

/// A MIDI note as a name: 33 is A1.
pub fn note_name(note: i32) -> String {
    format!("{}{}", pitch_name(note), note.div_euclid(12) - 1)
}

/// Where a line sits: two octaves of MIDI notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    Sub,
    Low,
    Mid,
    High,
}

impl Register {
    pub const ALL: [Register; 4] = [Register::Sub, Register::Low, Register::Mid, Register::High];

    pub const fn name(self) -> &'static str {
        match self {
            Register::Sub => "sub",
            Register::Low => "low",
            Register::Mid => "mid",
            Register::High => "high",
        }
    }

    pub fn from_name(text: &str) -> Option<Register> {
        let t = text.trim().to_ascii_lowercase();
        Register::ALL.into_iter().find(|r| r.name() == t)
    }

    /// The lowest and highest note it holds, two octaves apart.
    pub const fn range(self) -> (i32, i32) {
        let lo = match self {
            Register::Sub => 24,  // C1
            Register::Low => 33,  // A1
            Register::Mid => 45,  // A2
            Register::High => 57, // A3
        };
        (lo, lo + 24)
    }

    /// The tonic's note: the key's pitch class in the register's lower part,
    /// between four and fifteen semitones above its floor, so a chord root
    /// a fifth either side still fits.
    pub fn tonic(self, pc: i32) -> i32 {
        let (lo, _) = self.range();
        let base = lo + 4;
        base + (pc - base).rem_euclid(12)
    }
}

/// A progression in the table: chords as degrees of a mode, read as triads
/// stacked within whatever mode it is played in.
#[derive(Debug, Clone, Copy)]
pub struct Progression {
    /// Its name: roman numerals in its home mode, accidentals against the
    /// major scale, case for the triad's quality (`i-bVI-bVII`).
    pub name: &'static str,
    /// The modes it is written for, its home mode first: every chord's root
    /// and quality is diatonic in each.
    pub modes: &'static [Mode],
    /// Each chord's degree, 1 to 7.
    pub degrees: &'static [u8],
    /// Feel tags: free-midi-chords' mood tags, lower case, or ours.
    pub tags: &'static [&'static str],
    /// The progression as free-midi-chords writes it, or None when it is
    /// hand-written here.
    pub source: Option<&'static str>,
}

impl Progression {
    pub fn home(&self) -> Mode {
        self.modes[0]
    }

    /// One line for a list: its feel and the modes it is written for.
    pub fn describe(&self) -> String {
        let modes: Vec<&str> = self.modes.iter().map(|m| m.name()).collect();
        format!("{}; in {}", self.tags.join(", "), modes.join(" or "))
    }
}

/// Every feel tag in the table, sorted.
pub fn feels() -> Vec<&'static str> {
    let mut all: Vec<&str> = PROGRESSIONS
        .iter()
        .flat_map(|p| p.tags.iter().copied())
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// The chords a payload asks for: a table name (exactly, or in any case
/// when that is unambiguous) or roman numerals separated by spaces, hyphens
/// or dashes, read as degrees of the mode played. Answers the degrees and
/// the table entry, if it is one.
pub fn chords(text: &str) -> Result<(Vec<i32>, Option<&'static Progression>), String> {
    let t = text.trim();
    let found = PROGRESSIONS.iter().find(|p| p.name == t).or_else(|| {
        let mut any = PROGRESSIONS
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(t));
        match (any.next(), any.next()) {
            (Some(p), None) => Some(p),
            _ => None,
        }
    });
    if let Some(p) = found {
        return Ok((p.degrees.iter().map(|d| i32::from(*d)).collect(), Some(p)));
    }
    let degrees: Option<Vec<i32>> = t
        .split(|c: char| c.is_whitespace() || matches!(c, '-' | '–' | '—' | ',' | '|'))
        .filter(|s| !s.is_empty())
        .map(numeral)
        .collect();
    match degrees {
        Some(d) if !d.is_empty() => Ok((d, None)),
        _ => Err(format!(
            "\"{text}\" is not a progression; send a name from list_progressions (\"i-bVI-bVII\"), or roman numerals such as \"i iv v\""
        )),
    }
}

/// The degree a roman numeral names, ignoring its accidental, case and any
/// suffix ("bVI7", "im", "vdim", "VIadd9"): 6, 1, 5, 6.
pub fn numeral(token: &str) -> Option<i32> {
    let t = token.trim_start_matches(['b', '#', '♭', '♯']);
    let upper = t.to_ascii_uppercase();
    ["VII", "VI", "IV", "V", "III", "II", "I"]
        .iter()
        .zip([7, 6, 4, 5, 3, 2, 1])
        .find(|(r, _)| upper.starts_with(*r))
        .map(|(_, d)| d)
}

/// A generator's randomness: `seed` mixed with a stream number, so the
/// drums and a line asked with the same seed don't move together, and
/// neighbouring seeds don't start alike (xorshift's first draws from small
/// seeds are close).
pub fn rng(seed: u32, stream: u32) -> Rng {
    let mut x = seed ^ stream.wrapping_mul(0x9E37_79B9);
    // murmur3's finaliser.
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    Rng::new(x)
}

/// A seed for a request that sent none: FNV-1a over its text, so the same
/// request always gets the same one, and the result can say it.
pub fn seed_of(text: &str) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in text.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    // Kept to what a model copies back without trouble.
    h % 100_000
}
