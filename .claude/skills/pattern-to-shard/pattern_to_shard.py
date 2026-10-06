#!/usr/bin/env python3
"""Turn a model's step-pattern JSON into a `.shard` document.

The pattern is the shape a prompted local model returns:

    {"bpm": 87, "tracks": {"bass": [
        {"step": 0, "midi_note": 29, "velocity": 120, "duration": 1.0}, ...]}}

One track sounds in Shard today, so one track is taken (`--track`, default `bass`,
else the first). Pitches are semitones from the FM operator's frequency, which is set to
the root note. Durations are in quarter notes, so 0.25 is one step; they
become step holds and the patch's Length is set to Hold.

The script writes a sketch (the tracker, rows by id, effects by kind) and
hands it to `just shard-write`, which makes the `.shard` through Shard's own
session, so the file is always the format the app opens. An id left out keeps
its default.
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]

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
    ap.add_argument("--track", help="which track to take; default bass, else the first")
    ap.add_argument("--root", type=int, help="MIDI root note; default the lowest note")
    ap.add_argument("--swing", type=float, default=0.0)
    ap.add_argument("--effect", action="append", default=[], metavar="KIND",
                    help="an effect on the patch's chain, in order, repeatable: drive, chorus, ...")
    ap.add_argument("--set", action="append", default=[], metavar="ID=VALUE",
                    help="a value, repeatable; arrangement.* ids go to the arrangement, "
                         "and an effect's rows (drive.amount) to the first --effect of that kind")
    args = ap.parse_args()

    pattern = json.load(open(args.pattern))
    tracks = pattern["tracks"]
    track = args.track or ("bass" if "bass" in tracks else next(iter(tracks)))
    root = args.root if args.root is not None else min(
        int(n["midi_note"]) for n in pattern["tracks"][track]
    )

    patch, arrangement = {}, {}
    effects = [{"kind": k} for k in args.effect]

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
        node, _, row = pid.partition(".")
        effect = next((e for e in effects if e["kind"] == node), None)
        if effect is not None:
            effect[row] = float(value)
        elif pid.startswith("arrangement."):
            arrangement[pid] = float(value)
        else:
            patch[pid] = float(value)

    sketch = {
        "tracker": tracker(pattern, track, root, args.swing),
        "patch": dict(sorted(patch.items())),
        "effects": effects,
        "arrangement": dict(sorted(arrangement.items())),
    }
    out = os.path.abspath(args.out)
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
        json.dump(sketch, f, indent=2)
    try:
        run = subprocess.run(["just", "shard-write", f.name, out], cwd=REPO,
                             capture_output=True, text=True)
    finally:
        os.unlink(f.name)
    if run.returncode != 0 or not os.path.exists(out):
        lines = (run.stdout + run.stderr).splitlines()
        why = [lines[i + 1] for i, l in enumerate(lines[:-1]) if "panicked at" in l]
        sys.exit("Shard refused the sketch: " + ("; ".join(why) or "\n".join(lines[-20:])))
    print(f"{out}: {track}, root {root} ({midi_hz(root):.2f} Hz), "
          f"{len(pattern['tracks'][track])} notes, {sketch['tracker']['tempo']:g} bpm")


if __name__ == "__main__":
    main()
