//! `just examples`: the node pages' sound clips. A voice reads `docs/quote.txt`
//! over the built-in drone, and that is the dry sample. Each recipe then plays
//! it, or a short line, through a new session with one node set up, offline,
//! through the audio callback's own render (`shard_lib::offline`). The clips
//! are AAC in `docs/nodes/audio/`, every one at the same loudness, and each
//! page's Examples block is rewritten.
//!
//! Needs macOS `say` and `ffmpeg`. The WAVs on the way go to
//! `~/rhizomatic-preset/renders/shard-examples/`, never into the repo.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use shard_dsp::fx::Kind;
use shard_lib::offline::{drone, Offline};

const SR: f32 = 48_000.0;
/// The voice and its pace, fixed so a rerun gives the same sample.
const VOICE: &str = "Daniel";
const WORDS_PER_MINUTE: &str = "150";
/// Where the voice comes in over the drone, and how long the drone rings on.
const LEAD_IN: f32 = 1.5;
const TAIL: f32 = 2.5;
/// How long a clip of the sample runs past its end, for echoes and tails.
const RING_OUT: f32 = 3.5;
/// The line: two bars at this tempo, played twice.
const LINE_TEMPO: f32 = 100.0;
/// Every clip is brought to this loudness, peaks under -1 dBFS, so no recipe
/// wins by being louder.
const LOUDNESS_LUFS: f32 = -18.0;

/// What a recipe plays.
#[derive(Clone, Copy, PartialEq)]
enum Plays {
    /// The dry sample, once, from the top.
    Sample,
    /// The line, on a generator that makes its own sound; the sample is
    /// taken out of the patch.
    Line,
    /// The line, played on the sample: each step starts it again at its
    /// pitch.
    SampleLine,
}

/// A control moved while the clip plays. Places are fractions of the
/// sample, so a new quote of another length keeps them in the sentence.
enum Move {
    /// From one value to another between two places.
    Ramp(&'static str, f32, f32, f32, f32),
    /// Up (1) for this many seconds from a place, and down (0) either side:
    /// a held pad.
    Held(&'static str, f32, f32),
    /// Up for the first half of every `period` seconds between two places.
    Pulse(&'static str, f32, f32, f32),
}

struct Recipe {
    node: &'static str,
    name: &'static str,
    /// The settings in the page's own words, as the panel shows them.
    settings: &'static str,
    plays: Plays,
    /// Generators added to the patch, beyond the page's own.
    adds: &'static [&'static str],
    set: &'static [(&'static str, f32)],
    moves: &'static [Move],
}

const fn r(
    node: &'static str,
    name: &'static str,
    settings: &'static str,
    set: &'static [(&'static str, f32)],
) -> Recipe {
    Recipe {
        node,
        name,
        settings,
        plays: Plays::Sample,
        adds: &[],
        set,
        moves: &[],
    }
}

const fn line(
    node: &'static str,
    name: &'static str,
    settings: &'static str,
    set: &'static [(&'static str, f32)],
) -> Recipe {
    Recipe {
        plays: Plays::Line,
        ..r(node, name, settings, set)
    }
}

const fn moving(
    node: &'static str,
    name: &'static str,
    settings: &'static str,
    set: &'static [(&'static str, f32)],
    moves: &'static [Move],
) -> Recipe {
    Recipe {
        moves,
        ..r(node, name, settings, set)
    }
}

