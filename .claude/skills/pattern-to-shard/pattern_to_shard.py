#!/usr/bin/env python3
"""Turn a model's step-pattern JSON into a `.shard` document.

The pattern is the shape a prompted local model returns:

    {"bpm": 87, "tracks": {"bass": [
        {"step": 0, "midi_note": 29, "velocity": 120, "duration": 1.0}, ...]}}

One track sounds in Shard today, so one track is taken (`--track`, default the
first). Pitches are semitones from the FM operator's frequency, which is set to
the root note. Durations are in quarter notes, so 0.25 is one step; they
become step holds and the patch's Length is set to Hold.

Patch values are a sparse map: an id left out keeps its default and the app
reports it as "left at default". Pass `--defaults` a tab-separated dump of
`P|A <id> <default>` lines to write every id instead.
"""

import argparse
import json
import sys

STEPS = 64
PITCH_RANGE = 24
HOLD_MAX = 16
LENGTHS = [4, 8, 16, 32, 64]


def midi_hz(note):
    return 440.0 * 2 ** ((note - 69) / 12)


def tracker(pattern, track, root, swing):
    notes = pattern["tracks"][track]
    steps = [
        {"on": False, "pitch": 0, "hold": 0, "velocity": 127, "nudge": 0}
        for _ in range(STEPS)
    ]
    last = 0
    for n in notes:
        i = int(n["step"])
        if not 0 <= i < STEPS:
            sys.exit(f"step {i} is outside 0..{STEPS - 1}")
        pitch = int(n["midi_note"]) - root
        if abs(pitch) > PITCH_RANGE:
            sys.exit(f"note {n['midi_note']} is more than {PITCH_RANGE} semitones from the root")
        hold = max(1, min(HOLD_MAX, round(float(n.get("duration", 0.25)) * 4)))
        steps[i] = {
            "on": True,
            "pitch": pitch,
            "hold": hold,
            "velocity": max(0, min(127, int(n.get("velocity", 127)))),
            "nudge": 0,
        }
        last = max(last, i)
    length = next(n for n in LENGTHS if n > last)
    return {
        "tempo": float(pattern.get("bpm", 120)),
        "swing": swing,
        "tracks": [{"on": True, "length": max(length, 16), "steps": steps}],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("pattern", help="the model's JSON")
    ap.add_argument("out", help="the .shard to write")
    ap.add_argument("--track", help="which track to take; default the first")
    ap.add_argument("--root", type=int, help="MIDI root note; default the lowest note")
    ap.add_argument("--swing", type=float, default=0.0)
    ap.add_argument("--set", action="append", default=[], metavar="ID=VALUE",
                    help="a patch value, repeatable; arrangement.* ids go to the arrangement")
    ap.add_argument("--defaults", help="tab-separated id defaults, to write every id")
    args = ap.parse_args()

    pattern = json.load(open(args.pattern))
    track = args.track or next(iter(pattern["tracks"]))
    root = args.root if args.root is not None else min(
        int(n["midi_note"]) for n in pattern["tracks"][track]
    )

    patch, arrangement = {}, {}
    if args.defaults:
        for line in open(args.defaults):
            kind, pid, value = line.rstrip("\n").split("\t")
            (arrangement if kind == "A" else patch)[pid] = float(value)

    # FM plays the line: the plain sample off, the operator at the root,
    # notes held for their step holds.
    patch.update({
        "material.on": 0.0,
        "fm.on": 1.0,
        "fm.freq": round(midi_hz(root), 4),
        "patch.length": 2.0,
    })
    for kv in args.set:
        pid, value = kv.split("=", 1)
        (arrangement if pid.startswith("arrangement.") else patch)[pid] = float(value)

    doc = {
        "version": 3,
        "tracker": tracker(pattern, track, root, args.swing),
        "arrangement": dict(sorted(arrangement.items())),
        "patch": {"params": dict(sorted(patch.items()))},
    }
    with open(args.out, "w") as f:
        json.dump(doc, f, indent=2)
        f.write("\n")
    print(f"{args.out}: {track}, root {root} ({midi_hz(root):.2f} Hz), "
          f"{len(pattern['tracks'][track])} notes, {doc['tracker']['tempo']:g} bpm")


if __name__ == "__main__":
    main()
