#!/usr/bin/env python3
"""Measure local models writing tracker patterns.

    agent_eval.py run --model qwen/qwen3.6-35b-a3b --variant fewshot schema --think off on --reps 3
    agent_eval.py report results/*.jsonl

`run` asks every prompt in `prompts.jsonl` under every model, prompt variant
and reasoning setting, `--reps` times, and appends one JSON line per answer to
`results/<timestamp>.jsonl` as it goes, so a stopped run keeps what it has.
`report` prints a Markdown summary of any number of result files.

The hard checks mirror what Shard's tracker accepts (`Tracker::sanitised`): a
step inside the track, pitch within 24 semitones of the track's root, a hold of
at most 16 steps, velocity 0 to 127, and one note per step, since a track is
monophonic. The rest describes the music rather than judging it.

Talks to an OpenAI-compatible server, LM Studio by default. Standard library only.
"""

import argparse
import datetime
import json
import pathlib
import statistics
import subprocess
import sys
import time
import urllib.request

HERE = pathlib.Path(__file__).parent
PITCH_RANGE = 24
HOLD_MAX = 16

SCHEMA = {
    "type": "object",
    "properties": {
        "genre": {"type": "string"},
        "bpm": {"type": "integer"},
        "time_signature": {"type": "string"},
        "tracks": {
            "type": "object",
            "additionalProperties": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "step": {"type": "integer"},
                        "midi_note": {"type": "integer"},
                        "velocity": {"type": "integer"},
                        "duration": {"type": "number"},
                    },
                    "required": ["step", "midi_note", "velocity", "duration"],
                },
            },
        },
    },
    "required": ["bpm", "tracks"],
}

STEP_ITEMS = SCHEMA["properties"]["tracks"]["additionalProperties"]


def tool(name, description, properties, required):
    return {"type": "function", "function": {"name": name, "description": description,
            "parameters": {"type": "object", "properties": properties, "required": required}}}


# What Shard's agent would get: coarse tools in its own vocabulary (design/agent-tools.md).
TOOLS = [
    tool("set_tempo", "Set the song's tempo in beats per minute.",
         {"bpm": {"type": "integer"}}, ["bpm"]),
    tool("set_track", "Write one whole track: a voice (kick, bass, lead) and its notes. "
         "Replaces the track if it exists. A track plays one note at a time.",
         {"name": {"type": "string"}, "length": {"type": "integer", "description": "steps: 16, 32 or 64"},
          "notes": STEP_ITEMS}, ["name", "notes"]),
    tool("read_song", "Read the song as it is now: tempo, swing and every track.", {}, []),
]
# Plausible extras, to see whether more tools make the model worse at the ones it needs.
DECOYS = [
    tool("set_swing", "Set swing, 50 (straight) to 75 (heavy).", {"percent": {"type": "number"}}, ["percent"]),
    tool("mute_track", "Mute or unmute a track by name.",
         {"name": {"type": "string"}, "muted": {"type": "boolean"}}, ["name", "muted"]),
    tool("set_patch_value", "Set one sound parameter on a track's patch, e.g. filter.cutoff.",
         {"track": {"type": "string"}, "id": {"type": "string"}, "value": {"type": "number"}},
         ["track", "id", "value"]),
    tool("transpose_track", "Move every note of a track up or down by semitones.",
         {"name": {"type": "string"}, "semitones": {"type": "integer"}}, ["name", "semitones"]),
    tool("clear_track", "Remove every note from a track.", {"name": {"type": "string"}}, ["name"]),
    tool("rotate_track", "Shift a track's notes left or right by steps.",
         {"name": {"type": "string"}, "steps": {"type": "integer"}}, ["name", "steps"]),
    tool("load_patch", "Load a saved patch onto a track by patch name.",
         {"track": {"type": "string"}, "patch": {"type": "string"}}, ["track", "patch"]),
]


def from_calls(calls):
    """The pattern a reply's tool calls would leave in the song."""
    pattern = {"tracks": {}, "calls": [c["name"] for c in calls]}
    for c in calls:
        try:
            a = json.loads(c["arguments"] or "{}")
        except json.JSONDecodeError:
            pattern.setdefault("bad_calls", 0)
            pattern["bad_calls"] = pattern.get("bad_calls", 0) + 1
            continue
        if c["name"] == "set_tempo":
            pattern["bpm"] = a.get("bpm")
        elif c["name"] == "set_track":
            pattern["tracks"][a.get("name", f"track{len(pattern['tracks'])}")] = a.get("notes", [])
    return pattern


KEYS = {"C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4, "F": 5, "F#": 6,
        "Gb": 6, "G": 7, "G#": 8, "Ab": 8, "A": 9, "A#": 10, "Bb": 10, "B": 11}
