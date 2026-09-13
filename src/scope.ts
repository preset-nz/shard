/**
 * The facets scopes, built from the Rust parameter table at runtime.
 *
 * This is the payoff of keeping the parameter table as data. Nothing here
 * knows what a grain is. It reads the table, maps each taper to a control
 * kind, and hands facets a schema. Adding a parameter in Rust adds a control
 * here with no change to this file.
 *
 * Taper is doing two jobs, as designed: it says how a position maps to a
 * value, and it says what the control should look like. Stepped becomes a
 * selector rather than a slider, because interpolating between enum values
 * produces a number that means nothing.
 *
 * **One scope per section, not one for the panel.** Shard owns each section's
 * header — folding now, on/off next — and facets renders only the rows under
 * it. See `components/ParamSection.tsx`.
 */
import { type PropertySchema, registerFieldRenderer, registerScope } from '@preset.nz/facets';
import { denormalise, format, normalise, type ParamInfo, setParam, WINDOW_NAMES } from '@/audio';
import { ParamRow } from '@/components/ParamRow';

/** Which section a parameter belongs to, by id prefix. Order here is panel order. */
const GROUPS: Array<{ id: string; label: string; prefix: string }> = [
  // How the source is read before anything shapes it. Trim belongs here in
  // spirit, but its ids are a wire format, so it keeps its own section.
  { id: 'material', label: 'Material', prefix: 'material.' },
  { id: 'trim', label: 'Trim', prefix: 'trim.' },
  { id: 'grain', label: 'Granular', prefix: 'grain.' },
  { id: 'ring', label: 'Ring modulation', prefix: 'ring.' },
  // `crush.env.*` lands here rather than in Envelope, which is the point: it
  // belongs to the crusher, not to the amplitude shape.
  { id: 'crush', label: 'Crush', prefix: 'crush.' },
  // The tape transport. Brake and reverse are driven from the header buttons
  // during a performance; these rows are for setting the feel — above all
  // `tape.time`, which is what the gesture actually sounds like.
  { id: 'tape', label: 'Tape', prefix: 'tape.' },
  { id: 'env', label: 'Envelope', prefix: 'env.' },
  { id: 'amp', label: 'Output', prefix: 'amp.' },
];

export interface ParamValues {
  [id: string]: number;
}

export interface ParamSectionInfo {
  id: string;
  label: string;
}

/** The facets scope key for one section. */
export function scopeKeyFor(section: string): string {
  return `shard.params.${section}`;
}

/**
 * A section's bypass switch, when the table has one: `grain.on` for Granular.
 * Drawn in the section header rather than as a row.
 */
export function sectionSwitchId(defs: ParamInfo[], section: string): string | null {
  const id = `${section}.on`;
  return defs.some((p) => p.id === id) ? id : null;
}

/** The sections that actually have parameters, in panel order. */
export function paramSections(defs: ParamInfo[]): ParamSectionInfo[] {
  return GROUPS.filter((g) => defs.some((p) => p.id.startsWith(g.prefix))).map(({ id, label }) => ({
    id,
    label,
  }));
}

function fieldFor(p: ParamInfo) {
  const base = { id: p.id, path: p.id, label: p.name };

  if (p.taper === 'stepped') {
    const n = Math.max(1, p.steps ?? 1);
    const names = p.id === 'grain.window' ? WINDOW_NAMES : null;
    return {
      ...base,
      kind: 'select' as const,
      // Option values are the real stepped values, not indices. `read` hands
      // facets the rounded real value, so an index only matched while every
      // stepped range started at zero; octave runs from −2 to +2. Steps are
      // assumed whole, which every stepped row in the table is.
      options: Array.from({ length: n }, (_, i) => {
        const real = Math.round(p.min + ((p.max - p.min) * i) / Math.max(1, n - 1));
        return { value: String(real), label: names?.[i] ?? format(p, real) };
      }),
    };
  }

  // A custom kind, so the row can draw its value beside the label. facets
  // looks renderers up by string and passes the whole field through, so the
  // extra props ride along untouched.
  return {
    ...base,
    kind: 'param' as const,
    def: p,
  };
}

/** The schema for one section. Untitled: the section header carries the name. */
export function buildSchema(defs: ParamInfo[], section: string): PropertySchema {
  const g = GROUPS.find((x) => x.id === section);
  return {
    version: 1,
    groups: g
      ? [
          {
            id: g.id,
            rows: defs
              .filter((p) => p.id.startsWith(g.prefix) && p.id !== `${g.id}.on`)
              .map((p) => fieldFor(p)),
          },
        ]
      : [],
  };
}

/**
 * Register every section's scope. `selection` is the current values object;
 * `ctx` carries the definitions so read and write can apply the right taper.
 */
export function registerParamScope(defs: ParamInfo[]) {
  const byId = new Map(defs.map((d) => [d.id, d]));
  registerFieldRenderer('param', ParamRow);

  for (const section of paramSections(defs)) {
    registerScope<ParamValues, Record<string, unknown>>(scopeKeyFor(section.id), {
      schema: buildSchema(defs, section.id),

      read: (values) => {
        const out: Record<string, unknown> = {};
        for (const d of defs) {
          const v = values[d.id] ?? d.default;
          out[d.id] = d.taper === 'stepped' ? String(Math.round(v)) : normalise(d, v);
        }
        return out;
      },

      write: (path, value) => {
        const d = byId.get(path);
        if (!d) return;
        const real = d.taper === 'stepped' ? Number(value) : denormalise(d, Number(value));
        void setParam(d.id, real);
      },
    });
  }
}
