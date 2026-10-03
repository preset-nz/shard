# Agent evals

How well a local model writes tracker patterns, and how fast. The first step
towards the agent player; the design and the results live in the guidance repo
(`projects/shard/design/agent-player.md`, `runbooks/local-llm-lm-studio.md`).

- `prompts.jsonl`: the requests, each with what a right answer has (track
  count and names, steps, bpm, key).
- `variants/`: system prompts. `fewshot` is the first trial's, with a drums and
  a bass example; `fewshot-bass` keeps only a bass example; `schema` has the
  rules and no examples, and sends a JSON schema as `response_format`.
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
