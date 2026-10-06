import { isTextField, redo, undo, useCommands, useCommandTable } from '@preset.nz/app-kit/core';
import { dismissNotification, notify, SnackbarProvider, SnackbarViewport } from '@preset.nz/ux-kit';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ARRANGEMENT_PREFIX,
  addDrone,
  addEnvelope,
  addLfo,
  addMaterial,
  arrangementDefs,
  auditionGrain,
  type ChainLevel,
  cancelLearn,
  EMPTY_MAPPINGS,
  editTracker,
  envelopeCurve,
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
  grainLog,
  type LfoLimits,
  learnMidi,
  lfoLimits,
  linkParam,
  loadPatch,
  type MappingsView,
  type MaterialsView,
  type MaterialView,
  type Meters,
  type ModulationView,
  mapMidi,
  materialWave,
  modulatorsOf,
  type ParamInfo,
  type PatchPreview as PatchPreviewData,
  paramDefs,
  patchPreview,
  mappings as readMappings,
  materials as readMaterials,
  meters as readMeters,
  modulation as readModulation,
  removeEnvelope,
  removeLfo,
  removeMaterial,
  resetSound,
  type SourceInfo,
  savePatch,
  setLfo,
  setMaterial,
  setEnvelope as setModEnvelope,
  setParam,
  setPlaying,
  setTracker,
  type Tracker,
  type TrackerEdit,
  takeOpenedFile,
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
import { isNodeOn, NodeCard } from '@/components/NodeCard';
import type { ParamRowContext } from '@/components/ParamRow';
import { PatchPreview } from '@/components/PatchPreview';
import { SettingsDialog } from '@/components/SettingsDialog';
import { StepStrip } from '@/components/StepStrip';
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
} from '@/scope';
import { useSelection } from '@/stores/selection';

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
 * The two ways of working (Georg, 2026-09-14). They change what plays as well
 * as what shows: tracker plays the steps, and sound scaping loops the patch
 * freely so it can be heard the whole time it is shaped. Remembered across
 * launches as UI state, and written to the track's switch, which stays the one
 * thing that decides whether steps play.
 */
