#!/usr/bin/env python3
"""A scripted demo of Shard: a prompt to the agent, the result, then shaping the bass.

    just demo                 # record ~/rhizomatic-preset/renders/shard-demo-<date>.mov,
                              # an edit (-edit.mp4, 1920x1200 H.264) with the model's
                              # wait at 6x and labelled so, and a poster (-poster.jpg)
    just demo --rehearse      # the same moves, Quick mode, no recording
    just demo --recut         # the edit again from today's take and its .json

The take is the window, not the screen: the display switches to 2056x1329
points ("More Space"), Shard's window to 1920x1200 points in the middle of it,
and the recording, at 2x, halves to 1920x1200 with every point one pixel. The
display goes back to what it was when the run ends, however it ends.

The run starts Shard itself: it kills any running Shard, unsaved work and
all, and launches a fresh one with `just run`, so every take, a rehearsal's
too, opens on an empty session. File > New would ask about unsaved changes,
and a lost dialog once jammed it. Shard stays running after the take.
Don't save a file under the repo while it runs: Vite reloads the webview.
Every control is found by name in the accessibility tree (`ax`), never by a
pixel read off a screenshot, so the run needs nobody watching it: the only
wait is the model thinking. Standard library only; the Swift tools are built
into target/demo/ by `just demo`.
"""

import argparse
import datetime
import json
import pathlib
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "demo"
PROMPT = "a dark industrial beat and a driving bass line in D minor, 125 bpm"
SCREEN = (2056, 1329)  # points; More Space on a 16-inch MacBook Pro
WINDOW = (1920, 1200)
MENU_BAR = 38  # points, with the notch
TEAL = "49a8b3"  # ux-kit's dark primary: the captions' plate
EVENTS: list[tuple[float, str, float]] = []  # when, caption, seconds shown


def note(caption: str, seconds: float = 1.8):
    """A caption for the edit's bottom centre, the way a key display shows a
    press: every menu command and held key, since a click shows itself."""
    EVENTS.append((time.monotonic(), caption, seconds))


def ax(*args) -> list[int]:
    # The webview rebuilds its tree as it renders: a miss is retried briefly.
    for _ in range(20):
        out = subprocess.run([BIN / "ax", *map(str, args)], capture_output=True, text=True)
        if out.returncode == 0:
            return [int(v) for v in out.stdout.split()]
        time.sleep(0.25)
    raise SystemExit(f"ax {' '.join(map(str, args))}: {out.stderr.strip()}")


def ui(*args):
    subprocess.run([BIN / "ui", *map(str, args)], check=True)


def click(name, n=1):
    x, y, *_ = ax("find", name, n)
    ui("click", x, y)


def hold(seconds):
    time.sleep(seconds)


def slide(label, to, n=-1, ms=1400):
    """Drags the slider after text `label` to fraction `to` of its track."""
    thumb, y, x0, w = ax("slider", label, n)
    ui("drag", thumb, y, round(x0 + to * w), y, ms)


def menu(bar, item):
    note(f"{bar} › {item}")
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to tell process "shard" to click menu item "{item}" '
         f'of menu 1 of menu bar item "{bar}" of menu bar 1'],
        check=True, capture_output=True,
    )


def display(*size) -> list[int]:
    out = subprocess.run([BIN / "display", *map(str, size)], capture_output=True, text=True)
    if out.returncode:
        raise SystemExit(f"display {' '.join(map(str, size))}: {out.stderr.strip()}")
    return [int(v) for v in out.stdout.split()]


def launch():
    """A fresh Shard: any running one killed, `just run` started detached,
    and back once its window shows Play. The log goes outside the repo,
    since a write under it reloads the webview."""
    subprocess.run(["pkill", "-x", "shard"])
    for _ in range(40):
        if subprocess.run(["pgrep", "-x", "shard"], capture_output=True).returncode:
            break
        time.sleep(0.25)
    log = pathlib.Path(tempfile.gettempdir()) / "shard-demo-run.log"
    with log.open("w") as f:
        subprocess.Popen(["just", "run"], cwd=ROOT, stdout=f, stderr=subprocess.STDOUT, start_new_session=True)
    for _ in range(300):
        time.sleep(1)
        if subprocess.run([BIN / "ax", "find", "Play"], capture_output=True).returncode == 0:
            time.sleep(2)  # the rest of the webview
            return
    raise SystemExit(f"Shard didn't start in 5 minutes; see {log}")


