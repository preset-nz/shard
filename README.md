# Shard

A personal sample-shaping instrument. Granular playback at the core,
destructive shaping around it. No recording: material arrives as samples,
cut-up samples, or synthesis rendered to sample.

`shard` is a working slug, not a product name.

**Planning lives in the guidance repo**, not here:
`~/rhizomatic-preset/guidance/projects/shard/`. Start with its `README.md`
and `open-questions.md`. Everything there is a lean from a greenfield,
evolving project. None of it is settled law.

## Hear it

```sh
just run                       # built-in drone, parameters drifting
just run path/to/sample.wav    # your own material
just still path/to/sample.wav  # same, parameters held at defaults
just render out.wav            # render 8s to a file instead of a device
```

There is no UI yet. `shard-play` is a playground: it opens the default output
device, loads a mono-summed WAV, and drifts position, size, density, jitter
and ring frequency on slow incommensurate oscillators so the texture moves.
Ring modulation fades in after about eight seconds.

`just render` exists so the signal path can be checked with no speakers and
no audio device at all.

## Layout

```
crates/shard-dsp    the engine. No device, no I/O, no UI, no async.
crates/shard-play   a CLI that makes shard-dsp audible.
```

`shard-dsp` is the part that matters and it is deliberately dependency-free.
It passes `cargo test` on any machine, because the parts that make sound
should not need a sound card to verify.

The rules it holds to, since all of it runs on the audio thread:

- No allocation, no locks, no logging in anything called per sample.
- Buffers sized once, at construction. The grain pool is fixed.
- Parameters arrive through an atomic bank, read once per block.

## The parameter table

`shard-dsp/src/params.rs` is one flat list with stable string ids. Ids are a
wire format: renaming one breaks every saved patch.

Each entry carries its own taper, which does two jobs. It says how a 0-to-1
control position maps to a value, and it says what the control should look
like. Bipolar wants a centre detent, stepped wants a selector rather than a
slider, exponential wants a perceptual curve. Adding a parameter is adding a
row, so a generated UI stays generated.

This is the same shape as `@preset.nz/facets`, which Strata and Oblique use
for their inspector panels. When Shard grows a UI, the table feeds it.

## Quality

```sh
just check    # fmt, clippy with -D warnings, tests, licence gate
```

Licensing follows the family rule: permissive only, no copyleft anywhere in
the tree. `just licenses` enforces it over the cargo graph. Symphonia is
deliberately absent, being MPL-2.0; `hound` covers WAV, which is all that is
needed so far.
