import { useEffect, useRef } from 'react';
import { centreOn, panBy, type View } from '@/waveView';

/**
 * The whole material in miniature, under a zoomed waveform, with a box over
 * the part the waveform shows. Drag the box to pan; press outside it to
 * centre the view there (and keep dragging). The caller hides it at fit.
 */
export function WaveformOverview({
  peaks,
  view,
  onView,
}: {
  /** The whole material's coarse peaks. */
  peaks: number[];
  view: View;
  onView: (v: View) => void;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  /** Where in the box a drag took hold, as a fraction of the file. */
  const grab = useRef<number | null>(null);

  useEffect(() => {
    const canvas = ref.current;
    const ctx = canvas?.getContext('2d');
    if (!canvas || !ctx) return;
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

    ctx.fillStyle = wave;
    const mid = h / 2;
    const step = w / Math.max(1, peaks.length);
    peaks.forEach((p, i) => {
      const half = Math.max(0.5, p * mid * 0.9);
      ctx.fillRect(i * step, mid - half, Math.max(1, step * 0.9), half * 2);
    });

    const x0 = view.start * w;
    const x1 = view.end * w;
    ctx.fillStyle = accent;
    ctx.globalAlpha = 0.18;
    ctx.fillRect(x0, 0, x1 - x0, h);
    ctx.globalAlpha = 1;
    ctx.strokeStyle = accent;
    ctx.lineWidth = 1.5;
    // A box never thinner than a few pixels, so a deep zoom stays grabbable.
    ctx.strokeRect(x0 + 0.75, 0.75, Math.max(3, x1 - x0 - 1.5), h - 1.5);
  }, [peaks, view]);

  const at = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    return Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
  };

  return (
    <canvas
      ref={ref}
      onPointerDown={(e) => {
        const t = at(e);
        e.currentTarget.setPointerCapture(e.pointerId);
        if (t >= view.start && t <= view.end) {
          grab.current = t - view.start;
        } else {
          grab.current = (view.end - view.start) / 2;
          onView(centreOn(view, t));
        }
      }}
      onPointerMove={(e) => {
        if (grab.current === null) return;
        const t = at(e);
        onView(panBy(view, t - grab.current - view.start));
      }}
      onPointerUp={() => {
        grab.current = null;
      }}
      onPointerCancel={() => {
        grab.current = null;
      }}
      aria-label="Waveform overview"
      title="Drag the box to pan; click to centre"
      className="mt-1 h-7 w-full cursor-grab rounded border border-border bg-card [--wave-accent:#e0a96d] [--wave-color:#6b7b93]"
    />
  );
}
