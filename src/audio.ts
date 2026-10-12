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
import type { Curve, CurvePoint } from '@preset.nz/math';
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
  /** Each track's step, in the tracks' order, from zero, or -1 while the transport is off. */
  steps: number[];
  /** The drum kit's step, the same way. Its loop has its own length. */
  kit_step: number;
  /** Where the current patch's plain playback has reached, 0 to 1. */
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
  /** Each track's mute and solo, in the tracks' order, as pressed and as heard. */
  mix: MixState[];
}

/**
 * A track's mute and solo. `mute` and `solo` are what was pressed;
 * `heard_mute` and `heard_solo` what the engine applies. They differ while a
 * press waits for the next bar. Mute wins: a muted track is silent even when
 * soloed, and any solo silences every track not soloed.
 */
export interface MixState {
  mute: boolean;
  solo: boolean;
  heard_mute: boolean;
  heard_solo: boolean;
}

/** Whether track `i` is heard under the applied mutes and solos. */
export function audible(mix: MixState[], i: number): boolean {
  const m = mix[i];
  if (!m) return true;
  const anySolo = mix.some((x) => x.heard_solo);
  return !m.heard_mute && (m.heard_solo || !anySolo);
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

/**
 * Every id of the level above the patch starts with this: the arrangement's
 * chain, the patch's fader and the limiter. They live in their own table in
 * Rust, so a patch load leaves them alone (`design/arrangement-layer.md`).
 */
export const ARRANGEMENT_PREFIX = 'arrangement.';
export const isArrangement = (id: string) => id.startsWith(ARRANGEMENT_PREFIX);
export const arrangementDefs = () => invoke<ParamInfo[]>('arrangement_defs');
export const getArrangement = () => invoke<number[]>('get_arrangement');

/** Set a value in whichever table owns it, by its id. */
export const setParam = (id: string, value: number) =>
  isArrangement(id)
    ? invoke<void>('set_arrangement_param', { id, value })
    : invoke<void>('set_param', { id, value });

/** Which level's chain a palette command acts on. */
export type ChainLevel = 'patch' | 'arrangement';

/**
 * The palette (`design/effect-palette.md`): add an effect to the end of a
 * level's chain, take one out, or move one a place. The order is held in the
 * Rust bank, so the answer arrives with the next poll. `n` is the instance
 * number in `fx.<n>.<kind>.<param>`.
 */
export const fxAdd = (layer: ChainLevel, kind: string) => invoke<number>('fx_add', { layer, kind });
export const fxRemove = (layer: ChainLevel, n: number) => invoke<void>('fx_remove', { layer, n });
/** Answers the instance the effect plays afterwards, or null at an end. */
export const fxMove = (layer: ChainLevel, n: number, by: number) =>
  invoke<number | null>('fx_move', { layer, n, by });

/**
 * Undo and redo (`design/undo.md`): step back or forward one edit. Each answers
 * with the name of what it undid or redid, or null when there was nothing.
 * The values arrive with the next poll; the tracker and the modulation are
 * fetched again by the caller.
 */
/** Every patch parameter back to its default: the sound only. One step of Undo. */
export const resetSound = () => invoke<boolean>('reset_sound');

/** A patch as the scene view lists it (epic 10). */
export interface PatchInfo {
  /** Runtime id, for `selectPatch`. Never saved. */
  id: number;
  name: string;
  /** The patch the engine plays and every patch command acts on. */
  current: boolean;
  /** Whether any track plays it. */
  played: boolean;
  /** Looping on its own over the mix: selected, and played by no track. */
  auditioned: boolean;
  /** Whether Delete may take it: not the last patch, and played by no track. */
  deletable: boolean;
}

/** The session's patches, in order. */
export const listPatches = () => invoke<PatchInfo[]>('patches');
/**
 * New Patch: a patch at its defaults, made current. One step of Undo. Rust
 * emits `document-opened` afterwards, so the panels read everything again.
 */
export const newPatch = () => invoke<number>('new_patch');
/**
 * Select a patch: it becomes current, and loops on its own while no track
 * plays it. Not an edit, and no track's patch changes. Followed by
 * `document-opened` when the current patch changed.
 */
export const selectPatch = (id: number) => invoke<void>('select_patch', { id });
/**
 * Select a track: its patch becomes current and nothing is auditioned. Not an
 * edit. Followed by `document-opened` when the current patch changed.
 */
export const selectTrack = (track: number) => invoke<void>('select_track', { track });
/** Add Track: a melody track at the end, playing the current patch. One step of Undo. */
export const addTrack = () => invoke<number>('add_track');
/** Remove Track: one step of Undo, "Remove Track 2". Refused for the last track. */
export const removeTrack = (track: number) => invoke<void>('remove_track', { track });
/**
 * Mutes a track, or unmutes it: on the next bar while playing and at once
 * while stopped. Flipped in Rust, so a second press before the bar always
 * cancels the first. Not an edit: no undo step, and the session stays saved.
 * Saved with the session all the same. Answers every track's mix.
 */
export const toggleTrackMute = (track: number) =>
  invoke<MixState[]>('toggle_track_mute', { track });
/** Solos a track, or unsolos it, the same way. */
export const toggleTrackSolo = (track: number) =>
  invoke<MixState[]>('toggle_track_solo', { track });

/**
 * The patch a track plays (its *Plays* picker). One step of Undo, "Set
 * Plays". Reread the tracker and `listPatches` afterwards; with that track
 * selected Rust also emits `document-opened`, because the current patch
 * follows it.
 */
export const setTrackPatch = (track: number, id: number) =>
  invoke<void>('set_track_patch', { track, id });

/**
 * Rename a patch. One step of Undo, "Rename Patch 2"; an empty or unchanged
 * name makes none. Answers whether it changed. Reread `listPatches` after.
 */
export const renamePatch = (id: number, name: string) =>
  invoke<boolean>('rename_patch', { id, name });
/**
 * Delete a patch. One step of Undo, "Delete Patch 2". Refused for the last
 * patch and one a track plays. When it was current, Rust emits
 * `document-opened`; reread `listPatches` either way.
 */
export const deletePatch = (id: number) => invoke<void>('delete_patch', { id });
/**
 * Duplicate a patch, straight after it, and make the copy current. One step
 * of Undo, "Duplicate Patch 2". Followed by `document-opened`.
 */
export const duplicatePatch = (id: number) => invoke<number>('duplicate_patch', { id });

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
  /** The note it sounds at, as a MIDI number (60 is C4), or null if unknown. */
  root: number | null;
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
export const setMaterial = (
  id: number,
  octave: number,
  trimStart: number,
  trimEnd: number,
  root: number | null,
) => invoke<MaterialsView>('set_material', { id, octave, trimStart, trimEnd, root });

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
  if (p.unit === 's') return v >= 10 ? `${v.toFixed(0)} s` : `${v.toFixed(1)} s`;
  if (p.unit === 'st') return `${v >= 0 ? '+' : ''}${v.toFixed(1)} st`;
  if (p.unit === 'dB') return `${v >= 0 ? '+' : ''}${v.toFixed(1)} dB`;
  return v.toFixed(2);
}

