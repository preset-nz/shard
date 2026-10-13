/**
 * The waveform's visible time window, and the arithmetic around it.
 *
 * A view is a pair of fractions of the whole material, `start < end`, both in
 * 0..1. It is webview state only: not in the document, no undo step, never
 * saved. Everything here is pure so it can be tested without a canvas.
 */

export interface View {
  start: number;
  end: number;
}

/** The whole material. */
export const FIT: View = { start: 0, end: 1 };

/** The closest the view may come to a sample, in seconds. */
export const MIN_VIEW_SECONDS = 0.005;

export const span = (v: View): number => v.end - v.start;

/** Whether the view is the whole material (to rounding). */
export const isFit = (v: View): boolean => span(v) >= 0.999999;

/** The narrowest span, as a fraction of a material `seconds` long. */
export function minSpanFor(seconds: number): number {
  if (!(seconds > 0)) return 1;
  return Math.min(1, MIN_VIEW_SECONDS / seconds);
}

/** `span` kept within [minSpan, 1], then slid to lie inside 0..1. */
export function clampView(v: View, minSpan: number): View {
  const want = Math.min(1, Math.max(Math.min(minSpan, 1), span(v)));
  const start = Math.min(1 - want, Math.max(0, v.start));
  return { start, end: start + want };
}

/** A file fraction to a fraction across the view (outside 0..1 when off-view). */
export const toView = (v: View, t: number): number => (t - v.start) / span(v);

/** A fraction across the view back to a file fraction. */
export const fromView = (v: View, x: number): number => v.start + x * span(v);

/**
 * Scales the span by `factor` (below 1 zooms in) keeping the file fraction
 * `anchor` at the same place across the view. An anchor outside the view is
 * pulled to its nearest edge.
 */
export function zoomAround(v: View, factor: number, anchor: number, minSpan: number): View {
  const a = Math.min(v.end, Math.max(v.start, anchor));
  const rel = toView(v, a);
  const next = Math.min(1, Math.max(Math.min(minSpan, 1), span(v) * factor));
  return clampView({ start: a - rel * next, end: a - rel * next + next }, minSpan);
}

/** Slides the view by `delta` of the whole material, staying inside it. */
export function panBy(v: View, delta: number): View {
  const s = span(v);
  const start = Math.min(1 - s, Math.max(0, v.start + delta));
  return { start, end: start + s };
}

/** Centres the view on `t`, keeping its span. */
export function centreOn(v: View, t: number): View {
  return panBy(v, t - (v.start + v.end) / 2);
}

/**
 * Zoom In and Out from the menu: halve or double the span around the
 * playhead when it is inside the view, else around the view's centre.
 */
export function stepZoom(
  v: View,
  direction: 'in' | 'out',
  playhead: number | null,
  minSpan: number,
): View {
  const inside = playhead !== null && playhead >= v.start && playhead <= v.end;
  const anchor = inside ? playhead : (v.start + v.end) / 2;
  return zoomAround(v, direction === 'in' ? 0.5 : 2, anchor, minSpan);
}

/** True when two views are the same to well under a pixel on any screen. */
export const sameView = (a: View, b: View): boolean =>
  Math.abs(a.start - b.start) < 1e-9 && Math.abs(a.end - b.end) < 1e-9;
