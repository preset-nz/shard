/** The model dropdown's value for "whatever is loaded", which saves as empty. */
export const AUTO = 'auto';

/**
 * What to save for a pick in the model dropdown, or null to ignore it.
 *
 * Base UI's Select reports `null` as the value when its item list changes and
 * the current value is no longer among the items (SelectPositioner's
 * onMapChange). That isn't a pick: String(null) saved "null" to agent.json.
 */
export function modelFromPick(picked: unknown, current: string): string | null {
  if (picked == null) return null;
  const next = picked === AUTO ? '' : String(picked);
  return next === current ? null : next;
}

/** The server dropdown's value for "a server not in the list" (Other…). */
export const OTHER = 'other';

/**
 * What a pick in the server dropdown means, or null to ignore it (null from
 * Base UI, or the pick already showing). A known server's URL is saved at once
 * (`save`); Other… saves nothing, it only shows the URL input (`other`).
 */
export function serverFromPick(
  picked: unknown,
  shown: string,
): { save: string } | { other: true } | null {
  if (picked == null) return null;
  const next = String(picked);
  if (next === shown) return null;
  return next === OTHER ? { other: true } : { save: next };
}

/** Which dropdown item a saved URL selects: its own, or Other… when no known server has it. */
export function serverShown(baseUrl: string, known: readonly string[]): string {
  const url = baseUrl.trim();
  return known.includes(url) ? url : OTHER;
}
