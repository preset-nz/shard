import { describe, expect, it } from 'vitest';
import {
  centreOn,
  clampView,
  FIT,
  fromView,
  isFit,
  minSpanFor,
  panBy,
  span,
  stepZoom,
  toView,
  zoomAround,
} from './waveView';

const near = (a: number, b: number) => expect(a).toBeCloseTo(b, 9);

describe('time and x mapping', () => {
  it('round-trips through a view', () => {
    const v = { start: 0.25, end: 0.5 };
    near(fromView(v, toView(v, 0.4)), 0.4);
    near(toView(v, 0.25), 0);
    near(toView(v, 0.5), 1);
    near(fromView(v, 0.5), 0.375);
  });
});

describe('minimum span', () => {
  it('is 5 ms of the material, and the whole of a shorter one', () => {
    near(minSpanFor(10), 0.0005);
    expect(minSpanFor(0.003)).toBe(1);
    expect(minSpanFor(0)).toBe(1);
  });
});

describe('clamp', () => {
  it('keeps the view inside the material', () => {
    const v = clampView({ start: -0.2, end: 0.1 }, 0.001);
    near(v.start, 0);
    near(span(v), 0.3);
    const w = clampView({ start: 0.9, end: 1.4 }, 0.001);
    near(w.end, 1);
    near(span(w), 0.5);
  });
  it('stops at the minimum and at the whole', () => {
    near(span(clampView({ start: 0.5, end: 0.5001 }, 0.01)), 0.01);
    expect(clampView({ start: -1, end: 3 }, 0.01)).toEqual(FIT);
  });
});

describe('zoom around a point', () => {
  it('keeps the anchor where it was across the view', () => {
    const v = { start: 0.2, end: 0.6 };
    const z = zoomAround(v, 0.5, 0.3, 0.001);
    near(toView(z, 0.3), toView(v, 0.3));
    near(span(z), 0.2);
  });
  it('zooms out to the whole and no further', () => {
    const z = zoomAround({ start: 0.4, end: 0.6 }, 100, 0.5, 0.001);
    expect(isFit(z)).toBe(true);
    expect(z).toEqual(FIT);
  });
  it('stops at the minimum span', () => {
    let v = FIT;
    for (let i = 0; i < 40; i++) v = zoomAround(v, 0.5, 0.5, 0.0005);
    near(span(v), 0.0005);
  });
  it('against an edge keeps the view inside the material', () => {
    const z = zoomAround(FIT, 0.5, 0, 0.001);
    near(z.start, 0);
    near(z.end, 0.5);
  });
  it('pulls an anchor outside the view to the nearest edge', () => {
    const v = { start: 0.4, end: 0.6 };
    const z = zoomAround(v, 0.5, 0.9, 0.001);
    near(z.end, 0.6);
  });
});

describe('pan', () => {
  it('slides and stops at both ends', () => {
    const v = { start: 0.2, end: 0.4 };
    expect(panBy(v, 0.1)).toEqual({ start: 0.30000000000000004, end: 0.5 });
    near(panBy(v, -5).start, 0);
    near(panBy(v, 5).end, 1);
    near(span(panBy(v, 5)), 0.2);
  });
  it('centres on a point', () => {
    const c = centreOn({ start: 0, end: 0.2 }, 0.5);
    near((c.start + c.end) / 2, 0.5);
    near(span(c), 0.2);
    near(centreOn({ start: 0, end: 0.2 }, 0.01).start, 0);
  });
});

describe('menu zoom steps', () => {
  it('halves around the playhead when it is in view', () => {
    const z = stepZoom({ start: 0, end: 1 }, 'in', 0.8, 0.001);
    near(span(z), 0.5);
    near(toView(z, 0.8), 0.8);
  });
  it('halves around the centre when the playhead is off screen or absent', () => {
    const v = { start: 0.2, end: 0.4 };
    for (const ph of [null, 0.9]) {
      const z = stepZoom(v, 'in', ph, 0.001);
      near((z.start + z.end) / 2, 0.3);
      near(span(z), 0.1);
    }
  });
  it('doubles going out', () => {
    near(span(stepZoom({ start: 0.4, end: 0.5 }, 'out', null, 0.001)), 0.2);
  });
});
