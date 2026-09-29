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
 * **Scopes follow nodes.** A node is what the work area draws as a card and
 * the inspector shows when it is selected. Most nodes are one table prefix.
 * The `material` prefix is Sample, the plain-playback generator; the files it
 * and Granular read are materials, with their own octave and trim, outside the
 * table. See `guidance/projects/shard/design/panel-layout.md`.
 *
 * **Two views of each node.** The inspector draws the full scope; the
 * work-area card draws the summary, which is the same operator folded down to
 * its material and its level. Both are built here from the same rows, so a
 * card is never a separate control with its own behaviour: its level links to
 * an LFO and takes a MIDI knob exactly as the inspector's does. Choosing which
 * rows a summary shows belongs in facets itself, once a second app wants it.
 */
import { type PropertySchema, registerFieldRenderer, registerScope } from '@preset.nz/facets';
import {
  ARRANGEMENT_PREFIX,
  DRIVE_TYPES,
  denormalise,
  FILTER_TYPES,
  FM_TYPES,
  format,
  LENGTH_NAMES,
  normalise,
  type ParamInfo,
  setParam,
  WINDOW_NAMES,
} from '@/audio';
import { MaterialField } from '@/components/MaterialPicker';
import { ParamRow } from '@/components/ParamRow';

/** Where a node's card sits in the work area. */
export type Lane = 'generate' | 'process' | 'master';

/**
 * Which level a node belongs to: the patch, edited in sound scaping, or the
 * arrangement over it, edited in the tracker (Georg, 2026-09-26).
 */
export type Layer = 'patch' | 'arrangement';

/**
 * In signal order. Generators make sound and are summed; processes shape it
 * (Georg, 2026-09-14: generators are not processes, so they get their own).
 * There is no pre-process lane: trim and octave belong to each material, and
 * are edited from the Materials tree.
 */
export const LANES: Array<{ id: Lane; label: string }> = [
  { id: 'generate', label: 'Generators' },
  { id: 'process', label: 'Process' },
  { id: 'master', label: 'Master' },
];

/**
 * The tracker's lanes: the same three, with the patches where the generators
 * were (Georg, 2026-09-26: *"generators replaced with patches, which will
 * become tracks in the future"*).
 */
export const ARRANGEMENT_LANES: Array<{ id: Lane; label: string }> = [
  { id: 'generate', label: 'Patches' },
  { id: 'process', label: 'Process' },
  { id: 'master', label: 'Master' },
];

export interface NodeInfo {
  id: string;
  label: string;
  layer: Layer;
  /** Null for a node with no card: Tape, which lives in Settings. */
  lane: Lane | null;
  /**
   * The table prefix its switch, level and presets use, such as `grain`. Null
   * when it has none of its own.
   */
  table: string | null;
  /** Whether a parameter id is one of this node's rows. */
  owns: (id: string) => boolean;
  /**
   * The patch's own settings rather than a node that makes or shapes sound.
   * Drawn as a card, but never listed or added as a generator.
   */
  setting?: boolean;
}

const under = (prefix: string) => (id: string) => id.startsWith(`${prefix}.`);

/**
 * The nodes a material is wired into, by node id (Georg, 2026-09-15). Each
 * reads its own; `src-tauri/src/materials.rs` holds the same two names.
 */
export const READS_MATERIAL: ReadonlySet<string> = new Set(['material', 'grain']);

/** Every node, in work-area order: each lane's cards in signal order. */
export const NODES: NodeInfo[] = [
  // The patch's settings, before the generators (Georg, 2026-09-27: "isn't
  // it before the generator, a setting for the patch?"), in the Generators
  // lane rather than a column of its own. Trial and error; may move.
  {
    layer: 'patch',
    id: 'patch',
    label: 'Patch',
    lane: 'generate',
    table: 'patch',
    owns: under('patch'),
    setting: true,
  },
  {
    layer: 'patch',
    id: 'material',
    label: 'Sample',
    lane: 'generate',
    table: 'material',
    owns: under('material'),
  },
  {
    layer: 'patch',
    id: 'grain',
    label: 'Granular',
    lane: 'generate',
    table: 'grain',
    owns: under('grain'),
  },
  {
    layer: 'patch',
    id: 'fm',
    label: 'FM',
    lane: 'generate',
    table: 'fm',
    owns: under('fm'),
  },
  // `crush.env.*` lands here rather than in Envelope, which is the point: it
  // belongs to the crusher, not to the amplitude shape.
  {
    layer: 'patch',
    id: 'drive',
    label: 'Drive',
    lane: 'process',
    table: 'drive',
    owns: under('drive'),
  },
  {
    layer: 'patch',
    id: 'crush',
    label: 'Crush',
    lane: 'process',
    table: 'crush',
    owns: under('crush'),
  },
  {
    layer: 'patch',
    id: 'ring',
    label: 'Ring modulation',
    lane: 'process',
    table: 'ring',
    owns: under('ring'),
  },
  // Last of the effects (Georg, 2026-09-29), so it thickens what they made.
  {
    layer: 'patch',
    id: 'chorus',
    label: 'Chorus',
    lane: 'process',
    table: 'chorus',
    owns: under('chorus'),
  },
  {
    layer: 'patch',
    id: 'env',
    label: 'Envelope',
    lane: 'process',
    table: 'env',
    owns: under('env'),
  },
  // On the master, after every effect (Georg, 2026-09-14): what takes away
  // the harmonics crush and ring add. An EQ, if one comes, sits here too.
  {
    layer: 'patch',
    id: 'filter',
    label: 'Filter',
    lane: 'master',
    table: 'filter',
    owns: under('filter'),
  },
  { layer: 'patch', id: 'amp', label: 'Output', lane: 'master', table: 'amp', owns: under('amp') },
  // The tape's feel, in Settings (Georg, 2026-09-14). Brake and reverse are
  // played from the header, so they are not rows anywhere.
  {
    layer: 'patch',
    id: 'tape',
    label: 'Tape',
    lane: null,
    table: null,
    owns: (id) => under('tape')(id) && id !== 'tape.brake' && id !== 'tape.reverse',
  },
];