def stage() -> list[int]:
    """A fresh Shard, the display at SCREEN and its window at WINDOW in
    the middle of it, before the recording starts. Returns the window's
    frame, x y w h."""
    launch()
    display(*SCREEN)
    time.sleep(2.5)  # the display settles, and windows move to fit
    w, h = WINDOW
    x, y = (SCREEN[0] - w) // 2, (SCREEN[1] - h + MENU_BAR) // 2
    frame = ax("window", x, y, w, h)
    if frame[2:] != list(WINDOW):
        raise SystemExit(f"the window is {frame[2]}x{frame[3]}, not {w}x{h}")
    subprocess.run(["osascript", "-e", 'tell application "System Events" to tell process "shard" to set frontmost to true'])
    time.sleep(1)
    # The take opens on the bare session: Song, not Loop or Once (the mode is
    # remembered between launches, and Loop goes quiet once Sustain is at
    # zero), and the agent panel already hidden.
    click("Song")
    agent_panel(False)
    time.sleep(1)
    return frame


def agent_panel(shown: bool):
    """View > Agent is a toggle: pressed only when it isn't as wanted."""
    mark = subprocess.run(
        ["osascript", "-e",
         'tell application "System Events" to tell process "shard" to get value of attribute '
         '"AXMenuItemMarkChar" of menu item "Agent" of menu 1 of menu bar item "View" of menu bar 1'],
        capture_output=True, text=True,
    ).stdout.strip()
    if (mark not in ("", "missing value")) != shown:
        menu("View", "Agent")


def undo_title() -> str:
    return subprocess.run(
        ["osascript", "-e",
         'tell application "System Events" to tell process "shard" to get name of menu item 1 '
         'of menu 1 of menu bar item "Edit" of menu bar 1'],
        capture_output=True, text=True,
    ).stdout.strip()


def bass_track() -> str:
    """The name of the melody track the agent made: the last 'Show this
    track's steps' button that isn't Track 1."""
    out = subprocess.run([BIN / "ax", "dump", "Show this track's steps"], capture_output=True, text=True).stdout
    names = [line.split("\t")[1].split(" | ")[0] for line in out.splitlines() if line.startswith("AXButton")]
    return [n for n in names if n != "Track 1"][-1]


