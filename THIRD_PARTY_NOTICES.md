Third-Party Notices

Data and code from other projects that Shard carries, with their licences.
Crates the build links are checked separately, by `just licences`.

## Free MIDI Chords (SHLD Free MIDI Chord Packs), Ludovic Drolez

- Source: https://github.com/ldrolez/free-midi-chords (`chords.py`, the
  `prog_min` and `prog_modal` lists), read 2026-10-15.
- Used in: `src-tauri/src/music/progressions.rs`, the progression table the
  agent's `make_bass` and `make_arp` tools read.
- What was taken: 70 progressions as roman numerals, with their mood tags.
  No MIDI files and no code.
- What changed: chords reduced to triads (sevenths, extensions and sus
  chords dropped); slash chords and progressions of more than eight chords
  left out; kept only where every chord is diatonic to one of Shard's seven
  modes; renamed by their degrees in that mode; merged where two read the
  same; tags lower-cased, "New" and "Cadence" dropped. Entries marked
  `source: None` in the table are Shard's own.
- Licence: MIT.

```
MIT License

Copyright (c) 2019 Ludovic Drolez

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
