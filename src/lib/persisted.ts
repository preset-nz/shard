import { useCallback, useState } from 'react';

/**
 * `useState` that survives a relaunch, for UI ephemera only: which sections
 * are folded, a panel width, the last tab. Never for anything a patch should
 * remember — that belongs in the document.
 *
 * The same hook Strata uses (guidance `design/persisted-ui-state.md`), so the
 * call sites can move to a settings backend later by changing this body.
 *
 * localStorage can be missing or throw — cleared site data, a locked-down
 * webview — so every read and write is guarded, and the app behaves the same
 * without it, just forgetfully.
 */
export function usePersistedState<T>(
  key: string,
  initial: T,
): [T, (next: T | ((prev: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw === null ? initial : (JSON.parse(raw) as T);
    } catch {
      return initial;
    }
  });

  const set = useCallback(
    (next: T | ((prev: T) => T)) => {
      setValue((prev) => {
        const resolved = typeof next === 'function' ? (next as (p: T) => T)(prev) : next;
        try {
          localStorage.setItem(key, JSON.stringify(resolved));
        } catch {
          // Not persisting is fine; losing the change in this session is not.
        }
        return resolved;
      });
    },
    [key],
  );

  return [value, set];
}
