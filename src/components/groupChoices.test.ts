import { describe, expect, it } from 'vitest';
import { groupChoices } from './groupChoices';

const categories = [
  { id: 'time', label: 'Time' },
  { id: 'filter', label: 'Filter' },
];

describe('groupChoices', () => {
  it('follows the category list, not the order the choices came in', () => {
    const groups = groupChoices(
      [
        { kind: 'ladder', label: 'Ladder', category: 'filter' },
        { kind: 'delay', label: 'Delay', category: 'time' },
        { kind: 'echo', label: 'Echo', category: 'time' },
      ],
      categories,
    );
    expect(groups.map((g) => g.id)).toEqual(['time', 'filter']);
    expect(groups[0]?.choices.map((c) => c.kind)).toEqual(['delay', 'echo']);
  });

  it('leaves out empty groups and puts the uncategorised last', () => {
    const groups = groupChoices(
      [
        { kind: 'x', label: 'X' },
        { kind: 'ladder', label: 'Ladder', category: 'filter' },
      ],
      categories,
    );
    expect(groups.map((g) => g.id)).toEqual(['filter', null]);
  });

  it('lists everything under one group when no categories came', () => {
    const groups = groupChoices([{ kind: 'x', label: 'X', category: 'filter' }], []);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.id).toBeNull();
  });
});
