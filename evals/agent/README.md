# Agent evals

How well a local model writes tracker patterns, and how fast. The first step
towards the agent player; the design and the results live in the guidance repo
(`projects/shard/design/agent-player.md`, `runbooks/local-llm-lm-studio.md`).

- `prompts.jsonl`: the requests, each with what a right answer has (track
  count and names, steps, bpm, key).
- `variants/`: system prompts. `fewshot` is the first trial's, with a drums and
  a bass example; `fewshot-bass` keeps only a bass example; `schema` has the
  rules and no examples, and sends a JSON schema as `response_format`;
  `tools` and `tools-many` answer through tool calls (`set_tempo`,
  `set_track`, `read_song`, and seven plausible extras in `tools-many`), the
  way the agent will.
- **Schema and reasoning don't mix on LM Studio.** The schema constrains the
  output from its first token, so the thinking block is forced into JSON and
  arrives as `reasoning_content` with an empty answer. Run `schema` with
  `--think off` only.
- `agent_eval.py run` asks every prompt under every model, variant and
  reasoning setting, appending to `results/` (not in git) as it goes.
  `agent_eval.py report` summarises.

```sh
just eval-agent run --model qwen/qwen3.6-35b-a3b --variant fewshot schema --think off on --reps 3
just eval-agent report evals/agent/results/*.jsonl
```

The hard checks mirror what the tracker accepts: steps inside the track, pitch
within 24 semitones, hold up to 16 steps, velocity 0 to 127, and one note per
step per track, since a track is monophonic. Plus the tracks and bpm asked for.

## References: what a good answer sounds like

`references/` holds `.shard` documents written by hand and kept by Georg
(2026-10-04: *"that's what the local llm needs to aim for"*). Each answers a
prompt in `prompts.jsonl`, named by its `reference` key. They are the bar a
model's answer is held to once the agent writes whole documents, and the hard
checks above are only the floor under it.

| Prompt | Reference | What makes it work |
|---|---|---|
| `industrial-1990` | `grim-industrial-1990.shard` | 118 bpm, straight. An EBM bass on every sixteenth but the beat, A Phrygian (the flat second is the grim part), inharmonic FM (ratio 1.41) through distortion and a crusher, a mod envelope snapping the filter on each note. Four-on-the-floor kick, backbeat snare, machine hats, a snare roll into the loop. A short dark plate over all. |
| `ambient-afterparty` | `ambient-chillout-afterparty.shard` | 96 bpm, 56 % swing. A four-bar phrase of long notes in D Dorian on a warm FM electric piano (ratio 1, low index, drift), through chorus, a dotted-eighth ping-pong delay and a big hall. Half-time drums, soft, tuned down, dry. |

**Measure an answer the way these were tuned:**

```sh
just shard-check evals/agent/references/grim-industrial-1990.shard
```

It reports whether the document loads clean (its nodes, and anything unknown
or refused), the mix level, and how much the drums add against the
track alone, and renders eight bars to `~/rhizomatic-preset/renders/`. The
references measure:

| Reference | Mix RMS | Drums against the track |
|---|---|---|
| industrial | −12.6 dBFS | 0.98: equal, the bass is the engine |
| ambient | −14.0 dBFS | 0.49: under, the lead leads |

Both load with nothing unknown, refused or missing. The balance is the part
that took tuning: the first drafts had the drums drowning the bass, and the
lead buried under the drums.
