#!/usr/bin/env python3
"""Index the agent sessions Shard saved, so what a local model did can be read and searched.

    agent_logs.py build                 # every saved session: digests and the index
    agent_logs.py search 'drone'        # full text over prompts, reasoning, replies, tool calls
    agent_logs.py search 'kind:tool refused'
    agent_logs.py replies               # one line a reply, newest first

File > Save Agent Session writes `<app data>/agent-sessions/<UTC stamp>/` with
`session.shard`, `agent.log` and `agent.jsonl`. `build` reads every folder there
(`agent.jsonl`, or `agent.log` for a folder saved before it existed) and writes:

- `digest.md` in each folder: every reply as a section, with its tool calls as a
  table, what the loop did on its own, how much of the reasoning repeats itself,
  and the reasoning in full;
- `index.md` beside the folders: one row a reply, across every session;
- `index.sqlite` beside the folders: tables `sessions`, `replies` and `tools`,
  and `blocks`, an FTS5 table over everything said, which `search` queries.

The index is rebuilt whole each time and stays in app data, never in the repo.
Standard library only.
"""

import argparse
import json
import pathlib
import re
import sqlite3
import sys
from collections import Counter

HERE = pathlib.Path(__file__).parent
CONF = HERE.parent.parent / "src-tauri" / "tauri.conf.json"

# A reasoning line shorter than this ("Done.", "Wait,") is not counted as a repeat.
SHORTEST_REPEAT = 4


def default_dir() -> pathlib.Path:
    identifier = json.loads(CONF.read_text())["identifier"]
    return pathlib.Path.home() / "Library" / "Application Support" / identifier / "agent-sessions"


# --- reading a session ------------------------------------------------------


def read_jsonl(path: pathlib.Path):
    lines = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
    header = lines[0] if lines and lines[0].get("kind") == "header" else {}
    events = [l for l in lines if l.get("kind") != "header"]
    return header, events


MARK = re.compile(r"^\[\s*([\d.]+)s\] (.*)$")


def read_log(path: pathlib.Path):
    """The text log, for a folder saved before `agent.jsonl`. Best effort: a
    line of reasoning that happens to look like a timestamp would split it."""
    text = path.read_text()
    head, _, body = text.partition("\n\n")
    header = {"kind": "header"}
    for line in head.splitlines():
        key, _, value = line.partition(": ")
        if key == "base url":
            header["base_url"] = value
        elif key == "model":
            header["model"] = value
        elif key == "sampling":
            header["sampling"] = json.loads(value) if value else None

    events, block = [], None
    for line in body.splitlines():
        m = MARK.match(line)
        if not m:
            if block is not None:
                block.append(line)
            elif events:
                ev = events[-1]
                key, _, value = line.partition(": ")
                if ev["kind"] == "start" and key == "prompt":
                    ev["prompt"] = value
                elif ev["kind"] == "tool" and key == "args":
                    ev["args"] = json.loads(value) if value else None
                elif ev["kind"] == "tool" and key == "result":
                    ev["result"] = value
            continue
        at, rest = float(m.group(1)), m.group(2)
        block = None
        if rest in ("reasoning:", "text:"):
            block = []
            events.append({"kind": rest[:-1], "at": at, "lines": block})
        elif rest.startswith("start"):
            model = re.search(r'model (".*")', rest)
            mode = re.search(r"start, (quick|considered),", rest)
            ev = {"kind": "start", "at": at, "model": json.loads(model.group(1)) if model else ""}
            if mode:
                ev["considered"] = mode.group(1) == "considered"
            events.append(ev)
        elif rest.startswith("tool "):
            _, name, ok = rest.split(" ", 2)
            events.append({"kind": "tool", "at": at, "name": name, "ok": ok == "ok"})
        elif rest.startswith("stopped: "):
            events.append({"kind": "stopped", "at": at, "reason": rest[9:].lower()})
        elif rest.startswith("note: "):
            events.append({"kind": "note", "at": at, "text": rest[6:]})
        elif rest.startswith("error: "):
            events.append({"kind": "error", "at": at, "message": rest[7:]})
        elif rest.startswith("done, "):
            label = re.search(r'undo step (".*")', rest)
            events.append(
                {
                    "kind": "done",
                    "at": at,
                    "applied": "applied true" in rest,
                    "label": json.loads(label.group(1)) if label else "",
                }
            )
    for ev in events:
        if "lines" in ev:
            ev["delta"] = "\n".join(ev.pop("lines")).strip("\n")
    return header, events


