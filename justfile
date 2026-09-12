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
    pnpm install
    cargo fetch

# The app.
[group('dev')]
run:
    pnpm tauri dev

# Generate starter material into assets/ — drones, metal, clicks, and spoken
# fragments through the system TTS. Nothing downloaded, nothing licensed.
[group('setup')]
material:
    ./scripts/make-material.sh

# The CLI playground: the same engine with no UI, drifting on its own.
[group('dev')]
play file="":
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
    ./node_modules/.bin/tsc --noEmit
    ./node_modules/.bin/biome check .
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
    ./node_modules/.bin/biome check --write .
    cargo fmt --all

[group('build')]
build:
    pnpm tauri build