/// The recipes, by page, in the order they are heard. Ids are written as the
/// kind's own (`delay.mix`); an effect's are given its instance on the way.
#[rustfmt::skip]
const RECIPES: &[Recipe] = &[
    // Effects.
    r("drive", "Warm grit", "Mix 50%, Amount 18",
        &[("drive.mix", 0.5), ("drive.amount", 18.0)]),
    r("drive", "Folded to metal", "Mix 100%, Amount 40, Tone 4 kHz, Type Fold",
        &[("drive.mix", 1.0), ("drive.amount", 40.0), ("drive.tone", 4000.0), ("drive.type", 3.0)]),
    r("crush", "Old sampler", "Mix 100%, Bits 8, Rate 12 kHz",
        &[("crush.mix", 1.0), ("crush.bits", 8.0), ("crush.rate", 12_000.0)]),
    r("crush", "Down to the bone", "Mix 100%, Bits 3, Rate 2 kHz",
        &[("crush.mix", 1.0), ("crush.bits", 3.0), ("crush.rate", 2000.0)]),
    r("ring", "Robot", "Mix 100%, Frequency 30 Hz",
        &[("ring.mix", 1.0), ("ring.freq", 30.0)]),
    r("ring", "Clang", "Mix 70%, Frequency 900 Hz",
        &[("ring.mix", 0.7), ("ring.freq", 900.0)]),
    r("chorus", "Wide and shimmering", "Mix 50%, Type Ensemble, Depth 70%",
        &[("chorus.mix", 0.5), ("chorus.type", 1.0), ("chorus.depth", 0.7)]),
    r("chorus", "A choir of one", "Mix 80%, Type Choir, Voices 8",
        &[("chorus.mix", 0.8), ("chorus.type", 2.0), ("chorus.voices", 8.0)]),
    r("overtone", "Sub under the voice", "Mix 60%, Sub 80%, Octave 0%",
        &[("overtone.mix", 0.6), ("overtone.sub", 0.8), ("overtone.octave", 0.0)]),
    r("overtone", "Organ", "Mix 60%, Sub 30%, Octave 60%, Fifth 50%",
        &[("overtone.mix", 0.6), ("overtone.sub", 0.3), ("overtone.octave", 0.6), ("overtone.fifth", 0.5)]),
    r("flanger", "Slow jet", "Mix 60%, Rate 0.15 Hz, Feedback 70%",
        &[("flanger.mix", 0.6), ("flanger.rate", 0.15), ("flanger.feedback", 0.7)]),
    r("flanger", "Metal comb", "Mix 70%, Manual 0.4 ms, Depth 10%, Feedback -90%",
        &[("flanger.mix", 0.7), ("flanger.manual", 0.4), ("flanger.depth", 0.1), ("flanger.feedback", -0.9)]),
    r("wear", "Worn cassette", "Mix 100%",
        &[("wear.mix", 1.0)]),
    r("wear", "Failing tape", "Mix 100%, Wow 80%, Unstable 70%, Dropouts 60%",
        &[("wear.mix", 1.0), ("wear.wow", 0.8), ("wear.unstable", 0.7), ("wear.dropouts", 0.6)]),
    r("delay", "Ping-pong", "Mix 40%, Time 375 ms, Feedback 50%, Ping-pong 100%",
        &[("delay.mix", 0.4), ("delay.time", 375.0), ("delay.feedback", 0.5), ("delay.pingpong", 1.0)]),
    r("delay", "Dark repeats", "Mix 50%, Time 600 ms, Feedback 70%, Tone 1.5 kHz",
        &[("delay.mix", 0.5), ("delay.time", 600.0), ("delay.feedback", 0.7), ("delay.tone", 1500.0)]),
    r("echo", "Tape echo", "Mix 50%, Time 400 ms, Feedback 55%",
        &[("echo.mix", 0.5), ("echo.time", 400.0), ("echo.feedback", 0.55)]),
    r("echo", "Runaway", "Mix 50%, Time 250 ms, Feedback 90%, Grit 70%",
        &[("echo.mix", 0.5), ("echo.time", 250.0), ("echo.feedback", 0.9), ("echo.grit", 0.7)]),
    r("rise", "Climbing fifths", "Mix 50%",
        &[("rise.mix", 0.5)]),
    r("rise", "Sinking", "Mix 50%, Feedback 80%, Shift -5 st",
        &[("rise.mix", 0.5), ("rise.feedback", 0.8), ("rise.shift", -5.0)]),
    r("reverb", "A small room", "Mix 30%",
        &[("reverb.mix", 0.3)]),
    r("reverb", "Cathedral", "Mix 50%, Type Hall, Decay 8 s, Size 90%, Pre-delay 60 ms",
        &[("reverb.mix", 0.5), ("reverb.type", 1.0), ("reverb.decay", 8.0), ("reverb.size", 0.9), ("reverb.predelay", 60.0)]),
    r("reverb", "Bright plate", "Mix 40%, Type Plate, Decay 3 s, Tone 12 kHz",
        &[("reverb.mix", 0.4), ("reverb.type", 2.0), ("reverb.decay", 3.0), ("reverb.tone", 12_000.0)]),
    moving("repeat", "Freeze a word", "Mix 100%, Length 400 ms, Hold raised for 2 s mid-sentence",
        &[("repeat.mix", 1.0), ("repeat.length", 400.0)],
        &[Move::Held("repeat.hold", 0.4, 2.0)]),
    moving("repeat", "Stutter", "Mix 100%, Length 60 ms, Hold raised twice a second through the middle",
        &[("repeat.mix", 1.0), ("repeat.length", 60.0)],
        &[Move::Pulse("repeat.hold", 0.3, 0.65, 0.5)]),
    moving("repeat", "Backwards and down", "Mix 100%, Length 500 ms, Speed -12 st, Reverse on, Hold raised for 2.5 s late in the sentence",
        &[("repeat.mix", 1.0), ("repeat.length", 500.0), ("repeat.speed", -12.0), ("repeat.reverse", 1.0)],
        &[Move::Held("repeat.hold", 0.5, 2.5)]),
    r("freqshift", "Shimmer", "Mix 50%, Shift 7 Hz, Spread 100%",
        &[("freqshift.mix", 0.5), ("freqshift.shift", 7.0), ("freqshift.spread", 1.0)]),
    r("freqshift", "Spiral", "Mix 60%, Shift 120 Hz, Feedback 80%",
        &[("freqshift.mix", 0.6), ("freqshift.shift", 120.0), ("freqshift.feedback", 0.8)]),
    r("freqshift", "Clang", "Mix 100%, Shift -300 Hz",
        &[("freqshift.mix", 1.0), ("freqshift.shift", -300.0)]),
    r("peaks", "Two resonances", "Mix 70%, Low 800 Hz, High 2.5 kHz, Boost 24 dB, Width 10%",
        &[("peaks.mix", 0.7), ("peaks.low", 800.0), ("peaks.high", 2500.0), ("peaks.boost", 24.0), ("peaks.width", 0.1)]),
    moving("peaks", "Swept whistle", "Mix 80%, Boost 30 dB, Width 5%, Low swept from 200 Hz to 3 kHz",
        &[("peaks.mix", 0.8), ("peaks.boost", 30.0), ("peaks.width", 0.05)],
        &[Move::Ramp("peaks.low", 200.0, 3000.0, 0.1, 1.0)]),
    r("shaper", "Sharper consonants", "Mix 100%, Attack 80%",
        &[("shaper.mix", 1.0), ("shaper.attack", 0.8)]),
    r("shaper", "Dried up", "Mix 100%, Sustain -80%",
        &[("shaper.mix", 1.0), ("shaper.sustain", -0.8)]),
    r("bands", "Harmonic echoes", "Mix 50%",
        &[("bands.mix", 0.5)]),
    moving("bands", "Bent by time", "Mix 60%, Feedback 70%, Time swept from 375 ms to 120 ms",
        &[("bands.mix", 0.6), ("bands.feedback", 0.7)],
        &[Move::Ramp("bands.time", 375.0, 120.0, 0.2, 0.9)]),
    // Sections.
    r("filter", "Behind a wall", "Cutoff 500 Hz, Slope 24 dB",
        &[("filter.on", 1.0), ("filter.cutoff", 500.0), ("filter.slope", 1.0)]),
    moving("filter", "Swept band", "Type Band-pass, Resonance 70%, Cutoff swept from 200 Hz to 5 kHz",
        &[("filter.on", 1.0), ("filter.type", 2.0), ("filter.resonance", 0.7)],
        &[Move::Ramp("filter.cutoff", 200.0, 5000.0, 0.1, 1.0)]),
    r("filter", "Thin", "Type High-pass, Cutoff 1.5 kHz",
        &[("filter.on", 1.0), ("filter.type", 1.0), ("filter.cutoff", 1500.0)]),
    r("ladder", "Dark growl", "Drive 80%, Cutoff 600 Hz, Resonance 50%",
        &[("ladder.on", 1.0), ("ladder.drive", 0.8), ("ladder.cutoff", 600.0), ("ladder.resonance", 0.5)]),
    moving("ladder", "Whistling sweep", "Resonance 90%, Cutoff swept from 300 Hz to 4 kHz",
        &[("ladder.on", 1.0), ("ladder.resonance", 0.9)],
        &[Move::Ramp("ladder.cutoff", 300.0, 4000.0, 0.1, 1.0)]),
    r("env", "Slow swell", "Attack 3 s",
        &[("env.attack", 3000.0)]),
    Recipe {
        plays: Plays::SampleLine,
        ..r("env", "Plucked words", "Decay 150 ms, Sustain 0%, played from a line",
            &[("env.decay", 150.0), ("env.sustain", 0.0)])
    },
    moving("tape", "Brake", "Time 1.5 s, Brake held for 2.5 s mid-sentence",
        &[("tape.time", 1500.0)],
        &[Move::Held("arrangement.tape.brake", 0.35, 2.5)]),
    moving("tape", "Reverse", "Time 100 ms, Reverse held for 1.5 s mid-sentence",
        &[("tape.time", 100.0)],
        &[Move::Held("arrangement.tape.reverse", 0.35, 1.5)]),
    // Generators.
    moving("material", "Scrubbed", "Scrub on, Position swept from 20% to 60% and back to 30%",
        &[("material.scrub", 1.0), ("material.position", 0.2)],
        &[Move::Ramp("material.position", 0.2, 0.6, 0.1, 0.55), Move::Ramp("material.position", 0.6, 0.3, 0.55, 1.0)]),
    Recipe {
        plays: Plays::SampleLine,
        ..r("material", "Played from a line", "each step starts the sample at its own pitch", &[])
    },
    r("grain", "A cloud", "Position 30%, Jitter 20%, Size 120 ms, Density 40 Hz",
        &[("grain.position", 0.3), ("grain.jitter", 0.2), ("grain.size", 120.0), ("grain.density", 40.0)]),
    r("grain", "Frozen shimmer", "Position 45%, Size 300 ms, Density 80 Hz, Pitch spread 12 st",
        &[("grain.position", 0.45), ("grain.size", 300.0), ("grain.density", 80.0), ("grain.spread", 12.0)]),
    r("grain", "Scattered backwards", "Jitter 100%, Size 60 ms, Density 15 Hz, Reverse 100%, Window Expodec",
        &[("grain.jitter", 1.0), ("grain.size", 60.0), ("grain.density", 15.0), ("grain.reverse", 1.0), ("grain.window", 2.0)]),
    line("fm", "Struck bell", "Frequency 220 Hz, Ratio 3.5, Index 3; Envelope Decay 600 ms, Sustain 0%",
        &[("fm.freq", 220.0), ("fm.ratio", 3.5), ("fm.index", 3.0), ("env.decay", 600.0), ("env.sustain", 0.0)]),
    line("fm", "Bass", "Frequency 55 Hz, Ratio 1, Index 2, Feedback 30%",
        &[("fm.freq", 55.0), ("fm.ratio", 1.0), ("fm.index", 2.0), ("fm.feedback", 0.3)]),
    line("fm", "Harsh metal", "Ratio 7.1, Index 8, Feedback 60%, Type Exponential",
        &[("fm.ratio", 7.1), ("fm.index", 8.0), ("fm.feedback", 0.6), ("fm.type", 2.0)]),
    line("noise", "Ticks", "Colour 0.8; Envelope Decay 60 ms, Sustain 0%",
        &[("noise.color", 0.8), ("env.decay", 60.0), ("env.sustain", 0.0)]),
    line("noise", "Pitched wind", "Band 90%, Width 10%",
        &[("noise.band", 0.9), ("noise.width", 0.1)]),
    Recipe {
        adds: &["material"],
        ..r("dust", "Crackle under the voice", "Gain 40%, Density 30, Tone 70%",
            &[("dust.gain", 0.4), ("dust.density", 30.0), ("dust.tone", 0.7)])
    },
    line("dust", "Rain", "Density 400, Decay 5 ms",
        &[("dust.density", 400.0), ("dust.decay", 5.0)]),
];