def read_session(folder: pathlib.Path):
    if (folder / "agent.jsonl").exists():
        return "jsonl", *read_jsonl(folder / "agent.jsonl")
    return "log", *read_log(folder / "agent.log")


# --- what a reply did -------------------------------------------------------


def repeats(reasoning: str):
    """How much of the reasoning says again what it already said: the share
    of its lines that are a line it held before, and the lines said most."""
    lines = [l.strip() for l in reasoning.splitlines()]
    lines = [l for l in lines if len(l) >= SHORTEST_REPEAT]
    if not lines:
        return 0.0, []
    counts = Counter(lines)
    again = sum(n - 1 for n in counts.values())
    most = [(l, n) for l, n in counts.most_common(8) if n >= 3]
    return again / len(lines), most


def replies(events):
    """The events cut into replies, each from its `start` to its `done`."""
    out, cur = [], None
    for ev in events:
        if ev["kind"] == "start" or cur is None:
            cur = []
            out.append(cur)
        cur.append(ev)
    return out


def summarise(evs):
    start = evs[0] if evs[0]["kind"] == "start" else {}
    done = next((e for e in evs if e["kind"] == "done"), {})
    tools = [e for e in evs if e["kind"] == "tool"]
    reasoning = "\n".join(e.get("delta", "") for e in evs if e["kind"] == "reasoning")
    texts = [e.get("delta", "").strip() for e in evs if e["kind"] == "text"]
    ratio, most = repeats(reasoning)
    t0 = start.get("at", evs[0].get("at", 0.0))
    considered = start.get("considered")
    return {
        "prompt": start.get("prompt", ""),
        "model": start.get("model", ""),
        "mode": {True: "considered", False: "quick", None: "unknown"}[considered],
        "at": t0,
        "seconds": round(evs[-1].get("at", t0) - t0, 2),
        "first_tool": round(tools[0]["at"] - t0, 2) if tools else None,
        "tools": tools,
        "tool_calls": len(tools),
        "refused": sum(not t.get("ok", True) for t in tools),
        "nothing_changed": sum(t.get("result", "").startswith("nothing changed") for t in tools),
        "notes": [e["text"] for e in evs if e["kind"] == "note"],
        "stopped": next((e["reason"] for e in evs if e["kind"] == "stopped"), None),
        "errors": [e["message"] for e in evs if e["kind"] == "error"],
        "applied": done.get("applied"),
        "label": done.get("label", ""),
        "reasoning": reasoning,
        "reasoning_chars": len(reasoning),
        "repeated": round(ratio, 3),
        "most_repeated": most,
        "reply": next((t for t in reversed(texts) if t), ""),
        "events": evs,
    }


# --- writing ----------------------------------------------------------------


def cell(s, most=160):
    s = " ".join(str(s).split()).replace("|", "\\|")
    return s if len(s) <= most else s[: most - 1] + "…"