MINOR = [0, 2, 3, 5, 7, 8, 10]
MAJOR = [0, 2, 4, 5, 7, 9, 11]
DRUM_WORDS = ("kick", "snare", "hat", "perc", "clap", "drum", "tom", "cymbal", "rim")


def power():
    try:
        out = subprocess.run(["pmset", "-g", "batt"], capture_output=True, text=True).stdout
        return "ac" if "AC Power" in out else "battery"
    except OSError:
        return None


MAX_TURNS = 10
# A reasoning model with tools can loop without end, and a stream that keeps
# sending never times out. The app needs the same two limits.
MAX_TOKENS = 8000
BUDGET_S = 240


def ask(base, model, system, prompt, think, schema, temperature, tools=None):
    """One request, or with tools the loop an agent runs: call, result, next
    call, until the model stops calling or `MAX_TURNS`. Timings add up."""
    messages = [{"role": "system", "content": system}, {"role": "user", "content": prompt}]
    if not tools:
        return turn(base, model, messages, think, schema, temperature, None)
    song = {"tracks": {}}
    calls, turns, out = [], 0, None
    started = time.monotonic()
    while turns < MAX_TURNS and time.monotonic() - started < BUDGET_S:
        t = turn(base, model, messages, think, schema, temperature, tools)
        turns += 1
        if out is None:
            out = dict(t)
        else:
            out["total_s"] = round(out["total_s"] + t["total_s"], 2)
            out["completion_tokens"] += t["completion_tokens"]
            out["reasoning_chars"] += t["reasoning_chars"]
            out["gen_s"] += t["gen_s"]
            out["text"] += t["text"]
            out["stopped"] = out["stopped"] or t["stopped"]
        if not t["calls"]:
            break
        calls += t["calls"]
        ids = [f"call_{len(calls) - len(t['calls']) + i}" for i in range(len(t["calls"]))]
        messages.append({"role": "assistant", "content": t["text"] or None, "tool_calls": [
            {"id": i, "type": "function", "function": c} for i, c in zip(ids, t["calls"])]})
        for i, c in zip(ids, t["calls"]):
            messages.append({"role": "tool", "tool_call_id": i, "content": result(c, song)})
    out["calls"] = calls
    out["turns"] = turns
    out["tok_per_s"] = round(out["completion_tokens"] / out["gen_s"], 1) if out["gen_s"] else None
    return out


def result(call, song):
    """What the app would answer a tool call with."""
    try:
        a = json.loads(call["arguments"] or "{}")
    except json.JSONDecodeError as e:
        return f"refused: arguments are not JSON ({e})"
    name = call["name"]
    if name == "set_tempo":
        song["bpm"] = a.get("bpm")
        return f"tempo is {a.get('bpm')} bpm"
    if name == "set_track":
        song["tracks"][a.get("name")] = a.get("notes", [])
        return f"track {a.get('name')} written, {len(a.get('notes', []))} notes"
    if name == "read_song":
        return json.dumps(song)
    return "done"