/// The line: two bars of sixteenths, as (step, semitones). A minor
/// pentatonic figure that leaves room between its notes.
#[rustfmt::skip]
const LINE: &[(usize, i8)] = &[
    (0, 0), (3, 12), (6, 7), (8, 3), (10, 5), (12, 0), (14, 10),
    (16, 0), (19, 12), (22, 7), (24, 15), (26, 12), (28, 7), (30, 3),
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("shard-examples: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let docs = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs"));
    let home = std::env::var("HOME").map_err(|_| "HOME is not set")?;
    let renders = PathBuf::from(home).join("rhizomatic-preset/renders/shard-examples");
    std::fs::create_dir_all(&renders).map_err(|e| e.to_string())?;
    let audio = docs.join("nodes/audio");
    let _ = std::fs::remove_dir_all(&audio);
    std::fs::create_dir_all(&audio).map_err(|e| e.to_string())?;

    let quote = std::fs::read_to_string(docs.join("quote.txt")).map_err(|e| e.to_string())?;
    let dry = dry_sample(quote.trim(), &renders)?;
    let sample_seconds = read_mono(&dry)?.len() as f32 / SR;

    // The dry clip goes through the session too, so it is what every recipe
    // starts from and not the file itself.
    let clip = render(&dry, sample_seconds, None)?;
    encode(&clip, &renders.join("dry.wav"), &audio.join("dry.m4a"))?;

    let mut pages: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut count: BTreeMap<&str, usize> = BTreeMap::new();
    for recipe in RECIPES {
        let n = count.entry(recipe.node).or_default();
        *n += 1;
        let stem = format!("{}-{n}", recipe.node);
        let clip = render(&dry, sample_seconds, Some(recipe))?;
        let wav = renders.join(format!("{stem}.wav"));
        encode(&clip, &wav, &audio.join(format!("{stem}.m4a")))?;
        println!("{stem}  {}", recipe.name);
        let block = pages.entry(recipe.node).or_default();
        if block.is_empty() && recipe.plays == Plays::Sample {
            block.push(clip_md("Dry", "the sample with nothing on it", "dry"));
        }
        block.push(clip_md(recipe.name, recipe.settings, &stem));
    }
    for (node, block) in pages {
        write_block(&docs.join(format!("nodes/{node}.md")), &block)?;
    }
    Ok(())
}