def digest(name, header, source, rs):
    out = [f"# Agent session {name}", ""]
    out.append(f"- Server: `{header.get('base_url', '')}`; model setting: `{header.get('model') or 'auto'}`")
    if header.get("version"):
        out.append(f"- Shard {header['version']}")
    if source == "log":
        out.append("- Read from `agent.log` (saved before `agent.jsonl`): the mode is unknown")
    out.append("")
    for i, r in enumerate(rs, 1):
        out += [f"## {i}. {r['prompt'] or '(no prompt)'}", ""]
        how = f"{r['mode']}, `{r['model'] or 'no model'}`, {r['seconds']} s"
        if r["first_tool"] is not None:
            how += f", first tool at {r['first_tool']} s"
        out.append(f"- {how}")
        ended = f"stopped: {r['stopped']}" if r["stopped"] else "finished"
        applied = f"applied as “{r['label']}”" if r["applied"] else "nothing applied"
        out.append(f"- {ended}; {applied}")
        for n in r["notes"]:
            out.append(f"- Note: {n}")
        for e in r["errors"]:
            out.append(f"- **Error:** {e}")
        out.append(
            f"- Reasoning: {r['reasoning_chars']} characters, {r['repeated']:.0%} of its lines said before"
        )
        out.append("")
        if r["tools"]:
            out += ["| s | Tool | | Arguments | Result |", "|---|---|---|---|---|"]
            for t in r["tools"]:
                ok = "ok" if t.get("ok", True) else "**refused**"
                out.append(
                    f"| {t['at'] - r['at']:.1f} | `{t['name']}` | {ok} | "
                    f"`{cell(json.dumps(t.get('args'), sort_keys=True), 120)}` | {cell(t.get('result', ''), 400)} |"
                )
            out.append("")
        if r["most_repeated"]:
            out += ["Said most often in the reasoning:", ""]
            out += [f"- {n}× “{cell(l, 120)}”" for l, n in r["most_repeated"]]
            out.append("")
        if r["reply"]:
            out += ["Reply:", "", "> " + r["reply"].replace("\n", "\n> "), ""]
        if r["reasoning"]:
            out += ["<details><summary>Reasoning</summary>", "", "```text", r["reasoning"].strip(), "```", "", "</details>", ""]
    return "\n".join(out)


def index_md(rows):
    out = [
        "# Agent sessions",
        "",
        "Written by `evals/agent/agent_logs.py build`. One row a reply; each session's `digest.md` has the detail.",
        "",
        "| Session | # | Prompt | Mode | Model | s | Tools | Refused | Ended | Applied | Repeated |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for name, i, r in rows:
        out.append(
            f"| [{name}]({name}/digest.md) | {i} | {cell(r['prompt'], 80)} | {r['mode']} | {cell(r['model'], 30)} "
            f"| {r['seconds']} | {r['tool_calls']} | {r['refused']} | {r['stopped'] or 'finished'} "
            f"| {'yes' if r['applied'] else 'no'} | {r['repeated']:.0%} |"
        )
    return "\n".join(out) + "\n"


SCHEMA = """
CREATE TABLE sessions (name TEXT PRIMARY KEY, source TEXT, version TEXT, base_url TEXT, model_setting TEXT, sampling TEXT);
CREATE TABLE replies (
    id INTEGER PRIMARY KEY, session TEXT, n INTEGER, prompt TEXT, model TEXT, mode TEXT,
    seconds REAL, first_tool REAL, tool_calls INTEGER, refused INTEGER, nothing_changed INTEGER,
    retries INTEGER, stopped TEXT, errors TEXT, applied INTEGER, label TEXT,
    reasoning_chars INTEGER, repeated REAL, reply TEXT
);
CREATE TABLE tools (reply INTEGER, at REAL, name TEXT, ok INTEGER, args TEXT, result TEXT);
CREATE VIRTUAL TABLE blocks USING fts5(session UNINDEXED, reply UNINDEXED, at UNINDEXED, kind, name, body);
"""


def body_of(ev):
    k = ev["kind"]
    if k in ("reasoning", "text"):
        return "", ev.get("delta", "")
    if k == "tool":
        return ev["name"], f"{json.dumps(ev.get('args'))}\n{ev.get('result', '')}"
    if k == "start":
        return ev.get("model", ""), ev.get("prompt", "")
    if k == "note":
        return "", ev["text"]
    if k == "error":
        return "", ev["message"]
    if k == "stopped":
        return "", ev["reason"]
    return None


def build(root: pathlib.Path):
    folders = sorted(p for p in root.iterdir() if p.is_dir() and ((p / "agent.jsonl").exists() or (p / "agent.log").exists()))
    if not folders:
        sys.exit(f"no saved agent sessions in {root}")
    db_path = root / "index.sqlite"
    db_path.unlink(missing_ok=True)
    db = sqlite3.connect(db_path)
    db.executescript(SCHEMA)
    rows = []
    for folder in folders:
        source, header, events = read_session(folder)
        rs = [summarise(evs) for evs in replies(events)]
        db.execute(
            "INSERT INTO sessions VALUES (?,?,?,?,?,?)",
            (folder.name, source, header.get("version"), header.get("base_url"), header.get("model"), json.dumps(header.get("sampling"))),
        )
        for i, r in enumerate(rs, 1):
            cur = db.execute(
                "INSERT INTO replies (session, n, prompt, model, mode, seconds, first_tool, tool_calls, refused, nothing_changed, retries, stopped, errors, applied, label, reasoning_chars, repeated, reply) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (
                    folder.name, i, r["prompt"], r["model"], r["mode"], r["seconds"], r["first_tool"],
                    r["tool_calls"], r["refused"], r["nothing_changed"], len(r["notes"]), r["stopped"],
                    "\n".join(r["errors"]), r["applied"], r["label"], r["reasoning_chars"], r["repeated"], r["reply"],
                ),
            )
            rid = cur.lastrowid
            for t in r["tools"]:
                db.execute(
                    "INSERT INTO tools VALUES (?,?,?,?,?,?)",
                    (rid, round(t["at"] - r["at"], 2), t["name"], t.get("ok", True), json.dumps(t.get("args")), t.get("result", "")),
                )
            for ev in r["events"]:
                b = body_of(ev)
                if b:
                    db.execute(
                        "INSERT INTO blocks VALUES (?,?,?,?,?,?)",
                        (folder.name, i, round(ev.get("at", 0.0) - r["at"], 2), ev["kind"], *b),
                    )
            rows.append((folder.name, i, r))
        (folder / "digest.md").write_text(digest(folder.name, header, source, rs))
    db.commit()
    db.close()
    (root / "index.md").write_text(index_md(rows))
    print(f"{len(folders)} sessions, {len(rows)} replies: {root / 'index.md'}, {db_path}")


