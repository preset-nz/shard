import type React from 'react';
import { useEffect, useRef } from 'react';
import type { GrainInfo } from '@/audio';

/**
 * The source, drawn from precomputed peaks. Raw samples never cross the Tauri
 * boundary, so this is all the UI ever sees of the audio.
 *
 * The overlay is the useful part: a line at the read position and a band
 * showing how far jitter scatters grains around it. Watching that band widen
 * is how jitter stops being an abstract number.
 *
 * Grains are drawn on the same principle. Each spawn is one horizontal bar:
 * `position` puts its left edge, `len` gives its width, `pan` its height in
 * the field, and age fades it out. A dozen bars appearing and fading per
 * second is the cloud's mechanism made visible — which is a different and
 * usually more useful thing than a frozen list of what is alive right now.
 */
export function Waveform({
  peaks,
  position,
  jitter,
  playhead,
  trimStart,
  trimEnd,
  envelope,
  grains,
  newestSeq,
  selected,
  totalSamples,
  pickable,
  onTrim,
  onPickGrain,
}: {
  peaks: number[];
  /** Grain read position, as a fraction of the *trimmed window*. */
  position: number;
  /** Grain jitter, also a fraction of the trimmed window. */
  jitter: number;
  /** Plain playback's position, already in whole-file terms. */
  playhead: number | null;
  trimStart: number;
  trimEnd: number;
  /** The envelope across the trimmed window, or null when it is flat. */
  envelope: number[] | null;
  /** Recent spawns, oldest first. Empty switches the overlay off. */
  grains: GrainInfo[];
  /** The highest sequence number seen, so age can fade the older bars. */
  newestSeq: number;
  /** The grain being auditioned, drawn solid. */
  selected: GrainInfo | null;
  /** Length of the whole source, for turning a grain's length into a width. */
  totalSamples: number;
  /**
   * Whether a click inspects a grain instead of dragging a trim handle.
   *
   * Two jobs on one canvas needs a rule, and the honest one is that they
   * belong to different moments: trimming happens while you listen, inspecting
   * while you are stopped or frozen. Deriving the mode from that keeps both
   * gestures available without a mode button nobody would find.
   */
  pickable: boolean;
  onTrim: (which: 'start' | 'end', value: number) => void;
  /** Clicking a drawn grain picks it. Null when the click hit no grain. */
  onPickGrain: (g: GrainInfo | null) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const h = canvas.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const style = getComputedStyle(canvas);
    const wave = style.getPropertyValue('--wave-color').trim() || '#7c8aa5';
    const accent = style.getPropertyValue('--wave-accent').trim() || '#e0a96d';

    // The trimmed window, in file coordinates. Everything below is expressed
    // against this, since the engine only ever reads inside it.
    const lo = Math.min(trimStart, trimEnd);
    const hi = Math.max(trimStart, trimEnd);

    if (peaks.length > 0) {
      ctx.fillStyle = wave;
      const mid = h / 2;
      const step = w / peaks.length;
      peaks.forEach((p, i) => {
        const half = Math.max(0.5, p * mid * 0.95);
        ctx.fillRect(i * step, mid - half, Math.max(1, step * 0.9), half * 2);
      });
    }

    if (lo > 0.001 || hi < 0.999) {
      ctx.fillStyle = '#000';
      ctx.globalAlpha = 0.55;
      ctx.fillRect(0, 0, lo * w, h);
      ctx.fillRect(hi * w, 0, w - hi * w, h);
      ctx.globalAlpha = 1;
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1;
      for (const x of [lo * w, hi * w]) {
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
    }

    // Grain position and jitter are fractions of the trimmed window, not of
    // the file, because the engine hands the granular voice a slice. Map them
    // back into file coordinates or the line walks outside the trim, which is
    // exactly what it looked like it was doing.
    const span = hi - lo;
    const grainX = lo + position * span;
    const jitterSpan = jitter * span;

    // Jitter band first, so the position line sits on top of it.
    if (jitterSpan > 0.001) {
      ctx.fillStyle = accent;
      ctx.globalAlpha = 0.15;
      const a = Math.max(lo, grainX - jitterSpan) * w;
      const b = Math.min(hi, grainX + jitterSpan) * w;
      ctx.fillRect(a, 0, b - a, h);
      ctx.globalAlpha = 1;
    }

    // Every logged spawn as a bar. Drawn under the playhead and the position
    // line so those stay readable through a dense cloud.
    if (grains.length > 0 && totalSamples > 0) {
      for (const g of grains) {
        const x = g.position * w;
        // Reverse grains read backwards from their spawn point, so the span
        // they actually touch lies to the *left* of it. Drawing them
        // rightwards would put the bar over audio the grain never reached.
        const width = Math.max(1.5, (Math.abs(g.len * g.rate) / totalSamples) * w);
        const left = g.rate < 0 ? x - width : x;
        // Pan across the vertical, so a wide stereo spread reads as a
        // scattered field rather than as a thicker line.
        const y = h * 0.12 + g.pan * h * 0.76;

        const age = Math.max(0, Math.min(1, (newestSeq - g.seq) / 120));
        const isPicked = selected !== null && selected.seq === g.seq;
        ctx.globalAlpha = isPicked ? 1 : 0.12 + (1 - age) * 0.5;
        ctx.fillStyle = isPicked ? '#fff' : accent;
        ctx.fillRect(left, y - 1.5, width, 3);
      }
      ctx.globalAlpha = 1;
    }

    // Plain playback's head, drawn thinner and cooler than the grain
    // position, so the two are never confused for each other.
    if (playhead !== null) {
      ctx.strokeStyle = wave;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(playhead * w, 0);
      ctx.lineTo(playhead * w, h);
      ctx.stroke();
    }

    // The envelope across the trimmed window. Drawn as a line over the
    // waveform rather than as a shape around it, so it reads as something
    // applied to the sample rather than as part of it.
    if (envelope && envelope.length > 1) {
      ctx.strokeStyle = accent;
      ctx.globalAlpha = 0.85;
      ctx.lineWidth = 1.5;
      ctx.setLineDash([4, 3]);
      ctx.beginPath();
      envelope.forEach((v, i) => {
        const x = (lo + (i / (envelope.length - 1)) * span) * w;
        const y = h - v * h * 0.94 - h * 0.03;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      });
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    // The grain read position, on top.
    ctx.strokeStyle = accent;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(grainX * w, 0);
    ctx.lineTo(grainX * w, h);
    ctx.stroke();
  }, [
    peaks,
    position,
    jitter,
    playhead,
    trimStart,
    trimEnd,
    envelope,
    grains,
    newestSeq,
    selected,
    totalSamples,
  ]);

  // Dragging near an edge moves it. Whichever edge is closer wins, so there
  // is no mode to be in and nothing to click first.
  const drag = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    const which = Math.abs(t - trimStart) <= Math.abs(t - trimEnd) ? 'start' : 'end';
    onTrim(which, t);
  };

  /**
   * With grains on screen the canvas is an inspector, not a trim handle, so a
   * click picks the nearest bar instead of dragging an edge. Nearest in both
   * axes, because pan spreads them vertically and two grains at the same
   * position are told apart only by height.
   */
  const pick = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const t = (e.clientX - rect.left) / rect.width;
    const v = (e.clientY - rect.top) / rect.height;

    let best: GrainInfo | null = null;
    let bestD = Infinity;
    for (const g of grains) {
      const span = totalSamples > 0 ? Math.abs(g.len * g.rate) / totalSamples : 0;
      const left = g.rate < 0 ? g.position - span : g.position;
      // Distance to the bar, which is zero anywhere along its length.
      const dx = Math.max(0, Math.max(left - t, t - (left + span)));
      const dy = v - (0.12 + g.pan * 0.76);
      const d = Math.hypot(dx, dy * 0.5);
      if (d < bestD) {
        bestD = d;
        best = g;
      }
    }
    onPickGrain(bestD < 0.08 ? best : null);
  };

  return (
    <canvas
      ref={ref}
      onPointerDown={(e) => {
        if (pickable) {
          pick(e);
          return;
        }
        e.currentTarget.setPointerCapture(e.pointerId);
        drag(e);
      }}
      onPointerMove={(e) => {
        if (!pickable && e.buttons === 1) drag(e);
      }}
      className={`h-40 w-full rounded border border-border bg-card [--wave-accent:#e0a96d] [--wave-color:#6b7b93] ${
        pickable ? 'cursor-crosshair' : 'cursor-ew-resize'
      }`}
    />
  );
}
