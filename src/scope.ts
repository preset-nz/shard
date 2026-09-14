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
 * **One scope per node.** A node is what the work area draws as a card and
 * the inspector shows when it is selected. Most nodes are one table prefix.
 * Material is split: trim and octave are the material every generator reads,
 * and its switch and gain are Sample, the plain-playback generator. Ids do not
 * change, being a wire format. See
 * `guidance/projects/shard/design/panel-layout.md`.
 */
import { type PropertySchema, registerFieldRenderer, registerScope } from '@preset.nz/facets';
import { denormalise, format, normalise, type ParamInfo, setParam, WINDOW_NAMES } from '@/audio';
import { ParamRow } from '@/components/ParamRow';

/** Where a node's card sits in the work area. */
export type Lane = 'generate' | 'process' | 'master';

/**
 * In signal order. Generators make sound and are summed; processes shape it
 * (Georg, 2026-09-14: generators are not processes, so they get their own).
 * There is no pre-process lane: trim and octave belong to the material every
 * generator reads, and are selected from the sample above the lanes.
 */
export const LANES: Array<{ id: Lane; label: string }> = [
  { id: 'generate', label: 'Generators' },
  { id: 'process', label: 'Process' },
  { id: 'master', label: 'Master' },
];

export interface NodeInfo {
  id: string;
  label: string;
  /**
   * Null for a node with no card: the material, selected from the sample, and
   * Tape, which lives in Settings.
   */
  lane: Lane | null;
  /**
   * The table prefix its switch, level and presets use, such as `grain`. Null
   * when it has none of its own.
   */
  table: string | null;
  /** Whether a parameter id is one of this node's rows. */
  owns: (id: string) => boolean;
}

const under = (prefix: string) => (id: string) => id.startsWith(`${prefix}.`);

/** Every node, in work-area order: each lane's cards in signal order. */
export const NODES: NodeInfo[] = [
  // The material itself: how much of it is read, and at what octave. Both
  // generators read through these, so they are not Sample's own. Selected by
  // clicking the sample's title above the lanes.
  {
    id: 'source',
    label: 'Material',
    lane: null,
    table: null,
    owns: (id) => under('trim')(id) || id === 'material.octave',
  },
  {
    id: 'material',
    label: 'Sample',
    lane: 'generate',
    table: 'material',
    owns: (id) => under('material')(id) && id !== 'material.octave',
  },
  { id: 'grain', label: 'Granular', lane: 'generate', table: 'grain', owns: under('grain') },
  // `crush.env.*` lands here rather than in Envelope, which is the point: it
  // belongs to the crusher, not to the amplitude shape.
  { id: 'crush', label: 'Crush', lane: 'process', table: 'crush', owns: under('crush') },
  { id: 'ring', label: 'Ring modulation', lane: 'process', table: 'ring', owns: under('ring') },
  { id: 'env', label: 'Envelope', lane: 'process', table: 'env', owns: under('env') },
  { id: 'amp', label: 'Output', lane: 'master', table: 'amp', owns: under('amp') },
  // The tape's feel, in Settings (Georg, 2026-09-14). Brake and reverse are
  // played from the header, so they are not rows anywhere.
  {
    id: 'tape',
    label: 'Tape',
    lane: null,
    table: null,
    owns: (id) => under('tape')(id) && id !== 'tape.brake' && id !== 'tape.reverse',
  },
];

export interface ParamValues {
  [id: string]: number;
}

export function nodeById(id: string): NodeInfo | null {
  return NODES.find((n) => n.id === id) ?? null;
}

/** The facets scope key for one node. */
export function scopeKeyFor(node: string): string {
  return `shard.params.${node}`;
}

/** A node's on/off switch, when the table gives it one: `grain.on`. */
export function switchIdOf(defs: ParamInfo[], node: NodeInfo): string | null {
  if (!node.table) return null;
  const id = `${node.table}.on`;
  return node.owns(id) && defs.some((p) => p.id === id) ? id : null;
}

/** A node's level, per the parameter pattern: Gain for a generator, Mix for an effect. */
export function levelIdOf(defs: ParamInfo[], node: NodeInfo): string | null {
  if (!node.table) return null;
  for (const suffix of ['gain', 'mix']) {
    const id = `${node.table}.${suffix}`;
    if (node.owns(id) && defs.some((p) => p.id === id)) return id;
  }
  return null;
}

/** The rows the inspector draws for a node: everything it owns but its switch. */
export function rowsOf(defs: ParamInfo[], node: NodeInfo): ParamInfo[] {
  const sw = switchIdOf(defs, node);
  return defs.filter((p) => node.owns(p.id) && p.id !== sw);
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

  // A custom kind, so the row can draw its value, its link and its moving
  // mark. facets looks renderers up by string and passes the whole field
  // through, so the extra props ride along untouched.
  return {
    ...base,
    kind: 'param' as const,
    def: p,
  };
}

/** The schema for one node. Untitled: the inspector's header carries the name. */
export function buildSchema(defs: ParamInfo[], node: NodeInfo): PropertySchema {
  return {
    version: 1,
    groups: [{ id: node.id, rows: rowsOf(defs, node).map((p) => fieldFor(p)) }],
  };
}

/**
 * Register every node's scope. `selection` is the current values object;
 * `ctx` carries the definitions, links and heard values for the rows.
 */
export function registerParamScope(defs: ParamInfo[]) {
  const byId = new Map(defs.map((d) => [d.id, d]));
  registerFieldRenderer('param', ParamRow);

  for (const node of NODES) {
    registerScope<ParamValues, Record<string, unknown>>(scopeKeyFor(node.id), {
      schema: buildSchema(defs, node),

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
