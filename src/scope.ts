/**
 * The facets scope, built from the Rust parameter table at runtime.
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
 */
import { type PropertySchema, registerFieldRenderer, registerScope } from '@preset.nz/facets';
import { denormalise, normalise, type ParamInfo, setParam, WINDOW_NAMES } from '@/audio';
import { ParamRow } from '@/components/ParamRow';

export const SCOPE_KEY = 'shard.params';

/** Which group a parameter belongs to, by id prefix. */
const GROUPS: Array<{ id: string; label: string; prefix: string }> = [
  { id: 'grain', label: 'Granular', prefix: 'grain.' },
  { id: 'ring', label: 'Ring modulation', prefix: 'ring.' },
  { id: 'amp', label: 'Output', prefix: 'amp.' },
];

export interface ParamValues {
  [id: string]: number;
}

function fieldFor(p: ParamInfo, drifting: Set<string>) {
  const base = { id: p.id, path: p.id, label: p.name };

  if (p.taper === 'stepped') {
    const n = Math.max(1, p.steps ?? 1);
    const names = p.id === 'grain.window' ? WINDOW_NAMES : null;
    return {
      ...base,
      kind: 'select' as const,
      options: Array.from({ length: n }, (_, i) => ({
        value: String(i),
        label: names?.[i] ?? String(i),
      })),
    };
  }

  // A custom kind, so one row can carry both the slider and its drift toggle.
  // facets looks renderers up by string and passes the whole field through,
  // so the extra props ride along untouched.
  return {
    ...base,
    kind: 'param' as const,
    def: p,
    drifting: drifting.has(p.id),
    onDriftToggle: onDriftToggle,
  };
}

/** Set by `registerParamScope`, so the row component can reach the app. */
let onDriftToggle: (id: string, on: boolean) => void = () => {};

export function buildSchema(defs: ParamInfo[], drifting: Set<string>): PropertySchema {
  return {
    version: 1,
    groups: GROUPS.map((g) => ({
      id: g.id,
      title: g.label,
      rows: defs.filter((p) => p.id.startsWith(g.prefix)).map((p) => fieldFor(p, drifting)),
    })).filter((g) => g.rows.length > 0),
  };
}

/**
 * Register the scope. `selection` is the current values object; `ctx` carries
 * the definitions so read and write can apply the right taper.
 */
export function registerParamScope(
  defs: ParamInfo[],
  drifting: Set<string>,
  toggle: (id: string, on: boolean) => void,
) {
  const byId = new Map(defs.map((d) => [d.id, d]));
  onDriftToggle = toggle;
  registerFieldRenderer('param', ParamRow);

  registerScope<ParamValues, Record<string, unknown>>(SCOPE_KEY, {
    schema: buildSchema(defs, drifting),

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