/// One clip: a new session, the dry sample as its material, the recipe set
/// up through the session as a person would, rendered offline.
fn render(dry: &Path, sample_seconds: f32, recipe: Option<&Recipe>) -> Result<Vec<f32>, String> {
    let off = Offline::new(SR);
    let s = &off.session;
    let material = use_material(&off, dry)?;
    let plays = recipe.map_or(Plays::Sample, |r| r.plays);
    let seconds = match plays {
        Plays::Sample => {
            let seconds = sample_seconds + RING_OUT;
            once(&off, seconds)?;
            seconds
        }
        Plays::Line | Plays::SampleLine => {
            play_line(&off)?;
            // The two bars twice.
            4.0 * 4.0 * 60.0 / LINE_TEMPO
        }
    };
    let Some(recipe) = recipe else {
        return Ok(off.render(SR, seconds));
    };

    // The page's node in the patch: an effect added to the chain, a
    // generator added (on its own when it plays the line), a section made by
    // its first write.
    let effect = Kind::from_name(recipe.node);
    let instance = match effect {
        Some(_) => Some(s.fx_add("patch", recipe.node)?),
        None => None,
    };
    if shard_dsp::params::GENERATORS.contains(&recipe.node) && recipe.node != "material" {
        s.add_generator(recipe.node)?;
        if recipe.node == "grain" {
            s.wire_material("grain", Some(material))?;
        }
        if recipe.plays == Plays::Line || recipe.node == "grain" {
            s.remove_generator("material")?;
        }
    }
    for g in recipe.adds {
        s.add_generator(g)?;
    }
    let id = |id: &str| match (effect, instance) {
        (Some(k), Some(n)) if id.starts_with(&format!("{}.", k.name())) => format!("fx.{n}.{id}"),
        _ => id.to_string(),
    };
    for (key, value) in recipe.set {
        s.set_param(&id(key), *value)?;
    }
    let mut failed = None;
    let at = |place: f32| place * sample_seconds;
    let out = off.render_with(SR, seconds, |t| {
        for m in recipe.moves {
            let (key, value) = match *m {
                Move::Ramp(key, from, to, start, end) => {
                    let (start, end) = (at(start), at(end));
                    if t < start || t > end {
                        continue;
                    }
                    (key, from + (to - from) * (t - start) / (end - start))
                }
                Move::Held(key, start, held) => {
                    let start = at(start);
                    (key, f32::from(u8::from(t >= start && t < start + held)))
                }
                Move::Pulse(key, start, end, period) => {
                    let (start, end) = (at(start), at(end));
                    let up = t >= start && t < end && (t - start) % period < period / 2.0;
                    (key, f32::from(u8::from(up)))
                }
            };
            if let Err(e) = s.set_param(&id(key), value) {
                failed = Some(e);
            }
        }
    });
    match failed {
        Some(e) => Err(format!("{}: {e}", recipe.name)),
        None => Ok(out),
    }
}

