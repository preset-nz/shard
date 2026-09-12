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

Two crates, and the boundary between them is the important part.

- `crates/shard-dsp` — the engine. **No audio device, no file I/O, no UI, no
  async, no dependencies.** If something here needs a crate, it probably
  belongs in the layer above. Everything is `cargo test`able with no sound
  card.
- `crates/shard-play` — a CLI that makes the engine audible. Owns cpal, hound
  and argument parsing.

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

## Parameters

`params.rs` holds one flat table. **Ids are a wire format** — renaming one
breaks every saved patch. Each entry declares its own smoothing time, and the
taper is both a value mapping and a hint about what control to draw.

Keep the table small. Do not add a parameter the DSP does not read.

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
