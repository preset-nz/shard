#!/usr/bin/env bash
# Generate starter material to granulate, into assets/.
#
# Exists because the hardest part of starting a granular instrument is having
# something to put through it. Everything here is synthesised or spoken on the
# machine, so there is no licensing question and no download.
#
# All of it is deliberately on-aesthetic: drones, metal, clicks, muttering.
# None of it is meant to sound good on its own.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p assets
SR=48000
w() { printf '  %-22s %s\n' "$1" "$2"; }

echo "writing to assets/ at ${SR} Hz, mono"

# A detuned drone. The staple: long, harmonically dense, rewards slow position
# sweeps because different moments have different partials.
sox -n -r $SR -c 1 assets/drone.wav \
  synth 12 sine 55 sine 82.4 sine 110.3 sine 164.9 sine 221.7 \
  remix - gain -12 tremolo 0.13 40 fade t 0.5 12 1 2>/dev/null
w drone.wav "12s detuned sine stack"

# Metal. Resonant filtered noise bursts — the contact-mic-on-a-radiator sound
# without the contact mic. Short grains on this give you percussion.
sox -n -r $SR -c 1 assets/metal.wav \
  synth 6 noise band -n 1200 60 band -n 2400 90 band -n 4700 140 \
  gain -8 reverb 60 50 90 overdrive 8 gain -6 2>/dev/null
w metal.wav "6s resonant noise, three bands"

# Clicks and impulses. Sparse, so jitter and density are obvious: at low
# density you hear individual grains, and the cloud assembles as you raise it.
sox -n -r $SR -c 1 assets/clicks.wav \
  synth 8 pinknoise gain -4 \
  tremolo 7 100 highpass 400 overdrive 15 gain -10 2>/dev/null
w clicks.wav "8s gated pink noise"

# Voices, if the system TTS is available. This is the "wrong throat" trick
# from the soundscape design: render high and fast, play back low and slow, so
# the formants no longer match the pitch. Nothing human sounds like that.
if command -v say >/dev/null 2>&1; then
  tmp=$(mktemp -t shard-say).aiff
  say -v Daniel -r 260 -o "$tmp" \
    "the gate was open. counting again. seven, eight. it does not hold. \
     someone left the light on. turn back. turn back. the same corridor. \
     nine. ten. not yet. not yet." 2>/dev/null || true
  if [ -s "$tmp" ]; then
    sox "$tmp" -r $SR -c 1 assets/voices.wav \
      speed 0.62 rate $SR gain -6 reverb 70 60 100 highpass 180 gain -8 2>/dev/null
    w voices.wav "spoken, pitched down, formants wrong"
  fi
  rm -f "$tmp"
fi

echo
echo "try:  just run       then load one from assets/"