/// One pass of the sample: a single step at the top of a four-bar track,
/// at a tempo slow enough that the four bars outlast the clip.
fn once(off: &Offline, seconds: f32) -> Result<(), String> {
    let s = &off.session;
    let mut t = s.tracker();
    t.tempo = (16.0 * 60.0 / seconds).floor();
    let track = &mut t.tracks[0];
    track.length = 64;
    for (i, step) in track.steps.iter_mut().enumerate() {
        step.on = i == 0;
    }
    s.set_tracker(t)?;
    Ok(())
}

/// The line on the first track, each note held a short while.
fn play_line(off: &Offline) -> Result<(), String> {
    let s = &off.session;
    let mut t = s.tracker();
    t.tempo = LINE_TEMPO;
    let track = &mut t.tracks[0];
    track.length = 32;
    for (i, step) in track.steps.iter_mut().enumerate() {
        let note = LINE.iter().find(|(at, _)| *at == i);
        step.on = note.is_some();
        step.pitch = note.map_or(0, |(_, p)| *p);
    }
    s.set_tracker(t)?;
    // Length: Hold, so a note lasts as long as Hold says.
    s.set_param("patch.length", 2.0)?;
    s.set_param("patch.hold", 250.0)
}

/// The voice over the drone, mono, peaking at -1 dBFS.
fn dry_sample(quote: &str, renders: &Path) -> Result<PathBuf, String> {
    let voice_path = renders.join("voice.wav");
    let status = Command::new("say")
        .args(["-v", VOICE, "-r", WORDS_PER_MINUTE])
        .arg("--file-format=WAVE")
        .arg("--data-format=LEF32@48000")
        .arg("-o")
        .arg(&voice_path)
        .arg(quote)
        .status()
        .map_err(|e| format!("say: {e}"))?;
    if !status.success() {
        return Err("say failed".into());
    }
    let voice = read_mono(&voice_path)?;
    let lead = (LEAD_IN * SR) as usize;
    let len = lead + voice.len() + (TAIL * SR) as usize;
    let bed = drone(SR, len as f32 / SR);
    // The drone under the voice, faded in and out so the sample starts and
    // ends in silence.
    let fade = (0.5 * SR) as usize;
    let mut mix: Vec<f32> = bed
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let edge = (i.min(len - 1 - i) as f32 / fade as f32).min(1.0);
            let v = i.checked_sub(lead).and_then(|j| voice.get(j)).copied();
            d * 0.3 * edge + v.unwrap_or(0.0)
        })
        .collect();
    let peak = mix.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let gain = 10f32.powf(-1.0 / 20.0) / peak.max(1e-6);
    for s in &mut mix {
        *s *= gain;
    }
    let path = renders.join("sample.wav");
    write(&path, &mix, 1)?;
    Ok(path)
}

