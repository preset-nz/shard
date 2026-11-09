import type { NodeCategory } from '@/audio';

/** One kind an Add menu offers. */
export interface AddChoice<K extends string> {
  kind: K;
  label: string;
  /** Its category id, from the kind's declaration in Rust. */
  category?: string;
  /** Why it cannot be added, shown greyed out. */
  disabled?: string;
}

export interface ChoiceGroup<K extends string> {
  /** Null for the kinds with no category, listed last. */
  id: string | null;
  label: string;
  choices: AddChoice<K>[];
}

/**
 * The choices grouped under their categories, in the list's order (Rust's
 * `CATEGORIES`). Within a group the choices keep the order they came in. A
 * choice whose category is missing from the list, or that has none, goes in a
 * last unheaded group; empty groups are left out.
 */
export function groupChoices<K extends string>(
  choices: AddChoice<K>[],
  categories: NodeCategory[],
): ChoiceGroup<K>[] {
  const groups: ChoiceGroup<K>[] = [];
  for (const c of categories) {
    const inside = choices.filter((x) => x.category === c.id);
    if (inside.length > 0) groups.push({ id: c.id, label: c.label, choices: inside });
  }
  const known = new Set(categories.map((c) => c.id));
  const rest = choices.filter((x) => x.category === undefined || !known.has(x.category));
  if (rest.length > 0) groups.push({ id: null, label: 'Other', choices: rest });
  return groups;
}
