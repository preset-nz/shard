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
  unit: string;
  smooth_ms: number;
}

export interface Meters {
  peak: number;
  /** The limiter's lowest gain since the last poll; one means it did nothing. */
  reduction: number;
  grains: number;
  playing: boolean;
  /** The sequencer's step, from zero, or -1 while it or the transport is off. */
  step: number;
  /** Where plain playback has reached, 0 to 1. */
  playhead: number;
  /** True while one grain is being played on its own. */
  auditioning: boolean;
  /**
   * Whether the tape is running backwards — not whether the button is down.
   * A tap keeps this true for the flick time after you let go, which is what
   * the button lights from.
   */
  reversing: boolean;
  /** The slowest audio block since the last poll, in microseconds. */
  block_us: number;
  /** What the device allows for one block, in microseconds. Past it is a dropout. */
  block_budget_us: number;
  /**
   * Allocator calls inside the audio callback since launch. Counted in debug
   * builds only; a release build has no guard and always reports zero.
   */
  audio_allocs: number;
}

export interface SourceInfo {
  name: string;
  seconds: number;
  /** snake_case, like every other field: nothing here renames on the wire. */
  sample_rate: number;
  peaks: number[];
}

/**
 * One grain, as the engine spawned it.
 *
 * A grain is an event, not an object with a lifetime — the pool reuses its
 * slots and stopping destroys every live one. What the inspector shows is a
 * log of spawns. These six numbers are the whole of a grain, which is why any
 * entry can be handed straight back to `auditionGrain` and heard again.
 */
export interface GrainInfo {
  /** 0 to 1 across the whole source, trim already accounted for. */
  position: number;
  /** Samples per output sample. Negative means it ran backwards. */
  rate: number;
  /** Length in samples. */
  len: number;
  /** 0 hard left, 1 hard right. */
  pan: number;
  /** Index into WINDOW_NAMES. */
  window: number;
  /** Spawn number since the engine started. Strictly increasing. */
  seq: number;
}

export const paramDefs = () => invoke<ParamInfo[]>('param_defs');
export const getParams = () => invoke<number[]>('get_params');
export const setParam = (id: string, value: number) => invoke<void>('set_param', { id, value });
export const meters = () => invoke<Meters>('meters');

/** A material as the tree and the inspector show it. */
export interface MaterialView {
  id: number;
  name: string;
  /** Where the WAV is. Null for the built-in drone. */
  path: string | null;
  /** The file is not where the document said it was. */
  missing: boolean;
  /** Whole octaves, −2 to +2. */
  octave: number;
  /** The window's ends, 0 to 1 of the file. */
  trim_start: number;
  trim_end: number;
}

/** Which material each generator reads, by node id. Null is silent. */
export interface Wires {
  material: number | null;
  grain: number | null;
}

/** The material pool in the tree's order, and what each generator reads. */
export interface MaterialsView {
  materials: MaterialView[];
  wires: Wires;
}

export const materials = () => invoke<MaterialsView>('materials');
/** A material's waveform. Rejects for one whose file could not be read. */
export const materialWave = (id: number) => invoke<SourceInfo>('material_wave', { id });
/** Decode a WAV and add it to the pool, whole and at its own pitch. A silent generator reads it. */
export const addMaterial = (path: string) => invoke<MaterialsView>('add_material', { path });
/** Put the built-in drone back in the pool after it was removed. */
export const addDrone = () => invoke<MaterialsView>('add_drone');
export const removeMaterial = (id: number) => invoke<MaterialsView>('remove_material', { id });
/** Wire a material into `material` (Sample) or `grain` (Granular). Null is silence. */
export const wireMaterial = (node: string, material: number | null) =>
  invoke<MaterialsView>('wire_material', { node, material });
/** Set a material's octave and trim. Tauri takes command arguments in camelCase. */
export const setMaterial = (id: number, octave: number, trimStart: number, trimEnd: number) =>
  invoke<MaterialsView>('set_material', { id, octave, trimStart, trimEnd });

/** Everything spawned since the last call. Drains, so poll it steadily. */
export const grainLog = () => invoke<GrainInfo[]>('grain_log');