/// Puts the dry sample in the pool and has the patch's material read it.
/// Answers its id.
fn use_material(off: &Offline, dry: &Path) -> Result<u64, String> {
    let s = &off.session;
    let path = dry.to_string_lossy();
    s.add_material(&path)?;
    let id = s
        .pool()
        .materials
        .iter()
        .find(|m| m.path.as_deref() == Some(&*path))
        .map(|m| m.id)
        .ok_or("the dry sample is not in the pool")?;
    s.wire_material("material", Some(id))?;
    Ok(id)
}

/// Writes the clip as a WAV, then encodes it to AAC at `LOUDNESS_LUFS`, by
/// gain alone: measured first, so the dynamics are left as they are.
fn encode(clip: &[f32], wav: &Path, m4a: &Path) -> Result<(), String> {
    if clip.iter().any(|s| !s.is_finite()) {
        return Err(format!("{}: not finite", wav.display()));
    }
    // A short fade at the end, so a clip cut mid-tail does not click.
    let mut clip = clip.to_vec();
    let fade = (0.05 * SR) as usize * 2;
    let n = clip.len();
    for (i, s) in clip[n - fade..].iter_mut().enumerate() {
        *s *= 1.0 - i as f32 / fade as f32;
    }
    write(wav, &clip, 2)?;
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(wav)
        .args(["-af", "loudnorm=print_format=json", "-f", "null", "-"])
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    let report = String::from_utf8_lossy(&out.stderr);
    let field = |key: &str| -> Result<f32, String> {
        let at = report
            .find(&format!("\"{key}\""))
            .ok_or(format!("ffmpeg reported no {key}"))?;
        let v = report[at..]
            .split('"')
            .nth(3)
            .ok_or(format!("ffmpeg reported no {key}"))?;
        v.parse::<f32>().map_err(|e| format!("{key}: {e}"))
    };
    let (lufs, peak) = (field("input_i")?, field("input_tp")?);
    let gain = (LOUDNESS_LUFS - lufs).min(-1.0 - peak);
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(wav)
        .args(["-af", &format!("volume={gain:.2}dB")])
        .args(["-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart"])
        .arg(m4a)
        .status()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !status.success() {
        return Err(format!("ffmpeg could not encode {}", m4a.display()));
    }
    Ok(())
}

