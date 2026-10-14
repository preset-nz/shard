//! Contracts of the tables and generators: in key, in range, repeatable,
//! monotonic. No golden patterns.

use super::drums::{self, STYLES};
use super::line::{self, Figure, Harmony, Pattern};
use super::*;

fn harmony(key: i32, mode: Mode, prog: &str, chord_steps: usize, length: usize) -> Harmony {
    let (degrees, _) = chords(prog).unwrap();
    Harmony {
        key,
        mode,
        degrees,
        chord_steps,
        length,
    }
}

fn on(lane: &[u8]) -> Vec<usize> {
    lane.iter()
        .enumerate()
        .filter(|(_, v)| **v > 0)
        .map(|(i, _)| i + 1)
        .collect()
}

#[test]
fn ebm_straight_without_glitch_is_four_kicks_and_a_backbeat() {
    let s = drums::style("ebm_straight").unwrap();
    for seed in [0, 1, 99, 12345] {
        let l = drums::generate(s, 1, 0.5, 0.0, seed);
        assert_eq!(on(&l.kick), [1, 5, 9, 13], "seed {seed}");
        assert_eq!(on(&l.snare), [5, 13], "seed {seed}");
    }
}

#[test]
fn every_style_at_half_density_without_glitch_plays_as_written() {
    for s in STYLES {
        let l = drums::generate(s, 1, 0.5, 0.0, 7);
        for (lane, row) in s.bar.iter().enumerate() {
            let written: Vec<u8> = row
                .bytes()
                .map(|c| match c {
                    b'-' => 0,
                    _ => 1,
                })
                .collect();
            let got: Vec<u8> = l.lane(lane).iter().map(|v| u8::from(*v > 0)).collect();
            assert_eq!(got, written, "{} {}", s.name, drums::LANES[lane]);
        }
    }
}

#[test]
fn every_style_grid_is_sixteen_steps_of_known_marks() {
    for s in STYLES {
        for bar in [Some(s.bar), s.variation, s.fill].into_iter().flatten() {
            for row in bar {
                assert_eq!(row.len(), 16, "{}: {row}", s.name);
                assert!(row.bytes().all(|c| b"Xx.-".contains(&c)), "{}", s.name);
            }
        }
    }
}

#[test]
fn the_same_seed_gives_the_same_drums_and_another_differs_once_glitched() {
    for s in STYLES {
        let a = drums::generate(s, 4, 0.6, 0.6, 1);
        assert_eq!(a, drums::generate(s, 4, 0.6, 0.6, 1), "{}", s.name);
        let differ = [2, 3, 4, 5]
            .iter()
            .any(|seed| drums::generate(s, 4, 0.6, 0.6, *seed) != a);
        assert!(differ, "{}: no seed changed it", s.name);
    }
}

#[test]
fn density_never_takes_a_hit_away() {
    for s in STYLES {
        for seed in [1, 2, 3] {
            let mut last = 0;
            for d in 0..=20 {
                let n = drums::generate(s, 2, d as f32 / 20.0, 0.0, seed).hits();
                assert!(
                    n >= last,
                    "{} seed {seed} at density {d}/20: {n} < {last}",
                    s.name
                );
                last = n;
            }
            assert!(
                drums::generate(s, 2, 1.0, 0.0, seed).hits()
                    > drums::generate(s, 2, 0.0, 0.0, seed).hits(),
                "{}",
                s.name
            );
        }
    }
}

#[test]
fn glitch_keeps_the_first_kick_of_every_bar() {
    for s in STYLES {
        for seed in 0..20 {
            let l = drums::generate(s, 4, 0.5, 1.0, seed);
            for bar in 0..4 {
                assert!(l.kick[bar * 16] > 0, "{} seed {seed} bar {bar}", s.name);
            }
        }
    }
}

#[test]
fn every_progression_parses_and_fits_its_modes() {
    assert!((40..=90).contains(&PROGRESSIONS.len()));
    for p in PROGRESSIONS {
        assert!(!p.tags.is_empty() && !p.modes.is_empty(), "{}", p.name);
        assert!((1..=8).contains(&p.degrees.len()), "{}", p.name);
        // Its name reads back as its degrees.
        let (degrees, found) = chords(p.name).unwrap();
        assert_eq!(found.map(|f| f.name), Some(p.name));
        let read: Vec<i32> = p.name.split('-').map(|t| numeral(t).unwrap()).collect();
        assert_eq!(read, degrees, "{}", p.name);
        // Each numeral's case says the triad's quality in every listed mode,
        // and its accidental where the root sits against the major scale.
        for mode in p.modes {
            for (tok, d) in p.name.split('-').zip(p.degrees) {
                let t = mode.triad(i32::from(*d));
                let third = t[1] - t[0];
                let upper = tok
                    .trim_start_matches(['b', '#'])
                    .starts_with(char::is_uppercase);
                assert_eq!(upper, third == 4, "{} in {}: {tok}", p.name, mode.name());
            }
        }
        let home = p.home();
        for (tok, d) in p.name.split('-').zip(p.degrees) {
            let semis = home.degree(i32::from(*d));
            let major = Mode::Major.degree(i32::from(*d));
            let flat = tok.starts_with('b');
            let sharp = tok.starts_with('#');
            assert_eq!(
                semis - major,
                i32::from(sharp) - i32::from(flat),
                "{}: {tok}",
                p.name
            );
        }
    }
    let mut names: Vec<&str> = PROGRESSIONS.iter().map(|p| p.name).collect();
    names.sort_unstable();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "names are unique");
}