const MODES = [
  {
    id: 'tracker',
    label: 'Tracker',
    title:
      'Play the steps, through the arrangement: the patch at its fader, and the arrangement’s own effects and output over it. (⌘1)',
  },
  {
    id: 'soundscape',
    label: 'Sound scaping',
    title:
      'Loop the patch freely and shape it, heard alone without the arrangement. The steps rest. (⌘2)',
  },
] as const;
type Mode = (typeof MODES)[number]['id'];

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
    step: -1,
    kit_step: -1,
    playhead: 0,
    auditioning: false,
    reversing: false,
    block_us: 0,
    block_budget_us: 0,
    audio_allocs: 0,
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
  const [patchName, setPatchName] = useState<string | null>(null);
  /** The open patch's LFOs and links, as Rust last answered with them. */
  const [mod, setMod] = useState<ModulationView>({
    lfos: [],
    envelopes: [],
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
  const editTrackerView = useCallback(async (edit: TrackerEdit) => {
    try {
      setTrackerView(await editTracker(edit));
    } catch (e) {
      setError(String(e));
    }
  }, []);
  const changeTracker = useCallback(async (next: Tracker) => {
    setTrackerView(next);
    const seq = ++trackerSeq.current;
    try {
      const played = await setTracker(next);
      // Only the newest answer, or a slow one would undo a quicker click.
      if (seq === trackerSeq.current) setTrackerView(played);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // The mode decides whether the steps play, so the track's switch follows
  // it: on launch, on every switch, and after a document brings its own.
  useEffect(() => {
    const track = tracker?.tracks[0];
    const want = mode === 'tracker';
    if (!tracker || !track || track.on === want) return;
    void changeTracker({
      ...tracker,
      tracks: [{ ...track, on: want }, ...tracker.tracks.slice(1)],
    });
  }, [mode, tracker, changeTracker]);

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
        const [pd, ad] = await Promise.all([paramDefs(), arrangementDefs()]);
        if (cancelled) return;
        const d = [...pd, ...ad];
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
        setTrackerView(await getTracker());
      } catch (e) {
        setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Poll. Thirty hertz is enough for a meter and for the controls to follow a
  // patch load or a preset, and it keeps the boundary quiet. Nothing here is on the
  // audio path, so a late frame costs nothing but a slightly stale number.
  useEffect(() => {
    if (!defs) return;
    let alive = true;
    const tick = async () => {
      if (!alive) return;
      try {
        const [m, v, fresh, h, mm, a] = await Promise.all([
          readMeters(),
          getParams(),
          grainLog(),
          getHeard(),
          readMappings(),
          getArrangement(),
        ]);
        if (!alive) return;
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
   * and it winds back up. It writes `tape.brake` through `setParam` like every
   * other control, which is the point — when this becomes a MIDI CC or a pedal
   * it takes exactly the same path, and a continuous value already means
   * something (half a brake is a tape running slow and flat).
   */
  const setBrake = useCallback((on: boolean) => {
    void setParam('tape.brake', on ? 1 : 0);
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
    void setParam('tape.reverse', on ? 1 : 0);
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

  // The document open when the app last closed, as a path. UI memory, not
  // the document: reopened at launch if the file is still there
  // (`native-apps.md` rule 4).
  const [lastDocument, setLastDocument] = usePersistedState<string | null>(
    'shard.lastDocument',
    null,
  );

  const doSave = useCallback(async () => {
    try {
      const path = await save({
        defaultPath: patchName ?? 'untitled.shard',
        filters: [{ name: 'Shard patch', extensions: ['shard'] }],
      });
      if (!path) return;
      await savePatch(path);
      setLastDocument(path);
      setPatchName(path.split('/').pop() ?? path);
      setNote(null);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [patchName, setLastDocument]);

  /**
   * Replace the document with the one at `path`. File > Open and a file
   * opened from Finder both come through here.
   */
  const openPath = useCallback(
    async (path: string): Promise<boolean> => {
      try {
        const report = await loadPatch(path);
        setLastDocument(path);
        setPatchName(path.split('/').pop() ?? path);
        setWaves({});
        setPool(await readMaterials());
        setMod(await readModulation());
        // The document's tracker replaces the one on screen, and no answer to
        // an edit made before the load may put the old one back.
        trackerSeq.current++;
        const loaded = await getTracker();
        setTrackerView(loaded);
        // The document's switch picks the mode it opens in. Correcting the
        // switch to the mode instead would overwrite what the file saved, and
        // the next Save would keep the damage.
        setMode(loaded.tracks[0]?.on ? 'tracker' : 'soundscape');
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
        if (report.missing.length > 0) {
          parts.push(`${report.missing.length} left at default`);
        }
        if (report.refused.length > 0) {
          parts.push(`${report.refused.length} LFO or link(s) could not be used`);
        }
        if (report.map_missing) {
          parts.push(`controller map "${report.map_missing}" not on this Mac`);
        }
        setNote(parts.length > 0 ? parts.join(' · ') : null);
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      }
    },
    [setMode, setLastDocument],
  );

  const doLoad = useCallback(async () => {
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: 'Shard patch', extensions: ['shard'] }],
      });
      if (typeof path === 'string') await openPath(path);
    } catch (e) {
      setError(String(e));
    }
  }, [openPath]);

  // A document opened from Finder or `open some.shard`. Listen first, then
  // ask Rust for one that arrived before the window had mounted: asking is
  // what tells Rust to emit from now on, so nothing falls between the two.
  // A file opened this way wins over anything restored at launch, so any
  // restore belongs after the answer, and only when it is empty.
  const openPathRef = useRef(openPath);
  openPathRef.current = openPath;
  const lastDocumentRef = useRef(lastDocument);
  lastDocumentRef.current = lastDocument;
  useEffect(() => {
    const unlisten = listen<string>('open-document', (e) => {
      void openPathRef.current(e.payload);
    });
    // Not gated on unmount: `take` hands a held path over exactly once, so
    // dropping it here would lose it for good.
    void unlisten.then(() =>
      takeOpenedFile().then(async (path) => {
        if (path) {
          void openPathRef.current(path);
          return;
        }
        // Nothing was opened from Finder: reopen the last document, and say
        // so if it has gone rather than opening on an empty patch in silence.
        const last = lastDocumentRef.current;
        if (last && !(await openPathRef.current(last))) {
          setLastDocument(null);
          setNote(`${last.split('/').pop() ?? last} was open last time and could not be reopened.`);
        }
      }),
    );
    return () => {
      void unlisten.then((off) => off());
    };
    // The setter is stable (its key never changes), so this runs once.
  }, [setLastDocument]);

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
      if (!node.fx) return;
      fxMove(level, node.fx.n, by).catch((e) => setError(String(e)));
    },
    [level],
  );

  // Undo and redo. Rust steps the document back; the values arrive with the
  // next poll, and the tracker and the modulation are asked for again here.
  // Undo and Redo are app-kit's: the Edit menu, its shortcut and the
  // fallback below all reach Rust's history, which says what it stepped.
  // Materials, modulation and the tracker are not polled, so read them again.
  useEffect(() => {
    const unlisten = listen<{ undid: boolean; label: string }>('history-stepped', async (e) => {
      try {
        setTrackerView(await getTracker());
        setMod(await readModulation());
        setPool(await readMaterials());
        setNote(`${e.payload.undid ? 'Undid' : 'Redid'} “${e.payload.label}”.`);
      } catch (err) {
        setError(String(err));
      }
    });
    return () => {
      void unlisten.then((off) => off());
    };
  }, []);

  const togglePlay = useCallback(async () => {
    await setPlaying(!meter.playing);
  }, [meter.playing]);

  // Space for play/stop, the way every other audio tool does it. Ignored while
  // a control has focus, so arrow keys on a slider still work.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault();
        void doSave();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'o') {
        e.preventDefault();
        void doLoad();
        return;
      }
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
        setMode(e.key === '1' ? 'tracker' : 'soundscape');
        return;
      }
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT|BUTTON)$/.test(t.tagName)) return;

      // The selected effect: Alt+Up and Alt+Down move it in its chain, Delete
      // takes it out. Anything else selected ignores these.
      const moving = e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown');
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
      if (e.code !== 'Space') return;
      e.preventDefault();
      void togglePlay();
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code === 'KeyB') setBrake(false);
      if (e.code === 'KeyR') setReverse(false);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [
    togglePlay,
    doSave,
    doLoad,
    setBrake,
    setReverse,
    setShowDebugger,
    setMode,
    moveEffect,
    removeEffect,
  ]);

  // The native menu bar: app-kit's, with Shard's commands (`menu.rs`). Each
  // item arrives as app-kit's `command` event carrying its id and runs the
  // same function the keyboard fallback above calls. Read through a ref, so
  // the bindings are made once. The tracker's Pattern items are the strip's
  // own. Undo and Redo, in a text field too, are app-kit's.
  const menuRef = useRef<(id: string) => void>(() => {});
  menuRef.current = (id) => {
    if (id.startsWith('fx-add:')) {
      addEffect(id.slice('fx-add:'.length));
      return;
    }
    const sel = useSelection.getState().selection;
    const node = sel?.kind === 'node' ? nodeById(sel.id) : null;
    switch (id) {
      case 'app.settings':
        setSettingsOpen(true);
        break;
      case 'file-open':
        void doLoad();
        break;
      case 'file-save':
        void doSave();
        break;
      case 'file-add-material':
        void pickFiles();
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
        setMode('tracker');
        break;
      case 'view-soundscape':
        setMode('soundscape');
        break;
      case 'view-inspector':
        setShowDebugger((v) => !v);
        break;
      default:
        break;
    }
  };
  const commandTable = useCommandTable();
  const bindings = useMemo(
    () =>
      Object.fromEntries(
        (commandTable ?? []).map((c) => [c.id, { run: () => menuRef.current(c.id) }]),
      ),
    [commandTable],
  );
  useCommands(bindings);

  // How many parameters follow each modulator, for the tree and the editors.
  const linkedCounts = new Map<number, number>();
  for (const link of Object.values(mod.links)) {
    linkedCounts.set(link.source, (linkedCounts.get(link.source) ?? 0) + 1);
  }
  // Every generator type and whether it is in the patch: on means present.
  const generators = NODES.filter(
    (n) => n.layer === 'patch' && n.lane === 'generate' && !n.setting,
  ).map((node) => ({ node, on: defs ? isNodeOn(defs, values, node) : true }));
  const absent = new Set(generators.filter((g) => !g.on).map((g) => g.node.id));
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
              (values['tape.brake'] ?? 0) > 0.01
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
            Reverse
          </button>

          <fieldset className="flex rounded border border-border p-0.5" aria-label="Mode">
            {MODES.map((m) => (
              <button
                key={m.id}
                type="button"
                aria-pressed={mode === m.id}
                title={m.title}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => setMode(m.id)}
                className={`rounded-sm px-2 py-0.5 text-xs ${
                  mode === m.id
                    ? 'bg-primary/15 text-primary'
                    : 'text-muted-foreground hover:text-foreground'
                }`}
              >
                {m.label}
              </button>
            ))}
          </fieldset>
          <span className="text-sm font-semibold tracking-tight">Shard</span>
          <span className="text-xs text-muted-foreground">{patchName ?? 'Untitled'}</span>
          <div className="flex-1" />
          <button
            type="button"
            title="Show or hide the grain inspector and the raw parameter list (⌘I)"
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => setShowDebugger((v) => !v)}
            className={`rounded border px-2 py-1 text-xs ${
              showDebugger
                ? 'border-primary bg-primary/15 text-primary'
                : 'border-border hover:bg-accent'
            }`}
          >
            Debug
          </button>
          <button
            type="button"
            onClick={doLoad}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            Open
          </button>
          <button
            type="button"
            onClick={doSave}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            Save
          </button>
          <button
            type="button"
            onClick={pickFiles}
            className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
          >
            Add WAV
          </button>
        </header>

        <div className="flex min-h-0 flex-1">
          {mode === 'soundscape' && (
            <nav className="w-48 shrink-0 overflow-y-auto border-r border-border">
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
              <GeneratorTree
                generators={generators}
                selected={selection?.kind === 'node' ? selection.id : null}
                onSelect={(id) => (id === null ? clear() : select({ kind: 'node', id }))}
                onAdd={(id) => {
                  void setParam(`${id}.on`, 1).catch((e) => setError(String(e)));
                  select({ kind: 'node', id });
                }}
                onRemove={(node) => {
                  void setParam(`${node.id}.on`, 0).catch((e) => setError(String(e)));
                  const now = useSelection.getState().selection;
                  if (now?.kind === 'node' && now.id === node.id) clear();
                }}
              />
              <ModulatorTree
                modulators={modulatorsOf(mod)}
                selected={selectedModulator}
                linked={linkedCounts}
                onSelect={(next) => (next === null ? clear() : select(next))}
                onAdd={(kind) =>
                  void editMod(async () => {
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
                    const orphaned = linkedCounts.get(m.id) ?? 0;
                    if (orphaned > 0) {
                      setNote(
                        `Removed ${m.name}. ${orphaned} parameter${orphaned === 1 ? '' : 's'} still link to it and stay at their own values until unlinked.`,
                      );
                    }
                    return view;
                  })
                }
              />
            </nav>
          )}

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
                <div className={clockOnly ? 'opacity-40' : ''}>
                  <Waveform
                    peaks={blank ? [] : (shownWave?.peaks ?? [])}
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
                step={meter.step}
                kitStep={meter.kit_step}
                root={pool.materials.find((m) => m.id === pool.wires.material)?.root ?? null}
                onEdit={(edit) => void editTrackerView(edit)}
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
                    {[
                      // The chain first, in its order; then the fixed nodes.
                      ...(lane.id === 'process' ? chain : []),
                      ...(mode === 'tracker' ? ARRANGEMENT_NODES : NODES)
                        .filter((n: NodeInfo) => n.lane === lane.id && !n.fx)
                        // A generator that is off is not in the patch; the tree adds it.
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
                onEnvelopeChange={(next) => void editMod(() => setModEnvelope(next))}
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
