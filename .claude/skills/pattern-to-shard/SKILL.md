---
name: pattern-to-shard
description: Turn a step pattern (a local model's JSON of step / midi_note / velocity / duration, or one written by hand) into a .shard document that opens in Shard, with FM or another generator playing it. Use when asked to "make a shard file from this pattern/bassline/beat", or to try what an LLM generated.
---

# Pattern to `.shard`

The first, file-writing stage of the agent player
(`~/rhizomatic-preset/guidance/projects/shard/design/agent-player.md`). The app
will later do this live through tools; until then this makes a model's output
audible.

## Input

The shape a prompted model returns (see the agent-player doc for the prompt):

```json
{"bpm": 87, "tracks": {"bass": [
  {"step": 0, "midi_note": 29, "velocity": 120, "duration": 1.0, "comment": "..."}
]}}
```

- `step` 0 to 63, sixteenths. `duration` in quarter notes, so 0.25 is one step.
- Trust `midi_note`, not the comments. Models name notes wrongly (28 is E1, not Eb1).

## Ask a model

`ask_model.py` sends `groove-engine.txt` (the system prompt from the first trial) and a request to LM Studio, prints timings and a JSON check, and writes the pattern:

```sh
python3 .claude/skills/pattern-to-shard/ask_model.py "a bass line that is claustrophobic, 87 bpm, 16 steps" \
  --model qwen/qwen3.6-35b-a3b --no-think --out pattern.json
```

`--no-think` sends `reasoning_effort: "none"`, which takes Qwen from about a minute to about six seconds. Setup and measurements: `~/rhizomatic-preset/guidance/runbooks/local-llm-lm-studio.md`.

## Run

```sh
python3 .claude/skills/pattern-to-shard/pattern_to_shard.py pattern.json ~/Downloads/name.shard \
  --root 29 --set fm.ratio=1 --set fm.index=2.4 --set filter.on=1 --set filter.cutoff=420 \
  --effect drive --set drive.amount=14
```

What it does:
- **One track.** One track sounds today. `--track drums` picks one; the default is `bass`, else the first. Models add tracks nobody asked for.
- **FM plays it.** `material.on` 0, `fm.on` 1, `fm.freq` the root's frequency.
  Step pitch is semitones from the root, within ±24. `--root` defaults to the lowest note.
- **Notes hold.** `patch.length` is 2 (Hold) and each step's `hold` is its duration × 4, from 1 to 16.
- **`--set id=value`** for any patch parameter, `arrangement.*` for the arrangement. Ids, ranges and step
  names are in `crates/shard-dsp/src/params.rs`. FM type, filter type and the like are stepped indices.
- **`--effect kind`** adds an effect to the patch's chain, in order (`drive`, `chorus`, `delay`, ...; the
  kinds are `shard_dsp::fx::Kind`). `--set drive.amount=14` sets the first drive's row.
- **Shard writes the file.** The script builds a sketch and runs `just shard-write`, which makes the
  `.shard` through the app's own session (`src-tauri/src/sketch.rs`). The file is rhizome's format, what
  the app saves. An unknown id or kind fails with its name, and nothing is written. Ids left out keep
  their defaults.
- The first run compiles the tests, so it takes a minute; later runs take seconds.

## Check

```sh
just shard-check ~/Downloads/name.shard
```

It prints whether the file loads clean (nodes, unknown, refused), its level and how the drums sit
against the track, and renders eight bars to `~/rhizomatic-preset/renders/`. Then open it in Shard
(`open ~/Downloads/name.shard`) and press play.

Save files to `~/Downloads/` unless told otherwise. Renders go to `~/rhizomatic-preset/renders/`.