def run(considered: bool):
    hold(1.5)

    # The empty session first: the built-in drone, without the agent.
    click("Play")
    hold(5)
    click("Stop")
    hold(1)
    agent_panel(True)
    hold(1.5)

    # The prompt.
    click("Considered" if considered else "Quick")
    hold(0.4)
    click("Ask for a pattern")
    ui("type", PROMPT)
    hold(0.6)
    click("Send")
    marks = {"sent": time.monotonic()}
    while "Agent" not in undo_title():
        time.sleep(0.5)
    marks["answered"] = time.monotonic()
    hold(3)

    # The result.
    click("Play")
    hold(6)

    # The bass, alone, in the patch view.
    bass = bass_track()
    click(bass)
    hold(0.6)
    click(f"Solo {bass}")
    menu("View", "Patch")
    hold(3)

    # The envelope first, while the sound is plain: a short pluck.
    click("Envelope")
    hold(1)
    slide("Decay", 0.08)
    hold(0.6)
    slide("Sustain", 0.0)
    hold(5)

    # The ladder: resonance up.
    click("Ladder")
    hold(1)
    slide("Resonance", 0.7)
    hold(4)

    # Drive: some grit, not noise.
    click("Drive")
    hold(1)
    slide("Mix", 0.6)
    hold(0.8)
    slide("Amount", 0.6)
    hold(0.8)
    click("Overdrive")
    hold(0.5)
    click("Fuzz")
    hold(0.8)
    slide("Tone", 0.6)
    hold(4)

    # Everything again, then stop.
    click(f"Solo {bass}")
    hold(8)
    # The tape brakes under a held B, and Stop lands while it's braking.
    # B is ignored while a button has focus, so a plain heading takes it.
    click("PROCESS")
    note("Hold B · Brake", 2.6)
    brake = subprocess.Popen([BIN / "ui", "hold", "11", "2600"])
    hold(1.8)
    click("Stop")
    brake.wait()
    hold(1.5)
    return marks


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rehearse", action="store_true", help="Quick mode, no recording")
    ap.add_argument("--recut", action="store_true", help="edit today's take again")
    a = ap.parse_args()
    out = pathlib.Path.home() / "rhizomatic-preset" / "renders" / f"shard-demo-{datetime.date.today():%Y%m%d}.mov"
    marks_file = out.with_suffix(".json")
    if a.recut:
        m = json.loads(marks_file.read_text())
        if "meta" not in m:  # a take from before the metadata: dated by its file, kept from now on
            stamp = datetime.datetime.fromtimestamp(out.stat().st_mtime).astimezone()
            m["meta"] = {**take_meta(), "date": stamp.isoformat(timespec="seconds")}
            marks_file.write_text(json.dumps(m, ensure_ascii=False, indent=1))
        edit(out, m["wait"][0], m["wait"][1], [tuple(e) for e in m["captions"]], m["meta"])
        return
    before = display()
    try:
        frame = stage()
        if a.rehearse:
            run(considered=False)
            return
        rec = subprocess.Popen([BIN / "rec", out, "600", *map(str, frame)], stdout=subprocess.PIPE, text=True)
        rec.stdout.readline()  # "recording"
        t0 = time.monotonic()
        try:
            marks = run(considered=True)
        finally:
            rec.send_signal(signal.SIGINT)
            rec.wait(timeout=30)
    finally:
        display(*before[:2])
    wait = (marks["sent"] - t0 + 2, marks["answered"] - t0 - 1)
    captions = [(t - t0, c, d) for t, c, d in EVENTS if t >= t0]
    meta = take_meta()
    marks_file.write_text(json.dumps({"wait": wait, "captions": captions, "meta": meta}, ensure_ascii=False, indent=1))
    edit(out, *wait, captions, meta)


def take_meta() -> dict:
    """What the take was made with: the app, its commit, the model, the day."""
    conf = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text())
    version = json.loads((ROOT / "package.json").read_text())["version"]
    commit = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
    model = None
    try:  # LM Studio lists every downloaded model; only a loaded one ran the take.
        with urllib.request.urlopen("http://localhost:1234/api/v0/models", timeout=3) as r:
            model = next((m["id"] for m in json.load(r)["data"] if m.get("state") == "loaded"), None)
    except OSError:
        pass
    return {"product": conf["productName"], "version": version, "commit": commit, "model": model,
            "date": datetime.datetime.now().astimezone().isoformat(timespec="seconds"), "prompt": PROMPT}


def chapters(path: pathlib.Path, marks: list[tuple[float, str]], end: float):
    """An ffmetadata file of chapters, each running to the next."""
    lines = [";FFMETADATA1"]
    for (start, title), nxt in zip(marks, [m[0] for m in marks[1:]] + [end]):
        lines += ["[CHAPTER]", "TIMEBASE=1/1000", f"START={round(start * 1000)}", f"END={round(nxt * 1000)}", f"title={title}"]
    path.write_text("\n".join(lines) + "\n")


