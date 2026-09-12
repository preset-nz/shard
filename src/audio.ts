/**
 * The bridge to the audio engine.
 *
 * Nothing here carries audio. Parameter writes go one way, meters and
 * precomputed waveform peaks come back. If a call takes a few milliseconds
 * the engine does not care: it reads a slightly older value from an atomic
 * bank and keeps running.
 *
 * The parameter table lives in Rust. This module asks for it rather than
 * duplicating it, so adding a parameter there adds a control here.
 */
import { invoke } from '@tauri-apps/api/core';

export type Taper = 'linear' | 'exponential' | 'bipolar' | 'stepped';

export interface ParamInfo {
  id: string;
  name: string;
  min: number;
  max: number;
  default: number;
  taper: Taper;
  steps: number | null;
  /** Stepped parameters cannot drift; walking enum values at random is a
   *  different feature. */
  can_drift: boolean;
  unit: string;
  smooth_ms: number;
}

export interface Meters {
  peak: number;
  grains: number;
  /** Master drift switch. */
  drift: boolean;
  /** Per-parameter drift flags, indexed as the definitions are. */
  drifting: boolean[];
  playing: boolean;
  /** Where plain playback has reached, 0 to 1. */
  playhead: number;
}

export interface SourceInfo {
  name: string;
  seconds: number;
  peaks: number[];
}

export const paramDefs = () => invoke<ParamInfo[]>('param_defs');
export const getParams = () => invoke<number[]>('get_params');
export const setParam = (id: string, value: number) => invoke<void>('set_param', { id, value });
export const meters = () => invoke<Meters>('meters');
export const setDrift = (on: boolean) => invoke<void>('set_drift', { on });
export const sourceInfo = () => invoke<SourceInfo>('source_info');
export const loadSample = (path: string) => invoke<SourceInfo>('load_sample', { path });

/**
 * Control position (0..1) to real value, mirroring the Rust table's taper.
 *
 * This is duplicated logic, and deliberately so: doing it here means a knob
 * drag is local arithmetic rather than a round trip per pixel. The Rust side
 * clamps everything it receives, so the worst a drift between the two can do
 * is make a control feel wrong, never put a bad value into the engine.
 */
export function denormalise(p: ParamInfo, t: number): number {
  const c = Math.min(1, Math.max(0, t));
  switch (p.taper) {
    case 'exponential':
      return p.min > 0 && p.max > 0 ? p.min * (p.max / p.min) ** c : p.min + (p.max - p.min) * c;
    case 'stepped': {
      const n = Math.max(1, p.steps ?? 1);
      const step = Math.min(n - 1, Math.floor(c * n));
      return p.min + (p.max - p.min) * (step / Math.max(1, n - 1));
    }
    default:
      return p.min + (p.max - p.min) * c;
  }
}

/** Real value back to control position. The inverse of `denormalise`. */
export function normalise(p: ParamInfo, v: number): number {
  const lo = Math.min(p.min, p.max);
  const hi = Math.max(p.min, p.max);
  const c = Math.min(hi, Math.max(lo, v));
  let t: number;
  if (p.taper === 'exponential' && p.min > 0 && p.max > 0) {
    t = Math.log(c / p.min) / Math.log(p.max / p.min);
  } else if (Math.abs(p.max - p.min) < 1e-9) {
    t = 0;
  } else {
    t = (c - p.min) / (p.max - p.min);
  }
  return Math.min(1, Math.max(0, t));
}

/** How a value is shown. Time and frequency get coarser as they get bigger. */
export function format(p: ParamInfo, v: number): string {
  if (p.taper === 'stepped') return String(Math.round(v));
  if (p.unit === '%') return `${Math.round(v * 100)}%`;
  if (p.unit === 'Hz') return v >= 100 ? `${Math.round(v)} Hz` : `${v.toFixed(1)} Hz`;
  if (p.unit === 'ms') return v >= 100 ? `${Math.round(v)} ms` : `${v.toFixed(1)} ms`;
  if (p.unit === 'st') return `${v >= 0 ? '+' : ''}${v.toFixed(1)} st`;
  return v.toFixed(2);
}

/** Start or stop. Stopping clears the grain pool, so stop means stop. */
export const setPlaying = (playing: boolean) => invoke<void>('set_playing', { playing });

/** Hand one parameter to the drift oscillator, or take it back. */
export const setParamDrift = (id: string, on: boolean) =>
  invoke<void>('set_param_drift', { id, on });

/** Labels for the stepped window parameter, matching `Window::ALL` in Rust. */
export const WINDOW_NAMES = ['Hann', 'Triangle', 'Expodec', 'Rexpodec'];
