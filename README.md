# Shard

A sample-shaping instrument. Granular playback at the core, destructive
shaping around it. There is no recording: material arrives as samples,
cut-up samples, or synthesis rendered to a sample.

A native desktop app: Rust for the audio, Tauri and React for the window.
The audio stays in the Rust process, and the webview never sees a sample.

## Build it

Developed on macOS. The stack is cross-platform, but no other platform has
been tried.

**Prerequisites**

- [Rust](https://rustup.rs), stable
- [Node](https://nodejs.org) and [pnpm](https://pnpm.io)
- [just](https://github.com/casey/just)
- Tauri's system dependencies. On macOS that is the Xcode Command Line
  Tools (`xcode-select --install`); for other platforms see
  [Tauri's prerequisites](https://v2.tauri.app/start/prerequisites/)
- Python 3, for the licence gate in `just check`
- [SoX](https://sourceforge.net/projects/sox/), only for `just material`

`just prep` prints which of the toolchain is present.

**Install and run**

```sh
just install     # pnpm install and cargo fetch
just material    # generate something to play, into assets/
just run         # the app, in dev mode
```

`just build` makes a release bundle.

## Play it

The app opens with a built-in drone, wired into both generators. Press
Space. You hear the plain sample, looping: Granular and every effect apart
from the Envelope start switched off. Switch on **Granular** in its section
header and the grain cloud joins in. Switch on Crush, Ring, Drive or Filter
and set its Mix, so every control has an audible before and after.

**Materials**
Add WAV files to the material list and wire one into **Sample** and another
into **Granular**. Each material keeps its own octave and trim. The waveform
shows whichever generator or material is selected.

**Two modes**
`Cmd-1` is the tracker: tempo, swing and a track of 4, 8 or 16 steps, each
with its own pitch, rewinding the material's pass. `Cmd-2` is soundscape:
no steps, the material loops.

**Modulation**
Add an LFO, then link parameters to it, each with its own depth. The row
marks the value the engine heard, not only the value you set.

**Tape**
Hold `B` to brake, the finger on the reel. Hold `R` to reverse.

**Controllers**
`Cmd-,` opens Settings, where a MIDI controller becomes one row. Turn a knob
right a few clicks, then back left, and Shard says whether it is a pot or an
endless encoder, and which encoding.

| Key | Does |
|---|---|
| Space | Play and stop |
| `Cmd-S` / `Cmd-O` | Save and open a `.shard` file |
| `Cmd-1` / `Cmd-2` | Tracker and soundscape |
| `Cmd-,` | Settings |
| `B` / `R` (held) | Tape brake and reverse |
| Esc | Deselect |

### Material

`just material` writes starting points to `assets/`, all synthesised or
spoken on the machine, so there is nothing to download and no licence to
check.

| File | What it is | What it does under grains |
|---|---|---|
| `drone.wav` | Detuned sine stack, 12 s | Long and harmonically dense, so slow position sweeps change the partials |
| `metal.wav` | Resonant filtered noise | Short grains turn it into percussion |
| `clicks.wav` | Gated pink noise | Sparse, so density and jitter are obvious |
| `voices.wav` | System speech, pitched down | Formants no longer match the pitch. Written only where macOS `say` exists |

WAV only. Symphonia would bring MP3, FLAC and OGG but is MPL-2.0; see
[Licensing](#licensing).

### Saving

A `.shard` file holds two levels. The **tracker** decides when things play:
tempo, swing, steps. The **patch** is the sound: every parameter value, the
LFOs and their links, node presets, and which material each generator reads.
Materials are referenced by path, never copied in, so a file is a few
kilobytes of JSON you can read and diff.

**Values are keyed by parameter id, never by index.** Inserting a parameter
into the middle of the table does not shift what a file loads. An id this
build does not know is ignored, and one the file lacks keeps its default.
Both are counted and reported, because a file that half-applied must not
look like one that applied cleanly. A file whose material has moved still
loads, and says which one is missing.

### Without the window

```sh
just play assets/drone.wav     # the engine alone, drifting on its own
just still assets/drone.wav    # the same, parameters held still
just render out.wav            # 8 s to a file instead of a device
```

`just render` checks the signal path with no speakers and no audio device.

## Layout

```
crates/shard-dsp     the engine. No device, no I/O, no UI, no async, no dependencies.
crates/shard-play    a CLI that makes the engine audible. Owns cpal and hound.
crates/shard-probe   a MIDI probe. Not part of the app.
src-tauri/           the app process: audio device, documents, materials, MIDI.
src/                 the React interface.
```

`shard-dsp` passes `cargo test` on any machine, because the parts that make
sound should not need a sound card to verify. All of it runs on the audio
thread, so it holds to three rules:

- No allocation, no locks, no logging in anything called per sample.
- Every buffer is sized once, at construction. The grain pool is fixed.
- Parameters arrive through an atomic bank, read once per block.

The first rule is enforced. In debug builds `shard_dsp::rt::GuardedAlloc`
counts allocator calls inside the audio callback, the app shows the count
beside its meters, and `crates/shard-dsp/tests/audio_thread.rs` holds it at
zero.

`shard-probe` prints what a controller sends and sends raw bytes back:

```sh
just probe                        # list ports
just probe "listen 30"            # print 30 s of incoming MIDI
just probe "send <port> B0,2C,05" # send raw hex
```

Every controller mapping in the app was measured with it.

## The parameter table

`crates/shard-dsp/src/params.rs` is one flat list with stable string ids.
Ids are a wire format: renaming one breaks every saved file.

Each entry carries its own taper, which does two jobs. It maps a 0-to-1
control position to a value, and it says what control to draw. Bipolar gets
a centre detent, stepped gets a selector, exponential gets a perceptual
curve. The panel is generated from the table by
[`@preset.nz/facets`](https://github.com/preset-nz/facets), in table order,
so adding a parameter is adding a row.

## Quality

```sh
just check    # tsc, biome, fmt, clippy with -D warnings, tests, licence gate
just fmt      # rewrite with biome and cargo fmt
```

## Licensing

Shard is [MIT](LICENSE).

Its dependencies are permissive only, with no copyleft anywhere in the tree.
`just licenses` checks the cargo graph against an allowlist and fails on
anything else. Symphonia is absent for that reason, being MPL-2.0; `hound`
reads WAV.