#[test]
fn free_numerals_and_keys_read_generously() {
    assert_eq!(chords("i – bVI – bVII").unwrap().0, [1, 6, 7]);
    assert_eq!(chords("im bIIM vdim VIadd9").unwrap().0, [1, 2, 5, 6]);
    assert!(chords("a nice one").is_err());
    assert_eq!(parse_key("A").unwrap(), (9, None));
    assert_eq!(parse_key("a minor").unwrap(), (9, Some(Mode::Minor)));
    assert_eq!(parse_key("Am").unwrap(), (9, Some(Mode::Minor)));
    assert_eq!(parse_key("F#").unwrap(), (6, None));
    assert_eq!(parse_key("Ebm").unwrap(), (3, Some(Mode::Minor)));
    assert_eq!(parse_key("Bbm").unwrap(), (10, Some(Mode::Minor)));
    assert_eq!(parse_key("Ab dorian").unwrap(), (8, Some(Mode::Dorian)));
    assert_eq!(parse_key("Bm").unwrap(), (11, Some(Mode::Minor)));
    assert_eq!(
        parse_key("Bb harmonic minor").unwrap(),
        (10, Some(Mode::HarmonicMinor))
    );
    assert_eq!(parse_key("E Phrygian").unwrap(), (4, Some(Mode::Phrygian)));
    assert!(parse_key("H").is_err());
}

#[test]
fn bass_and_arp_stay_in_the_mode_and_on_the_chord() {
    for mode in Mode::ALL {
        for p in PROGRESSIONS.iter().step_by(3) {
            let h = harmony(9, mode, p.name, 8, 64);
            for f in Figure::ALL {
                for n in line::bass(&h, f, Register::Low, 5) {
                    let pc = n.note - 9;
                    assert!(
                        mode.holds(pc),
                        "{} {} {}: {n:?}",
                        mode.name(),
                        p.name,
                        f.name()
                    );
                    let chord = mode.triad(h.degree_at(n.step));
                    assert!(
                        chord.iter().any(|c| (c - pc).rem_euclid(12) == 0),
                        "{} {}: {n:?} is not on its chord",
                        p.name,
                        f.name()
                    );
                }
            }
            for pat in Pattern::ALL {
                for n in line::arp(&h, pat, 16, 2, Register::Mid, 5) {
                    let chord = mode.triad(h.degree_at(n.step));
                    assert!(chord.iter().any(|c| (c - (n.note - 9)).rem_euclid(12) == 0));
                }
            }
        }
    }
}

#[test]
fn every_register_keeps_its_notes_in_range() {
    for reg in Register::ALL {
        let (lo, hi) = reg.range();
        for key in 0..12 {
            for p in PROGRESSIONS.iter().step_by(7) {
                let h = harmony(key, p.home(), p.name, 16, 64);
                let notes: Vec<i32> = Figure::ALL
                    .iter()
                    .flat_map(|f| line::bass(&h, *f, reg, 3))
                    .map(|n| n.note)
                    .collect();
                for pat in Pattern::ALL {
                    for oct in 1..=line::OCTAVES_MAX {
                        let (alo, ahi) = line::arp_range(reg, oct);
                        for n in line::arp(&h, pat, 8, oct, reg, 3) {
                            assert!((alo..=ahi).contains(&n.note), "arp {}: {n:?}", reg.name());
                        }
                    }
                }
                assert!(
                    notes.iter().all(|n| (lo..=hi).contains(n)),
                    "{} in {key}: {notes:?}",
                    reg.name()
                );
            }
        }
    }
}

#[test]
fn the_octave_pulse_leaps_an_octave_in_every_register() {
    for reg in Register::ALL {
        for key in 0..12 {
            let h = harmony(key, Mode::Minor, "i-bVI-bVII", 16, 48);
            let n = line::bass(&h, Figure::OctavePulse, reg, 0);
            // Odd steps play the octave; some are left out.
            let odd = n.iter().find(|n| n.step % 2 == 1).unwrap();
            assert_eq!((odd.note - n[0].note).abs(), 12, "{} {key}", reg.name());
        }
    }
}

