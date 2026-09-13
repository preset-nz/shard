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
just material    # generate something to granulate, into assets/
just run         # the app
```

Press play. You hear the sample as it is, looping. **Dry / Granular** fades
from that into the grain cloud, so every control has an audible before and
after. Space toggles play.

The wave icon beside a control hands that parameter to its own slow
oscillator, at a rate no other control shares. Click it again to take it
back. That is the modulation matrix in embryo: one internal source per
destination now, any source with a depth and a curve later.

### Saving

`Cmd-S` writes a `.shard` patch, `Cmd-O` reads one back.

A patch is the sound, not the material: every parameter value, the LFOs and
the parameters linked to them, node presets, and the path to the sample. A
few kilobytes of readable JSON you can diff.

**Values are keyed by parameter id, never by index**, so inserting a
parameter into the middle of the table does not shift what an old patch
loads. An id this build does not know is ignored and one it has but the file
lacks keeps its default — both are counted and reported, because a patch
that half-applied must not look like one that applied cleanly. A patch whose
sample has moved still loads, and says which file is missing.

### Material

`just material` writes four starting points to `assets/`, all synthesised or
spoken on this machine, so there is nothing to download and no licence to
think about.

| File | What it is | Why it is useful |
|---|---|---|
| `drone.wav` | Detuned sine stack, 12s | Long and harmonically dense, rewards slow position sweeps |
| `metal.wav` | Resonant filtered noise | Short grains turn it into percussion |
| `clicks.wav` | Gated pink noise | Sparse, so density and jitter are obvious |
| `voices.wav` | System TTS, pitched down | Formants no longer match the pitch, so nothing human sounds like it |

WAV only. Symphonia would bring MP3, FLAC and OGG but is MPL-2.0; see the
licensing note below.

### Without the UI

```sh
just play assets/drone.wav    # engine only, drifting on its own
just render out.wav           # render 8s to a file instead of a device
```

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
