#!/usr/bin/env python3
"""Write THIRD-PARTY-LICENSES at the repo root, for the friends-build zip.

Scope is what ships: the crates compiled into the `shard` binary for this
machine's target (normal dependencies, no dev or build ones), and the npm
production dependencies bundled into the interface. One line per package,
name, version and licence, the same level as Oblique's notices file.

`just licenses` is the gate; this only reports.
"""

import json
import subprocess

OWN = {"shard", "shard-dsp", "shard-play", "shard-probe"}


def crates():
    raw = subprocess.run(
        ["cargo", "tree", "-p", "shard", "-e", "normal", "--prefix", "none",
         "--format", "{p}\t{l}"],
        check=True, capture_output=True, text=True,
    ).stdout
    seen = {}
    for line in raw.splitlines():
        line = line.removesuffix(" (*)")
        if "\t" not in line:
            continue
        pkg, licence = line.split("\t", 1)
        parts = pkg.split(" ")
        name, version = parts[0], parts[1].lstrip("v")
        if name in OWN:
            continue
        seen[(name, version)] = licence.strip() or "unknown"
    return sorted((n, v, l) for (n, v), l in seen.items())


def npm():
    raw = subprocess.run(
        ["pnpm", "licenses", "list", "--json", "--prod"],
        check=True, capture_output=True, text=True,
    ).stdout
    out = []
    for licence, pkgs in json.loads(raw).items():
        for p in pkgs:
            for v in p.get("versions") or [""]:
                out.append((p["name"], v, licence))
    return sorted(set(out))


def section(title, rows):
    lines = [title, "=" * len(title), ""]
    lines += [f"{n} {v}  {l}" for n, v, l in rows]
    return lines + [""]


def main():
    lines = [
        "Shard is MIT licensed. It includes the following third-party",
        "software, each under its own licence.",
        "",
    ]
    lines += section("Rust crates", crates())
    lines += section("npm packages", npm())
    with open("THIRD-PARTY-LICENSES", "w") as f:
        f.write("\n".join(lines))


if __name__ == "__main__":
    main()
