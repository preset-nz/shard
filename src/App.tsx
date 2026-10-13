import {
  isTextField,
  redo,
  undo,
  useCommands,
  useCommandTable,
  useDocument,
  useDocumentNotes,
  useTextFocus,
} from '@preset.nz/app-kit/core';
import { dismissNotification, notify, SnackbarProvider, SnackbarViewport } from '@preset.nz/ux-kit';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ARRANGEMENT_PREFIX,
  addDrone,
  addEnvelope,
  addGenerator,
  addGlobalLfo,
  addLfo,
  addMaterial,
  addRegion,
  addTrack,
  arrangementDefs,
  auditionGrain,
  type ChainLevel,
  cancelLearn,
  deletePatch,
  detectRegions,
  duplicatePatch,
  EMPTY_MAPPINGS,
  editTracker,
  envelopeCurve,
  envelopeGestureApply,
  envelopeGestureBegin,
  envelopeGestureCancel,
  envelopeGestureEnd,
  extractRegion,
  forgetMidi,
  format,
  fxAdd,
  fxMove,
  fxRemove,
  type GrainInfo,
  getArrangement,
  getHeard,
  getParams,
  getTracker,
  globalsOf,
  grainLog,
  type LfoLimits,
  type LoadReport,
  learnMidi,
  lfoLimits,
  linkNew,
  linkParam,
  listGenerators,
  listPatches,
  MAX_DRUMS_TRACKS,
  MAX_MELODY_TRACKS,
  type MappingsView,
  type MaterialsView,
  type MaterialView,
  type Meters,
  type ModulationView,
  mapMidi,
  materialPeaks,
  materialWave,
  modulatorsOf,
  newPatch,
  nodeWords,
  type ParamInfo,
  type PatchInfo,
  type PatchPreview as PatchPreviewData,
  type PlayMode,
  paramDefs,
  patchPreview,
  type RecordingState,
  type RecordSource,
  mappings as readMappings,
  materials as readMaterials,
  meters as readMeters,
  modulation as readModulation,
  recording as readRecording,
  recordStart,
  recordStop,
  removeEnvelope,
  removeGenerator,
  removeLfo,
  removeMaterial,
  removeRegion,
  removeTrack,
  renamePatch,
  resetSound,
  type SourceInfo,
  selectPatch,
  selectTrack,
  setLfo,
  setMaterial,
  setEnvelope as setModEnvelope,
  setParam,
  setPlaying,
  setPlayMode,
  setRecordMenu,
  setRegion,
  setTrackDuck,
  setTracker,
  setTrackPatch,
  type Tracker,
  type TrackerEdit,
  toggleTrackMute,
  toggleTrackSolo,
  trackName,
  unlinkParam,
  wireMaterial,
} from '@/audio';
import { AddMenu } from '@/components/AddMenu';
import { GeneratorTree } from '@/components/GeneratorTree';
import { GrainInspector } from '@/components/GrainInspector';
import { Inspector } from '@/components/Inspector';
import { MaterialTree } from '@/components/MaterialTree';
import { Meter } from '@/components/Meter';
import { ModulatorTree } from '@/components/ModulatorTree';
import { NodeCard } from '@/components/NodeCard';
import type { ParamRowContext } from '@/components/ParamRow';
import { PatchPreview } from '@/components/PatchPreview';
import { SceneView } from '@/components/SceneView';
import { SettingsDialog } from '@/components/SettingsDialog';
import { StepStrip } from '@/components/StepStrip';
import { TrackFader } from '@/components/TrackFader';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Waveform } from '@/components/Waveform';
import { usePersistedState } from '@/lib/persisted';
import {
  ARRANGEMENT_LANES,
  ARRANGEMENT_NODES,
  FX_ORDER_LEN,
  fxChain,
  fxKinds,
  LANES,
  NODES,
  type NodeInfo,
  nodeById,
  type ParamValues,
  registerParamScope,
  setNodeWords,
} from '@/scope';
import { useSelection } from '@/stores/selection';
import { FIT, minSpanFor, stepZoom, type View } from '@/waveView';

/**
 * How many spawns the inspector keeps.
 *
 * This is what the table lists and what the frozen overlay draws, so it is a
 * legibility budget rather than a memory one: 400 bars at a busy density
 * overlap into a solid block you cannot pick anything out of. Two hundred is
 * about three seconds at the default density. Tune freely — it and
 * `LIVE_TRAIL` in the waveform are the two numbers that decide how much of the
 * cloud you see at once.
 */
const GRAIN_SCROLLBACK = 200;

/**
 * What the middle shows: a track's steps (`tracker`) or a patch's lanes
 * (`soundscape`). The scene view picks it: selecting a track is the one,
 * selecting a patch the other. Since epic 28 it never changes what plays:
 * every track plays while the transport runs, and a selected patch no track
 * plays loops on its own over them. What Play plays is the header's own
 * choice (`PlayMode`), since Georg, 2026-10-13: *"play always plays the
 * sequencer, I can't just loop the note"*: Song is all of the above, Loop
 * and Once play the current patch alone. Remembered across launches as UI
 * state.
 */
type Mode = 'tracker' | 'soundscape';

/** What Play plays, with the words the header shows and its tooltip. */
const PLAY_MODES: [PlayMode, string, string][] = [
  ['song', 'Song', 'Play the song'],
  ['loop', 'Loop', 'Loop the patch on its own'],
  ['once', 'Once', 'Play the patch once, start to end'],
];

/** Seconds as m:ss. */
/** Whether two lists hold the same strings in the same order. */
function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