#[test]
fn sparse_leaves_at_least_half_the_steps_empty() {
    for seed in 0..30 {
        let h = harmony(4, Mode::Minor, "i-bVI-bIII-bVII", 16, 64);
        let notes = line::bass(&h, Figure::Sparse, Register::Sub, seed);
        assert!(notes.len() <= 32, "seed {seed}: {} notes", notes.len());
        let mut covered = [false; 64];
        for n in &notes {
            for s in n.step..n.step + n.hold as usize {
                covered[s.min(63)] = true;
            }
        }
        assert!(covered.iter().filter(|c| !**c).count() >= 32, "seed {seed}");
    }
}

/// Steps no note sounds on, holds included.
fn rests(notes: &[line::Note], length: usize) -> usize {
    let mut covered = vec![false; length];
    for n in notes {
        let end = (n.step + n.hold as usize).min(length);
        covered[n.step..end].fill(true);
    }
    covered.iter().filter(|c| !**c).count()
}

#[test]
fn the_pulses_leave_space_but_keep_every_beat() {
    let h = harmony(9, Mode::Minor, "i-bVI-bVII-iv", 16, 64);
    for f in [Figure::EbmPulse, Figure::OctavePulse] {
        for seed in 0..30 {
            let notes = line::bass(&h, f, Register::Low, seed);
            let r = rests(&notes, 64);
            assert!((8..=32).contains(&r), "{} seed {seed}: {r} rests", f.name());
            for beat in (0..64).step_by(4) {
                assert!(
                    notes.iter().any(|n| n.step == beat),
                    "{} seed {seed}: beat {beat}",
                    f.name()
                );
            }
        }
    }
}

#[test]
fn rolling_rests_where_the_kick_lands() {
    let h = harmony(9, Mode::Minor, "i-bVI", 16, 32);
    for seed in 0..10 {
        let notes = line::bass(&h, Figure::Rolling, Register::Low, seed);
        assert!(notes.iter().all(|n| n.step % 4 != 0), "seed {seed}");
        assert_eq!(notes.len(), 24);
    }
}

#[test]
fn acid_is_a_different_line_every_bar_with_accents_and_rests() {
    let h = harmony(9, Mode::Minor, "i", 16, 64);
    for seed in 0..20 {
        let notes = line::bass(&h, Figure::Acid, Register::Low, seed);
        let bar = |b: usize| -> Vec<(usize, i32, u8)> {
            notes
                .iter()
                .filter(|n| n.step / 16 == b)
                .map(|n| (n.step % 16, n.note, n.hold))
                .collect()
        };
        assert!((1..4).any(|b| bar(b) != bar(0)), "seed {seed}");
        assert!(rests(&notes, 64) >= 8, "seed {seed}");
        let mut pitches: Vec<i32> = notes.iter().map(|n| n.note).collect();
        pitches.sort_unstable();
        pitches.dedup();
        assert!(pitches.len() >= 2, "seed {seed}: one pitch");
        assert!(
            notes
                .iter()
                .filter(|n| n.step % 4 != 0)
                .any(|n| n.velocity > 110),
            "seed {seed}: no offbeat accent"
        );
    }
}

#[test]
fn a_held_note_never_rings_into_the_next_chord() {
    let h = harmony(9, Mode::Minor, "i-bVI-bVII-iv", 8, 64);
    for f in Figure::ALL {
        for n in line::bass(&h, f, Register::Low, 9) {
            assert_eq!(
                n.step / 8,
                (n.step + n.hold as usize - 1) / 8,
                "{}: {n:?}",
                f.name()
            );
        }
    }
}

#[test]
fn lines_repeat_with_their_seed() {
    let h = harmony(9, Mode::Phrygian, "i-bII", 16, 64);
    for f in Figure::ALL {
        assert_eq!(
            line::bass(&h, f, Register::Sub, 4),
            line::bass(&h, f, Register::Sub, 4)
        );
    }
    let a = line::arp(&h, Pattern::Random, 16, 2, Register::Mid, 4);
    assert_eq!(a, line::arp(&h, Pattern::Random, 16, 2, Register::Mid, 4));
    assert_ne!(a, line::arp(&h, Pattern::Random, 16, 2, Register::Mid, 5));
}

#[test]
fn the_arp_walks_its_pattern() {
    let h = harmony(4, Mode::Minor, "i", 16, 16);
    let up: Vec<i32> = line::arp(&h, Pattern::Up, 16, 1, Register::Mid, 0)
        .iter()
        .map(|n| n.note)
        .collect();
    // E minor from E3 (52): E G B, again.
    assert_eq!(&up[..4], [52, 55, 59, 52]);
    let ud: Vec<i32> = line::arp(&h, Pattern::UpDown, 8, 2, Register::Mid, 0)
        .iter()
        .map(|n| n.note)
        .collect();
    assert_eq!(ud.len(), 8, "eighths");
    assert_eq!(&ud[..8], [52, 55, 59, 64, 67, 71, 67, 64]);
}

#[test]
fn seeds_from_requests_are_stable() {
    assert_eq!(seed_of("abc"), seed_of("abc"));
    assert_ne!(seed_of("abc"), seed_of("abd"));
    assert!(seed_of("anything") < 100_000);
}