const BEGIN: &str =
    "<!-- examples:begin — `just examples` rewrites this block; write outside it -->";
const END: &str = "<!-- examples:end -->";

fn clip_md(name: &str, settings: &str, stem: &str) -> String {
    format!(
        "*{name}: {settings}.*\n\n<audio controls preload=\"none\" src=\"audio/{stem}.m4a\"></audio>"
    )
}

/// Replaces the page's Examples block, or adds one at the end.
fn write_block(page: &Path, clips: &[String]) -> Result<(), String> {
    let text = std::fs::read_to_string(page).map_err(|e| format!("{}: {e}", page.display()))?;
    let block = format!("{BEGIN}\n{}\n{END}", clips.join("\n\n"));
    let next = match (text.find(BEGIN), text.find(END)) {
        (Some(a), Some(b)) => format!("{}{block}{}", &text[..a], &text[b + END.len()..]),
        _ => format!("{}\n\n## Examples\n\n{block}\n", text.trim_end()),
    };
    std::fs::write(page, next).map_err(|e| e.to_string())
}

fn read_mono(path: &Path) -> Result<Vec<f32>, String> {
    let mut r = hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = r.spec();
    if spec.sample_rate != SR as u32 {
        return Err(format!("{}: not at 48 kHz", path.display()));
    }
    let ch = spec.channels as usize;
    let all: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
        hound::SampleFormat::Int => {
            let full = (1u32 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>()
                .map(|s| s.unwrap_or(0) as f32 / full)
                .collect()
        }
    };
    Ok(all
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / ch as f32)
        .collect())
}

fn write(path: &Path, x: &[f32], channels: u16) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: SR as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for s in x {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}
