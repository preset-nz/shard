import { create } from 'zustand';

/**
 * What the inspector shows: one node from the work area, one material, LFO or
 * envelope from the tree, or nothing. The waveform follows it too.
 *
 * Its own tiny store rather than part of any document, per guidance
 * `design/interaction-state.md`. The tree and the work area both write it,
 * and the inspector reads it. It is never saved with the patch: it describes
 * how you are working, not the sound.
 */
export type Selection =
  | { kind: 'node'; id: string }
  | { kind: 'material'; id: number }
  | { kind: 'lfo'; id: number }
  | { kind: 'envelope'; id: number }
  | null;

interface SelectionState {
  selection: Selection;
  select: (next: Selection) => void;
  clear: () => void;
}

export const useSelection = create<SelectionState>()((set) => ({
  selection: null,
  select: (selection) => set({ selection }),
  clear: () => set({ selection: null }),
}));