const arrangementNode = (node: string, label: string, lane: Lane): NodeInfo => {
  const table = `arrangement.${node}`;
  return { layer: 'arrangement', id: table, label, lane, table, owns: under(table) };
};

/**
 * The arrangement's nodes, in the tracker's lanes. One patch today, shown by
 * its fader; one per track with roadmap row 10. The crusher has no envelope
 * here, since the arrangement has no pass for one to follow.
 */
export const ARRANGEMENT_NODES: NodeInfo[] = [
  arrangementNode('track', 'Patch', 'generate'),
  arrangementNode('drive', 'Drive', 'process'),
  arrangementNode('crush', 'Crush', 'process'),
  arrangementNode('ring', 'Ring modulation', 'process'),
  arrangementNode('chorus', 'Chorus', 'process'),
  arrangementNode('filter', 'Filter', 'master'),
  arrangementNode('amp', 'Output', 'master'),
];

export interface ParamValues {
  [id: string]: number;
}

export function nodeById(id: string): NodeInfo | null {
  return NODES.find((n) => n.id === id) ?? ARRANGEMENT_NODES.find((n) => n.id === id) ?? null;
}

/** Which of a node's two scopes: the inspector's, or the card's. */
export type NodeView = 'full' | 'summary';

/** The facets scope key for one view of one node. */
export function scopeKeyFor(node: string, view: NodeView = 'full'): string {
  return view === 'full' ? `shard.params.${node}` : `shard.params.${node}.summary`;
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

/** A two-step row from 0 to 1 is a yes or no, not a choice of two. */
export function isSwitch(p: ParamInfo): boolean {
  return p.taper === 'stepped' && p.steps === 2 && p.min === 0 && p.max === 1;
}

/**
 * Names for the stepped rows whose steps are choices rather than numbers.
 * The arrangement borrows these rows, so its ids are looked up without their
 * prefix.
 */
const STEP_NAMES: Record<string, string[]> = {
  'grain.window': WINDOW_NAMES,
  'filter.type': FILTER_TYPES,
  'drive.type': DRIVE_TYPES,
  'patch.length': LENGTH_NAMES,
  'fm.type': FM_TYPES,
};

function fieldFor(p: ParamInfo) {
  const base = { id: p.id, path: p.id, label: p.name };

  if (isSwitch(p)) {
    return { ...base, kind: 'checkbox' as const };
  }

  if (p.taper === 'stepped') {
    const n = Math.max(1, p.steps ?? 1);
    const names = STEP_NAMES[p.id.replace(ARRANGEMENT_PREFIX, '')] ?? null;
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

/**
 * The schema for one view of a node. Untitled: the header carries the name.
 * A generator that reads a material leads with its picker in both views.
 */
export function buildSchema(defs: ParamInfo[], node: NodeInfo, view: NodeView): PropertySchema {
  const level = levelIdOf(defs, node);
  // A card folds to the level; a node with none, such as Patch, to its first row.
  const all = rowsOf(defs, node);
  const summary = level ?? all[0]?.id ?? null;
  const rows = all.filter((p) => view === 'full' || p.id === summary);
  const picker = READS_MATERIAL.has(node.id)
    ? [{ kind: 'material', id: `${node.id}.wire`, path: `${node.id}.wire`, node: node.id }]
    : [];
  return {
    version: 1,
    groups: [{ id: node.id, rows: [...picker, ...rows.map((p) => fieldFor(p))] }],
  };
}

/**
 * Register every node's scope. `selection` is the current values object;
 * `ctx` carries the definitions, links and heard values for the rows.
 */
export function registerParamScope(defs: ParamInfo[]) {
  const byId = new Map(defs.map((d) => [d.id, d]));
  registerFieldRenderer('param', ParamRow);
  registerFieldRenderer('material', MaterialField);

  for (const node of [...NODES, ...ARRANGEMENT_NODES]) {
    for (const view of ['full', 'summary'] as const) {
      registerScope<ParamValues, Record<string, unknown>>(scopeKeyFor(node.id, view), {
        schema: buildSchema(defs, node, view),

        read: (values) => {
          const out: Record<string, unknown> = {};
          for (const d of defs) {
            const v = values[d.id] ?? d.default;
            out[d.id] = isSwitch(d)
              ? v >= 0.5
              : d.taper === 'stepped'
                ? String(Math.round(v))
                : normalise(d, v);
          }
          return out;
        },

        write: (path, value) => {
          const d = byId.get(path);
          if (!d) return;
          const real = isSwitch(d)
            ? value
              ? 1
              : 0
            : d.taper === 'stepped'
              ? Number(value)
              : denormalise(d, Number(value));
          void setParam(d.id, real);
        },
      });
    }
  }
}