/** Play one logged grain on its own. Rejects while the transport is running. */
export const auditionGrain = (g: GrainInfo) =>
  invoke<void>('audition_grain', {
    position: g.position,
    rate: g.rate,
    len: g.len,
    pan: g.pan,
    window: g.window,
  });

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
  if (p.taper === 'stepped') {
    const step = Math.round(v);
    return p.unit === 'oct' ? `${step > 0 ? '+' : ''}${step} oct` : String(step);
  }
  if (p.unit === '%') return `${Math.round(v * 100)}%`;
  if (p.unit === 'Hz') return v >= 100 ? `${Math.round(v)} Hz` : `${v.toFixed(1)} Hz`;
  if (p.unit === 'ms') return v >= 100 ? `${Math.round(v)} ms` : `${v.toFixed(1)} ms`;
  if (p.unit === 'st') return `${v >= 0 ? '+' : ''}${v.toFixed(1)} st`;
  return v.toFixed(2);
}

export interface LoadReport {
  applied: number;
  /** Ids in the file this build does not have. Usually a newer patch. */
  unknown: string[];
  /** Ids this build has that the file did not carry; left at their defaults. */
  missing: string[];
  sample_path: string | null;
  /** The patch named a sample that is no longer where it said it was. */
  sample_missing: boolean;
  /** A controller map the patch wanted and this Mac does not have. */
  map_missing: string | null;
  /** LFOs and links the engine could not use. They stay in the patch. */
  refused: Refused[];
}

/** Something in the patch's modulation the engine could not use. */
export interface Refused {
  /** A parameter id for a link, or `lfo:<id>` for an LFO. */
  id: string;
  reason: string;
}

/** What applying a preset did. `unknown` lists values and links it could not apply. */
export interface PresetReport {
  applied: number;
  unknown: string[];
  /** Links the preset restored that the engine could not use. */
  refused: Refused[];
}

/** Node presets live in the patch; `node` is a section id such as `grain`. */
export const presetNames = (node: string) => invoke<string[]>('preset_names', { node });
export const savePreset = (node: string, name: string) =>
  invoke<void>('save_preset', { node, name });
export const updatePreset = (node: string, name: string) =>
  invoke<void>('update_preset', { node, name });
export const applyPreset = (node: string, name: string) =>
  invoke<PresetReport>('apply_preset', { node, name });

/** One LFO in the open patch. `id` is stable and never reused. */
export interface LfoRecord {
  id: number;
  name: string;
  /** Hz. */
  rate: number;
  /** One of `LfoLimits.shapes`, such as `smooth-random`. */
  shape: string;
  /** 0 to 1. */
  phase: number;
}

export interface LinkRecord {
  /** The id of the LFO the parameter follows. */
  lfo: number;
  /**
   * The ends of the sweep, 0 to 1 of the parameter's range. The LFO's trough
   * lands on `lo` and its crest on `hi`.
   */
  lo: number;
  hi: number;
}

/** The open patch's LFOs and links, and what in them the engine refused. */
export interface ModulationView {
  lfos: LfoRecord[];
  /** By parameter id. */
  links: Record<string, LinkRecord>;
  refused: Refused[];
}

export interface LfoLimits {
  shapes: string[];
  min_rate: number;
  max_rate: number;
}

// Every modulation edit answers with the whole document, so the UI draws
// exactly what the engine now has.
export const modulation = () => invoke<ModulationView>('modulation');
export const lfoLimits = () => invoke<LfoLimits>('lfo_limits');
export const addLfo = () => invoke<ModulationView>('add_lfo');
export const removeLfo = (id: number) => invoke<ModulationView>('remove_lfo', { id });
export const setLfo = (lfo: LfoRecord) => invoke<ModulationView>('set_lfo', { lfo });
export const linkParam = (id: string, lfo: number, lo: number, hi: number) =>
  invoke<ModulationView>('link_param', { id, lfo, lo, hi });
export const unlinkParam = (id: string) => invoke<ModulationView>('unlink_param', { id });

/** Every parameter as the engine last heard it, indexed as `getParams`. */
export const getHeard = () => invoke<number[]>('get_heard');

export const savePatch = (path: string) => invoke<void>('save_patch', { path });
export const loadPatch = (path: string) => invoke<LoadReport>('load_patch', { path });

/** The envelope sampled across the trimmed window, for drawing. */
export const envelopeCurve = () => invoke<number[]>('envelope_curve');

/** Start or stop. Stopping clears the grain pool, so stop means stop. */
export const setPlaying = (playing: boolean) => invoke<void>('set_playing', { playing });

