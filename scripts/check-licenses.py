#!/usr/bin/env python3
"""Licence gate for the Rust tree.

Family policy: permissive only, no copyleft anywhere in what ships, so
staying closed, publishing, or monetising all remain open. The npm side of
the family uses scripts/check-licenses.mjs; this is the cargo equivalent.

Allowlist changes happen here AND in the policy doc, in the same commit.
"""
import json
import subprocess
import sys

ALLOWED = {
    "MIT", "MIT-0", "ISC", "Apache-2.0", "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause", "BSD-3-Clause", "0BSD", "Zlib", "Unlicense", "CC0-1.0",
    "BlueOak-1.0.0",
}

# Reviewed per-crate exceptions. Key = crate name, value = the reasoning.
# Empty on purpose: nothing has needed one yet. Notably absent is symphonia
# (MPL-2.0, file-level copyleft) — hound covers WAV, which is all that is
# needed. Revisit only if MP3 or FLAC input is ever wanted.
EXCEPTIONS: dict[str, str] = {}


def allowed(expr: str | None) -> bool:
    if not expr:
        return False
    # Older crates use `A/B` where SPDX would say `A OR B`.
    e = expr.replace("(", " ").replace(")", " ").replace("/", " OR ").strip()
    if " OR " in e:
        return any(allowed(p.strip()) for p in e.split(" OR "))
    if " AND " in e:
        return all(allowed(p.strip()) for p in e.split(" AND "))
    return e in ALLOWED


def main() -> int:
    raw = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--all-features"],
        capture_output=True, text=True, check=True,
    ).stdout
    packages = json.loads(raw)["packages"]

    bad = []
    for p in packages:
        if p["name"].startswith("shard-") or p["name"] in EXCEPTIONS:
            continue
        if not allowed(p.get("license")):
            bad.append((p["name"], p["version"], p.get("license") or "UNSPECIFIED"))

    if bad:
        print(f"licence gate: {len(bad)} crate(s) outside the permissive allowlist\n")
        for name, version, lic in sorted(set(bad)):
            print(f"  {name} {version}  ->  {lic}")
        print("\nEither drop the dependency or add a reviewed exception here")
        print("and in the policy doc, in the same commit.")
        return 1

    print(f"licence gate: {len(packages)} crates, all permissive")
    return 0


if __name__ == "__main__":
    sys.exit(main())
