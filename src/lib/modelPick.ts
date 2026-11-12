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
