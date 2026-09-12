import { PropertyPanel } from '@preset.nz/facets';
import { open, save } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  auditionGrain,
  envelopeCurve,
  format,
  type GrainInfo,
  getParams,
  grainLog,
  loadPatch,
  loadSample,
  type Meters,
  type ParamInfo,
  paramDefs,
  meters as readMeters,
  type SourceInfo,
  savePatch,
  setDrift,
  setParam,
  setParamDrift,
  setPlaying,
  sourceInfo,
} from '@/audio';
import { GrainInspector } from '@/components/GrainInspector';
import { Meter } from '@/components/Meter';
import { Waveform } from '@/components/Waveform';
import { type ParamValues, registerParamScope, SCOPE_KEY } from '@/scope';

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

export default function App() {
  const [defs, setDefs] = useState<ParamInfo[] | null>(null);
  const [values, setValues] = useState<ParamValues>({});
  const [source, setSource] = useState<SourceInfo | null>(null);
  const [meter, setMeter] = useState<Meters>({
    peak: 0,
    grains: 0,
    drift: true,
    drifting: [],
    playing: false,
    playhead: 0,
    auditioning: false,
    reversing: false,
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
  // Re-registering the scope is how the drift flags reach the field
  // definitions, since facets reads them from the schema rather than from the
  // values. Keyed on the flag pattern so it only happens when one flips.
  const driftKey = useRef('');
  const [error, setError] = useState<string | null>(null);
  const defsRef = useRef<ParamInfo[] | null>(null);
  const [schemaVersion, setSchemaVersion] = useState(0);
  const [envelope, setEnvelope] = useState<number[] | null>(null);
  const [patchName, setPatchName] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  // Startup: ask Rust for the table, register the facets scope from it, then
  // read the current values. The table is the single source of truth.
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const d = await paramDefs();
        if (cancelled) return;
        const m = await readMeters();
        const on = new Set(d.filter((_, i) => m.drifting[i]).map((x) => x.id));
        driftKey.current = d.map((_, i) => (m.drifting[i] ? '1' : '0')).join('');
        registerParamScope(d, on, (id, next) => {
          void setParamDrift(id, next);
        });
        defsRef.current = d;
        setDefs(d);
        const v = await getParams();
        const next: ParamValues = {};
        d.forEach((p, i) => {
          next[p.id] = v[i];
        });
        setValues(next);
        setSource(await sourceInfo());
      } catch (e) {
        setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Poll. Thirty hertz is enough for a meter and for watching the drift move
  // the controls, and it keeps the boundary quiet. Nothing here is on the
  // audio path, so a late frame costs nothing but a slightly stale number.
  useEffect(() => {
    if (!defs) return;
    let alive = true;
    const tick = async () => {
      if (!alive) return;
      try {
        const [m, v, fresh] = await Promise.all([readMeters(), getParams(), grainLog()]);
        if (!alive) return;
        setMeter(m);
        // Drain regardless of the freeze, or resuming would dump a backlog of
        // everything that happened while it was frozen. Frozen means "stop
        // adding to what I am looking at", not "stop the engine logging".
        if (!frozenRef.current && fresh.length > 0) {
          setGrains((prev) => [...prev, ...fresh].slice(-GRAIN_SCROLLBACK));
        }
        const d = defsRef.current;
        if (d) {
          const key = m.drifting.map((b) => (b ? '1' : '0')).join('');
          if (key !== driftKey.current) {
            driftKey.current = key;
            const on = new Set(d.filter((_, i) => m.drifting[i]).map((x) => x.id));
            registerParamScope(d, on, (id, next) => {
              void setParamDrift(id, next);
            });
            setSchemaVersion((n) => n + 1);
          }
          const next: ParamValues = {};
          d.forEach((p, i) => {
            next[p.id] = v[i];
          });
          setValues(next);
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

  // Refetch the curve only when an envelope or trim control actually moves.
  // Polling it thirty times a second would be free but pointless; this way
  // the drawn curve is the one Rust computes, not a copy of the maths.
  const envKey = [
    values['env.amount'],
    values['env.attack'],
    values['env.decay'],
    values['env.sustain'],
    values['env.release'],
    values['trim.start'],
    values['trim.end'],
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

  const doSave = useCallback(async () => {
    try {
      const path = await save({
        defaultPath: patchName ?? 'untitled.shard',
        filters: [{ name: 'Shard patch', extensions: ['shard'] }],
      });
      if (!path) return;
      await savePatch(path);
      setPatchName(path.split('/').pop() ?? path);
      setNote(null);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [patchName]);

  const doLoad = useCallback(async () => {
    try {
      const path = await open({
        multiple: false,
        filters: [{ name: 'Shard patch', extensions: ['shard'] }],
      });
      if (typeof path !== 'string') return;
      const report = await loadPatch(path);
      setPatchName(path.split('/').pop() ?? path);
      setSource(await sourceInfo());
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
      setNote(parts.length > 0 ? parts.join(' · ') : null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const pickFile = useCallback(async () => {
    try {
      const picked = await open({
        multiple: false,
        filters: [{ name: 'Audio', extensions: ['wav'] }],
      });
      if (typeof picked !== 'string') return;
      setSource(await loadSample(picked));
      // A trim from the previous sample means nothing against a new one.
      await setParam('trim.start', 0);
      await setParam('trim.end', 1);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const toggleDrift = useCallback(async () => {
    await setDrift(!meter.drift);
  }, [meter.drift]);

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
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT|BUTTON)$/.test(t.tagName)) return;

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
  }, [togglePlay, doSave, doLoad, setBrake, setReverse]);

  return (
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
        <span className="text-sm font-semibold tracking-tight">Shard</span>
        <span className="text-xs text-muted-foreground">
          {patchName ? `${patchName} — ` : ''}
          {source ? `${source.name} · ${source.seconds.toFixed(1)}s` : 'loading'}
        </span>
        <div className="flex-1" />
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
          onClick={pickFile}
          className="rounded border border-border px-2 py-1 text-xs hover:bg-accent"
        >
          Load WAV
        </button>
        <button
          type="button"
          onClick={toggleDrift}
          className={`rounded border px-2 py-1 text-xs ${
            meter.drift
              ? 'border-primary bg-primary/15 text-primary'
              : 'border-border hover:bg-accent'
          }`}
        >
          Drift {meter.drift ? 'on' : 'off'}
        </button>
      </header>

      {note && !error && (
        <div className="border-b border-border bg-muted/40 px-4 py-2 text-xs text-muted-foreground">
          {note}
        </div>
      )}

      {error && (
        <div className="border-b border-destructive/40 bg-destructive/10 px-4 py-2 text-xs text-destructive">
          {error}
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <main className="flex min-w-0 flex-1 flex-col gap-4 p-4">
          <Waveform
            peaks={source?.peaks ?? []}
            position={values['grain.position'] ?? 0}
            jitter={values['grain.jitter'] ?? 0}
            playhead={meter.playing ? meter.playhead : null}
            trimStart={values['trim.start'] ?? 0}
            trimEnd={values['trim.end'] ?? 1}
            envelope={envelope}
            grains={grains}
            newestSeq={grains.length > 0 ? grains[grains.length - 1].seq : 0}
            selected={picked}
            totalSamples={source ? Math.round(source.seconds * source.sample_rate) : 0}
            // Stopped or frozen, the waveform is an inspector; playing, it is
            // still the trim control it has always been.
            inspecting={grains.length > 0 && (frozen || !meter.playing)}
            onTrim={(which, v) => {
              void setParam(`trim.${which}`, v);
            }}
            onPickGrain={(g) => {
              if (g) void doAudition(g);
            }}
          />

          <div className="flex items-center gap-6 text-xs text-muted-foreground">
            <Meter peak={meter.peak} />
            <span title="Concurrent grains. Roughly density x grain length.">
              {meter.grains} grains
            </span>
            <span title="Density x grain length: how many grains overlap at these settings.">
              {((values['grain.density'] ?? 0) * (values['grain.size'] ?? 0) * 0.001).toFixed(1)}{' '}
              expected
            </span>
          </div>

          <p className="max-w-prose text-xs leading-relaxed text-muted-foreground">
            Drift walks position, size, density, jitter and ring frequency on slow oscillators that
            never quite line up, so the texture keeps moving on its own. Those five controls are
            held while it runs. Everything else is yours either way.
          </p>

          <GrainInspector
            grains={grains}
            frozen={frozen}
            selected={picked}
            playing={meter.playing}
            auditioning={meter.auditioning}
            sampleRate={source?.sample_rate ?? 48000}
            onFreeze={setFrozen}
            onClear={() => {
              setGrains([]);
              setPicked(null);
            }}
            onSelect={(g) => void doAudition(g)}
          />
        </main>

        <aside className="w-80 shrink-0 overflow-y-auto border-l border-border p-3">
          {defs ? (
            <PropertyPanel
              key={schemaVersion}
              scopeKey={SCOPE_KEY}
              selection={values}
              ctx={{ defs }}
              emptyState={<p className="text-xs">No parameters.</p>}
            />
          ) : (
            <p className="text-xs text-muted-foreground">Loading parameters…</p>
          )}

          {defs && (
            <div className="mt-4 space-y-1 border-t border-border pt-3">
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
    </div>
  );
}
