import type React from 'react';
import { useEffect, useRef, useState } from 'react';
import type { GrainInfo, Region } from '@/audio';
import { WaveformOverview } from '@/components/WaveformOverview';
import { FIT, fromView, isFit, panBy, span as spanOf, type View, zoomAround } from '@/waveView';

/**
 * How many recent spawns the live overlay shows before a bar is gone.
 *
 * Counted in grains rather than in seconds on purpose. At a dense setting that
 * makes the trail short, so the cloud reads as motion instead of smearing into
 * a haze; at a sparse one it makes bars persist, which is exactly what you want
 * when you have turned the density down to watch them arrive one at a time.
 */
const LIVE_TRAIL = 120;

/** The strip along the top that belongs to regions, in pixels. */
const LANE = 16;
/** How near an edge a press must be to grab it, in pixels. */
const GRAB = 6;
/** The narrowest region a drag may make, as a fraction of the file. */
const MIN_REGION = 0.002;

/** Pixels per zoomed bucket asked of Rust: one bucket per this many. */
const PX_PER_BUCKET = 2;
/** How long a view must sit still before finer peaks are asked for. */
const PEAKS_DEBOUNCE_MS = 40;

/** Peaks fetched for a zoomed view, with the range they cover. */
interface ZoomPeaks {
  key: number | null;
  start: number;
  end: number;
  peaks: number[];
}

