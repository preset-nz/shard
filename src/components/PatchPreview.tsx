import { useEffect, useRef } from 'react';
import type { PatchPreview as Preview } from '@/audio';

/**
 * One pass of the patch, drawn (Georg, 2026-09-27: *"just want to see how the
 * patch looks like, how the different things affect it"*).
 *
 * Rust renders it offline with the patch's own values, materials and
 * modulation, heard alone as sound scaping hears it, and sends peaks. The
 * overview is the whole pass, where envelopes, gain and density show; the zoom
 * is 20 ms a quarter of the way in, where timbre shows: FM index, drive, the
 * filter.
 *
 * **A fixed scale, ±1.** Normalising would hide exactly what a gain, a mix or
 * a filter does to the level, which is half of what this is for.
 */
export function PatchPreview({ preview, busy }: { preview: Preview | null; busy: boolean }) {
  const overview = useRef<HTMLCanvasElement>(null);
  const zoom = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    draw(overview.current, preview?.min ?? [], preview?.max ?? [], preview);
    draw(zoom.current, preview?.zoom_min ?? [], preview?.zoom_max ?? [], null);
  }, [preview]);

  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2 text-xs">
        <span className="font-medium text-muted-foreground">Patch</span>
        <span className="truncate text-muted-foreground">
          {preview
            ? `one pass · ${preview.seconds.toFixed(2)}s${preview.capped ? ' · cut' : ''}`
            : 'rendering…'}
        </span>
        {busy && preview && <span className="text-[10px] text-muted-foreground">updating</span>}
      </div>
      <div className="flex gap-2">
        <canvas
          ref={overview}
          className="h-20 min-w-0 flex-1 rounded border border-border bg-card [--wave-accent:#e0a96d] [--wave-color:#6b7b93]"
        />
        <canvas
          ref={zoom}
          title={preview ? `${preview.zoom_ms.toFixed(0)} ms, a quarter of the way in` : undefined}
          className="h-20 w-40 shrink-0 rounded border border-border bg-card [--wave-accent:#e0a96d] [--wave-color:#6b7b93]"
        />
      </div>
    </div>
  );
}

/** Min and max per column, filled, on a fixed ±1 scale. */
function draw(
  canvas: HTMLCanvasElement | null,
  min: number[],
  max: number[],
  marks: Preview | null,
) {
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
  const y = (v: number) => h / 2 - (Math.max(-1, Math.min(1, v)) * h) / 2;

  // The centre line, so silence reads as silence rather than as nothing.
  ctx.fillStyle = wave;
  ctx.globalAlpha = 0.35;
  ctx.fillRect(0, h / 2, w, 1);
  ctx.globalAlpha = 1;

  const n = min.length;
  if (n > 0) {
    const col = w / n;
    for (let i = 0; i < n; i++) {
      const top = y(max[i]);
      const bottom = y(min[i]);
      ctx.fillRect(i * col, top, Math.max(col, 1), Math.max(bottom - top, 1));
    }
  }

  // Where the zoom is taken from.
  if (marks && marks.seconds > 0) {
    const x = marks.zoom_at * w;
    const width = Math.max((marks.zoom_ms / 1000 / marks.seconds) * w, 2);
    ctx.strokeStyle = accent;
    ctx.strokeRect(x, 1, width, h - 2);
  }
}