export interface LoadReport {
  applied: number;
  /** Ids in the file this build does not have. Usually a newer patch. */
  unknown: string[];
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
  /** A parameter id for a link, `lfo:<id>` for an LFO, `envelope:<id>` for an envelope. */
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

/**
 * One modulation envelope, a curve on the pass's clock. Its id shares the
 * LFOs' space. Called "Mod envelope" so it is not mistaken for the Envelope
 * node, which shapes the amplitude.
 */
export interface EnvelopeRecord {
  id: number;
  name: string;
  /** 'adsr': four points, sustain on the third, sliders; 'free': any curve. */
  mode: 'adsr' | 'free';
  /** x runs 0 to 1 across the note, or the pass. */
  curve: Curve;
}

/**
 * A curve as Rust hands it back: optional fields arrive as `null`, which the
 * curve editor's equality check and `adsrOf` do not expect.
 */
export function toCurve(v: unknown): Curve {
  const raw = (v ?? {}) as {
    points?: (CurvePoint & { tension?: number | null })[];
    sustain?: number | null;
  };
  const points = (raw.points ?? []).map(({ tension, ...p }) =>
    tension == null ? p : { ...p, tension },
  );
  return raw.sustain == null ? { points } : { points, sustain: raw.sustain };
}

export interface LinkRecord {
  /** The id of the LFO or envelope the parameter follows. */
  source: number;
  /**
   * The ends of the sweep, 0 to 1 of the parameter's range. An LFO's trough,
   * or an envelope at rest, lands on `lo`; the crest or the peak on `hi`.
   */
  lo: number;
  hi: number;
}

/** The open patch's LFOs and links, and what in them the engine refused. */
export interface ModulationView {
  lfos: LfoRecord[];
  envelopes: EnvelopeRecord[];
  /** By parameter id. */
  links: Record<string, LinkRecord>;
  refused: Refused[];
}

export interface LfoLimits {
  shapes: string[];
  min_rate: number;
  max_rate: number;
}

/** Anything a parameter can follow, LFOs first, as the link menu lists them. */
export interface Modulator {
  id: number;
  name: string;
  kind: 'lfo' | 'envelope';
}

export const modulatorsOf = (mod: ModulationView): Modulator[] => [
  ...mod.lfos.map((l) => ({ id: l.id, name: l.name, kind: 'lfo' as const })),
  ...mod.envelopes.map((e) => ({ id: e.id, name: e.name, kind: 'envelope' as const })),
];

// Every modulation edit answers with the whole document, so the UI draws
// exactly what the engine now has.
export const modulation = () => invoke<ModulationView>('modulation');
export const lfoLimits = () => invoke<LfoLimits>('lfo_limits');
export const addLfo = () => invoke<ModulationView>('add_lfo');
export const removeLfo = (id: number) => invoke<ModulationView>('remove_lfo', { id });
export const setLfo = (lfo: LfoRecord) => invoke<ModulationView>('set_lfo', { lfo });
export const addEnvelope = () => invoke<ModulationView>('add_envelope');
export const removeEnvelope = (id: number) => invoke<ModulationView>('remove_envelope', { id });
/**
 * `coalesce` is for streamed input, a slider or a keyboard nudge: consecutive
 * calls share one undo step. A discrete edit leaves it false.
 */
export const setEnvelope = (envelope: EnvelopeRecord, coalesce = false) =>
  invoke<ModulationView>('set_envelope', { envelope, coalesce });
/** A curve drag is one undo step: begin, apply per move, then end or cancel. */
export const envelopeGestureBegin = (id: number) => invoke<void>('envelope_gesture_begin', { id });
export const envelopeGestureApply = (envelope: EnvelopeRecord) =>
  invoke<ModulationView>('envelope_gesture_apply', { envelope });
export const envelopeGestureEnd = () => invoke<void>('envelope_gesture_end');
export const envelopeGestureCancel = () => invoke<ModulationView>('envelope_gesture_cancel');
export const linkParam = (id: string, source: number, lo: number, hi: number) =>
  invoke<ModulationView>('link_param', { id, source, lo, hi });
export const unlinkParam = (id: string) => invoke<ModulationView>('unlink_param', { id });

/** Every parameter as the engine last heard it, indexed as `getParams`. */
export const getHeard = () => invoke<number[]>('get_heard');

/** The envelope sampled across the trimmed window, for drawing. */
export const envelopeCurve = () => invoke<number[]>('envelope_curve');

/** One pass of the patch as sound scaping plays it, folded to peaks for drawing. */
export interface PatchPreview {
  /** Per column, lowest and highest across both channels. */
  min: number[];
  max: number[];
  /** The zoom window, folded the same way. */
  zoom_min: number[];
  zoom_max: number[];
  /** Where the zoom starts, 0 to 1 of the pass, and how long it is. */
  zoom_at: number;
  zoom_ms: number;
  seconds: number;
  /** The pass ran past the longest the preview draws, and was cut. */
  capped: boolean;
}
export const patchPreview = () => invoke<PatchPreview>('patch_preview');

/** Start or stop. Stopping clears the grain pool, so stop means stop. */
export const setPlaying = (playing: boolean) => invoke<void>('set_playing', { playing });

/** One sixteenth of a track. */
export interface Step {
  /** Whether it triggers the patch. */
  on: boolean;
  /** Semitones from the sample, −24 to 24. Kept for a step that is off. */
  pitch: number;
  /**
   * How long its note is held, in sixteenths, 0 to 16. Zero is the patch's own
   * Hold time. Only heard while the patch's Length is Hold.
   */
  hold: number;
  /** How hard it plays, 0 to 127; 127 is full. */
  velocity: number;
  /** Its timing in ms, −50 to 50: negative is early, positive late. */
  nudge: number;
}

/**
 * A track of steps, playing one patch on its own voice. Monophonic: a new
 * step cuts the last. Track *n* is "Track n" by its place.
 */
export interface Track {
  /** 4, 8, 16, 32 or 64 sixteenths: up to four bars. */
  length: number;
  /** Its fader in the mix, 0 to 2. */
  gain: number;
  /** The patch it plays, by id. Read only: a pick goes through `setTrackPatch`. */
  patch: number | null;
  /** Sixty-four steps, step one first. Those past `length` are kept, not played. */
  steps: Step[];
}

/**
 * The level above the patch: the arrangement's tempo and swing, and its
 * tracks, up to eight, each playing its own patch.
 */
export interface Tracker {
  tempo: number;
  /** Of a sixteenth, 0 to 0.75, on the even steps. */
  swing: number;
  tracks: Track[];
  /** The drum kit beside the tracks, on their clock. */
  kit: Kit;
}

/** The kit's voices, in the order its rows are drawn. */
export const KIT_VOICES = ['kick', 'snare', 'hat'] as const;
export type KitVoice = (typeof KIT_VOICES)[number];

/**
 * The drum kit: kick, snare and hat, synthesised. Each row is a velocity a
 * step, 0 to 127, zero for no hit, so several drums land on one step. A
 * stop-gap until a kit sliced from a material (roadmap row 25).
 */
export interface Kit extends Record<KitVoice, number[]> {
  /** 4, 8, 16, 32 or 64 sixteenths, as a track's. */
  length: number;
}

export const getTracker = () => invoke<Tracker>('tracker');
/**
 * Takes the tempo, swing, kit and each track's steps, length and fader, by
 * place; never adds, removes or re-patches a track. Answers with it clamped
 * and snapped, as the engine plays it.
 */
export const setTracker = (tracker: Tracker) => invoke<Tracker>('set_tracker', { tracker });

/**
 * A whole-pattern edit on one track (`tracker::Edit` in Rust): clear,
 * paste or repeat a bar, rotate the steps, transpose. One step of Undo.
 * Answers with the tracker as it now is.
 */
export type TrackerEdit =
  | { op: 'clear_bar'; bar: number }
  | { op: 'paste_bar'; bar: number; steps: Step[] }
  | { op: 'repeat_bar'; from: number }
  | { op: 'duplicate' }
  | { op: 'rotate'; by: number }
  | { op: 'transpose'; semis: number; bar: number | null };
export const editTracker = (track: number, edit: TrackerEdit) =>
  invoke<Tracker>('edit_tracker', { track, edit });

/** The most tracks a session plays at once: the engine's voices. */
export const MAX_TRACKS = 8;

/** Labels for the stepped window parameter, matching `Window::ALL` in Rust. */
export const WINDOW_NAMES = ['Hann', 'Triangle', 'Expodec', 'Rexpodec'];
/** `FilterType::NAMES`, in stepped order. */
export const FILTER_TYPES = ['Low-pass', 'High-pass', 'Band-pass'];
/** `DriveType::NAMES`, in stepped order. */
export const DRIVE_TYPES = ['Overdrive', 'Distortion', 'Fuzz', 'Fold'];
/** `ChorusType::NAMES`, in stepped order. */
export const CHORUS_TYPES = ['Chorus', 'Ensemble', 'Choir'];
/** `ReverbType::NAMES`, in stepped order. */
export const REVERB_TYPES = ['Room', 'Hall', 'Plate'];
/** `FmType::NAMES`, in stepped order. */
export const FM_TYPES = ['Phase', 'Linear', 'Exponential'];
/** `note::Length::NAMES`, in stepped order: what sets a note's length. */
export const LENGTH_NAMES = ['Loop', 'Sample', 'Hold'];

/** A MIDI device as Settings → Controllers lists it. */
export interface DeviceView {
  /** The port name CoreMIDI gives it. Identity; the name is yours to edit. */
  port: string;
  name: string;
  connected: boolean;
  /** The profile's name when shard drives this device, else null. */
  surface: string | null;
}

/** One knob or pad, learned the first time it was touched. */
export interface ControlView {
  id: number;
  device: string;
  channel: number;
  kind: 'cc' | 'note';
  number: number;
  role: 'knob' | 'pad';
  /** 'absolute' for a pot, or the encoding for an endless encoder. */
  mode: string;
  /** What this channel is called: 'Ch 5', or the name you gave it. */
  bank: string;
  name: string;
  /** Moved within the last quarter second. */
  active: boolean;
  /** The last value it sent, 0 to 127. */
  last: number | null;
  /**
   * For an endless encoder, what that last value decoded to. The turn-left
   * test: one click anticlockwise reads −1 under the right encoding.
   */
  delta: number | null;
}

export interface ControllersView {
  devices: DeviceView[];
  controls: ControlView[];
  roles: string[];
  modes: string[];
  /** The control being calibrated, and what to tell the person to do. */
  calibrating: number | null;
  instruction: string | null;
  /** What calibration last worked out, in plain words. */
  found: { seq: number; text: string } | null;
  /** What went wrong with `controllers.json` at startup, if anything. */
  trouble: string | null;
}

export const controllers = () => invoke<ControllersView>('controllers');
export const renameControl = (id: number, name: string) =>
  invoke<ControllersView>('rename_control', { id, name });
export const setControlRole = (id: number, role: string) =>
  invoke<ControllersView>('set_control_role', { id, role });
export const setControlMode = (id: number, mode: string) =>
  invoke<ControllersView>('set_control_mode', { id, mode });
export const calibrateControl = (id: number) =>
  invoke<ControllersView>('calibrate_control', { id });
export const cancelCalibrate = () => invoke<ControllersView>('cancel_calibrate');
export const renameBank = (port: string, channel: number, name: string) =>
  invoke<ControllersView>('rename_bank', { port, channel, name });
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
  /** The channel's name. Device plus bank is the namespace the menu groups by. */
  bank: string;
  /** An endless encoder, which never arms and draws no ghost mark. */
  relative: boolean;
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