/** A track of steps. Monophonic: a new step cuts the last. */
export interface Track {
  on: boolean;
  /** 4, 8 or 16 sixteenths. */
  length: number;
  /** One bit per step, step one first. */
  pattern: number;
  /** Sixteen pitches in semitones, −24 to 24, step one first. */
  pitches: number[];
}

/**
 * The level above the patch: the song's tempo and swing, and its tracks. One
 * track today, playing the document's one patch.
 */
export interface Tracker {
  tempo: number;
  /** Of a sixteenth, 0 to 0.75, on the even steps. */
  swing: number;
  tracks: Track[];
}

export const getTracker = () => invoke<Tracker>('tracker');
/** Replaces the tracker whole; answers with it clamped and snapped, as the engine plays it. */
export const setTracker = (tracker: Tracker) => invoke<Tracker>('set_tracker', { tracker });

/** Labels for the stepped window parameter, matching `Window::ALL` in Rust. */
export const WINDOW_NAMES = ['Hann', 'Triangle', 'Expodec', 'Rexpodec'];
/** `FilterType::NAMES`, in stepped order. */
export const FILTER_TYPES = ['Low-pass', 'High-pass', 'Band-pass'];
/** `DriveType::NAMES`, in stepped order. */
export const DRIVE_TYPES = ['Overdrive', 'Distortion', 'Fuzz', 'Fold'];

/** A MIDI device as Settings → Controllers lists it. */
export interface DeviceView {
  /** The port name CoreMIDI gives it. Identity; the name is yours to edit. */
  port: string;
  name: string;
  connected: boolean;
}

/** One knob or pad, learned the first time it was touched. */
export interface ControlView {
  id: number;
  device: string;
  channel: number;
  kind: 'cc' | 'note';
  number: number;
  role: 'knob' | 'pad';
  name: string;
  /** Moved within the last quarter second. */
  active: boolean;
  /** The last value it sent, 0 to 127. */
  last: number | null;
}

export interface ControllersView {
  devices: DeviceView[];
  controls: ControlView[];
  roles: string[];
}

export const controllers = () => invoke<ControllersView>('controllers');
export const renameControl = (id: number, name: string) =>
  invoke<ControllersView>('rename_control', { id, name });
export const setControlRole = (id: number, role: string) =>
  invoke<ControllersView>('set_control_role', { id, role });
export const forgetControl = (id: number) => invoke<ControllersView>('forget_control', { id });
export const renameDevice = (port: string, name: string) =>
  invoke<ControllersView>('rename_device', { port, name });
export const forgetDevice = (port: string) => invoke<ControllersView>('forget_device', { port });

/** One mapped parameter, as its row draws it. */
export interface MappingView {
  control: number;
  control_name: string;
  /** Waiting for the knob to pass through the value. */
  armed: boolean;
  /** Where the knob physically is, 0 to 1, once it has said. */
  knob: number | null;
}

export interface MapInfo {
  id: number;
  name: string;
}

/** A knob a row can pick from its menu. */
export interface KnobInfo {
  id: number;
  name: string;
  device: string;
}

export interface LearnReport {
  seq: number;
  ok: boolean;
  text: string;
}

/** The active controller map, polled beside the parameters. */
export interface MappingsView {
  active: MapInfo | null;
  maps: MapInfo[];
  /** Every knob known, so a row can choose one rather than learn it. */
  knobs: KnobInfo[];
  /** By parameter id. */
  mappings: Record<string, MappingView>;
  /** The parameter waiting for a control to move. */
  learning: string | null;
  report: LearnReport | null;
}

export const EMPTY_MAPPINGS: MappingsView = {
  active: null,
  maps: [],
  knobs: [],
  mappings: {},
  learning: null,
  report: null,
};

export const mappings = () => invoke<MappingsView>('mappings');
export const learnMidi = (id: string) => invoke<void>('learn_midi', { id });
export const mapMidi = (id: string, control: number) => invoke<void>('map_midi', { id, control });
export const cancelLearn = () => invoke<void>('cancel_learn');
export const forgetMidi = (id: string) => invoke<void>('forget_midi', { id });
export const setActiveMap = (id: number) => invoke<MappingsView>('set_active_map', { id });
export const addMap = (name: string) => invoke<MappingsView>('add_map', { name });
export const renameMap = (id: number, name: string) =>
  invoke<MappingsView>('rename_map', { id, name });
