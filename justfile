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

# What a controller sends, and what it does when you send something back.
# No args lists the ports; `just probe "listen 30"`; `just probe "send <port> 90,3C,7F"`.
[group('dev')]
probe args="":
    cargo run --quiet -p shard-probe -- {{args}}

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
    #!/usr/bin/env bash
    set -euo pipefail
    pnpm tauri build
    # Reseal the ad-hoc signature over the whole bundle. Oblique found
    # tauri-bundler's signature can fail `codesign --verify`, which a
    # downloaded copy shows as "app is damaged" (Oblique Epic 14).
    product=$(node -p "require('./src-tauri/tauri.conf.json').productName")
    app="target/release/bundle/macos/${product}.app"
    # Finder draws a .shard file with the app's icon only when the document
    # type names an icon file, and Tauri's fileAssociations has no field for
    # one. A partial src-tauri/Info.plist would replace the declarations
    # rather than merge into them, so the keys go in here, before the reseal.
    plist="$app/Contents/Info.plist"
    plutil -replace 'CFBundleDocumentTypes.0.CFBundleTypeIconFile' -string icon.icns "$plist"
    plutil -replace 'UTExportedTypeDeclarations.0.UTTypeIconFile' -string icon.icns "$plist"
    codesign --deep --force --sign - "$app"

# The friends build: the .app, a README with the Gatekeeper fix, and the
# third-party notices, zipped for a GitHub release. Oblique's recipe.
[group('build')]
package: build
    #!/usr/bin/env bash
    set -euo pipefail
    ./scripts/generate-third-party-licenses.py
    product=$(node -p "require('./src-tauri/tauri.conf.json').productName")
    version=$(node -p "require('./src-tauri/tauri.conf.json').version")
    outdir="dist-friends-build"
    rm -rf "$outdir"
    mkdir -p "$outdir"
    cp -R "target/release/bundle/macos/${product}.app" "$outdir/"
    sed "s/{{"{{"}}PRODUCT_NAME{{"}}"}}/${product}/g" packaging/friends-build-README.txt > "$outdir/README.txt"
    cp THIRD-PARTY-LICENSES LICENSE "$outdir/"
    # ditto, not zip, for the .app: plain zip drops the metadata the
    # signature depends on, which is another way to get "app is damaged".
    (cd "$outdir" && \
        ditto -c -k --keepParent "${product}.app" "${product}-v${version}.zip" && \
        zip -q "${product}-v${version}.zip" README.txt LICENSE THIRD-PARTY-LICENSES)
    echo "Packaged: $outdir/${product}-v${version}.zip"

# Example: just eval-agent run --model qwen/qwen3.6-35b-a3b --think off on
# Local models writing tracker patterns, timed and checked (evals/agent)
[group('eval')]
eval-agent *args:
    python3 evals/agent/agent_eval.py {{args}}
