# Shard

A sample-shaping instrument. Granular playback at the core, destructive
shaping around it.

A desktop app: Rust for the audio, Tauri and React for the window. The audio
stays in the Rust process; the webview never sees a sample.

---

## Build it

Built and run on macOS. The stack is cross-platform.

**Prerequisites**
[Rust](https://rustup.rs) stable, [Node](https://nodejs.org),
[pnpm](https://pnpm.io), [just](https://github.com/casey/just), Python 3 for
the licence gate, and Tauri's system dependencies. On macOS those are the
Xcode Command Line Tools (`xcode-select --install`); other platforms are in
[Tauri's prerequisites](https://v2.tauri.app/start/prerequisites/).
[SoX](https://sourceforge.net/projects/sox/) generates the starter material.
`just prep` reports what is installed.

**Install and run**

```sh
just install     # pnpm install and cargo fetch
just material    # starter material, synthesised into assets/
just run         # the app, in dev mode
```

`just build` makes a release bundle.

---

## Play it

The app opens with a built-in drone. Press Space and it loops, plain:
granular and the effects start switched off. Each section has its switch in
its header. Turn one on and set its Mix, and the control has an audible
before and after.

Material is WAV. `just material` synthesises drones, metal, clicks and
pitched-down speech on the machine, so there is nothing to download and no
licence to check.

`Cmd-S` saves a `.shard` file and `Cmd-O` opens one. `Cmd-,` opens Settings,
where MIDI controllers are set up.

**Saving**
A `.shard` file holds two levels. The tracker decides when things play. The
patch is the sound: parameter values, LFOs and their links, node presets, and
which material each generator reads. Material is referenced by path, never
copied in, so a file is a few kilobytes of JSON you can read and diff.

Values are keyed by parameter id, never by index, so inserting a parameter
into the middle of the table does not shift what a file loads. An id this
build does not know is ignored and one the file lacks keeps its default.
Both are reported, because a file that half-applied must not look like one
that applied cleanly.

**Without the window**
`just play <file>` runs the engine alone, drifting on its own. `just render
out.wav` writes eight seconds to a file instead of a device, which checks the
signal path with no speakers and no audio device.

---

## The engine

`shard-dsp` is the engine, with no audio device, no file I/O, no UI, no async
and no dependencies. It passes `cargo test` on any machine: the parts that
make sound do not need a sound card to verify.

All of it runs on the audio thread, so nothing called per sample allocates,
locks or logs. Every buffer is sized once, at construction. Parameters arrive
through an atomic bank, read once per block.

The allocation rule is enforced. In debug builds `shard_dsp::rt::GuardedAlloc`
counts allocator calls inside the audio callback, the app shows the count
beside its meters, and a test holds it at zero.

**The parameter table**
`params.rs` is one flat table with stable string ids. Ids are a wire format:
renaming one breaks every saved file.

Each entry carries its own taper, which does two jobs. It maps a 0-to-1
control position to a value, and it says what control to draw: bipolar gets a
centre detent, stepped gets a selector, exponential gets a perceptual curve.
[`@preset.nz/facets`](https://github.com/preset-nz/facets) generates the panel
from the table, in table order, so adding a parameter is adding a row.

---

## Quality

```sh
just check    # tsc, biome, fmt, clippy with -D warnings, tests, licence gate
just fmt      # rewrite with biome and cargo fmt
```

---

## Licensing

Shard is [MIT](LICENSE).

Dependencies are permissive only, with no copyleft anywhere in the tree.
`just licenses` checks the cargo graph against an allowlist and fails on
anything else. That rules out Symphonia, which is MPL-2.0, so WAV is read by
`hound` and MP3, FLAC and OGG are not read at all.
