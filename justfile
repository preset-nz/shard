# Shard — sample shaping instrument.
# Standard verbs: prep, install, run, check, build.

default:
    @just --list

[group('setup')]
prep:
    @echo "rustc:  $(rustc --version 2>/dev/null || echo MISSING)"
    @echo "cargo:  $(cargo --version 2>/dev/null || echo MISSING)"
    @echo "node:   $(node --version 2>/dev/null || echo 'MISSING (needed for the Tauri shell)')"
    @echo "pnpm:   $(pnpm --version 2>/dev/null || echo 'MISSING (needed for the Tauri shell)')"

[group('setup')]
install:
    cargo fetch

# The playground. No UI yet — this is the engine with a slow drift on the
# parameters so you can hear it move.
[group('dev')]
run file="":
    cargo run --release -p shard-play -- {{file}}

# Same engine, parameters held still, so you can hear one setting.
[group('dev')]
still file="":
    cargo run --release -p shard-play -- {{file}} --still

# Render to a WAV instead of a device. Proves the signal path with no
# speakers and no audio device, which is also how CI checks it.
[group('dev')]
render out="shard-render.wav" file="":
    cargo run --release -p shard-play -- {{file}} --render {{out}}

[group('quality')]
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    just licenses

# Licence gate: permissive allowlist, no copyleft anywhere in the tree.
# Family policy, see guidance/projects/oblique/design/licensing.md.
[group('quality')]
licenses:
    @./scripts/check-licenses.py

[group('quality')]
fmt:
    cargo fmt --all

[group('build')]
build:
    cargo build --release --workspace
