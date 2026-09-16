# CLAUDE.md — shard

Sample-shaping instrument. Granular playback plus destructive shaping.

## Read this first

**Planning lives at `~/rhizomatic-preset/guidance/projects/shard/`, not here.**
Read its `README.md` and `open-questions.md` before proposing work.

**Those documents are leans, not law.** This is a greenfield project that is
evolving, and it was explicitly asked that a later session not argue from an
earlier session's decisions as if they were settled. If contact with the code
contradicts a guidance doc, the code wins and the doc gets updated. Say so
rather than working around it.

## Architecture

Three crates, and the boundary around the first is the important part.

- `crates/shard-dsp` — the engine. **No audio device, no file I/O, no UI, no
  async, no dependencies.** If something here needs a crate, it probably
  belongs in the layer above. Everything is `cargo test`able with no sound
  card.
- `crates/shard-play` — a CLI that makes the engine audible. Owns cpal, hound
  and argument parsing.
- `crates/shard-probe` — a MIDI probe, and not part of the app. `just probe`
  lists ports, `just probe "listen 30"` prints what a controller sends, and
  `just probe "send <port> B0,2C,05"` sends raw hex back. Every controller so
  far has needed measuring and every assumption made without it has been
  wrong, so this is kept rather than rewritten each time. The LPD8 table and
  the whole Launch Control 3 protocol were measured with it.

A Tauri and React shell is intended, matching Oblique, Strata and Fault
(Tauri 2, Vite, React, TypeScript, shadcn/ui, zustand, biome). It does not
exist yet. When it lands, audio stays in the Rust process; the webview never
sees a sample. That rule forbids DSP in JavaScript, not a web UI.

## Audio thread rules

All of `shard-dsp` ends up on the audio thread. Non-negotiable:

- No allocation, no locks, no logging in anything called per sample.
- Every buffer is sized once, at construction.
- Parameters cross in through the atomic `ParamBank`, read once per block.
- Anything that allocates (loading a sample) happens above, before the stream
  starts or across a queue.

The first rule is enforced. `shard_dsp::rt::GuardedAlloc` counts allocator
calls (frees included) inside the audio callback in debug builds, and the app
shows the count beside its meters. `crates/shard-dsp/tests/audio_thread.rs`
holds it at zero. **When you add something the callback calls, add it to that
test.** Two traps it has already caught: a buffer dropped on the audio thread
(hand it back instead), and a mutex's first lock, which allocates on macOS
(lock every hand-off slot once before the stream starts).

## Parameters

`params.rs` holds one flat table. **Ids are a wire format** — renaming one
breaks every saved patch. Each entry declares its own smoothing time, and the
taper is both a value mapping and a hint about what control to draw.

Keep the table small. Do not add a parameter the DSP does not read.

**The table is the patch, and the tracker is the level above it.** Tempo,
swing and tracks of steps belong to the song, not to a sound, so they are not
parameters: they live in `src-tauri/src/tracker.rs`, save above the patch in
the `.shard` document, and reach the engine through `steps::StepBank` and
`Engine::set_steps`, once a block. A value that should survive loading a
different patch does not belong in `params.rs`.

**Every switchable node follows one pattern, by role.**
- Its switch is `<node>.on`, drawn in the section header.
- A **generator** makes sound, and its first row after the switch is
  `<node>.gain`, named "Gain". It is listed in `GENERATORS`; today that is the
  plain sample (`material`) and the cloud (`grain`). Generators are summed.
- An **effect** shapes sound, and its first row is `<node>.mix`, named "Mix".- `amp.gain` is the master gain, after everything.
- No row's name repeats its node's name, because the header already says it.
- The panel draws rows in table order, so order in `params.rs` is layout.

The tests `every_switchable_node_leads_with_its_level` and
`no_parameter_repeats_its_node_name` hold this, so a node that breaks the
pattern fails `just check`.

## Testing

Prefer tests that assert a contract rather than a value: windows reach silence
at both ends, a parameter sweep produces no discontinuity, density does not
double as a volume control, output never leaves the valid range. Those catch
real regressions; a golden sample buffer does not.

## Conventions

- `just check` before committing: fmt, clippy with `-D warnings`, tests,
  licence gate.
- Licensing is permissive-only, no copyleft in the tree. `just licenses`
  enforces it. Symphonia is excluded deliberately, being MPL-2.0.
- NZ English in user-facing strings. Identifiers stay standard.
- Dates absolute, `YYYY-MM-DD`.