/** What a press that began on a region (or a Shift-drag) is doing. */
type RegionDrag =
  | { kind: 'add'; anchor: number }
  | { kind: 'edge'; index: number; edge: 'start' | 'end' }
  | { kind: 'move'; index: number; grab: number; start: number; end: number };

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
  showGrain = true,
  playhead,
  trimStart,
  trimEnd,
  envelope,
  grains,
  newestSeq,
  selected,
  totalSamples,
  inspecting,
  onTrim,
  onPickGrain,
  scrub = 'off',
  scrubAt = null,
  onScrub,
  regions = [],
  selectedRegion = null,
  onAddRegion,
  onSetRegion,
  onSelectRegion,
  onRemoveRegion,
  view = FIT,
  onView,
  minSpan = 0,
  fetchPeaks,
  fetchKey = null,
}: {
  peaks: number[];
  /** Grain read position, as a fraction of the *trimmed window*. */
  position: number;
  /** Grain jitter, also a fraction of the trimmed window. */
  jitter: number;
  /**
   * Whether to draw the grain read position and its jitter band. Only over
   * the material the cloud reads; over any other they point at nothing.
   */
  showGrain?: boolean;
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
   * Whether the canvas is being used to inspect grains rather than to play.
   *
   * Two jobs on one canvas needs a rule, and the honest one is that they
   * belong to different moments: trimming happens while you listen, inspecting
   * while you are stopped or frozen. Deriving the mode from that keeps both
   * gestures available without a mode button nobody would find.
   *
   * It governs the drawing as well as the click, because the two modes want
   * opposite things from the overlay. Live, it should show a moving window of
   * what is happening now. Inspecting, it should show the whole scrollback at
   * an even weight, because anything drawn is something you can click.
   */
  inspecting: boolean;
  onTrim: (which: 'start' | 'end', value: number) => void;
  /** Clicking a drawn grain picks it. Null when the click hit no grain. */
  onPickGrain: (g: GrainInfo | null) => void;
  /**
   * Scrubbing (roadmap row 45). Tape and Freeze turn a press or drag on the
   * canvas into a position instead of a trim or a pick; Off leaves both as
   * they were.
   */
  scrub?: 'off' | 'tape' | 'freeze';
  /** Where the scrub row is asked to be, in whole-file terms; null hides it. */
  scrubAt?: number | null;
  /** The position dragged to, as a fraction of the trimmed window. */
  onScrub?: (t: number) => void;
  /**
   * Regions (roadmap row 49), in whole-file terms, drawn as shaded spans with
   * their numbers. They have a strip of their own along the top, so a plain
   * drag below it is still the trim: Shift-drag anywhere adds one, a drag in
   * the strip moves one or, at its edge, resizes it, a click there selects
   * it, and Backspace removes the selected one while the waveform has focus.
   */
  regions?: Region[];
  selectedRegion?: number | null;
  onAddRegion?: (start: number, end: number) => void;
  onSetRegion?: (index: number, start: number, end: number) => void;
  onSelectRegion?: (index: number | null) => void;
  onRemoveRegion?: () => void;
  /**
   * The visible time window, as fractions of the whole material. Webview
   * state held by the caller, since the View menu's zoom commands move it
   * too. Everything drawn and every pointer position goes through it.
   */
  view?: View;
  onView?: (v: View) => void;
  /** The narrowest span the view may take, as a fraction of the material. */
  minSpan?: number;
  /**
   * Finer peaks for the visible range, from Rust: `buckets` of them across
   * the fractions `start` to `end`. Asked for only while zoomed; until they
   * arrive the coarse peaks are drawn stretched.
   */
  fetchPeaks?: (start: number, end: number, buckets: number) => Promise<number[]>;
  /** Which material `fetchPeaks` reads, so one's peaks are never drawn for another. */
  fetchKey?: number | null;
}) {
  const ref = useRef<HTMLCanvasElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const [zoomPeaks, setZoomPeaks] = useState<ZoomPeaks | null>(null);
  const live = useRef({ view, onView, minSpan });
  live.current = { view, onView, minSpan };
  const zoomed = !isFit(view);
  const vs = spanOf(view);
  const regionDrag = useRef<RegionDrag | null>(null);
  /** A region being swept out by Shift-drag, before it is added. */
  const [draft, setDraft] = useState<Region | null>(null);

  // Finer peaks for the visible range, once the view has settled.
  const { start: viewStart, end: viewEnd } = view;
  useEffect(() => {
    if (!fetchPeaks || viewEnd - viewStart >= 0.999999) return;
    const width = ref.current?.clientWidth ?? 600;
    const buckets = Math.min(2048, Math.max(64, Math.round(width / PX_PER_BUCKET)));
    let alive = true;
    const timer = window.setTimeout(() => {
      fetchPeaks(viewStart, viewEnd, buckets)
        .then((p) => {
          if (alive) setZoomPeaks({ key: fetchKey, start: viewStart, end: viewEnd, peaks: p });
        })
        .catch(() => {
          // Keep drawing the coarse peaks.
        });
    }, PEAKS_DEBOUNCE_MS);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [fetchPeaks, fetchKey, viewStart, viewEnd]);

  // Pinch and two-finger scroll. Native listeners, because React's wheel is
  // passive and the page must not zoom or scroll under a gesture here.
  useEffect(() => {
    const el = wrap.current;
    const canvas = ref.current;
    if (!el || !canvas) return;
    const across = (clientX: number) => {
      const rect = canvas.getBoundingClientRect();
      return Math.min(1, Math.max(0, (clientX - rect.left) / Math.max(1, rect.width)));
    };
    const onWheel = (e: WheelEvent) => {
      const { view: v, onView: set, minSpan: min } = live.current;
      if (!set) {
        if (e.ctrlKey) e.preventDefault();
        return;
      }
      if (e.ctrlKey) {
        // A trackpad pinch arrives as Ctrl+wheel; deltaY < 0 spreads (zoom in).
        e.preventDefault();
        const dy = Math.max(-50, Math.min(50, e.deltaY));
        set(zoomAround(v, Math.exp(dy * 0.01), fromView(v, across(e.clientX)), min));
        return;
      }
      if (isFit(v)) return;
      const horizontal = e.shiftKey && e.deltaX === 0 ? e.deltaY : e.deltaX;
      if (horizontal === 0 || (!e.shiftKey && Math.abs(e.deltaY) > Math.abs(e.deltaX))) return;
      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      set(panBy(v, (horizontal / Math.max(1, rect.width)) * spanOf(v)));
    };
    // WebKit's own pinch events, for a webview that sends those instead.
    let base: { view: View; anchor: number } | null = null;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      const g = e as unknown as { clientX: number };
      const v = live.current.view;
      base = { view: v, anchor: fromView(v, across(g.clientX)) };
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as unknown as { scale: number };
      const { onView: set, minSpan: min } = live.current;
      if (!base || !set || !(g.scale > 0)) return;
      set(zoomAround(base.view, 1 / g.scale, base.anchor, min));
    };
    const onGestureEnd = (e: Event) => {
      e.preventDefault();
      base = null;
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    el.addEventListener('gesturestart', onGestureStart);
    el.addEventListener('gesturechange', onGestureChange);
    el.addEventListener('gestureend', onGestureEnd);
    return () => {
      el.removeEventListener('wheel', onWheel);
      el.removeEventListener('gesturestart', onGestureStart);
      el.removeEventListener('gesturechange', onGestureChange);
      el.removeEventListener('gestureend', onGestureEnd);
    };
  }, []);

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

    // Every file fraction below goes to a pixel through the view, and every
    // span (a grain's length, a jitter band) is scaled by it.
    const X = (t: number) => ((t - view.start) / vs) * w;
    const D = (dt: number) => (dt / vs) * w;

    // The finest peaks in hand for this view: the fetched ones while they
    // belong to this material, else the coarse set over the whole file. Each
    // carries its own range, so a stale fetch stays put under a moving view.
    const fine = zoomed && zoomPeaks && zoomPeaks.key === fetchKey ? zoomPeaks : null;
    const src = fine ?? { start: 0, end: 1, peaks };
    if (src.peaks.length > 0) {
      ctx.fillStyle = wave;
      const mid = h / 2;
      const n = src.peaks.length;
      const per = src.end - src.start;
      src.peaks.forEach((p, i) => {
        const x0 = X(src.start + (i / n) * per);
        const x1 = X(src.start + ((i + 1) / n) * per);
        if (x1 < 0 || x0 > w) return;
        const half = Math.max(0.5, p * mid * 0.95);
        ctx.fillRect(x0, mid - half, Math.max(1, (x1 - x0) * 0.9), half * 2);
      });
    }

    if (lo > 0.001 || hi < 0.999) {
      ctx.fillStyle = '#000';
      ctx.globalAlpha = 0.55;
      const xl = Math.min(w, Math.max(0, X(lo)));
      const xh = Math.min(w, Math.max(0, X(hi)));
      ctx.fillRect(0, 0, xl, h);
      ctx.fillRect(xh, 0, w - xh, h);
      ctx.globalAlpha = 1;
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1;
      for (const x of [X(lo), X(hi)]) {
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
    }

    // Regions: a shaded span each, a numbered tab in the strip along the top.
    const drawRegion = (r: Region, label: string, on: boolean, dashed: boolean) => {
      const x0 = X(r.start);
      const x1 = X(r.end);
      ctx.fillStyle = accent;
      ctx.globalAlpha = on ? 0.3 : 0.14;
      ctx.fillRect(x0, 0, x1 - x0, h);
      ctx.globalAlpha = on ? 0.9 : 0.45;
      ctx.fillRect(x0, 0, x1 - x0, LANE);
      ctx.globalAlpha = on ? 1 : 0.7;
      ctx.strokeStyle = accent;
      ctx.lineWidth = on ? 2 : 1;
      ctx.setLineDash(dashed ? [4, 3] : []);
      for (const x of [x0, x1]) {
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
      // The tab's number stays on screen while any of the region is.
      const shown = Math.max(0, x0);
      if (label && Math.min(w, x1) - shown > 12) {
        ctx.fillStyle = '#000';
        ctx.font = '10px ui-monospace, monospace';
        ctx.textBaseline = 'middle';
        ctx.fillText(label, shown + 4, LANE / 2 + 0.5);
      }
    };
    regions.forEach((r, i) => {
      drawRegion(r, String(i + 1), i === selectedRegion, false);
    });
    if (draft) drawRegion(draft, '', true, true);

    // Grain position and jitter are fractions of the trimmed window, not of
    // the file, because the engine hands the granular voice a slice. Map them
    // back into file coordinates or the line walks outside the trim, which is
    // exactly what it looked like it was doing.
    const span = hi - lo;
    const grainX = lo + position * span;
    const jitterSpan = jitter * span;

    // Jitter band first, so the position line sits on top of it.
    if (showGrain && jitterSpan > 0.001) {
      ctx.fillStyle = accent;
      ctx.globalAlpha = 0.15;
      const a = X(Math.max(lo, grainX - jitterSpan));
      const b = X(Math.min(hi, grainX + jitterSpan));
      ctx.fillRect(a, 0, b - a, h);
      ctx.globalAlpha = 1;
    }

    // Every logged spawn as a bar. Drawn under the playhead and the position
    // line so those stay readable through a dense cloud.
    if (grains.length > 0 && totalSamples > 0) {
      for (const g of grains) {
        const x = X(g.position);
        // Reverse grains read backwards from their spawn point, so the span
        // they actually touch lies to the *left* of it. Drawing them
        // rightwards would put the bar over audio the grain never reached.
        const width = Math.max(1.5, D(Math.abs(g.len * g.rate) / totalSamples));
        const left = g.rate < 0 ? x - width : x;
        // Pan across the vertical, so a wide stereo spread reads as a
        // scattered field rather than as a thicker line.
        const y = h * 0.12 + g.pan * h * 0.76;

        const age = (newestSeq - g.seq) / LIVE_TRAIL;
        // Past the trail a bar is gone, not dimmed. A floor here instead of a
        // zero is what turns a moving overlay into an accumulating smear: the
        // oldest bars never leave, they just wait to fall off the scrollback.
        if (!inspecting && age > 1) continue;
        const isPicked = selected !== null && selected.seq === g.seq;
        ctx.globalAlpha = isPicked ? 1 : inspecting ? 0.4 : (1 - age) * 0.62;
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
      ctx.moveTo(X(playhead), 0);
      ctx.lineTo(X(playhead), h);
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
        const x = X(lo + (i / (envelope.length - 1)) * span);
        const y = h - v * h * 0.94 - h * 0.03;
        if (i === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      });
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    // The scrub position: where the hand asked the head to be. The playhead
    // above is where the head is, a moment behind.
    if (scrubAt !== null) {
      ctx.strokeStyle = accent;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(X(scrubAt), 0);
      ctx.lineTo(X(scrubAt), h);
      ctx.stroke();
    }

    // The grain read position, on top.
    if (showGrain) {
      ctx.strokeStyle = accent;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(X(grainX), 0);
      ctx.lineTo(X(grainX), h);
      ctx.stroke();
    }
  }, [
    peaks,
    position,
    jitter,
    showGrain,
    playhead,
    trimStart,
    trimEnd,
    envelope,
    grains,
    newestSeq,
    selected,
    totalSamples,
    inspecting,
    scrubAt,
    regions,
    selectedRegion,
    draft,
    view,
    vs,
    zoomed,
    zoomPeaks,
    fetchKey,
  ]);

  // Dragging near an edge moves it. Whichever edge is closer wins, so there
  // is no mode to be in and nothing to click first.
  const drag = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const t = where(e).t;
    if (scrub !== 'off' && onScrub) {
      // Across the trimmed window, which is what both rows address.
      const lo = Math.min(trimStart, trimEnd);
      const hi = Math.max(trimStart, trimEnd);
      onScrub(hi > lo ? Math.min(1, Math.max(0, (t - lo) / (hi - lo))) : 0);
      return;
    }
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
    const t = fromView(view, (e.clientX - rect.left) / rect.width);
    const v = (e.clientY - rect.top) / rect.height;

    let best: GrainInfo | null = null;
    let bestD = Infinity;
    for (const g of grains) {
      const span = totalSamples > 0 ? Math.abs(g.len * g.rate) / totalSamples : 0;
      const left = g.rate < 0 ? g.position - span : g.position;
      // Distance to the bar, which is zero anywhere along its length.
      const dx = Math.max(0, Math.max(left - t, t - (left + span))) / vs;
      const dy = v - (0.12 + g.pan * 0.76);
      const d = Math.hypot(dx, dy * 0.5);
      if (d < bestD) {
        bestD = d;
        best = g;
      }
    }
    onPickGrain(bestD < 0.08 ? best : null);
  };

  /** Where a pointer is on the canvas, as a file fraction and pixels down. */
  const where = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    return {
      t: fromView(view, Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))),
      y: e.clientY - rect.top,
      width: rect.width,
    };
  };

  /**
   * A press that belongs to regions: Shift anywhere starts one, a press in
   * the strip grabs an edge (the nearest within a few pixels), else the body
   * of the region it is over, else selects nothing. Returns whether it took
   * the press.
   */
  const regionDown = (e: React.PointerEvent<HTMLCanvasElement>): boolean => {
    const { t, y, width } = where(e);
    if (e.shiftKey && onAddRegion) {
      regionDrag.current = { kind: 'add', anchor: t };
      setDraft({ start: t, end: t });
      return true;
    }
    if (y >= LANE || regions.length === 0) return false;
    const reach = (GRAB / width) * vs;
    let best: { index: number; edge: 'start' | 'end'; d: number } | null = null;
    regions.forEach((r, index) => {
      for (const edge of ['start', 'end'] as const) {
        const d = Math.abs(t - r[edge]);
        if (d <= reach && (best === null || d < best.d)) best = { index, edge, d };
      }
    });
    if (best) {
      const hit = best as { index: number; edge: 'start' | 'end' };
      regionDrag.current = { kind: 'edge', index: hit.index, edge: hit.edge };
      onSelectRegion?.(hit.index);
      return true;
    }
    const inside = regions.findIndex((r) => t >= r.start && t <= r.end);
    onSelectRegion?.(inside >= 0 ? inside : null);
    if (inside >= 0) {
      const r = regions[inside];
      regionDrag.current = { kind: 'move', index: inside, grab: t, start: r.start, end: r.end };
    }
    return true;
  };

  /** The narrowest region a drag may make; zoomed in, a finer one. */
  const minRegion = Math.min(MIN_REGION, vs * 0.01);

  const regionMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const g = regionDrag.current;
    if (!g) return;
    const { t } = where(e);
    if (g.kind === 'add') {
      setDraft({ start: Math.min(g.anchor, t), end: Math.max(g.anchor, t) });
    } else if (g.kind === 'edge') {
      const r = regions[g.index];
      if (!r) return;
      if (g.edge === 'start') {
        onSetRegion?.(g.index, Math.min(t, r.end - minRegion), r.end);
      } else {
        onSetRegion?.(g.index, r.start, Math.max(t, r.start + minRegion));
      }
    } else {
      const width = g.end - g.start;
      const start = Math.min(1 - width, Math.max(0, g.start + (t - g.grab)));
      onSetRegion?.(g.index, start, start + width);
    }
  };

  const regionUp = () => {
    const g = regionDrag.current;
    regionDrag.current = null;
    if (g?.kind === 'add' && draft) {
      if (draft.end - draft.start >= minRegion) onAddRegion?.(draft.start, draft.end);
    }
    setDraft(null);
  };

  return (
    <div ref={wrap}>
      <canvas
        ref={ref}
        tabIndex={0}
        onKeyDown={(e) => {
          // Only a selected region takes Backspace, and only here; anything
          // else leaves the key to whoever else wants it.
          if ((e.key === 'Backspace' || e.key === 'Delete') && selectedRegion !== null) {
            e.preventDefault();
            e.stopPropagation();
            onRemoveRegion?.();
          }
        }}
        onPointerUp={regionUp}
        onPointerCancel={regionUp}
        onPointerDown={(e) => {
          e.currentTarget.focus({ preventScroll: true });
          if (regionDown(e)) {
            e.currentTarget.setPointerCapture(e.pointerId);
            return;
          }
          if (inspecting && scrub === 'off') {
            pick(e);
            return;
          }
          e.currentTarget.setPointerCapture(e.pointerId);
          drag(e);
        }}
        onPointerMove={(e) => {
          if (regionDrag.current) {
            regionMove(e);
            return;
          }
          if ((!inspecting || scrub !== 'off') && e.buttons === 1) drag(e);
        }}
        className={`h-40 w-full rounded border border-border bg-card outline-none focus-visible:ring-1 focus-visible:ring-ring [--wave-accent:#e0a96d] [--wave-color:#6b7b93] ${
          inspecting && scrub === 'off' ? 'cursor-crosshair' : 'cursor-ew-resize'
        }`}
      />
      {zoomed && onView && <WaveformOverview peaks={peaks} view={view} onView={onView} />}
    </div>
  );
}
