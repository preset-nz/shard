import type { ReactNode } from 'react';
import type { MaterialsView, MaterialView } from '@/audio';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Slider } from '@/components/ui/slider';
import { noteName, ROOT_NOTES } from '@/lib/notes';

/** Whole octaves, as the engine reads them. */
const OCTAVES = [-2, -1, 0, 1, 2];

/**
 * One material's octave and trim (Georg, 2026-09-15). Every generator wired to
 * it reads through them. The trim is also dragged on the waveform while this
 * material is shown there.
 */
export function MaterialEditor({
  material,
  pool,
  onChange,
}: {
  material: MaterialView;
  pool: MaterialsView;
  onChange: (next: MaterialView) => void;
}) {
  const readers = [
    pool.wires.material === material.id ? 'Sample' : null,
    pool.wires.grain === material.id ? 'Granular' : null,
  ].filter((r) => r !== null);

  return (
    <section className="space-y-3 p-3">
      <div>
        <div className="truncate text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          {material.name}
        </div>
        <p className="mt-1 break-all text-[11px] text-muted-foreground">
          {material.path ?? 'Built in, no file.'}
        </p>
        {material.missing && (
          <p className="mt-1 text-xs text-destructive">The file is no longer where it was.</p>
        )}
        <p className="mt-1 text-xs text-muted-foreground">
          {readers.length > 0
            ? `Read by ${readers.join(' and ')}.`
            : 'Nothing reads it. Pick it on Sample or Granular.'}
        </p>
      </div>

      <Row label="Octave">
        <Select
          items={Object.fromEntries(OCTAVES.map((o) => [String(o), `${o > 0 ? `+${o}` : o} oct`]))}
          value={String(material.octave)}
          onValueChange={(v) => onChange({ ...material, octave: Number(v) })}
        >
          <SelectTrigger size="sm" aria-label="Octave" className="h-7 w-24 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {OCTAVES.map((o) => (
              <SelectItem key={o} value={String(o)} className="text-xs">
                {o > 0 ? `+${o}` : o} oct
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Root">
        <Select
          items={{
            none: 'Unknown',
            ...Object.fromEntries(ROOT_NOTES.map((n) => [String(n), noteName(n)])),
          }}
          value={material.root === null ? 'none' : String(material.root)}
          onValueChange={(v) => onChange({ ...material, root: v === 'none' ? null : Number(v) })}
        >
          <SelectTrigger
            size="sm"
            aria-label="Root note"
            title="The note this sounds at. With it, the tracker shows each step as a note name."
            className="h-7 w-24 text-xs"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="none" className="text-xs">
              Unknown
            </SelectItem>
            {ROOT_NOTES.map((n) => (
              <SelectItem key={n} value={String(n)} className="text-xs">
                {noteName(n)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Row>
      <Row label="Start" value={percent(material.trim_start)}>
        <Slider
          min={0}
          max={1}
          step={0.001}
          value={[material.trim_start]}
          onValueChange={(v) =>
            onChange({ ...material, trim_start: Array.isArray(v) ? v[0] : (v as number) })
          }
          aria-label="Trim start"
          className="flex-1"
        />
      </Row>
      <Row label="End" value={percent(material.trim_end)}>
        <Slider
          min={0}
          max={1}
          step={0.001}
          value={[material.trim_end]}
          onValueChange={(v) =>
            onChange({ ...material, trim_end: Array.isArray(v) ? v[0] : (v as number) })
          }
          aria-label="Trim end"
          className="flex-1"
        />
      </Row>
    </section>
  );
}

function percent(t: number): string {
  return `${Math.round(t * 100)} %`;
}

function Row({ label, value, children }: { label: string; value?: string; children: ReactNode }) {
  return (
    <div className="flex items-center gap-2 text-xs">
      <span className="w-12 shrink-0 text-muted-foreground">{label}</span>
      {children}
      {value !== undefined && (
        <span className="w-10 shrink-0 text-right font-mono text-[10px] text-muted-foreground tabular-nums">
          {value}
        </span>
      )}
    </div>
  );
}
