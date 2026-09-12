import { PropertyPanel } from '@preset.nz/facets';
import { open } from '@tauri-apps/plugin-dialog';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  envelopeCurve,
  format,
  getParams,
  loadSample,
  type Meters,
  type ParamInfo,
  paramDefs,
  meters as readMeters,
  type SourceInfo,
  setDrift,
  setParam,
  setParamDrift,
  setPlaying,
  sourceInfo,
} from '@/audio';
import { Meter } from '@/components/Meter';
import { Waveform } from '@/components/Waveform';
import { type ParamValues, registerParamScope, SCOPE_KEY } from '@/scope';

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
  });
  // Re-registering the scope is how the drift flags reach the field
  // definitions, since facets reads them from the schema rather than from the
  // values. Keyed on the flag pattern so it only happens when one flips.
  const driftKey = useRef('');
  const [error, setError] = useState<string | null>(null);
  const defsRef = useRef<ParamInfo[] | null>(null);
  const [schemaVersion, setSchemaVersion] = useState(0);
  const [envelope, setEnvelope] = useState<number[] | null>(null);

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
        const [m, v] = await Promise.all([readMeters(), getParams()]);
        if (!alive) return;
        setMeter(m);
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
      if (e.code !== 'Space') return;
      const t = e.target as HTMLElement | null;
      if (t && /^(INPUT|TEXTAREA|SELECT|BUTTON)$/.test(t.tagName)) return;
      e.preventDefault();
      void togglePlay();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [togglePlay]);

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
        <span className="text-sm font-semibold tracking-tight">Shard</span>
        <span className="text-xs text-muted-foreground">
          {source ? `${source.name} · ${source.seconds.toFixed(1)}s` : 'loading'}
        </span>
        <div className="flex-1" />
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
            onTrim={(which, v) => {
              void setParam(`trim.${which}`, v);
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
