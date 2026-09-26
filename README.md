# Shard

Sample-shaping and sound-scaping.

A desktop app for macOS. Material goes through a grain cloud and a chain of
effects that break it, played from steps or left to run as a soundscape.

---

## Play it

The app opens with a built-in drone. Press Space and it loops, plain:
granular and the effects start switched off. Each section has its switch in
its header. Turn one on, set its Mix, and hear the before and after.

Material is WAV files, added to the list on the left and wired into the
sample player or the grain cloud.

`Cmd-S` saves a `.shard` file and `Cmd-O` opens one. `Cmd-,` opens Settings,
where MIDI controllers are set up.

**Saving**
A `.shard` file holds the sound and the steps that play it. Material stays
where it is on disk and the file points to it, so a file is a few kilobytes.
Move a sample and the file still opens, and says which one is missing.

---

## Build it

There is no download. Shard builds from source.

**Prerequisites**
[Rust](https://rustup.rs), [Node](https://nodejs.org),
[pnpm](https://pnpm.io), [just](https://github.com/casey/just), Python 3,
and the Xcode Command Line Tools (`xcode-select --install`).
[SoX](https://sourceforge.net/projects/sox/) makes the starter material.
`just prep` reports what is installed.

```sh
just install     # dependencies
just material    # starter material, synthesised into assets/
just run         # the app
```

`just material` makes drones, metal, clicks and pitched-down speech on your
machine, so there is nothing to download and no licence to check.

`just build` makes a release bundle. `just check` runs the tests, linters and
licence gate.

---

## Licensing

Shard is [MIT](LICENSE).

Its dependencies are permissive only, with no copyleft anywhere in the tree,
and `just check` fails on anything else. That rules out the MPL-2.0 decoder
that reads MP3, FLAC and OGG, which is why material is WAV.
