//! A session with no sound card, rendered through the audio callback's own
//! hand-offs. The tests drive it block by block (`session_tests::rig`), and
//! `just examples` renders the node pages' clips with it.

use std::sync::Arc;

use shard_dsp::arrangement;
use shard_dsp::{Engine, ParamBank};

use crate::session::{Feeds, Session, Voices};
use crate::source;

pub use crate::source::drone;

/// A session and the engine inputs it feeds.
pub struct Offline {
    pub session: Session,
    /// The current patch's values, as the patch view reads them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) bank: Arc<ParamBank>,
    pub(crate) arrangement: Arc<ParamBank>,
    pub(crate) voices: Arc<Voices>,
    /// What the voice playing the current patch heard, as `render` writes it.
    pub(crate) heard: ParamBank,
}

impl Offline {
    /// A new session, as the app starts one: the built-in drone in the pool.
    pub fn new(sample_rate: f32) -> Self {
        let bank = Arc::new(ParamBank::new());
        let arrangement = Arc::new(ParamBank::for_table(arrangement::params()));
        let voices = Arc::new(Voices::new());
        voices.prime();
        let feeds = Feeds {
            bank: Arc::clone(&bank),
            arrangement: Arc::clone(&arrangement),
            voices: Arc::clone(&voices),
        };
        let drone = Arc::new(source::startup_drone(sample_rate));
        let session = Session::new(feeds, drone, sample_rate).expect("a session starts");
        Self {
            session,
            bank,
            arrangement,
            voices,
            heard: ParamBank::new(),
        }
    }

    /// Voice `i`'s feed: track `i`'s, or the audition's.
    #[cfg(test)]
    pub(crate) fn voice(&self, i: usize) -> &crate::session::VoiceFeed {
        &self.voices.voice[i]
    }

    /// One block through the audio callback's own hand-offs and render, as
    /// `lib.rs` makes them, then what `meters` frees collected.
    pub(crate) fn block(&self, e: &mut Engine, out: &mut [f32], held: &mut crate::Held) {
        crate::hand_over(e, &self.voices, &self.arrangement, held);
        crate::render(e, &self.voices, &self.heard, out);
        self.voices.collect_retired();
    }

    /// `seconds` of interleaved stereo with the transport running, as the app
    /// plays it after Play.
    pub fn render(&self, sample_rate: f32, seconds: f32) -> Vec<f32> {
        self.render_with(sample_rate, seconds, |_| {})
    }

    /// `render`, calling `at` before every block with the time in seconds, so
    /// a control can be moved or played while it runs.
    pub fn render_with(&self, sample_rate: f32, seconds: f32, mut at: impl FnMut(f32)) -> Vec<f32> {
        const BLOCK: usize = 256;
        let mut e = Engine::new(sample_rate, crate::LIVE_MAX_GRAINS);
        let mut held = crate::Held::new();
        let frames = (sample_rate * seconds) as usize;
        let mut out = vec![0.0f32; BLOCK * 2];
        // One block stopped, so the session's hand-offs have landed before
        // Play, as they have in the app: a step on the downbeat sounds.
        at(0.0);
        self.block(&mut e, &mut out, &mut held);
        e.set_playing(true);
        let mut all = Vec::with_capacity(frames * 2 + BLOCK * 2);
        while all.len() < frames * 2 {
            at((all.len() / 2) as f32 / sample_rate);
            self.block(&mut e, &mut out, &mut held);
            all.extend_from_slice(&out);
        }
        all.truncate(frames * 2);
        all
    }
}