def turn(base, model, messages, think, schema, temperature, tools):
    body = {
        "model": model,
        "messages": messages,
        "temperature": temperature,
        "stream": True,
        "stream_options": {"include_usage": True},
        "max_tokens": MAX_TOKENS,
    }
    if not think:
        body["reasoning_effort"] = "none"
    if schema:
        body["response_format"] = {
            "type": "json_schema",
            "json_schema": {"name": "pattern", "strict": True, "schema": SCHEMA},
        }
    if tools:
        body["tools"] = tools
    req = urllib.request.Request(f"{base}/chat/completions", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    start = time.monotonic()
    stopped = False
    first = first_answer = None
    answer, reasoning, usage, chunks = [], [], None, 0
    calls = {}
    with urllib.request.urlopen(req, timeout=900) as resp:
        for raw in resp:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            if time.monotonic() - start > BUDGET_S:
                stopped = True
                break
            msg = json.loads(data)
            usage = msg.get("usage") or usage
            for choice in msg.get("choices", []):
                delta = choice.get("delta", {})
                r = delta.get("reasoning_content") or delta.get("reasoning")
                c = delta.get("content")
                if (r or c) and first is None:
                    first = time.monotonic()
                if r:
                    reasoning.append(r)
                    chunks += 1
                if c:
                    if first_answer is None:
                        first_answer = time.monotonic()
                    answer.append(c)
                    chunks += 1
                for tc in delta.get("tool_calls") or []:
                    if first_answer is None:
                        first_answer = time.monotonic()
                    if first is None:
                        first = time.monotonic()
                    slot = calls.setdefault(tc.get("index", 0), {"name": "", "arguments": ""})
                    fn = tc.get("function") or {}
                    slot["name"] += fn.get("name") or ""
                    slot["arguments"] += fn.get("arguments") or ""
                    chunks += 1
    end = time.monotonic()
    completion = (usage or {}).get("completion_tokens", chunks)
    gen = end - (first or end)
    return {
        "text": "".join(answer),
        "calls": [calls[i] for i in sorted(calls)],
        "reasoning_chars": len("".join(reasoning)),
        "first_token_s": round((first or end) - start, 2),
        "first_answer_s": round((first_answer or end) - start, 2),
        "total_s": round(end - start, 2),
        "prompt_tokens": (usage or {}).get("prompt_tokens"),
        "completion_tokens": completion,
        "tok_per_s": round(completion / gen, 1) if gen > 0 else None,
        "gen_s": gen,
        "stopped": stopped,
    }


def parse(text):
    text = text.strip()
    if "</think>" in text:
        text = text.split("</think>", 1)[1].strip()
    if text.startswith("```"):
        text = text.split("\n", 1)[1].rsplit("```", 1)[0]
    return json.loads(text)


def in_key(notes, key):
    tonic, mode = key.split()
    scale = MINOR if mode.lower() == "minor" else MAJOR
    root = KEYS[tonic]
    hits = [((n - root) % 12) in scale for n in notes]
    return sum(hits) / len(hits) if hits else None


def check(pattern, spec):
    """Hard checks (would Shard take it as asked?) and descriptions."""
    out = {"valid_json": True}
    tracks = pattern.get("tracks")
    if not isinstance(tracks, dict) or not tracks:
        out["schema_ok"] = False
        return out
    try:
        notes = {name: [(int(n["step"]), int(n["midi_note"]), int(n["velocity"]),
                         float(n.get("duration", 0.25))) for n in ns]
                 for name, ns in tracks.items()}
    except (KeyError, TypeError, ValueError):
        out["schema_ok"] = False
        return out
    out["schema_ok"] = True
    # A prompt that names no length leaves it to the model, up to four bars.
    steps = spec.get("steps", 64)
    want = spec["tracks"]
    names = [n.lower() for n in notes]
    out["track_count"] = len(notes)
    out["extra_tracks"] = max(0, len(notes) - want)
    out["missing_tracks"] = max(0, want - len(notes))
    if spec.get("names"):
        out["names_ok"] = all(any(w in n for n in names) for w in spec["names"])
    out["bpm_ok"] = pattern.get("bpm") == spec["bpm"] if "bpm" in spec else None

    out_of_range = collisions = 0
    for ns in notes.values():
        seen = set()
        if ns:
            low = min(n[1] for n in ns)
        for step, note, vel, dur in ns:
            if not 0 <= step < steps:
                out_of_range += 1
            if not 0 <= vel <= 127:
                out_of_range += 1
            if note - low > 2 * PITCH_RANGE:
                out_of_range += 1
            if dur * 4 > HOLD_MAX:
                out_of_range += 1
            if step in seen:
                collisions += 1
            seen.add(step)
    out["out_of_range"] = out_of_range
    out["collisions"] = collisions
    out["hard_pass"] = bool(
        out["extra_tracks"] == 0 and out["missing_tracks"] == 0
        and out.get("names_ok", True) and out["bpm_ok"] is not False
        and out_of_range == 0 and collisions == 0
    )

    pitched = [n for name, ns in notes.items() if not any(w in name.lower() for w in DRUM_WORDS)
               for n in ns]
    if spec.get("key") and pitched:
        out["in_key"] = round(in_key([n[1] for n in pitched], spec["key"]), 2)
    all_notes = [n for ns in notes.values() for n in ns]
    out["notes"] = len(all_notes)
    out["density"] = round(len(all_notes) / (len(notes) * steps), 2)
    vels = [n[2] for n in all_notes]
    out["velocity_sd"] = round(statistics.pstdev(vels), 1) if vels else 0
    out["distinct_pitches"] = len({n[1] for n in pitched})
    return out


def run(args):
    prompts = [json.loads(l) for l in open(HERE / "prompts.jsonl") if l.strip()]
    if args.only:
        prompts = [p for p in prompts if p["id"] in args.only]
    outdir = HERE / "results"
    outdir.mkdir(exist_ok=True)
    path = outdir / f"{datetime.datetime.now():%Y-%m-%dT%H%M%S}.jsonl"
    cells = [(m, v, t) for m in args.model for v in args.variant for t in args.think]
    total = len(cells) * len(prompts) * args.reps
    done = 0
    for model, variant, think in cells:
        system = (HERE / "variants" / f"{variant}.txt").read_text()
        schema = variant == "schema"
        tools = {"tools": TOOLS, "tools-many": TOOLS + DECOYS}.get(variant)
        for rep in range(args.reps):
            for spec in prompts:
                done += 1
                row = {"model": model, "variant": variant, "think": think, "prompt": spec["id"],
                       "rep": rep, "power": power(), "temperature": args.temperature,
                       "at": datetime.datetime.now().isoformat(timespec="seconds")}
                try:
                    row.update(ask(args.base, model, system, spec["prompt"], think == "on",
                                   schema, args.temperature, tools))
                    try:
                        row["pattern"] = from_calls(row["calls"]) if tools else parse(row["text"])
                        row["checks"] = check(row["pattern"], spec)
                    except (json.JSONDecodeError, IndexError, AttributeError):
                        row["checks"] = {"valid_json": False, "hard_pass": False}
                except Exception as e:  # a failed call is a result too
                    row["error"] = repr(e)
                    row["checks"] = {"valid_json": False, "hard_pass": False}
                with open(path, "a") as f:
                    f.write(json.dumps(row) + "\n")
                c = row["checks"]
                print(f"[{done}/{total}] {model} {variant} think={think} {spec['id']} "
                      f"{row.get('total_s', '-')}s pass={c.get('hard_pass')} "
                      f"tracks={c.get('track_count')} coll={c.get('collisions')} turns={row.get('turns', 1)}",
                      flush=True)
    print(path)


def mean(xs):
    xs = [x for x in xs if x is not None]
    return round(statistics.mean(xs), 1) if xs else None


def pct(rows, key):
    xs = [r["checks"].get(key) for r in rows]
    xs = [x for x in xs if x is not None]
    return f"{100 * sum(bool(x) for x in xs) / len(xs):.0f} %" if xs else "–"


def report(args):
    rows = [json.loads(l) for p in args.files for l in open(p) if l.strip()]
    # Re-score from the saved answers, so a fixed check applies to old runs.
    specs = {s["id"]: s for s in (json.loads(l) for l in open(HERE / "prompts.jsonl") if l.strip())}
    for r in rows:
        if r.get("pattern") and r["prompt"] in specs:
            r["checks"] = check(r["pattern"], specs[r["prompt"]])
    groups = {}
    for r in rows:
        groups.setdefault((r["model"], r["variant"], r["think"]), []).append(r)
    print("| Model | Variant | Think | n | Valid | Hard pass | Extra tracks | Collisions | Prompt tok | First answer | Total | Tok/s | Turns |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for (m, v, t), rs in sorted(groups.items()):
        extra = sum(1 for r in rs if r["checks"].get("extra_tracks"))
        print(f"| {m.split('/')[-1]} | {v} | {t} | {len(rs)} | {pct(rs, 'valid_json')} | "
              f"{pct(rs, 'hard_pass')} | {extra}/{len(rs)} | "
              f"{mean([r['checks'].get('collisions') for r in rs])} | "
              f"{mean([r.get('prompt_tokens') for r in rs])} | "
              f"{mean([r.get('first_answer_s') for r in rs])} s | "
              f"{mean([r.get('total_s') for r in rs])} s | {mean([r.get('tok_per_s') for r in rs])} | "
              f"{mean([r.get('turns', 1) for r in rs])} |")
    print()
    print("| Prompt | Think | n | Hard pass | Tracks right | Collisions | Notes | Density | Vel. sd | In key | Total |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    by = {}
    for r in rows:
        by.setdefault((r["prompt"], r["think"]), []).append(r)
    for (p, t), rs in sorted(by.items()):
        right = sum(1 for r in rs if r["checks"].get("extra_tracks") == 0
                    and r["checks"].get("missing_tracks") == 0)
        print(f"| {p} | {t} | {len(rs)} | {pct(rs, 'hard_pass')} | {right}/{len(rs)} | "
              f"{mean([r['checks'].get('collisions') for r in rs])} | "
              f"{mean([r['checks'].get('notes') for r in rs])} | "
              f"{mean([r['checks'].get('density') for r in rs])} | "
              f"{mean([r['checks'].get('velocity_sd') for r in rs])} | "
              f"{mean([r['checks'].get('in_key') for r in rs])} | "
              f"{mean([r.get('total_s') for r in rs])} s |")
    power_states = sorted({r.get("power") or "?" for r in rows})
    print(f"\nPower: {', '.join(power_states)}. {len(rows)} answers.")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    r.add_argument("--model", nargs="+", required=True)
    r.add_argument("--variant", nargs="+", default=["fewshot"],
                   help="files in variants/; 'schema' also sends a JSON schema, "
                        "'tools' and 'tools-many' answer through tool calls")
    r.add_argument("--think", nargs="+", default=["off"], choices=["off", "on"])
    r.add_argument("--reps", type=int, default=3)
    r.add_argument("--only", nargs="+", help="prompt ids")
    r.add_argument("--temperature", type=float, default=0.7)
    r.add_argument("--base", default="http://localhost:1234/v1")
    p = sub.add_parser("report")
    p.add_argument("files", nargs="+")
    args = ap.parse_args()
    (run if args.cmd == "run" else report)(args)


if __name__ == "__main__":
    sys.exit(main())
