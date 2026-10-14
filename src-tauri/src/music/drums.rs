//! Drum styles and the drums generator. A style is a bar of sixteen steps on
//! the kit's three lanes (kick, snare, hat), written as the agent-formats
//! grid: `X` an accent, `x` a hit, `.` a ghost, `-` nothing.
//!
//! The generator plays the style, then thins or fills it by density and
//! breaks it by glitch. Glitch uses only what a step holds today:
//! displacement, bursts of adjacent hits, dropouts and ghost velocities.
//! A drums track has no nudge (a kit's lanes are velocities alone), so
//! there is none; ratchets and probability are roadmap row 61.

use super::rng;

/// The kit's lanes, in order.
pub const LANES: [&str; 3] = ["kick", "snare", "hat"];
const KICK: usize = 0;
const SNARE: usize = 1;
const HAT: usize = 2;

/// One bar: a grid string per lane.
pub type Bar = [&'static str; 3];

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub name: &'static str,
    pub describe: &'static str,
    pub tags: &'static [&'static str],
    /// The tempo it is written for, as a suggestion.
    pub bpm: u16,
    /// Glitch when the request names none.
    pub glitch: f32,
    pub bar: Bar,
    /// The second bar of two, and the second and third of four.
    pub variation: Option<Bar>,
    /// The last bar of four.
    pub fill: Option<Bar>,
}

pub const STYLES: &[Style] = &[
    Style {
        name: "ebm_straight",
        describe: "four-on-the-floor kick, snare on 2 and 4, eighth hats; sixteenths as density rises",
        tags: &["ebm", "industrial", "driving", "dark"],
        bpm: 128,
        glitch: 0.0,
        bar: ["X---X---X---X---", "----X-------X---", "x-x-x-x-x-x-x-x-"],
        variation: None,
        fill: Some(["X---X---X---X---", "----X-------X.XX", "x-x-x-x-x-x-x---"]),
    },
    Style {
        name: "ebm_shuffle",
        describe: "EBM with a pushed kick before beats 3 and 1 and offbeat hats; pair with swing around 58 %",
        tags: &["ebm", "industrial", "swung", "dark"],
        bpm: 122,
        glitch: 0.0,
        bar: ["X---X--xX---X--x", "----X-------X-.-", "--x---x---x---x-"],
        variation: Some(["X---X--xX---X---", "----X-------X---", "--x---x---x-x-x-"]),
        fill: Some(["X---X--xX---X-x-", "----X-------X.X.", "--x---x---x-----"]),
    },
    Style {
        name: "four_on_floor",
        describe: "kick on every beat, clap-like snare on 2 and 4, open offbeat hats",
        tags: &["techno", "house", "driving"],
        bpm: 126,
        glitch: 0.0,
        bar: ["X---X---X---X---", "----x-------x---", "--x---x---x---x-"],
        variation: None,
        fill: Some(["X---X---X---X---", "----x-------x.x.", "--x---x---x-x-x-"]),
    },
    Style {
        name: "half_time",
        describe: "a heavy half-time: snare only on beat 3, kick around it, eighth hats",
        tags: &["half-time", "heavy", "dark", "slow"],
        bpm: 85,
        glitch: 0.0,
        bar: ["X------x--x-----", "--------X-------", "x-x-x-x-x-x-x-x-"],
        variation: Some(["X------x--x---x-", "--------X-----.-", "x-x-x-x-x-x-x-x-"]),
        fill: Some(["X------x--x-----", "--------X-.-X.X.", "x-x-x-x-x-x-----"]),
    },
    Style {
        name: "glitch_sparse",
        describe: "a few off-grid kicks and snares, scattered hats, lots of space",
        tags: &["glitch", "idm", "sparse", "broken"],
        bpm: 100,
        glitch: 0.35,
        bar: ["X------x--X-----", "----x------X----", "--x---.---x--x--"],
        variation: Some(["X---------x--x--", "-------X------.-", "--x-.-----x-----"]),
        fill: None,
    },
    Style {
        name: "glitch_broken",
        describe: "a broken beat: kicks off the beat, a busy snare, stuttering hats",
        tags: &["glitch", "idm", "broken", "busy"],
        bpm: 110,
        glitch: 0.35,
        bar: ["X-x---x-X--x----", "----X--.-x--X-.-", "x.xx-x.x-xx.x-xx"],
        variation: Some(["X-x----xX---x---", "----X-.--x--X.X-", "x.x-xx.x-x.xx-x."]),
        fill: Some(["X-x---x-X--x-x--", "----X--.-xX-XXXX", "x.xx-x.x-xx-----"]),
    },
];

pub fn style(name: &str) -> Option<&'static Style> {
    let t = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    STYLES.iter().find(|s| s.name == t)
}

/// A drums track's hits: a velocity per step on each lane, 0 for none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lanes {
    pub kick: Vec<u8>,
    pub snare: Vec<u8>,
    pub hat: Vec<u8>,
}

impl Lanes {
    fn new(len: usize) -> Self {
        Self {
            kick: vec![0; len],
            snare: vec![0; len],
            hat: vec![0; len],
        }
    }

    pub fn lane(&self, l: usize) -> &Vec<u8> {
        [&self.kick, &self.snare, &self.hat][l]
    }

    fn lane_mut(&mut self, l: usize) -> &mut Vec<u8> {
        match l {
            KICK => &mut self.kick,
            SNARE => &mut self.snare,
            _ => &mut self.hat,
        }
    }

