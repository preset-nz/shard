#!/usr/bin/env python3
"""A scripted demo of Shard: a prompt to the agent, the result, then shaping the bass.

    just demo                 # record ~/rhizomatic-preset/renders/shard-demo-<date>.mov,
                              # and an edit (-edit.mp4) with the model's wait at 6x
    just demo --rehearse      # the same moves, Quick mode, no recording

Shard must be running (`just run`) on a freshly launched, empty session.
Don't save a file under the repo while it runs: Vite reloads the webview.
Every control is found by name in the accessibility tree (`ax`), never by a
pixel read off a screenshot, so the run needs nobody watching it: the only
wait is the model thinking. Standard library only; the Swift tools are built
into target/demo/ by `just demo`.
"""

import argparse
import datetime
import pathlib
import signal
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
BIN = ROOT / "target" / "demo"
PROMPT = "a dark industrial beat and a driving bass line in D minor, 125 bpm"


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
    subprocess.run(
        ["osascript", "-e",
         f'tell application "System Events" to tell process "shard" to click menu item "{item}" '
         f'of menu 1 of menu bar item "{bar}" of menu bar 1'],
        check=True, capture_output=True,
    )


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
    subprocess.run(["osascript", "-e", 'tell application "System Events" to tell process "shard" to set frontmost to true'])
    hold(1.5)

    # The empty session first: the built-in drone, without the agent.
    agent_panel(False)
    hold(1)
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
    brake = subprocess.Popen([BIN / "ui", "hold", "11", "2600"])
    hold(1.8)
    click("Stop")
    brake.wait()
    hold(1.5)
    return marks


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rehearse", action="store_true", help="Quick mode, no recording")
    a = ap.parse_args()
    if a.rehearse:
        run(considered=False)
        return
    out = pathlib.Path.home() / "rhizomatic-preset" / "renders" / f"shard-demo-{datetime.date.today():%Y%m%d}.mov"
    rec = subprocess.Popen([BIN / "rec", out, "600"], stdout=subprocess.PIPE, text=True)
    rec.stdout.readline()  # "recording"
    t0 = time.monotonic()
    try:
        marks = run(considered=True)
    finally:
        rec.send_signal(signal.SIGINT)
        rec.wait(timeout=30)
    edit(out, marks["sent"] - t0 + 2, marks["answered"] - t0 - 1)


def edit(raw: pathlib.Path, a: float, b: float, speed: int = 6):
    """The take with the model's wait, `a` to `b` seconds in, `speed` times faster."""
    out = raw.with_name(raw.stem + "-edit.mp4")
    v = (f"[0:v]trim=0:{a},setpts=PTS-STARTPTS[v0];[0:a]atrim=0:{a},asetpts=PTS-STARTPTS[a0];"
         f"[0:v]trim={a}:{b},setpts=(PTS-STARTPTS)/{speed}[v1];"
         f"[0:a]atrim={a}:{b},asetpts=PTS-STARTPTS,atempo=2,atempo={speed / 2}[a1];"
         f"[0:v]trim={b},setpts=PTS-STARTPTS[v2];[0:a]atrim={b},asetpts=PTS-STARTPTS[a2];"
         "[v0][a0][v1][a1][v2][a2]concat=n=3:v=1:a=1[v][a]")
    subprocess.run(
        ["ffmpeg", "-hide_banner", "-v", "error", "-y", "-i", raw, "-filter_complex", v,
         "-map", "[v]", "-map", "[a]", "-c:v", "hevc_videotoolbox", "-q:v", "60", "-tag:v", "hvc1",
         "-c:a", "aac", "-b:a", "192k", out],
        check=True,
    )
    print(f"{raw}\n{out}")


if __name__ == "__main__":
    sys.exit(main())
