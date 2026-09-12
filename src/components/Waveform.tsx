import type React from 'react';
import { useEffect, useRef } from 'react';

/**
 * The source, drawn from precomputed peaks. Raw samples never cross the Tauri
 * boundary, so this is all the UI ever sees of the audio.
 *
 * The overlay is the useful part: a line at the read position and a band
 * showing how far jitter scatters grains around it. Watching that band widen
 * is how jitter stops being an abstract number.
 */
export function Waveform({
  peaks,
  position,
  jitter,
  playhead,
  trimStart,
  trimEnd,
  onTrim,
}: {
  peaks: number[];
  position: number;
  jitter: number;
  /** Plain playback's position, or null when stopped. */
  playhead: number | null;
  trimStart: number;
  trimEnd: number;
  onTrim: (which: 'start' | 'end', value: number) => void;
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

    // What trim excludes, dimmed. Drawn first so the waveform sits on top of
    // it and the excluded part still reads as part of the same file.
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

    // Jitter band first, so the position line sits on top of it.
    if (jitter > 0.001) {
      ctx.fillStyle = accent;
      ctx.globalAlpha = 0.15;
      const lo = Math.max(0, position - jitter) * w;
      const hi = Math.min(1, position + jitter) * w;
      ctx.fillRect(lo, 0, hi - lo, h);
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

    // The grain read position, on top.
    ctx.strokeStyle = accent;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(position * w, 0);
    ctx.lineTo(position * w, h);
    ctx.stroke();
  }, [peaks, position, jitter, playhead, trimStart, trimEnd]);

  // Dragging near an edge moves it. Whichever edge is closer wins, so there
  // is no mode to be in and nothing to click first.
  const drag = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const t = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    const which = Math.abs(t - trimStart) <= Math.abs(t - trimEnd) ? 'start' : 'end';
    onTrim(which, t);
  };

  return (
    <canvas
      ref={ref}
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId);
        drag(e);
      }}
      onPointerMove={(e) => {
        if (e.buttons === 1) drag(e);
      }}
      className="h-40 w-full cursor-ew-resize rounded border border-border bg-card [--wave-accent:#e0a96d] [--wave-color:#6b7b93]"
    />
  );
}
