import type { GrainInfo } from '@/audio';
import { WINDOW_NAMES } from '@/audio';

/**
 * The grain inspector.
 *
 * The question this answers is "what is the cloud actually doing?", and the
 * honest answer is that it is firing events, not holding objects. So the panel
 * shows a *log of spawns* rather than a list of live grains: at the default
 * settings only about a dozen grains exist at any instant, and stopping the
 * transport destroys all of them, so "stop and show me the grains" would show
 * an arbitrary handful of twelve.
 *
 * Two ways to use it, and the first is usually the one that teaches:
 *
 * - **Running.** Turn the density down and watch bars appear on the waveform.
 *   Position, jitter, size, pan and reverse all become visible as movement.
 * - **Frozen.** Stop the flow, then click any bar to hear that one grain on
 *   its own. A grain is six numbers, so this is exact reconstruction, not a
 *   recording — the same numbers go back to the engine.
 */
export function GrainInspector({
  grains,
  frozen,
  selected,
  playing,
  auditioning,
  sampleRate,
  onFreeze,
  onClear,
  onSelect,
}: {
  /** Recent spawns, oldest first. */
  grains: GrainInfo[];
  frozen: boolean;
  selected: GrainInfo | null;
  playing: boolean;
  auditioning: boolean;
  sampleRate: number;
  onFreeze: (frozen: boolean) => void;
  onClear: () => void;
  onSelect: (g: GrainInfo) => void;
}) {
  // Newest first: the interesting end of a log is the recent end.
  const rows = [...grains].reverse().slice(0, 40);

  /**
   * Grain length, in tape time.
   *
   * Not wall-clock time, and it cannot be: tape speed is applied per sample
   * while a grain is alive, so braking mid-grain makes that grain last longer
   * than it was asked to. There is no real-world duration to show, only the
   * length the engine asked for — which is what this is.
   */
  const ms = (samples: number) => ((samples / sampleRate) * 1000).toFixed(0);
  const semitones = (rate: number) => {
    const st = 12 * Math.log2(Math.abs(rate) || 1);
    return `${st >= 0 ? '+' : ''}${st.toFixed(1)}`;
  };
  const panLabel = (pan: number) => {
    const off = (pan - 0.5) * 200;
    if (Math.abs(off) < 3) return 'C';
    return `${off < 0 ? 'L' : 'R'}${Math.abs(off).toFixed(0)}`;
  };

  return (
    <section className="space-y-2">
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          Grains
        </h2>
        <button
          type="button"
          onClick={() => onFreeze(!frozen)}
          className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent"
        >
          {frozen ? 'Resume' : 'Freeze'}
        </button>
        <button
          type="button"
          onClick={onClear}
          className="rounded border border-border px-2 py-0.5 text-xs hover:bg-accent"
        >
          Clear
        </button>
        <span className="text-xs tabular-nums text-muted-foreground">{grains.length} logged</span>
        {auditioning && <span className="text-xs text-[#e0a96d]">playing one grain</span>}
      </div>

      <p className="max-w-prose text-xs leading-relaxed text-muted-foreground">
        {playing && !frozen
          ? 'Each bar on the waveform is one grain: where it read from, how long it was, and where it sat in the stereo field. Drop the density right down to watch them arrive one at a time — that is the cloud’s mechanism, visible. Stop, or freeze, to hear a single grain.'
          : 'Click a bar on the waveform, or a row below, to hear that grain by itself — windowed, pitched and panned exactly as it was, with no cloud around it and none of the shaping after it. Playback must be stopped.'}
      </p>

      {rows.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          Nothing logged yet. Press play and grains will appear here.
        </p>
      ) : (
        <div className="max-h-48 overflow-y-auto rounded border border-border">
          <table className="w-full text-[11px] tabular-nums">
            <thead className="sticky top-0 bg-card text-muted-foreground">
              <tr>
                <th className="px-2 py-1 text-left font-normal">#</th>
                <th className="px-2 py-1 text-right font-normal">At</th>
                <th
                  className="px-2 py-1 text-right font-normal"
                  title="The length this grain was asked for, in tape time. Not wall-clock: braking stretches a grain that is already playing, so its real duration is not known when it spawns."
                >
                  Length
                </th>
                <th className="px-2 py-1 text-right font-normal">Pitch</th>
                <th className="px-2 py-1 text-right font-normal">Pan</th>
                <th className="px-2 py-1 text-left font-normal">Shape</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((g) => (
                <tr
                  key={g.seq}
                  onClick={() => onSelect(g)}
                  className={`cursor-pointer border-t border-border hover:bg-accent ${
                    selected?.seq === g.seq ? 'bg-accent' : ''
                  }`}
                >
                  <td className="px-2 py-0.5 text-muted-foreground">{g.seq}</td>
                  <td className="px-2 py-0.5 text-right">{(g.position * 100).toFixed(1)}%</td>
                  <td className="px-2 py-0.5 text-right">{ms(g.len)} ms</td>
                  <td className="px-2 py-0.5 text-right">
                    {semitones(g.rate)}
                    {g.rate < 0 && <span title="played backwards"> ←</span>}
                  </td>
                  <td className="px-2 py-0.5 text-right">{panLabel(g.pan)}</td>
                  <td className="px-2 py-0.5">{WINDOW_NAMES[g.window] ?? g.window}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
