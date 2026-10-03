#!/usr/bin/env python3
"""Ask a local model for a pattern and time the answer.

Streams one chat completion from an OpenAI-compatible server (LM Studio by
default), prints time to first token, tokens per second and token counts, checks
the JSON, and writes it to `--out` for `pattern_to_shard.py`.

    python3 ask_model.py "a bass line that is claustrophobic, 87 bpm, 16 steps" \
        --model qwen/qwen3.6-35b-a3b --out pattern.json

Reasoning arrives as `reasoning_content` on LM Studio and is counted separately,
since it is the time spent before the answer starts.
"""

import argparse
import json
import pathlib
import sys
import time
import urllib.request

HERE = pathlib.Path(__file__).parent


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("prompt")
    ap.add_argument("--model", required=True)
    ap.add_argument("--base", default="http://localhost:1234/v1")
    ap.add_argument("--system", default=str(HERE / "groove-engine.txt"))
    ap.add_argument("--no-think", action="store_true",
                    help='send reasoning_effort "none"; LM Studio honours it, /no_think it ignores')
    ap.add_argument("--temperature", type=float, default=0.7)
    ap.add_argument("--out", help="where to write the pattern JSON")
    args = ap.parse_args()

    user = args.prompt
    body = {
        "model": args.model,
        "messages": [
            {"role": "system", "content": open(args.system).read()},
            {"role": "user", "content": user},
        ],
        "temperature": args.temperature,
        "stream": True,
        "stream_options": {"include_usage": True},
    }
    if args.no_think:
        body["reasoning_effort"] = "none"
    req = urllib.request.Request(
        f"{args.base}/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )

    start = time.monotonic()
    first = first_answer = None
    answer, reasoning, usage, chunks = [], [], None, 0
    with urllib.request.urlopen(req, timeout=600) as resp:
        for raw in resp:
            line = raw.decode().strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
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
    end = time.monotonic()

    text = "".join(answer).strip()
    if text.startswith("```"):
        text = text.split("\n", 1)[1].rsplit("```", 1)[0]
    # A model that thinks inline puts <think>…</think> before the JSON.
    if "</think>" in text:
        text = text.split("</think>", 1)[1].strip()

    completion = (usage or {}).get("completion_tokens", chunks)
    gen_time = end - (first or end)
    report = {
        "model": args.model,
        "no_think": args.no_think,
        "first_token_s": round((first or end) - start, 2),
        "first_answer_s": round((first_answer or end) - start, 2),
        "total_s": round(end - start, 2),
        "prompt_tokens": (usage or {}).get("prompt_tokens"),
        "completion_tokens": completion,
        "reasoning_chars": len("".join(reasoning)),
        "tok_per_s": round(completion / gen_time, 1) if gen_time > 0 else None,
    }
    try:
        pattern = json.loads(text)
        notes = sum(len(v) for v in pattern.get("tracks", {}).values())
        report.update(valid_json=True, bpm=pattern.get("bpm"), notes=notes)
        if args.out:
            pathlib.Path(args.out).write_text(json.dumps(pattern, indent=2) + "\n")
    except json.JSONDecodeError as e:
        report.update(valid_json=False, error=str(e))
        print(text, file=sys.stderr)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