def connect(root):
    db_path = root / "index.sqlite"
    if not db_path.exists():
        build(root)
    return sqlite3.connect(db_path)


def search(root, query, most):
    db = connect(root)
    # `kind:tool refused` searches the kind column; FTS5 reads that itself.
    found = db.execute(
        "SELECT session, reply, at, kind, name, snippet(blocks, 5, '[', ']', ' … ', 16) FROM blocks WHERE blocks MATCH ? ORDER BY rank LIMIT ?",
        (query, most),
    ).fetchall()
    for session, reply, at, kind, name, snip in found:
        label = f"{kind} {name}".strip()
        print(f"{session} #{reply} +{at:.1f}s {label}: {' '.join(snip.split())}")
    if not found:
        print("nothing found")


def list_replies(root):
    db = connect(root)
    for row in db.execute(
        "SELECT session, n, mode, seconds, tool_calls, refused, coalesce(stopped, 'finished'), repeated, prompt FROM replies ORDER BY session DESC, n DESC"
    ):
        session, n, mode, s, calls, refused, ended, rep, prompt = row
        print(f"{session} #{n} {mode:10} {s:6.1f}s {calls} tools {refused} refused {ended:9} {rep:4.0%} repeated  {cell(prompt, 70)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--dir", type=pathlib.Path, default=None, help="the agent-sessions folder (default: Shard's app data)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build")
    s = sub.add_parser("search")
    s.add_argument("query", help="an FTS5 query: words, \"a phrase\", kind:tool, name:make_bass, OR, NOT")
    s.add_argument("-n", type=int, default=30)
    sub.add_parser("replies")
    a = ap.parse_args()
    root = a.dir or default_dir()
    if a.cmd == "build":
        build(root)
    elif a.cmd == "search":
        search(root, a.query, a.n)
    else:
        list_replies(root)


if __name__ == "__main__":
    main()
