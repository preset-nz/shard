---
title: Material
node: sample
group: material
summary: A WAV file, or the built-in drone, that a generator reads and plays.
---

# Sample

<!-- generated:begin — rhizome-docs rewrites this block; write outside it -->
A WAV file, or the built-in drone, that a generator reads and plays.

| Control | Range | Default | What it does |
|---|---|---|---|
| Name | text | empty | What the sample is called in the list of materials. |
| File | text | empty | Where the WAV lives on disk; empty for the built-in drone. |
| Octave | -2 oct to 2 oct | 0 oct | Moves the sample up or down whole octaves before anything plays it. |
| Start | 0 to 1 | 0 | Where playback begins, as a fraction of the file. |
| End | 0 to 1 | 1 | Where playback ends, as a fraction of the file. |
| Root note | structured | empty | The MIDI note the sample sounds at, when known. |
| Regions | structured | empty | Marked stretches of the file, as fractions of its length. |
<!-- generated:end -->

## What it's for