def edit(raw: pathlib.Path, a: float, b: float, captions=(), meta=None, speed: int = 6):
    """The take with the model's wait, `a` to `b` seconds in, `speed` times
    faster under a "6×" plate, and each caption on the same plate at the
    bottom centre for its seconds; halved, as H.264 for every browser, with
    title, credits and chapters, and a poster from the result playing."""
    out = raw.with_name(raw.stem + "-edit.mp4")

    def at(t):  # a time in the take, in the edit
        return t if t <= a else a + (t - a) / speed if t <= b else t - (b - a) * (1 - 1 / speed)

    shown = [(f"▶▶  {speed}×", a, a + (b - a) / speed)]
    shown += [(c, at(t), at(t) + d) for t, c, d in captions if not a < t < b]
    plates, inputs, chain, last = {}, [], "", "vc"
    for i, (text, start, end) in enumerate(shown):
        if text not in plates:
            png = BIN / "labels" / f"{len(plates)}.png"
            png.parent.mkdir(exist_ok=True)
            # At the take's 2x: 52 px high in the edit, 48 px off the bottom.
            subprocess.run([BIN / "label", text, png, "104", TEAL], check=True)
            plates[text] = len(plates) + 1
            inputs += ["-i", png]
        chain += (f"[{last}][{plates[text]}:v]overlay=x=(W-w)/2:y=H-h-96:"
                  f"enable='between(t,{start:.3f},{end:.3f})'[o{i}];")
        last = f"o{i}"
    v = (f"[0:v]trim=0:{a},setpts=PTS-STARTPTS[v0];[0:a]atrim=0:{a},asetpts=PTS-STARTPTS[a0];"
         f"[0:v]trim={a}:{b},setpts=(PTS-STARTPTS)/{speed}[v1];"
         f"[0:a]atrim={a}:{b},asetpts=PTS-STARTPTS,atempo=2,atempo={speed / 2}[a1];"
         f"[0:v]trim={b},setpts=PTS-STARTPTS[v2];[0:a]atrim={b},asetpts=PTS-STARTPTS[a2];"
         "[v0][a0][v1][a1][v2][a2]concat=n=3:v=1:a=1[vc][a];"
         + chain +
         f"[{last}]scale=trunc(iw/4)*2:trunc(ih/4)*2:flags=lanczos,fps=60,format=yuv420p[v]")

    meta = meta or take_meta()
    length = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", raw],
                                  capture_output=True, text=True).stdout) - (b - a) * (1 - 1 / speed)
    when = {c: at(t) for t, c, _ in captions}
    marks = [(0.0, "The empty session"), (when.get("View › Agent", a - 8), "The prompt"),
             (a, f"The model thinking, at {speed}×"), (a + (b - a) / speed, "The result"),
             (when.get("View › Patch"), "Shaping the bass"), (when.get("Hold B · Brake"), "Brake and stop")]
    chapter_file = BIN / "chapters.txt"
    chapters(chapter_file, [m for m in marks if m[0] is not None], length)
    product, model = meta["product"], meta["model"] or "a local model"
    tags = {
        "title": f"{product}: a prompt to the agent, then shaping the bass",
        "artist": "preset.nz",
        "copyright": f"© {meta['date'][:4]} preset.nz",
        "date": meta["date"][:10],
        "creation_time": meta["date"],
        "description": f"{product} {meta['version']}. {model}, running locally, answers “{meta['prompt']}”; "
                       f"its wait is shown at {speed}×. Then the bass is shaped by hand: envelope, ladder, drive.",
        "comment": f"Recorded by just demo at {meta['commit']}.",
    }
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-v", "error", "-y", "-i", raw, *inputs, "-i", chapter_file,
         "-filter_complex", v, "-map", "[v]", "-map", "[a]",
         "-map_metadata", str(len(plates) + 1), "-map_chapters", str(len(plates) + 1),
         *[x for k, val in tags.items() for x in ("-metadata", f"{k}={val}")],
         "-metadata:s:v", "handler_name=Video", "-metadata:s:a", "handler_name=Audio",
         "-metadata:s:a", "language=zxx",
         "-c:v", "libx264", "-preset", "slow", "-crf", "23", "-profile:v", "high",
         "-c:a", "aac", "-b:a", "128k", "-movflags", "+faststart", out],
        check=True,
    )
    # The result playing: the answer, the 3 s hold, then 4 s of Play.
    poster = raw.with_name(raw.stem + "-poster.jpg")
    at = a + (b - a) / speed + 7
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-v", "error", "-y", "-ss", f"{at:.2f}", "-i", out,
         "-frames:v", "1", "-q:v", "3", poster],
        check=True,
    )
    print(f"{raw}\n{out} ({out.stat().st_size / 1e6:.1f} MB)\n{poster}")
    quiet(out)


def quiet(video: pathlib.Path):
    """Says where the take is silent for 4 s or more, so a take that lost
    its sound is caught before anyone watches it."""
    log = subprocess.run(
        ["ffmpeg", "-hide_banner", "-i", video, "-vn", "-af", "silencedetect=n=-60dB:d=4", "-f", "null", "-"],
        capture_output=True, text=True,
    ).stderr
    gaps = [line.split("] ")[1] for line in log.splitlines() if "silence_start" in line or "silence_end" in line]
    print("silent: " + ("; ".join(gaps) if gaps else "nowhere"))


if __name__ == "__main__":
    sys.exit(main())