function formatElapsed(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, '0')}`;
}

export default function App() {
  const [defs, setDefs] = useState<ParamInfo[] | null>(null);
  const [values, setValues] = useState<ParamValues>({});
  /** The material pool and what each generator reads, as Rust last answered. */
  const [pool, setPool] = useState<MaterialsView>({
    materials: [],
    wires: { material: null, grain: null },
  });
  /** Waveforms by material id, each fetched once. */
  const [waves, setWaves] = useState<Record<number, SourceInfo>>({});
  const [meter, setMeter] = useState<Meters>({
    peak: 0,
    reduction: 1,
    grains: 0,
    playing: false,
    play_mode: 'song',
    steps: [],
    playhead: 0,
    auditioning: false,
    reversing: false,
    block_us: 0,
    block_budget_us: 0,
    audio_allocs: 0,
    mix: [],
  });
  /**
   * The grain scrollback. The Rust side drains — it hands over what has
   * happened since the last poll and forgets it — so the history lives here.
   * Capped, because at the ceiling of 200 grains a second an uncapped list
   * would be a memory leak with a nice view.
   */
  const [grains, setGrains] = useState<GrainInfo[]>([]);
  const [frozen, setFrozen] = useState(false);
  const [picked, setPicked] = useState<GrainInfo | null>(null);
  // Read inside the poll without making it a dependency, which would tear the
  // interval down and rebuild it on every freeze.
  const frozenRef = useRef(false);
  frozenRef.current = frozen;
  /** The patch's rows, then the arrangement's. `patchRowsRef` says where one ends. */
  const defsRef = useRef<ParamInfo[] | null>(null);
  const patchRowsRef = useRef(0);
  const [envelope, setEnvelope] = useState<number[] | null>(null);
  /** The open patch's LFOs and links, as Rust last answered with them. */
  const [mod, setMod] = useState<ModulationView>({
    lfos: [],
    envelopes: [],
    globals: [],
    links: {},
    refused: [],
  });
  const [limits, setLimits] = useState<LfoLimits | null>(null);
  const { selection, select, clear } = useSelection();
  const [settingsOpen, setSettingsOpen] = useState(false);
  // The grain inspector and the raw parameter list are for debugging, so they
  // are hidden unless asked for, and the choice is remembered.
  const [showDebugger, setShowDebugger] = usePersistedState('shard.debugger', false);
  const [mode, setMode] = usePersistedState<Mode>('shard.mode', 'soundscape');
  // How the waveform scrubs (roadmap row 45). UI state only: Tape is the
  // Sample's Scrub row on, Freeze is the cloud's Position, Off leaves the
  // waveform's trim as it was.
  const [scrub, setScrub] = usePersistedState<'off' | 'tape' | 'freeze'>('shard.scrub', 'off');
  // Regions (roadmap row 49): how readily Detect finds onsets, and which
  // region of which material is selected. UI state; the regions themselves
  // are the session's.
  const [detectSensitivity, setDetectSensitivity] = usePersistedState(
    'shard.detectSensitivity',
    0.5,
  );
  const [regionSel, setRegionSel] = useState<{ id: number; index: number } | null>(null);
  // What Play plays. UI state, remembered across launches and told to the
  // backend on launch and whenever it changes; the backend keeps it for the
  // session, outside the document.
  const [playMode, setPlayModeState] = usePersistedState<PlayMode>('shard.playMode', 'song');
  useEffect(() => {
    void setPlayMode(playMode).catch((e) => setError(String(e)));
  }, [playMode]);
  // Read once at startup, to tell Rust a track is selected.
  const startMode = useRef(mode);
  /** The session's patches, in order, as the scene view lists them. */
  const [patches, setPatches] = useState<PatchInfo[]>([]);
  // The generators the current patch holds, polled with the values.
  const [present, setPresent] = useState<string[]>([]);
  /**
   * The track the tracker shows and edits, by place. Rust keeps the same
   * index, brought within the tracks there are, so both clamp alike.
   */
  const [selectedTrack, setSelectedTrack] = useState(0);
  /** Each parameter as the engine last heard it, LFOs included. */
  const [heard, setHeard] = useState<ParamValues>({});
  /** The active controller map, as the rows draw it. */
  const [midi, setMidi] = useState<MappingsView>(EMPTY_MAPPINGS);
  const learnSeen = useRef(0);
  /**
   * The slowest audio block, held for two seconds. Rust already holds the
   * worst case between polls; this holds it long enough to read, because a
   * spike shown for one 33 ms frame is a spike nobody sees.
   */
  const [blockWorst, setBlockWorst] = useState(0);
  const worstRef = useRef({ us: 0, at: 0 });
  /** The tracker, the level above the patch. Shown at once, then as Rust answers. */
  const [tracker, setTrackerView] = useState<Tracker | null>(null);
  const trackerSeq = useRef(0);
  /** A tracker as Rust answered with it, the selected track kept within it. */
  const showTracker = useCallback((t: Tracker) => {
    setTrackerView(t);
    setSelectedTrack((s) => Math.min(s, Math.max(0, t.tracks.length - 1)));
  }, []);
  const editTrackerView = useCallback(
    async (track: number, edit: TrackerEdit) => {
      try {
        showTracker(await editTracker(track, edit));
      } catch (e) {
        setError(String(e));
      }
    },
    [showTracker],
  );
  const changeTracker = useCallback(
    async (next: Tracker) => {
      setTrackerView(next);
      const seq = ++trackerSeq.current;
      try {
        const played = await setTracker(next);
        // Only the newest answer, or a slow one would undo a quicker click.
        if (seq === trackerSeq.current) showTracker(played);
      } catch (e) {
        setError(String(e));
      }
    },
    [showTracker],
  );

  // Each mode edits its own level, so a selection does not cross between
  // them: a patch node, a material or an LFO means nothing in the tracker,
  // and an arrangement node nothing in sound scaping. Only a selection from
  // the other level is cleared, so a click that selects and then switches,
  // such as the waveform's title, keeps what it selected.
  useEffect(() => {
    const now = useSelection.getState().selection;
    if (!now) return;
    const layer = now.kind === 'node' ? nodeById(now.id)?.layer : 'patch';
    if (layer !== (mode === 'tracker' ? 'arrangement' : 'patch')) {
      useSelection.getState().clear();
    }
  }, [mode]);

  // Startup: ask Rust for the table, register the facets scope from it, then
  // read the current values. The table is the single source of truth.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        // One list of rows for the panels, patch and arrangement together.
        // Their ids never collide, since the arrangement's all start with
        // `arrangement.`, and `setParam` sends each to its own table.
        const [pd, ad, words] = await Promise.all([paramDefs(), arrangementDefs(), nodeWords()]);
        if (cancelled) return;
        const d = [...pd, ...ad];
        setNodeWords(words);
        registerParamScope(d);
        defsRef.current = d;
        patchRowsRef.current = pd.length;
        setDefs(d);
        const [v, a] = await Promise.all([getParams(), getArrangement()]);
        const next: ParamValues = {};
        d.forEach((p, i) => {
          next[p.id] = i < pd.length ? v[i] : a[i - pd.length];
        });
        setValues(next);
        setPool(await readMaterials());
        setMod(await readModulation());
        setLimits(await lfoLimits());
        showTracker(await getTracker());
        // Remembered on the tracker: its first track is the selection, so
        // nothing is auditioned.
        if (startMode.current === 'tracker') await selectTrack(0);
        setPatches(await listPatches());
      } catch (e) {
        setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [showTracker]);

  // Poll. Thirty hertz is enough for a meter and for the controls to follow a
  // patch load or a preset, and it keeps the boundary quiet. Nothing here is on the
  // audio path, so a late frame costs nothing but a slightly stale number.
  useEffect(() => {
    if (!defs) return;
    let alive = true;
    const tick = async () => {
      if (!alive) return;
      try {
        const [m, v, fresh, h, mm, a, g] = await Promise.all([
          readMeters(),
          getParams(),
          grainLog(),
          getHeard(),
          readMappings(),
          getArrangement(),
          listGenerators(),
        ]);
        if (!alive) return;
        setPresent((prev) => (sameList(prev, g) ? prev : g));
        setMeter(m);
        setMidi(mm);
        // Learning answers once, through the poll. Say it once.
        if (mm.report && mm.report.seq !== learnSeen.current) {
          learnSeen.current = mm.report.seq;
          if (mm.report.ok) setNote(mm.report.text);
          else setError(mm.report.text);
        }
        const now = performance.now();
        if (m.block_us >= worstRef.current.us || now - worstRef.current.at > 2000) {
          worstRef.current = { us: m.block_us, at: now };
          setBlockWorst(m.block_us);
        }
        // Drain regardless of the freeze, or resuming would dump a backlog of
        // everything that happened while it was frozen. Frozen means "stop
        // adding to what I am looking at", not "stop the engine logging".
        if (!frozenRef.current && fresh.length > 0) {
          setGrains((prev) => [...prev, ...fresh].slice(-GRAIN_SCROLLBACK));
        }
        const d = defsRef.current;
        if (d) {
          const next: ParamValues = {};
          const nextHeard: ParamValues = {};
          // No LFO moves an arrangement row, so it is heard as it is set.
          const n = patchRowsRef.current;
          d.forEach((p, i) => {
            next[p.id] = i < n ? v[i] : a[i - n];
            nextHeard[p.id] = i < n ? h[i] : a[i - n];
          });
          setValues(next);
          setHeard(nextHeard);
        }
      } catch {
        // A dropped poll is not worth surfacing; the next one will land.
      }
    };
    const id = setInterval(tick, 33);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, [defs]);

  // The patch preview: re-rendered in Rust a moment after anything that
  // could change the sound settles, and only the newest answer is drawn.
  // Values are polled, so the key is what changed, not the poll.
  const [preview, setPreview] = useState<PatchPreviewData | null>(null);
  const [previewBusy, setPreviewBusy] = useState(false);
  const previewSeq = useRef(0);
  const previewKey = [
    Object.entries(values)
      .map(([k, v]) => `${k}=${v}`)
      .join(','),
    JSON.stringify(mod),
    JSON.stringify(pool.wires),
    pool.materials.map((m) => `${m.id}:${m.octave}:${m.trim_start}:${m.trim_end}`).join(','),
  ].join('|');
  // previewKey is the trigger: the command reads everything on the Rust side.
  // biome-ignore lint/correctness/useExhaustiveDependencies: see above
  useEffect(() => {
    if (!defs || mode !== 'soundscape') return;
    const seq = ++previewSeq.current;
    setPreviewBusy(true);
    const t = setTimeout(() => {
      void patchPreview()
        .then((p) => {
          if (seq === previewSeq.current) setPreview(p);
        })
        .catch((e) => setError(String(e)))
        .finally(() => {
          if (seq === previewSeq.current) setPreviewBusy(false);
        });
    }, 120);
    return () => clearTimeout(t);
  }, [defs, previewKey, mode]);

  // Refetch the curve only when an envelope control or Sample's material
  // actually moves. Polling it thirty times a second would be free but
  // pointless; this way the drawn curve is the one Rust computes, not a copy
  // of the maths.
  const playerMaterial = pool.materials.find((m) => m.id === pool.wires.material) ?? null;
  const envKey = [
    values['env.on'],
    values['env.mix'],
    values['env.attack'],
    values['env.decay'],
    values['env.sustain'],
    values['env.release'],
    pool.wires.material,
    playerMaterial?.trim_start,
    playerMaterial?.trim_end,
  ].join(',');
  // envKey is the trigger, not an input: the command reads the values on the
  // Rust side, so nothing in this effect references them, but it still has to
  // re-run when they change.
  // biome-ignore lint/correctness/useExhaustiveDependencies: see above
  useEffect(() => {
    if (!defs) return;
    let alive = true;
    void envelopeCurve().then((c) => {
      if (!alive) return;
      // A flat curve is not worth drawing over the waveform.
      setEnvelope(c.every((v) => v >= 0.999) ? null : c);
    });
    return () => {
      alive = false;
    };
  }, [defs, envKey]);

  // What the waveform shows follows the selection (Georg, 2026-09-15): a
  // material picked in the tree, else the one wired into the selected
  // generator, else Sample's.
  // Sample's material by default, but a Sample that is off makes no sound of
  // its own, so the cloud's is shown instead when the cloud is on. With
  // neither on, Sample's is still shown, dimmed: it is the clock, so its trim
  // sets how long a pass is even when nothing plays it (Georg, 2026-09-27).
  const sampleOn = (values['material.on'] ?? 1) >= 0.5;
  const grainOn = (values['grain.on'] ?? 0) >= 0.5;
  const shownFor =
    (selection?.kind === 'node' && selection.id === 'grain') ||
    (!sampleOn && grainOn && !(selection?.kind === 'node' && selection.id === 'material'))
      ? 'Granular'
      : 'Sample';
  const shownId: number | null =
    selection?.kind === 'material'
      ? selection.id
      : shownFor === 'Granular'
        ? pool.wires.grain
        : pool.wires.material;
  const shownRecord = pool.materials.find((m) => m.id === shownId) ?? null;
  const shownWave = shownId === null ? null : (waves[shownId] ?? null);
  const shownRegions = shownRecord?.regions ?? [];
  // The waveform's visible window (View > Zoom). Webview state, never in the
  // document: a different material shown starts again at fit.
  const [viewFor, setViewFor] = useState<{ id: number | null; view: View }>({
    id: null,
    view: FIT,
  });
  const waveView = viewFor.id === shownId ? viewFor.view : FIT;
  const setWaveView = (v: View) => setViewFor({ id: shownId, view: v });
  const waveMinSpan = minSpanFor(shownWave?.seconds ?? 0);
  const fetchWavePeaks = useMemo(
    () =>
      shownId === null
        ? undefined
        : (start: number, end: number, buckets: number) =>
            materialPeaks(shownId, start, end, buckets),
    [shownId],
  );
  const selectedRegion =
    regionSel !== null && regionSel.id === shownId && regionSel.index < shownRegions.length
      ? regionSel.index
      : null;
  const shownIsPlayer = shownId !== null && shownId === pool.wires.material;
  const shownIsGrain = shownId !== null && shownId === pool.wires.grain;
  // Only the generators that are on read anything you can hear.
  const readers = [
    shownIsPlayer && sampleOn ? 'Sample' : null,
    shownIsGrain && grainOn ? 'Granular' : null,
  ].filter((r): r is string => r !== null);
  // Shown only because it is the clock: nothing that is on reads it. That is
  // true only while the patch's Length is Sample; under Loop or Hold the
  // sample is no clock, so an unheard Sample shows nothing.
  const lengthIsSample = Math.round(values['patch.length'] ?? 1) === 1;
  const unheard = shownIsPlayer && readers.length === 0 && selection?.kind !== 'material';
  const clockOnly = unheard && lengthIsSample;
  const blank = unheard && !lengthIsSample;

  // Each waveform is fetched once. Ids are never reused within a document,
  // and opening a document clears the lot.
  useEffect(() => {
    if (shownId === null || waves[shownId]) return;
    let alive = true;
    materialWave(shownId)
      .then((w) => {
        if (alive) setWaves((prev) => ({ ...prev, [shownId]: w }));
      })
      .catch(() => {
        // A material whose file could not be read has no waveform to draw.
      });
    return () => {
      alive = false;
    };
  }, [shownId, waves]);

  /**
   * The tape brake — a finger on the reel, not a mute.
   *
   * Momentary on purpose: you press it, the tape slows and garbles, you let go
   * and it winds back up. It writes `arrangement.tape.brake` through `setParam` like every
   * other control, which is the point — when this becomes a MIDI CC or a pedal
   * it takes exactly the same path, and a continuous value already means
   * something (half a brake is a tape running slow and flat).
   */
  const setBrake = useCallback((on: boolean) => {
    void setParam('arrangement.tape.brake', on ? 1 : 0);
  }, []);

  /**
   * Reverse is a gate, like the brake — the engine decides what a tap means.
   *
   * Tap and you get a flick of `tape.flick`, whatever your finger actually
   * did; hold and the reel stays backwards until you let go. Keeping that rule
   * in the engine rather than here is what lets a MIDI note or a footswitch
   * play the same gesture without a path of its own.
   */
  const setReverse = useCallback((on: boolean) => {
    void setParam('arrangement.tape.reverse', on ? 1 : 0);
  }, []);

  const doAudition = useCallback(async (g: GrainInfo) => {
    setPicked(g);
    try {
      await auditionGrain(g);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // The document's name and unsaved mark, from app-kit, which owns File's
  // New, Open, Open Recent, Save and Revert, reopens the last document at
  // launch and takes documents from Finder.
  const document_ = useDocument();
  useDocumentNotes(setNote);

  // A document replaced the one on screen: opened, reverted, restored at
  // launch, from Finder, or New. Values and meters are polled; the rest is
  // read again here, and a partial load says so.
  useEffect(() => {
    const unlisten = listen<LoadReport>('document-opened', async (e) => {
      const report = e.payload;
      try {
        setWaves({});
        setPool(await readMaterials());
        setMod(await readModulation());
        setPatches(await listPatches());
        // The document's tracker replaces the one on screen, and no answer to
        // an edit made before the load may put the old one back.
        trackerSeq.current++;
        // What the middle shows is the session's, not the document's, so it
        // stays; the selected track is kept within the tracks there are.
        showTracker(await getTracker());
        useSelection.getState().clear();
        setError(null);

        // A partial load must not look like a clean one.
        const parts: string[] = [];
        if (report.sample_missing) {
          parts.push(`sample not found: ${report.sample_path ?? 'unknown'}`);
        }
        if (report.unknown.length > 0) {
          parts.push(`${report.unknown.length} unknown parameter(s) ignored`);
        }
        if (report.refused.length > 0) {
          parts.push(`${report.refused.length} LFO or link(s) could not be used`);
        }
        if (report.map_missing) {
          parts.push(`controller map "${report.map_missing}" not on this Mac`);
        }
        setNote(parts.length > 0 ? parts.join(' · ') : null);
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [showTracker]);

  /** Run one material command, and draw the pool Rust answers with. */
  const editMaterials = useCallback(async (run: () => Promise<MaterialsView>) => {
    try {
      setPool(await run());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /** Wire a material into Sample (`material`) or Granular (`grain`), or nothing. */
  const wire = useCallback(
    (node: string, material: number | null) =>
      void editMaterials(() => wireMaterial(node, material)),
    [editMaterials],
  );

  /**
   * A material's octave or trim. Shown at once, then as Rust answers. Only
   * the newest answer lands, or a slow one would undo a quicker drag.
   */
  const materialSeq = useRef(0);
  const changeMaterial = useCallback(async (m: MaterialView) => {
    setPool((prev) => ({
      ...prev,
      materials: prev.materials.map((x) => (x.id === m.id ? m : x)),
    }));
    const seq = ++materialSeq.current;
    try {
      const view = await setMaterial(m.id, m.octave, m.trim_start, m.trim_end, m.root);
      if (seq === materialSeq.current) setPool(view);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /** Regions of the material on show. Rust holds them; each gesture is a step. */
  const regionSeq = useRef(0);
  const markRegion = useCallback(async (id: number, start: number, end: number) => {
    try {
      const added = await addRegion(id, start, end);
      setPool(added.materials);
      setRegionSel({ id, index: added.index });
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  const moveRegion = useCallback(async (id: number, index: number, start: number, end: number) => {
    setPool((prev) => ({
      ...prev,
      materials: prev.materials.map((m) =>
        m.id === id
          ? { ...m, regions: m.regions.map((r, i) => (i === index ? { start, end } : r)) }
          : m,
      ),
    }));
    const seq = ++regionSeq.current;
    try {
      const view = await setRegion(id, index, start, end);
      if (seq === regionSeq.current) setPool(view);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  const dropRegion = useCallback(
    async (id: number, index: number) => {
      await editMaterials(() => removeRegion(id, index));
      setRegionSel(null);
    },
    [editMaterials],
  );
  const findRegions = useCallback(async (id: number, sensitivity: number) => {
    try {
      const view = await detectRegions(id, sensitivity);
      setPool(view);
      setRegionSel(null);
      const n = view.materials.find((m) => m.id === id)?.regions.length ?? 0;
      setNote(
        n === 0
          ? 'No transients found. Raise the sensitivity, or Shift-drag to mark by hand.'
          : `Marked ${n} region${n === 1 ? '' : 's'}. Undo brings the old ones back.`,
      );
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  const extractSelected = useCallback(async (id: number, index: number) => {
    try {
      const out = await extractRegion(id, index);
      setPool(out.materials);
      setNote(`Extracted region ${index + 1} to Materials as ${out.name}.`);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /** Add one or more WAVs to the pool. A silent generator reads the first. */
  const pickFiles = useCallback(async () => {
    try {
      const picked = await open({
        multiple: true,
        filters: [{ name: 'Audio', extensions: ['wav'] }],
      });
      if (!picked || picked.length === 0) return;
      const failed: string[] = [];
      let view: MaterialsView | null = null;
      for (const path of picked) {
        try {
          view = await addMaterial(path);
        } catch (e) {
          failed.push(String(e));
        }
      }
      if (view) setPool(view);
      setError(failed.length > 0 ? failed.join(' · ') : null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /** Run one modulation command and draw the document Rust answers with. */
  const editMod = useCallback(async (run: () => Promise<ModulationView>) => {
    try {
      setMod(await run());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // The palette (`design/effect-palette.md`): the process lane's effects are
  // added, taken out and moved on whichever level's chain is showing. Rust
  // holds the order; the next poll draws it.
  const level: ChainLevel = mode === 'tracker' ? 'arrangement' : 'patch';
  const addEffect = useCallback(
    (kind: string) => {
      fxAdd(level, kind)
        .then((n) => {
          const prefix = level === 'arrangement' ? ARRANGEMENT_PREFIX : '';
          // Select it, so it can be set up at once.
          select({ kind: 'node', id: `${prefix}fx.${n}.${kind}` });
        })
        .catch((e) => setError(String(e)));
    },
    [level, select],
  );
  const removeEffect = useCallback(
    (node: NodeInfo) => {
      if (!node.fx) return;
      fxRemove(level, node.fx.n).catch((e) => setError(String(e)));
      const now = useSelection.getState().selection;
      if (now?.kind === 'node' && now.id === node.id) clear();
    },
    [level, clear],
  );
  const moveEffect = useCallback(
    (node: NodeInfo, by: number) => {
      const fx = node.fx;
      if (!fx) return;
      fxMove(level, fx.n, by)
        .then((n) => {
          // Passing another of its kind swaps their copies (the k-th of a
          // kind plays copy k), so a selection on either follows its effect.
          if (n === null || n === fx.n) return;
          const prefix = level === 'arrangement' ? ARRANGEMENT_PREFIX : '';
          const was = `${prefix}fx.${fx.n}.${fx.kind}`;
          const now = `${prefix}fx.${n}.${fx.kind}`;
          const sel = useSelection.getState().selection;
          if (sel?.kind !== 'node') return;
          if (sel.id === was) select({ kind: 'node', id: now });
          else if (sel.id === now) select({ kind: 'node', id: was });
        })
        .catch((e) => setError(String(e)));
    },
    [level, select],
  );

  // Undo and redo. Rust steps the document back; the values arrive with the
  // next poll, and the tracker and the modulation are asked for again here.
  // Undo and Redo are app-kit's: the Edit menu, its shortcut and the
  // fallback below all reach Rust's history, which says what it stepped.
  // Materials, modulation and the tracker are not polled, so read them again.
  useEffect(() => {
    const unlisten = listen<{ undid: boolean; label: string }>('history-stepped', async (e) => {
      try {
        showTracker(await getTracker());
        setMod(await readModulation());
        setPool(await readMaterials());
        setPatches(await listPatches());
        setNote(`${e.payload.undid ? 'Undid' : 'Redid'} “${e.payload.label}”.`);
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, [showTracker]);

  const togglePlay = useCallback(async () => {
    await setPlaying(!meter.playing);
  }, [meter.playing]);

  // Record a take (epic 44). The source is the window's own state, like the
  // selection: not an edit, not undoable, not saved. A take is independent of
  // the transport; the backend adds the finished WAV to the pool.
  const [recSource, setRecSource] = useState<RecordSource>('master');
  const [rec, setRec] = useState<RecordingState | null>(null);
  const recActive = rec?.active ?? false;
  const recArmed = rec?.active === true && rec.armed;
  const recActiveRef = useRef(false);
  recActiveRef.current = recActive;
  const recSourceRef = useRef<RecordSource>('master');
  recSourceRef.current = recSource;
  const recBusy = useRef(false);
  const toggleRecord = useCallback(async (armed = false) => {
    if (recBusy.current) return;
    recBusy.current = true;
    try {
      if (recActiveRef.current) {
        const take = await recordStop();
        setRec(null);
        if (!take) {
          setNote('Disarmed. No sound came, so nothing was recorded.');
          setError(null);
          return;
        }
        setPool(await readMaterials());
        setNote(
          take.dropped > 0
            ? `Added the take, but the disk fell behind and dropped ${take.dropped} frames.`
            : 'Added the take to Materials.',
        );
        setError(null);
      } else {
        setRec(await recordStart(recSourceRef.current, armed));
        setError(null);
      }
    } catch (e) {
      setError(String(e));
      try {
        setRec(await readRecording());
      } catch {
        setRec(null);
      }
    } finally {
      recBusy.current = false;
    }
  }, []);
  // The elapsed time and dropped count come from the recorder, a few times a
  // second while a take runs. Once on load too, in case one is already going.
  useEffect(() => {
    void readRecording()
      .then((r) => setRec(r.active ? r : null))
      .catch(() => {});
  }, []);
  useEffect(() => {
    if (!recActive) return;
    const timer = window.setInterval(() => {
      void readRecording()
        .then((r) => setRec(r.active ? r : null))
        .catch(() => {});
    }, 250);
    return () => window.clearInterval(timer);
  }, [recActive]);
  // File > Record Take reads Stop Recording while a take runs.
  useEffect(() => {
    void setRecordMenu(recActive, recArmed).catch(() => {});
  }, [recActive, recArmed]);
  // A removed track can't be the source: fall back to the master.
  const trackCount = tracker?.tracks.length ?? 0;
  useEffect(() => {
    if (recSource !== 'master' && recSource >= trackCount) setRecSource('master');
  }, [recSource, trackCount]);

  // The keyboard fallbacks: undo, view switches, held tape keys, and Space for
  // play/stop, the way every other audio tool does it. Other keys are ignored
  // while a control has focus, so arrow keys on a slider still work.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // A text field keeps its own undo; everything else steps the document.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'z') {
        if (isTextField(e.target)) return;
        e.preventDefault();
        void (e.shiftKey ? redo() : undo());
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === ',') {
        e.preventDefault();
        setSettingsOpen(true);
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'i') {
        e.preventDefault();
        setShowDebugger((v) => !v);
        return;
      }
      if ((e.metaKey || e.ctrlKey) && (e.key === '1' || e.key === '2')) {
        e.preventDefault();
        menuRef.current(e.key === '1' ? 'view-tracker' : 'view-soundscape');
        return;
      }
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;

      // The selected effect: Alt+Up and Alt+Down move it in its chain, Delete
      // takes it out. Anything else selected ignores these. Ahead of the
      // button check, because clicking a card's title focuses its button.
      // With Cmd the chord is the menu's accelerator, which has run already.
      const moving = e.altKey && !e.metaKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown');
      // Not while a slider has focus: arrows and keys there belong to the slider.
      const inSlider = t?.closest('[role="slider"], [role="spinbutton"], [contenteditable="true"]');
      if (!inSlider && (moving || e.key === 'Backspace' || e.key === 'Delete')) {
        const sel = useSelection.getState().selection;
        const node = sel?.kind === 'node' ? nodeById(sel.id) : null;
        if (node?.fx) {
          e.preventDefault();
          if (moving) moveEffect(node, e.key === 'ArrowUp' ? -1 : 1);
          else removeEffect(node);
          return;
        }
      }
      if (t?.tagName === 'BUTTON') return;

      // Deselect all. The inspector empties with it. A row waiting for a
      // MIDI control stops waiting, and a banner goes away.
      if (e.key === 'Escape') {
        useSelection.getState().clear();
        void cancelLearn();
        setError(null);
        setNote(null);
        return;
      }

      // Held, not toggled, so the keyboard feels like the button does. The
      // repeat guard matters: without it every auto-repeat rewrites the
      // parameter, which is harmless but makes the log noise misleading.
      if (e.code === 'KeyB') {
        if (!e.repeat) setBrake(true);
        e.preventDefault();
        return;
      }
      if (e.code === 'KeyR') {
        if (!e.repeat) setReverse(true);
        e.preventDefault();
        return;
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === 'KeyB') setBrake(false);
      if (e.code === 'KeyR') setReverse(false);
    };
    // Space is play and stop wherever focus is, except where it types a
    // space (Georg, 2026-10-13). Caught on the way down, so a focused
    // button, dropdown or slider never sees it: a button clicks on Space's
    // keyup, so that is swallowed too.
    const typesSpace = (t: EventTarget | null) => {
      if (!(t instanceof HTMLElement)) return false;
      if (t.isContentEditable || t.tagName === 'TEXTAREA') return true;
      if (t.tagName !== 'INPUT') return false;
      const type = (t as HTMLInputElement).type;
      return !/^(checkbox|radio|range|button|submit|reset|color|file)$/.test(type);
    };
    const onSpace = (e: KeyboardEvent) => {
      if (e.code !== 'Space' || e.metaKey || e.ctrlKey || e.altKey) return;
      if (typesSpace(e.target)) return;
      e.preventDefault();
      e.stopPropagation();
      if (e.type === 'keydown' && !e.repeat) void togglePlay();
    };
    window.addEventListener('keydown', onSpace, true);
    window.addEventListener('keyup', onSpace, true);
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onSpace, true);
      window.removeEventListener('keyup', onSpace, true);
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [togglePlay, setBrake, setReverse, setShowDebugger, moveEffect, removeEffect]);

  // The native menu bar: app-kit's, with Shard's commands (`menu.rs`). Each
  // item arrives as app-kit's `command` event carrying its id and runs the
  // same function the keyboard fallback above calls. Read through a ref, so
  // the bindings are made once. The tracker's Pattern items are the strip's
  // own. Undo and Redo, in a text field too, are app-kit's.
  // The scene view's selections decide what the middle shows (epics 10 and
  // 28). None of them edits the document, and none changes what a track
  // plays. A track shows the tracker on its steps, and its patch becomes the
  // current one; a patch becomes current and shows its lanes, and loops on
  // its own while no track plays it. Rust says so along `document-opened`
  // when the current patch changes.
  const selectTrackRow = async (track: number) => {
    try {
      await selectTrack(track);
      setSelectedTrack(track);
      setMode('tracker');
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  // A track's *Plays* picker: one undo step, "Set Plays".
  const pickPlays = async (track: number, id: number) => {
    try {
      await setTrackPatch(track, id);
      showTracker(await getTracker());
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  // A track's *Ducked by* picker: one undo step, "Set Ducked by".
  const pickDuckedBy = async (track: number, key: number | null) => {
    try {
      showTracker(await setTrackDuck(track, key));
    } catch (e) {
      setError(String(e));
    }
  };
  const selectPatchRow = async (id: number) => {
    try {
      await selectPatch(id);
      setMode('soundscape');
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  // Add Track › Melody: one undo step, a track at the end playing the
  // current patch; › Drums, a drums track at the end. Either becomes the
  // selection, each greyed once its pool is full. Remove Track: one undo
  // step, named for it; the last melody track stays.
  const melodyTracks = tracker?.tracks.filter((t) => t.drums == null).length ?? 1;
  const drumsTracks = (tracker?.tracks.length ?? 1) - melodyTracks;
  const canAddMelody = melodyTracks < MAX_MELODY_TRACKS;
  const canAddDrums = drumsTracks < MAX_DRUMS_TRACKS;
  const canRemoveTrack = tracker?.tracks[selectedTrack]?.drums != null || melodyTracks > 1;
  const addTrackRow = async (drums: boolean) => {
    try {
      const added = await addTrack(drums);
      showTracker(await getTracker());
      await selectTrackRow(added);
    } catch (e) {
      setError(String(e));
    }
  };
  const removeTrackRow = async (track: number) => {
    try {
      await removeTrack(track);
      // The selection stays on the track it was on, as Rust keeps it.
      setSelectedTrack((s) => (s > track ? s - 1 : s));
      showTracker(await getTracker());
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  // Mute and solo, per track (epic 28, story c). Session state, not edits:
  // no undo step, no unsaved mark. While playing they land on the next bar;
  // the poll shows a press still waiting.
  const toggleMute = async (track: number) => {
    try {
      const mix = await toggleTrackMute(track);
      setMeter((m) => ({ ...m, mix }));
    } catch (e) {
      setError(String(e));
    }
  };
  const toggleSolo = async (track: number) => {
    try {
      const mix = await toggleTrackSolo(track);
      setMeter((m) => ({ ...m, mix }));
    } catch (e) {
      setError(String(e));
    }
  };
  const newPatchRow = () => {
    // The panels read the new patch along `document-opened`.
    void newPatch()
      .then(() => setMode('soundscape'))
      .catch((e) => setError(String(e)));
  };

  // The patch whose name is being edited in the scene view, by id, so a
  // reread of the list (selecting emits `document-opened`) keeps the field
  // open, and Edit › Rename can open it.
  const [renaming, setRenaming] = useState<number | null>(null);
  // Story d's patch commands. Each is one undo step named for the patch.
  // Duplicate and a Delete of the current patch move the engine, and Rust
  // says so along `document-opened`; the list is read again here either way.
  const renamePatchRow = async (id: number, name: string) => {
    try {
      if (await renamePatch(id, name)) setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  const duplicatePatchRow = async (id: number) => {
    try {
      await duplicatePatch(id);
      setMode('soundscape');
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  const deletePatchRow = async (id: number) => {
    try {
      await deletePatch(id);
      setPatches(await listPatches());
    } catch (e) {
      setError(String(e));
    }
  };
  // Edit's Rename, Duplicate and Delete act on the selection (menu
  // standard): with the track selected Duplicate doubles the pattern, with a
  // patch selected it copies the patch. Delete takes a selected effect, as
  // Backspace does, and the patch only when nothing in it is selected, so a
  // card selected in the inspector never takes its whole patch with it.
  const currentPatch = patches.find((p) => p.current);
  const selectedFx = selection?.kind === 'node' ? (nodeById(selection.id)?.fx ?? null) : null;
  const patchSelected = mode === 'soundscape' ? (currentPatch ?? null) : null;
  const deletablePatch = selection === null && patchSelected?.deletable ? patchSelected : null;
  // Cmd+D reaches the menu even while a name is being typed.
  const typing = useTextFocus();
  const canRename = !typing && patchSelected !== null;
  const canDuplicate = !typing && (mode === 'tracker' ? tracker !== null : patchSelected !== null);
  // With a track showing and nothing in it selected, Delete removes the track.
  const deletableTrack = mode === 'tracker' && selection === null && canRemoveTrack;
  const canDelete = selectedFx !== null || deletablePatch !== null || deletableTrack;

  // View > Zoom works on the material waveform, when sound scaping shows one.
  const waveZoomable = mode === 'soundscape' && shownWave !== null && !blank;
  const menuRef = useRef<(id: string) => void>(() => {});
  menuRef.current = (id) => {
    if (id.startsWith('fx-add:')) {
      addEffect(id.slice('fx-add:'.length));
      return;
    }
    const sel = useSelection.getState().selection;
    const node = sel?.kind === 'node' ? nodeById(sel.id) : null;
    switch (id) {
      case 'view-zoom-in':
      case 'view-zoom-out':
        if (waveZoomable) {
          setWaveView(
            stepZoom(
              waveView,
              id === 'view-zoom-in' ? 'in' : 'out',
              meter.playing && shownIsPlayer ? meter.playhead : null,
              waveMinSpan,
            ),
          );
        }
        break;
      case 'view-zoom-fit':
        if (waveZoomable) setWaveView(FIT);
        break;
      case 'app.settings':
        setSettingsOpen(true);
        break;
      case 'file-add-material':
        void pickFiles();
        break;
      case 'file-record-take':
        void toggleRecord();
        break;
      case 'file-arm-take':
        void toggleRecord(true);
        break;
      case 'file-extract-region':
        if (shownRecord?.path && selectedRegion !== null) {
          void extractSelected(shownRecord.id, selectedRegion);
        } else {
          setNote('Select a region in the waveform first (click its numbered tab), then Extract.');
        }
        break;
      case 'file-detect-regions':
        if (shownRecord) void findRegions(shownRecord.id, detectSensitivity);
        break;
      case 'file-new-patch':
        newPatchRow();
        break;
      case 'file-reset-sound':
        void resetSound()
          .then(async (changed) => {
            if (!changed) return;
            // Removing the chain's effects takes their links with them.
            setMod(await readModulation());
            setNote('Reset the sound to its defaults. Undo brings it back.');
          })
          .catch((e) => setError(String(e)));
        break;
      case 'edit.duplicate':
        if (mode === 'tracker') void editTrackerView(selectedTrack, { op: 'duplicate' });
        else if (patchSelected) void duplicatePatchRow(patchSelected.id);
        break;
      case 'edit.rename':
        if (patchSelected) setRenaming(patchSelected.id);
        break;
      case 'edit.delete':
        if (node?.fx) removeEffect(node);
        else if (deletablePatch) void deletePatchRow(deletablePatch.id);
        else if (deletableTrack) void removeTrackRow(selectedTrack);
        break;
      case 'track.add:melody':
        if (canAddMelody) void addTrackRow(false);
        break;
      case 'track.add:drums':
        if (canAddDrums) void addTrackRow(true);
        break;
      case 'track.remove':
        if (mode === 'tracker' && canRemoveTrack) void removeTrackRow(selectedTrack);
        break;
      case 'track.mute':
        if (mode === 'tracker') void toggleMute(selectedTrack);
        break;
      case 'track.solo':
        if (mode === 'tracker') void toggleSolo(selectedTrack);
        break;
      case 'fx-remove':
        if (node?.fx) removeEffect(node);
        break;
      case 'fx-earlier':
        if (node?.fx) moveEffect(node, -1);
        break;
      case 'fx-later':
        if (node?.fx) moveEffect(node, 1);
        break;
      case 'view-tracker':
        void selectTrackRow(selectedTrack);
        break;
      case 'view-soundscape': {
        const current = patches.find((p) => p.current);
        if (current) void selectPatchRow(current.id);
        else setMode('soundscape');
        break;
      }
      case 'view-inspector':
        setShowDebugger((v) => !v);
        break;
      default:
        break;
    }
  };
  const commandTable = useCommandTable();
  const selectedMute = meter.mix[selectedTrack]?.mute ?? false;
  const selectedSolo = meter.mix[selectedTrack]?.solo ?? false;
  // Repeat Bar copies a bar into the ones after it, so a one-bar pattern has
  // nowhere to repeat to.
  const canRepeatBar = mode === 'tracker' && (tracker?.tracks[selectedTrack]?.length ?? 0) > 16;
  const bindings = useMemo(() => {
    const enabled: Record<string, boolean> = {
      'edit.rename': canRename,
      'edit.duplicate': canDuplicate,
      'edit.delete': canDelete,
      'view-zoom-in': waveZoomable,
      'view-zoom-out': waveZoomable,
      'view-zoom-fit': waveZoomable,
      'track.add:melody': canAddMelody,
      'track.add:drums': canAddDrums,
      'track.remove': mode === 'tracker' && canRemoveTrack,
      'track.mute': mode === 'tracker',
      'track.solo': mode === 'tracker',
      'pattern-repeat': canRepeatBar,
    };
    // Checked as the selected track is pressed.
    const pressed: Record<string, boolean> = {
      'track.mute': mode === 'tracker' && selectedMute,
      'track.solo': mode === 'tracker' && selectedSolo,
    };
    return Object.fromEntries(
      (commandTable ?? []).map((c) => [
        c.id,
        {
          run: () => menuRef.current(c.id),
          enabled: enabled[c.id] ?? true,
          pressed: pressed[c.id],
        },
      ]),
    );
  }, [
    commandTable,
    canRename,
    canDuplicate,
    canDelete,
    canAddMelody,
    canAddDrums,
    canRemoveTrack,
    mode,
    selectedMute,
    selectedSolo,
    waveZoomable,
    canRepeatBar,
  ]);
  useCommands(bindings);

  // How many parameters follow each modulator, for the tree and the editors.
  const linkedCounts = new Map<number, number>();
  for (const link of Object.values(mod.links)) {
    linkedCounts.set(link.source, (linkedCounts.get(link.source) ?? 0) + 1);
  }
  // Every generator type and whether the patch holds it: Rust says, from the
  // nodes. A generator switched off is still in the patch, drawn off.
  const generators = NODES.filter(
    (n) => n.layer === 'patch' && n.lane === 'generate' && !n.setting,
  ).map((node) => ({ node, present: present.includes(node.id) }));
  const absent = new Set(generators.filter((g) => !g.present).map((g) => g.node.id));
  // The process lane's effects, in the order the chain runs them, and what
  // can still be added to it: a kind has two copies, and a chain holds sixteen.
  const chain = fxChain(values, level);
  const addChoices = (defs ? fxKinds(defs) : []).map((k) => ({
    kind: k.kind,
    label: k.label,
    disabled:
      chain.length >= FX_ORDER_LEN
        ? 'chain full'
        : chain.filter((n) => n.fx?.kind === k.kind).length >= 2
          ? 'both in use'
          : undefined,
  }));
  const selectedModulator =
    selection?.kind === 'lfo' || selection?.kind === 'envelope' ? selection : null;
  const refreshMod = () => void editMod(readModulation);

  // Links and what the engine heard reach the rows through the panel's
  // context, which facets hands every renderer without rebuilding the schema.
  const panelCtx: ParamRowContext = {
    defs: defs ?? [],
    mod,
    heard,
    midi,
    pool,
    onWire: wire,
    onLink: (id, source, lo, hi) => void editMod(() => linkParam(id, source, lo, hi)),
    onUnlink: (id) => void editMod(() => unlinkParam(id)),
    onLinkNew: (id, kind, lo, hi) => void editMod(() => linkNew(id, kind, lo, hi)),
    onLearn: (id) => void learnMidi(id).catch((e) => setError(String(e))),
    onMapMidi: (id, control) => void mapMidi(id, control).catch((e) => setError(String(e))),
    onForgetMidi: (id) => void forgetMidi(id).catch((e) => setError(String(e))),
  };

  return (
    <SnackbarProvider>
      <div className="flex h-screen flex-col bg-background text-foreground">
        <header className="flex items-center gap-3 border-b border-border px-4 py-2">
          <button
            type="button"
            onClick={togglePlay}
            className={`rounded px-3 py-1 text-xs font-medium ${
              meter.playing
                ? 'bg-primary text-primary-foreground'
                : 'border border-border hover:bg-accent'
            }`}
          >
            {meter.playing ? 'Stop' : 'Play'}
          </button>
          <fieldset aria-label="What Play plays" className="m-0 flex min-w-0 border-0 p-0">
            {PLAY_MODES.map(([value, label, hint], i) => (
              <button
                key={value}
                type="button"
                title={hint}
                aria-pressed={playMode === value}
                onClick={() => setPlayModeState(value)}
                className={`border border-border px-2 py-1 text-xs ${
                  i === 0 ? 'rounded-l' : '-ml-px'
                } ${i === PLAY_MODES.length - 1 ? 'rounded-r' : ''} ${
                  playMode === value
                    ? 'bg-accent font-medium text-accent-foreground'
                    : 'text-muted-foreground hover:bg-accent'
                }`}
              >
                {label}
              </button>
            ))}
          </fieldset>

          <button
            type="button"
            title={
              recArmed
                ? 'Armed: the take starts at the first sound. Press to disarm. (Opt+Cmd+R)'
                : 'Record a take from the source beside it. It joins Materials when you stop. Option-click arms it instead: it waits for the first sound. (Cmd+R, arm Opt+Cmd+R)'
            }
            onClick={(e) => void toggleRecord(e.altKey)}
            className={`rounded px-3 py-1 text-xs font-medium ${
              recArmed
                ? 'border border-destructive text-destructive'
                : recActive
                  ? 'bg-destructive text-white'
                  : 'border border-border hover:bg-accent'
            }`}
          >
            {recArmed ? 'Armed' : recActive ? 'Stop Recording' : 'Record'}
          </button>
          <Select
            items={{
              master: 'Master',
              ...Object.fromEntries(
                (tracker?.tracks ?? []).map((_, i) => [
                  String(i),
                  trackName(tracker?.tracks ?? [], i),
                ]),
              ),
            }}
            value={recSource === 'master' ? 'master' : String(recSource)}
            disabled={recActive}
            onValueChange={(v) => setRecSource(v === 'master' ? 'master' : Number(v))}
          >
            <SelectTrigger
              size="sm"
              aria-label="Record source"
              title="What to record: the master, or one track before its fader"
              className="h-6 w-24 px-1.5 text-xs"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="master" className="text-xs">
                Master
              </SelectItem>
              {(tracker?.tracks ?? []).map((_, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: a track is its place
                <SelectItem key={i} value={String(i)} className="text-xs">
                  {trackName(tracker?.tracks ?? [], i)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {rec?.active && (
            <span className="text-xs tabular-nums text-muted-foreground">
              {rec.armed
                ? 'waiting for sound'
                : `${formatElapsed(rec.elapsed_seconds)}${
                    rec.dropped > 0 ? ` · ${rec.dropped} dropped` : ''
                  }`}
            </span>
          )}

          <button
            type="button"
            title="Hold to press a finger against the reel — the tape slows, drops in pitch and garbles. Tape time sets how long it takes. (B)"
            onPointerDown={(e) => {
              // Capture, so letting go outside the button still releases the
              // brake. Without it the tape stays stopped and looks broken.
              e.currentTarget.setPointerCapture(e.pointerId);
              setBrake(true);
            }}
            onPointerUp={() => setBrake(false)}
            onPointerCancel={() => setBrake(false)}
            className={`rounded px-3 py-1 text-xs font-medium ${
              (values['arrangement.tape.brake'] ?? 0) > 0.01
                ? 'bg-primary text-primary-foreground'
                : 'border border-border hover:bg-accent'
            }`}
          >
            Brake
          </button>

          <button
            type="button"
            title="Tap for a flick backwards, hold to stay there. Shares the brake's slew, so it slows to a stop and climbs back the other way. Flick sets how long a tap lasts. (R)"
            onPointerDown={(e) => {
              e.currentTarget.setPointerCapture(e.pointerId);
              setReverse(true);
            }}
            onPointerUp={() => setReverse(false)}
            onPointerCancel={() => setReverse(false)}
            // Lit from the engine's resolved state, not from the button: a tap's
            // flick outlives the finger, and the light should say so.
            className={`rounded px-3 py-1 text-xs font-medium ${
              meter.reversing
                ? 'bg-primary text-primary-foreground'
                : 'border border-border hover:bg-accent'
            }`}
          >
            Rewind
          </button>

          <span className="text-sm font-semibold tracking-tight">Shard</span>
          <span className="text-xs text-muted-foreground">{document_?.title ?? 'Untitled-1'}</span>
        </header>

        <div className="flex min-h-0 flex-1">
          {/* The scene view: the tracks, the patches and the materials. Its
            selection decides what the middle shows. The current patch
            carries its generators and modulators. */}
          <SceneView
            tracks={tracker?.tracks ?? []}
            patches={patches}
            selectedTrack={mode === 'tracker' ? selectedTrack : null}
            selectedPatch={
              mode === 'soundscape' ? (patches.find((p) => p.current)?.id ?? null) : null
            }
            onSelectTrack={(i) => void selectTrackRow(i)}
            onPlays={(track, id) => void pickPlays(track, id)}
            onAddTrack={(drums) => void addTrackRow(drums)}
            onRemoveTrack={(i) => void removeTrackRow(i)}
            mix={meter.mix}
            onMute={(i) => void toggleMute(i)}
            onSolo={(i) => void toggleSolo(i)}
            onSelectPatch={(id) => void selectPatchRow(id)}
            onNewPatch={newPatchRow}
            renaming={renaming}
            onRenaming={setRenaming}
            onRename={(id, name) => void renamePatchRow(id, name)}
            onDuplicate={(id) => void duplicatePatchRow(id)}
            onDelete={(id) => void deletePatchRow(id)}
            patchChildren={
              <>
                <GeneratorTree
                  nested
                  generators={generators}
                  selected={selection?.kind === 'node' ? selection.id : null}
                  onSelect={(id) => {
                    if (id === null) return clear();
                    select({ kind: 'node', id });
                    setMode('soundscape');
                  }}
                  onAdd={(id) => {
                    void addGenerator(id)
                      .then(listGenerators)
                      .then(setPresent)
                      .catch((e) => setError(String(e)));
                    select({ kind: 'node', id });
                    setMode('soundscape');
                  }}
                  onRemove={(node) => {
                    void removeGenerator(node.id)
                      .then(listGenerators)
                      .then(setPresent)
                      .catch((e) => setError(String(e)));
                    const now = useSelection.getState().selection;
                    if (now?.kind === 'node' && now.id === node.id) clear();
                  }}
                />
                <ModulatorTree
                  nested
                  modulators={modulatorsOf(mod)}
                  selected={selectedModulator}
                  linked={linkedCounts}
                  onSelect={(next) => {
                    if (next === null) return clear();
                    select(next);
                    setMode('soundscape');
                  }}
                  onAdd={(kind) =>
                    void editMod(async () => {
                      setMode('soundscape');
                      const view = kind === 'lfo' ? await addLfo() : await addEnvelope();
                      // The new one is last; select it so it can be set up at once.
                      const list = kind === 'lfo' ? view.lfos : view.envelopes;
                      const added = list[list.length - 1];
                      if (added) select({ kind, id: added.id });
                      return view;
                    })
                  }
                  onRemove={(m) =>
                    void editMod(async () => {
                      const view =
                        m.kind === 'lfo' ? await removeLfo(m.id) : await removeEnvelope(m.id);
                      const now = useSelection.getState().selection;
                      if (now?.kind === m.kind && now.id === m.id) clear();
                      const unlinked = linkedCounts.get(m.id) ?? 0;
                      if (unlinked > 0) {
                        setNote(
                          `Removed ${m.name} and the ${unlinked} link${unlinked === 1 ? '' : 's'} that followed it. Undo brings them back.`,
                        );
                      }
                      return view;
                    })
                  }
                />
              </>
            }
            globals={
              <ModulatorTree
                title="Global"
                kinds={[{ kind: 'lfo', label: 'global LFO' }]}
                empty="No global LFOs yet. Add one with +, and any patch can follow it."
                modulators={globalsOf(mod)}
                selected={selectedModulator}
                linked={linkedCounts}
                onSelect={(next) => {
                  if (next === null) return clear();
                  select(next);
                  setMode('soundscape');
                }}
                onAdd={() =>
                  void editMod(async () => {
                    setMode('soundscape');
                    const view = await addGlobalLfo();
                    const added = view.globals[view.globals.length - 1];
                    if (added) select({ kind: 'lfo', id: added.id });
                    return view;
                  })
                }
                onRemove={(m) =>
                  void editMod(async () => {
                    const view = await removeLfo(m.id);
                    const now = useSelection.getState().selection;
                    if (now?.kind === 'lfo' && now.id === m.id) clear();
                    setNote(
                      `Removed ${m.name} and every link that followed it, in every patch. Undo brings them back.`,
                    );
                    return view;
                  })
                }
              />
            }
            materials={
              <MaterialTree
                pool={pool}
                selected={selection?.kind === 'material' ? selection.id : null}
                onSelect={(id) => select({ kind: 'material', id })}
                onAdd={() => void pickFiles()}
                onAddDrone={() => void editMaterials(addDrone)}
                onRemove={(id) =>
                  void editMaterials(async () => {
                    const view = await removeMaterial(id);
                    const now = useSelection.getState().selection;
                    if (now?.kind === 'material' && now.id === id) clear();
                    return view;
                  })
                }
              />
            }
          />

          <main className="flex min-w-0 flex-1 flex-col gap-4 p-4">
            {/* The waveform follows the selection. Its title selects the
              material shown, for its octave and trim. The playhead and the
              envelope are drawn only over Sample's material, and the grains
              only over Granular's. Sound scaping only (Georg, 2026-10-04):
              in the tracker it was mostly empty, and the room goes to the
              steps. Trim is still in the material's inspector. */}
            {mode === 'soundscape' && (
              <>
                <div className="-mb-2 flex items-baseline gap-2">
                  <button
                    type="button"
                    disabled={shownId === null}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => {
                      if (shownId === null) return;
                      // Editing lives in sound scaping, where the inspector is.
                      select({ kind: 'material', id: shownId });
                      setMode('soundscape');
                    }}
                    title="Show this material's octave and trim"
                    className={`text-[11px] font-semibold uppercase tracking-wider ${
                      selection?.kind === 'material' && selection.id === shownId
                        ? 'text-primary'
                        : 'text-muted-foreground hover:text-foreground'
                    }`}
                  >
                    Material
                  </button>
                  <span className="truncate text-xs text-muted-foreground">
                    {shownRecord
                      ? `${shownRecord.name}${shownWave ? ` · ${shownWave.seconds.toFixed(1)}s` : ''} · ${
                          blank
                            ? 'Sample is off'
                            : clockOnly
                              ? 'Sample is off · its trim still sets how long a pass is'
                              : readers.length > 0
                                ? `read by ${readers.join(' and ')}`
                                : 'not wired'
                        }`
                      : `Nothing is wired into ${shownFor}`}
                  </span>
                </div>
                <div className="-mb-2 flex items-center gap-2 text-[11px]">
                  <span className="font-semibold uppercase tracking-wider text-muted-foreground">
                    Scrub
                  </span>
                  {(['off', 'tape', 'freeze'] as const).map((m) => (
                    <button
                      key={m}
                      type="button"
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => {
                        setScrub(m);
                        // Tape is the Sample scrubbing; the other two leave it
                        // playing as it was.
                        const want = m === 'tape' ? 1 : 0;
                        if ((values['material.scrub'] ?? 0) !== want) {
                          void setParam('material.scrub', want).catch((e) => setError(String(e)));
                        }
                      }}
                      className={`rounded px-1.5 py-0.5 capitalize ${
                        scrub === m
                          ? 'bg-primary text-primary-foreground'
                          : 'text-muted-foreground hover:text-foreground'
                      }`}
                    >
                      {m}
                    </button>
                  ))}
                  {scrub === 'tape' && (
                    <span className="text-muted-foreground">
                      Drag: the sample follows your hand, pitch and all
                    </span>
                  )}
                  {scrub === 'freeze' && (
                    <span className={grainOn ? 'text-muted-foreground' : 'text-destructive'}>
                      {grainOn
                        ? 'Drag: the cloud holds where you are'
                        : 'Switch Granular on to hear the freeze'}
                    </span>
                  )}
                </div>
                <div className="-mb-2 flex items-center gap-2 text-[11px]">
                  <span className="font-semibold uppercase tracking-wider text-muted-foreground">
                    Regions
                  </span>
                  <button
                    type="button"
                    disabled={!shownRecord || blank}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() =>
                      shownRecord && void findRegions(shownRecord.id, detectSensitivity)
                    }
                    title="Mark a region from each transient to the next, replacing the ones here"
                    className="rounded px-1.5 py-0.5 text-muted-foreground hover:text-foreground disabled:opacity-40"
                  >
                    Detect
                  </button>
                  <input
                    type="range"
                    min={0}
                    max={1}
                    step={0.01}
                    value={detectSensitivity}
                    onChange={(e) => setDetectSensitivity(Number(e.target.value))}
                    aria-label="Detect sensitivity"
                    title={`Sensitivity ${Math.round(detectSensitivity * 100)}%: higher finds softer hits`}
                    className="h-1 w-20 accent-primary"
                  />
                  <span className="text-muted-foreground">
                    {shownRegions.length > 0
                      ? `${shownRegions.length} marked · click a numbered tab to select, drag its edge to resize`
                      : 'Shift-drag to mark one by hand'}
                  </span>
                  {selectedRegion !== null && (
                    <button
                      type="button"
                      disabled={!shownRecord?.path}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() =>
                        shownRecord && void extractSelected(shownRecord.id, selectedRegion)
                      }
                      title="Write region to a new WAV in Materials (Cmd+E)"
                      className="rounded px-1.5 py-0.5 text-muted-foreground hover:text-foreground disabled:opacity-40"
                    >
                      Extract {selectedRegion + 1}
                    </button>
                  )}
                </div>
                <div className={clockOnly ? 'opacity-40' : ''}>
                  <Waveform
                    peaks={blank ? [] : (shownWave?.peaks ?? [])}
                    view={waveView}
                    onView={setWaveView}
                    minSpan={waveMinSpan}
                    fetchPeaks={fetchWavePeaks}
                    fetchKey={shownId}
                    position={values['grain.position'] ?? 0}
                    jitter={values['grain.jitter'] ?? 0}
                    showGrain={shownIsGrain}
                    playhead={meter.playing && shownIsPlayer ? meter.playhead : null}
                    trimStart={shownRecord?.trim_start ?? 0}
                    trimEnd={shownRecord?.trim_end ?? 1}
                    envelope={shownIsPlayer ? envelope : null}
                    grains={shownIsGrain ? grains : []}
                    newestSeq={grains.length > 0 ? grains[grains.length - 1].seq : 0}
                    selected={picked}
                    totalSamples={
                      shownWave ? Math.round(shownWave.seconds * shownWave.sample_rate) : 0
                    }
                    // Stopped or frozen, the waveform is an inspector; playing, it is
                    // still the trim control it has always been.
                    inspecting={shownIsGrain && grains.length > 0 && (frozen || !meter.playing)}
                    onTrim={(which, v) => {
                      if (!shownRecord) return;
                      void changeMaterial(
                        which === 'start'
                          ? { ...shownRecord, trim_start: v }
                          : { ...shownRecord, trim_end: v },
                      );
                    }}
                    regions={blank ? [] : shownRegions}
                    selectedRegion={selectedRegion}
                    onAddRegion={(a, b) => shownRecord && void markRegion(shownRecord.id, a, b)}
                    onSetRegion={(i, a, b) =>
                      shownRecord && void moveRegion(shownRecord.id, i, a, b)
                    }
                    onSelectRegion={(i) =>
                      setRegionSel(
                        i === null || !shownRecord ? null : { id: shownRecord.id, index: i },
                      )
                    }
                    onRemoveRegion={() =>
                      shownRecord &&
                      selectedRegion !== null &&
                      void dropRegion(shownRecord.id, selectedRegion)
                    }
                    scrub={scrub}
                    scrubAt={
                      scrub === 'tape' && shownIsPlayer && shownRecord
                        ? Math.min(shownRecord.trim_start, shownRecord.trim_end) +
                          (heard['material.position'] ?? values['material.position'] ?? 0) *
                            Math.abs(shownRecord.trim_end - shownRecord.trim_start)
                        : null
                    }
                    onScrub={(t) => {
                      // The cloud follows the hand in both feels, so its
                      // grains move wherever you scrub (Georg, 2026-10-13).
                      if (scrub === 'tape') {
                        if (shownIsPlayer) {
                          // A press on Tape switches Scrub on, if it is off.
                          if ((values['material.scrub'] ?? 0) < 0.5) {
                            void setParam('material.scrub', 1).catch((e) => setError(String(e)));
                          }
                          void setParam('material.position', t).catch((e) => setError(String(e)));
                        }
                        if (shownIsGrain) {
                          void setParam('grain.position', t).catch((e) => setError(String(e)));
                        }
                      } else if (shownIsGrain) {
                        void setParam('grain.position', t).catch((e) => setError(String(e)));
                      }
                    }}
                    onPickGrain={(g) => {
                      if (g) void doAudition(g);
                    }}
                  />
                </div>
              </>
            )}

            {/* The patch only: the tracker's preview may be something else. */}
            {mode === 'soundscape' && <PatchPreview preview={preview} busy={previewBusy} />}

            <div className="flex items-center gap-6 text-xs text-muted-foreground">
              <Meter peak={meter.peak} reduction={meter.reduction} />
              <span title="Concurrent grains. Roughly density x grain length.">
                {meter.grains} grains
              </span>
              <span title="Density x grain length: how many grains overlap at these settings.">
                {((values['grain.density'] ?? 0) * (values['grain.size'] ?? 0) * 0.001).toFixed(1)}{' '}
                expected
              </span>
              <span
                title="The slowest audio block in the last two seconds, against the time the device allows for one. A block past its budget is a dropout."
                className={
                  meter.block_budget_us > 0 && blockWorst > meter.block_budget_us * 0.75
                    ? 'text-destructive'
                    : undefined
                }
              >
                {meter.block_budget_us > 0
                  ? `${(blockWorst / 1000).toFixed(2)} of ${(meter.block_budget_us / 1000).toFixed(1)} ms`
                  : '— ms'}
              </span>
              {meter.audio_allocs > 0 && (
                <span
                  className="text-destructive"
                  title="Allocator calls inside the audio callback since launch. Each can cause a dropout. Counted in debug builds only."
                >
                  {meter.audio_allocs} audio-thread allocations
                </span>
              )}
            </div>

            {mode === 'tracker' && tracker && (
              <StepStrip
                tracker={tracker}
                index={selectedTrack}
                step={meter.steps[selectedTrack] ?? -1}
                // The current patch is the selected track's.
                root={pool.materials.find((m) => m.id === pool.wires.material)?.root ?? null}
                patches={patches}
                onPlays={(id) => void pickPlays(selectedTrack, id)}
                onEdit={(edit) => void editTrackerView(selectedTrack, edit)}
                onChange={(next) => void changeTracker(next)}
              />
            )}

            {/* The work area. A click anywhere but on a card deselects,
              including a lane's empty space below its cards; Escape does the
              same from the keyboard. Sound scaping shows the patch's nodes;
              the tracker shows the arrangement's, the same three lanes with
              the patches first (Georg, 2026-09-26). Selecting a card shows
              it in the inspector in either mode. */}
            {defs && (
              // biome-ignore lint/a11y/noStaticElementInteractions: Escape deselects from the keyboard
              // biome-ignore lint/a11y/useKeyWithClickEvents: Escape deselects from the keyboard
              <div
                className="grid min-h-0 flex-1 grid-cols-3 content-start gap-4 overflow-y-auto"
                onClick={(e) => {
                  // Clicks from a menu's portal bubble here through React; only the work area's own count.
                  if (!e.currentTarget.contains(e.target as Node)) return;
                  if (!(e.target as Element).closest('[data-node-card]')) clear();
                }}
              >
                {(mode === 'tracker' ? ARRANGEMENT_LANES : LANES).map((lane) => (
                  <section key={lane.id} className="flex min-w-0 flex-col gap-2">
                    <div className="flex items-center justify-between">
                      <h2 className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                        {lane.label}
                      </h2>
                      {lane.id === 'process' && (
                        <AddMenu title="Add an effect" choices={addChoices} onAdd={addEffect} />
                      )}
                    </div>
                    {/* The tracker's Patches lane: every track's fader first,
                      a melody track labelled by the patch it plays, a drums
                      track with its kit's switch, Tune and Decay. */}
                    {mode === 'tracker' &&
                      lane.id === 'generate' &&
                      tracker?.tracks.map((t, i) => (
                        <TrackFader
                          // biome-ignore lint/suspicious/noArrayIndexKey: a track is its place
                          key={i}
                          name={trackName(tracker.tracks, i)}
                          detail={patches.find((p) => p.id === t.patch)?.name}
                          kit={t.drums}
                          onKit={(drums) =>
                            void changeTracker({
                              ...tracker,
                              tracks: tracker.tracks.map((x, j) => (j === i ? { ...x, drums } : x)),
                            })
                          }
                          gain={t.gain}
                          duck={t.duck}
                          keys={tracker.tracks
                            .map((_, j) => ({ place: j, name: trackName(tracker.tracks, j) }))
                            .filter((k) => k.place !== i)}
                          onDuckedBy={(key) => void pickDuckedBy(i, key)}
                          onDuck={(duck) =>
                            void changeTracker({
                              ...tracker,
                              tracks: tracker.tracks.map((x, j) => (j === i ? { ...x, duck } : x)),
                            })
                          }
                          selected={selectedTrack === i && selection === null}
                          mix={meter.mix[i]}
                          onMute={() => void toggleMute(i)}
                          onSolo={() => void toggleSolo(i)}
                          onSelect={() => void selectTrackRow(i)}
                          onGain={(gain) =>
                            void changeTracker({
                              ...tracker,
                              tracks: tracker.tracks.map((x, j) => (j === i ? { ...x, gain } : x)),
                            })
                          }
                        />
                      ))}
                    {[
                      // The chain first, in its order; then the sections, drawn
                      // at their defaults until a write makes them.
                      ...(lane.id === 'process' ? chain : []),
                      ...(mode === 'tracker' ? ARRANGEMENT_NODES : NODES)
                        .filter((n: NodeInfo) => n.lane === lane.id && !n.fx)
                        // A generator the patch lacks has no card; the tree adds it.
                        .filter((n: NodeInfo) => mode === 'tracker' || !absent.has(n.id)),
                    ].map((node) => (
                      <NodeCard
                        key={node.id}
                        node={node}
                        defs={defs}
                        values={values}
                        ctx={panelCtx}
                        selected={selection?.kind === 'node' && selection.id === node.id}
                        onSelect={() => select({ kind: 'node', id: node.id })}
                        onError={setError}
                        onNote={setNote}
                        onPresetApplied={refreshMod}
                        onMove={node.fx ? (by) => moveEffect(node, by) : undefined}
                        onRemove={node.fx ? () => removeEffect(node) : undefined}
                      />
                    ))}
                  </section>
                ))}
              </div>
            )}

            {showDebugger && (
              <GrainInspector
                grains={grains}
                frozen={frozen}
                selected={picked}
                playing={meter.playing}
                auditioning={meter.auditioning}
                sampleRate={shownWave?.sample_rate ?? 48000}
                windowNames={defs?.find((d) => d.id === 'grain.window')?.step_names ?? []}
                onFreeze={setFrozen}
                onClear={() => {
                  setGrains([]);
                  setPicked(null);
                }}
                onSelect={(g) => void doAudition(g)}
              />
            )}
          </main>

          <aside className="w-80 shrink-0 overflow-y-auto border-l border-border">
            {defs ? (
              <Inspector
                selection={selection}
                defs={defs}
                values={values}
                ctx={panelCtx}
                pool={pool}
                onMaterialChange={(m) => void changeMaterial(m)}
                limits={limits}
                linkedCounts={linkedCounts}
                onSelect={select}
                onLfoChange={(next) => void editMod(() => setLfo(next))}
                onEnvelopeChange={(next, coalesce) =>
                  void editMod(() => setModEnvelope(next, coalesce))
                }
                envelopeGesture={{
                  begin: (id) => void envelopeGestureBegin(id).catch((e) => setError(String(e))),
                  apply: (next) => void editMod(() => envelopeGestureApply(next)),
                  end: () => void envelopeGestureEnd().catch((e) => setError(String(e))),
                  cancel: () => void editMod(envelopeGestureCancel),
                }}
                onError={setError}
                onNote={setNote}
                onPresetApplied={refreshMod}
              />
            ) : (
              <p className="p-3 text-xs text-muted-foreground">Loading parameters…</p>
            )}

            {defs && showDebugger && (
              <div className="m-3 mt-4 space-y-1 border-t border-border pt-3">
                {defs.map((d) => (
                  <div
                    key={d.id}
                    className="flex justify-between gap-2 text-[11px] text-muted-foreground"
                  >
                    <span className="truncate font-mono">{d.id}</span>
                    <span className="shrink-0 tabular-nums">
                      {format(d, values[d.id] ?? d.default)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </aside>
        </div>

        <SettingsDialog
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          values={values}
          ctx={panelCtx}
          midi={midi}
          onError={setError}
        />
      </div>
      <SnackbarViewport className="fixed right-3 bottom-3 left-auto" />
    </SnackbarProvider>
  );
}

/** What just happened, or what went wrong, as the kit's snackbar: passing, so the layout never
 * shifts. A new message replaces the last of its kind, and `null` takes it away. Errors stay
 * until dismissed. Module-level: they hold no React state. */
function message(kind: 'plain' | 'error') {
  let last: string | null = null;
  return (text: string | null) => {
    if (last) dismissNotification(last);
    last = text ? notify({ message: text, kind, persistent: kind === 'error' }) : null;
  };
}
const setNote = message('plain');
const setError = message('error');