    pub fn hits(&self) -> usize {
        (0..3)
            .map(|l| self.lane(l).iter().filter(|v| **v > 0).count())
            .sum()
    }
}

const ACCENT: u8 = 120;
const HIT: u8 = 96;
const GHOST: u8 = 45;

fn velocity(c: u8) -> u8 {
    match c {
        b'X' => ACCENT,
        b'x' => HIT,
        b'.' => GHOST,
        _ => 0,
    }
}

/// The bar a style plays at `bar` of `bars`.
fn bar_at(s: &Style, bar: usize, bars: usize) -> Bar {
    if bars >= 4 && bar == bars - 1 {
        if let Some(f) = s.fill.or(s.variation) {
            return f;
        }
    }
    if bar % 2 == 1 {
        if let Some(v) = s.variation {
            return v;
        }
    }
    s.bar
}

/// How readily a written hit goes as density falls: kept while
/// `rank < density`. Accented kicks and snares are the groove and never go.
fn written_rank(lane: usize, step: usize, c: u8) -> f32 {
    match (lane, c) {
        (KICK | SNARE, b'X') => -1.0,
        (KICK | SNARE, b'x') => 0.2,
        (_, b'.') => 0.45,
        (HAT, _) if step.is_multiple_of(4) => 0.1,
        (HAT, _) if step.is_multiple_of(2) => 0.25,
        _ => 0.4,
    }
}

/// `style` over `bars` bars of sixteen steps.
///
/// - **Density** 0 to 1, the style as written at 0.5. Below, the hits that
///   matter least go first: ghosts, then off-beat hats, then plain kicks
///   and snares; accents stay. Above, eighth then sixteenth hats are added,
///   then snare ghosts. Each step's place in that order is fixed by the
///   seed, so more density never takes a hit away.
/// - **Glitch** 0 to 1, none at 0 whatever the seed. Raised, each hit may be
///   dropped or moved a step or two, a hat or snare may burst into two or
///   three adjacent sixteenths, and empty steps may get ghosts. The first
///   kick of every bar stays put, so the bar still lands.
///
/// Every step draws the same numbers whatever density and glitch are, so
/// turning one changes only what it should.
pub fn generate(s: &Style, bars: usize, density: f32, glitch: f32, seed: u32) -> Lanes {
    let density = density.clamp(0.0, 1.0);
    let glitch = glitch.clamp(0.0, 1.0);
    let len = bars * 16;
    let mut r = rng(seed, 3);
    // Seven draws per lane and step, in one fixed order.
    let draws: Vec<[[f32; 7]; 3]> = (0..len)
        .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| r.next_f32())))
        .collect();

    // The style, thinned or filled.
    let mut out = Lanes::new(len);
    for (step, here) in draws.iter().enumerate() {
        let bar = bar_at(s, step / 16, bars);
        let at = step % 16;
        for (lane, row) in bar.iter().enumerate() {
            let c = row.as_bytes()[at];
            let d = here[lane];
            let v = velocity(c);
            let on = if v > 0 {
                written_rank(lane, at, c) + d[0] * 0.04 < density
            } else {
                let rank = match lane {
                    HAT if at.is_multiple_of(2) => 0.55 + d[0] * 0.1,
                    HAT => 0.65 + d[0] * 0.25,
                    SNARE if !at.is_multiple_of(4) => 0.75 + d[0] * 0.2,
                    _ => f32::INFINITY,
                };
                rank < density
            };
            if on {
                out.lane_mut(lane)[step] = if v > 0 {
                    v
                } else if lane == SNARE {
                    GHOST
                } else {
                    70 + (d[1] * 20.0) as u8
                };
            }
        }
    }
    if glitch <= 0.0 {
        return out;
    }

    // Broken by glitch: dropouts and displacement, from the thinned grid.
    let before = out.clone();
    let mut moved = Lanes::new(len);
    for (step, here) in draws.iter().enumerate() {
        for (lane, d) in here.iter().enumerate() {
            let v = before.lane(lane)[step];
            if v == 0 {
                continue;
            }
            let anchor = lane == KICK && step.is_multiple_of(16);
            if !anchor && d[2] < glitch * 0.3 {
                continue; // dropped
            }
            let to = if !anchor && d[3] < glitch * 0.35 {
                let by = if d[4] < 0.5 {
                    -1
                } else if d[4] < 0.8 {
                    1
                } else {
                    2
                };
                (step as i32 + by).rem_euclid(len as i32) as usize
            } else {
                step
            };
            let slot = &mut moved.lane_mut(lane)[to];
            *slot = (*slot).max(v);
        }
    }
    // Bursts and ghosts, read from the moved grid so a burst never bursts.
    let base = moved.clone();
    for (step, here) in draws.iter().enumerate() {
        for lane in [SNARE, HAT] {
            let d = here[lane];
            let v = base.lane(lane)[step];
            if v > 0 && d[5] < glitch * 0.18 {
                let n = if d[6] < 0.5 { 1 } else { 2 };
                for k in 1..=n {
                    let at = (step + k) % len;
                    let fade = v.saturating_sub(18 * k as u8).max(GHOST);
                    let slot = &mut moved.lane_mut(lane)[at];
                    *slot = (*slot).max(fade);
                }
            } else if v == 0 && d[5] > 1.0 - glitch * 0.1 {
                let slot = &mut moved.lane_mut(lane)[step];
                *slot = (*slot).max(30 + (d[6] * 25.0) as u8);
            }
        }
    }
    moved
}
